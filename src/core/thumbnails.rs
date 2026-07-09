//! 缩略图加载器：worker pool + 优先级队列 + 分桶磁盘缓存
//!
//! - 多个 tokio blocking worker 并行处理缩略图生成/读取
//! - 按 `path + mtime` 计算 blake3 哈希作为缓存键（mtime 变了自动失效）
//! - 缓存目录按 `thumbnails/{small|medium|large}/<hash 前两位>/<hash>.(jpg|webp)` 分桶
//! - 内存 LRU 缓存已加载的 `Texture`，避免重复解码
//! - **优先级队列**：可见 tile 可经 `prioritize_keys` 提到队首（BOOST），
//!   先于普通（NORMAL）请求被 worker 取走，消除分页 rebuild / 滚动时的优先级倒置。
mod cache;
mod decode;
mod jpeg_turbo;
mod queue;
mod video;

use crate::core::db::DbPool;
use crate::core::runtime_config;
#[cfg(test)]
use cache::cache_stem_for;
#[cfg(test)]
use cache::load_pixbuf_sync;
use cache::{cache_key_str, existing_cache_path, load_pixbuf_sync_or_remove};
use decode::pixbuf_is_light;
#[cfg(test)]
use decode::{
    ensure_opaque, generate, generate_unavailable_placeholder, generate_via_pixbuf,
    scale_pixbuf_to_fit,
};
use gtk4::gdk::Texture;
use lru::LruCache;
use queue::worker_loop;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime};
use tokio::sync::oneshot;
use tracing::{debug, warn};
#[cfg(test)]
use video::{
    extract_video_frame_ffmpeg, ffmpeg_thumbnail_temp_path, overlay_play_icon, read_video_rotation,
};

/// 缩略图尺寸档位
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThumbnailSize {
    #[default]
    Small, // 256
    Medium, // 512
    Large,  // 1024
}

impl ThumbnailSize {
    pub fn max_dim(self) -> u32 {
        match self {
            Self::Small => 256,
            Self::Medium => 512,
            Self::Large => 1024,
        }
    }

    pub fn quality(self) -> u8 {
        match self {
            Self::Small => 82,
            Self::Medium => 85,
            Self::Large => 88,
        }
    }

    pub fn subdir(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// 缩略图加载结果：texture + 在 worker 端顺带算好的派生数据。
///
/// 把亮度判定（`is_light`）从主线程的 `Texture::download`（每张全像素回读 +
/// 大 buffer 分配）下沉到 worker：worker 手里就有 pixbuf，直接就地采样，
/// 主线程零分配、零回读。`PhotoTile`/相册/回收站等不需要亮度的调用方只取
/// `.texture` 即可。
#[derive(Clone)]
pub struct LoadedThumb {
    pub texture: Texture,
    pub is_light: Option<bool>,
}

// ── 优先级队列 ──────────────────────────────────────────────────────────────
/// 优先级档：`BOOST`（可见，值小）先于 `NORMAL` 被 worker 取走。
pub const TIER_BOOST: u8 = 0;
pub const TIER_NORMAL: u8 = 1;
/// 后台预热拉取项以此 tier 标记，仅在 worker 从 DB 拉取时使用。
const TIER_BACKGROUND: u8 = 2;

/// 队列里的一条工作项。
///
/// `tier` + `seq` 决定弹出顺序（`BinaryHeap<Reverse<PriItem>>` 弹最小项 =
/// 最小 (tier, seq) = 最高优先级，tier 内按 seq FIFO）。同一 `cache_key` 被
/// `prioritize_keys` 提权时，只更新 `queued_tiers` 并 push 一条新 tier 的项；
/// 旧 tier 的项在弹出时因 `tier` 与 `queued_tiers` 不符而被惰性丢弃。
#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::core::thumbnails) struct PriItem {
    tier: u8,
    seq: u64,
    cache_key: String,
    uri: String,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
    enqueued_at: Instant,
    /// DB `media_items.id`（BACKGROUND 拉取时携带，供批量更新缩略图状态）。
    media_id: i64,
}

impl PriItem {
    /// 全字段有序，保证 `Ord` 与派生 `Eq` 一致（Rust 对 `Ord` 的全序契约）；
    /// `(tier, seq)` 在最前以实现优先级语义，其余字段仅作 tie-breaker。
    fn priority_cmp(&self, other: &Self) -> Ordering {
        self.tier
            .cmp(&other.tier)
            .then_with(|| self.seq.cmp(&other.seq))
            .then_with(|| self.cache_key.cmp(&other.cache_key))
            .then_with(|| self.uri.cmp(&other.uri))
            .then_with(|| self.size.cmp(&other.size))
            .then_with(|| self.mtime.cmp(&other.mtime))
    }
}
impl Ord for PriItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority_cmp(other)
    }
}
impl PartialOrd for PriItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 一条排队请求的"真相"：tier + 生成所需的 uri/size/mtime。
///
/// 提权时无法从堆里就地改某条 `PriItem`，只能 push 新 tier 的堆项；但新堆项必须
/// 携带**真实**的 uri/size/mtime（否则 worker 拿空 uri 去 generate 会失败）。所以
/// 这些字段缓存在 `queued` 里，提权时据此重建 `PriItem`。
pub(in crate::core::thumbnails) struct QueuedEntry {
    tier: u8,
    uri: String,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
    enqueued_at: Instant,
    media_id: i64,
}

/// 优先级队列的可变状态。
pub(in crate::core::thumbnails) struct QueueState {
    /// 工作项堆（`Reverse` 让最大堆弹出最小 (tier, seq)）。
    heap: BinaryHeap<Reverse<PriItem>>,
    /// `cache_key` → 排队中的请求（含 tier 与生成参数）。弹出时据此校验堆项是否
    /// 过期、提权时据此重建 `PriItem`。
    queued: HashMap<String, QueuedEntry>,
    /// 单调递增的入队序号，用于 tier 内 FIFO。
    seq: u64,
    /// 关闭标志：`shutdown()` 置位后唤醒的 worker 立即退出。
    closed: bool,
}

/// 队列 + 唤醒条件变量。worker（spawn_blocking OS 线程）在 `cvar` 上阻塞等待，
/// 故用 **std `Condvar`**（不是 tokio 的——它需要 reactor，而 worker 不 `.await`）。
pub(in crate::core::thumbnails) type SharedQueue = Arc<(Mutex<QueueState>, Condvar)>;
pub(in crate::core::thumbnails) type StatsDirtyCallback = Arc<dyn Fn() + Send + Sync>;
pub(in crate::core::thumbnails) type SharedStatsDirtyCallback =
    Arc<Mutex<Option<StatsDirtyCallback>>>;

/// 加载器的可变缓存状态，用单一 Mutex 保护。
///
/// 把 `mem_cache` 与 `in_flight` 放在同一把锁后，request 端的
/// "查 mem_cache → 查 in_flight → 登记并入队" 与 worker 端的
/// "写 mem_cache → 取走等待者" 互斥执行，杜绝二者之间的竞态窗口
/// （否则一个刚完成的 key 可能被新请求当作未生成而重复入队）。
pub(in crate::core::thumbnails) struct LoaderState {
    mem_cache: LruCache<String, LoadedThumb>,
    /// `cache_key` → 正在生成的请求的等待者列表。
    ///
    /// 同 key 的后续 request 直接 append 到这里、**不再单独入队**，因此：
    ///   - 同一张缩略图永远不会被重复生成；
    ///   - 重复请求永远不会因为队列满而被丢弃。
    in_flight: HashMap<String, Vec<oneshot::Sender<LoadedThumb>>>,
}

/// 后台预热拉取状态：worker 在队列为空时据此从 DB 拉取下一个需生成的项。
pub(in crate::core::thumbnails) struct BackgroundPullState {
    enabled: AtomicBool,
    offset: Mutex<u32>,
    /// 预热缩略图尺寸（跟随当前视图模式，默认 Small）。
    size: Mutex<ThumbnailSize>,
    /// worker 数量：预热拉取一次取这么多条，一次性喂饱所有 worker。
    worker_count: Mutex<usize>,
}

/// 缩略图加载器单例
///
/// 内部用优先级队列把请求分发给一组 worker；worker 在 tokio 阻塞线程上
/// 完成 CPU/IO 密集的解码/编码后通过 oneshot 归还 `LoadedThumb`。request 端
/// 做在途去重，保证同一 (uri, size) 只生成一次、且永不丢请求；可见 tile 可经
/// `prioritize_keys` 提前。
/// 队列空时 worker 自动从 DB 拉取下一张未缓存的缩略图生成（拉模型），
/// 不会一次性灌入队列。
pub struct ThumbnailLoader {
    pool: DbPool,
    cache_dir: PathBuf,
    queue_capacity: usize,
    queue: SharedQueue,
    state: Arc<Mutex<LoaderState>>,
    background_pull: Arc<BackgroundPullState>,
    stats_dirty_callback: SharedStatsDirtyCallback,
}

impl ThumbnailLoader {
    /// 队列中当前排队项数（不含已在途/正在 worker 中生成的）。
    pub fn queue_len(&self) -> usize {
        let (lock, _) = &*self.queue;
        lock.lock().map(|q| q.queued.len()).unwrap_or(0)
    }

    /// 在途（正在生成或等待 worker）项数。
    pub fn in_flight_len(&self) -> usize {
        self.state.lock().map(|s| s.in_flight.len()).unwrap_or(0)
    }

    /// 构造加载器（不自动启动 worker；调用 `spawn_workers` 启动）
    pub fn new(pool: DbPool, cache_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        let runtime = runtime_config::load();
        let state = Arc::new(Mutex::new(LoaderState {
            mem_cache: LruCache::new(NonZeroUsize::new(runtime.thumbnail_mem_cache_cap).unwrap()),
            in_flight: HashMap::new(),
        }));
        let queue = Arc::new((
            Mutex::new(QueueState {
                heap: BinaryHeap::new(),
                queued: HashMap::new(),
                seq: 0,
                closed: false,
            }),
            Condvar::new(),
        ));
        // 启动时按 mtime LRU 清理超限缓存。
        // 用裸线程异步执行，避免在首绘前于主线程上 walkdir 整个缓存目录 +
        // 逐文件 stat + 全量排序（数千文件时是可观的启动延迟）。这里不需要
        // tokio 运行时上下文，所以用 std 线程，在测试中 `new()` 也能安全调用。
        let cleanup_dir = cache_dir.clone();
        let disk_cache_bytes = runtime.thumbnail_disk_cache_bytes;
        std::thread::spawn(move || {
            let _ = crate::core::cache::enforce_size_limit(
                &cleanup_dir.join("thumbnails"),
                disk_cache_bytes,
            );
        });
        Self {
            pool,
            cache_dir,
            queue_capacity: runtime.thumbnail_queue_capacity,
            queue,
            state,
            background_pull: Arc::new(BackgroundPullState {
                enabled: AtomicBool::new(false),
                offset: Mutex::new(0),
                size: Mutex::new(ThumbnailSize::Small),
                worker_count: Mutex::new(1),
            }),
            stats_dirty_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// 数据库连接池引用（用于查询媒体总数等）。
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// DB 中已生成缩略图的媒体项总数。
    pub fn generated_count(&self) -> usize {
        crate::core::db::count_thumbnail_generated(&self.pool).unwrap_or(0)
    }

    pub fn set_stats_dirty_callback(&self, callback: StatsDirtyCallback) {
        if let Ok(mut slot) = self.stats_dirty_callback.lock() {
            *slot = Some(callback);
        }
    }

    /// 后台预热是否仍在进行（标志位未关）。
    pub fn is_prewarm_active(&self) -> bool {
        self.background_pull.enabled.load(AtomicOrdering::Relaxed)
    }

    /// 启动后台预热拉取：设置标志，worker 在队列空时自动从 DB 拉取下一张
    /// 未缓存的缩略图生成。幂等——多次调用与一次等效。
    pub fn start_background_prewarm(&self) {
        self.background_pull
            .enabled
            .store(true, AtomicOrdering::Relaxed);
        if let Ok(mut off) = self.background_pull.offset.lock() {
            *off = 0;
        }
        // 唤醒可能在 cvar 上阻塞的 worker，让它们转入 background pull
        let (_, cvar) = &*self.queue;
        cvar.notify_all();
    }

    /// 设置后台预热的缩略图尺寸（跟随当前视图模式: Year→Small, Month→Medium, Day→Medium）。
    /// 切换时重置 DB 拉取偏移，让新尺寸从头扫。
    pub fn set_prewarm_thumbnail_size(&self, size: ThumbnailSize) {
        let span = tracing::info_span!(
            "thumb:set_prewarm_thumbnail_size",
            size = ?size,
            changed = tracing::field::Empty
        );
        let _trace = span.enter();
        if let Ok(mut s) = self.background_pull.size.lock() {
            if *s == size {
                span.record("changed", false);
                return;
            }
            *s = size;
        }
        span.record("changed", true);
        if let Ok(mut off) = self.background_pull.offset.lock() {
            *off = 0;
        }
    }

    /// 把后台预热的拉取起点重定向到 `offset`（全局 DESC 偏移，0 = 最新）。
    ///
    /// 预热默认从最新（offset 0）往最旧推进；但用户能用滚动条瞬间跳到任意
    /// 区域。在虚拟分页落地时调用本方法，把预热起点移到用户当前浏览的区间，
    /// 使**屏外邻域**（用户即将滚到的地方）先于无关的最新批次被暖。可见 tile
    /// 仍走 `TIER_BOOST`（最高优先级），不受影响——这里只决定屏外预热工作的
    /// 位置，不改预热的拉模型、DESC 顺序、tier 或限流（"预热的逻辑没问题"）。
    ///
    /// 仅改 `background_pull.offset` 并唤醒空闲 worker 立即按新起点拉取；线程
    /// 安全（std mutex，从 GTK 线程调用），锁只在赋值期间持有，绝不在 DB 查询
    /// 期间持有——与 `set_prewarm_thumbnail_size` 同一模式。
    pub fn redirect_prewarm_to_offset(&self, offset: u32) {
        debug!(
            target: crate::core::log_targets::THUMBNAILS,
            "PREWARM redirect_to_offset={} enabled={}",
            offset,
            self.is_prewarm_active()
        );
        if let Ok(mut off) = self.background_pull.offset.lock() {
            *off = offset;
        }
        // 唤醒可能在 cvar 上阻塞的 worker，让它们立刻从新起点拉取，
        // 而不是等下一次 prewarm 轮询超时。
        let (_, cvar) = &*self.queue;
        cvar.notify_all();
    }

    /// 启动 n 个 worker 消费请求
    pub fn spawn_workers(&self, n: usize) {
        if n == 0 {
            return;
        }
        if let Ok(mut wc) = self.background_pull.worker_count.lock() {
            *wc = n;
        }
        for _ in 0..n {
            let pool = self.pool.clone();
            let cache_dir = self.cache_dir.clone();
            let queue = self.queue.clone();
            let state = self.state.clone();
            let bg = self.background_pull.clone();
            let stats_dirty_callback = self.stats_dirty_callback.clone();
            tokio::task::spawn_blocking(move || {
                worker_loop(queue, pool, cache_dir, state, bg, stats_dirty_callback);
            });
        }
    }

    /// 提交一个缩略图请求。
    ///
    /// 走三层短路，确保**同一个 (uri, size) 永远不会被重复生成，也永远不会
    /// 因为队列满而被静默丢弃**：
    ///   1. mem-cache 命中 → 立刻同步回送；
    ///   2. 已有在途生成 → 把 reply 挂到该在途请求的等待者列表（不入队）；
    ///   3. 否则登记一条在途项并以指定 `tier` 优先级入队。
    ///
    /// `mtime`：若调用方已知源文件 mtime（如 `MediaItem.file_mtime`），传入可
    /// **跳过主线程 stat**（B5：mtime 已在扫描/notify 时入库，无需每次请求再 stat）；
    /// 传 `None` 则现场 stat 兜底。只有当队列里**已塞满彼此不同的**未缓存工作项
    /// （远超库规模才会发生）时，第 3 步才会失败；此时回滚在途项，调用方收到 `Err`。
    ///
    /// `tier`：`TIER_BOOST`（可见/视口优先）、`TIER_NORMAL`（默认）、
    /// `TIER_BACKGROUND`（全局预热，不入 mem_cache，受 worker 限流）。
    ///
    /// 锁序：`state{drop}` → `queue{drop}` →（回滚）`state{drop}`。两锁从不嵌套，无死锁。
    pub fn request(
        &self,
        uri: String,
        size: ThumbnailSize,
        mtime: Option<SystemTime>,
        reply: oneshot::Sender<LoadedThumb>,
        tier: u8,
    ) {
        self.request_inner(0, uri, size, mtime, reply, tier);
    }

    /// Submit a thumbnail request for a known DB media row.
    ///
    /// UI-visible requests use this so a successfully generated thumbnail
    /// updates `media_items.thumbnail_generated_at` immediately, keeping the
    /// library stats label in sync even before background prewarm reaches it.
    pub fn request_for_media(
        &self,
        media_id: i64,
        uri: String,
        size: ThumbnailSize,
        mtime: Option<SystemTime>,
        reply: oneshot::Sender<LoadedThumb>,
        tier: u8,
    ) {
        self.request_inner(media_id, uri, size, mtime, reply, tier);
    }

    pub fn try_load_cached(
        &self,
        uri: &str,
        size: ThumbnailSize,
        mtime: Option<SystemTime>,
    ) -> Option<LoadedThumb> {
        let cache_key = cache_key_str(uri, size, mtime)?;
        if let Ok(mut st) = self.state.lock() {
            if let Some(loaded) = st.mem_cache.get(&cache_key).cloned() {
                debug!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB_LOADER_TRACE try_load_cached_mem_hit uri={} size={:?} cache_key={}",
                    uri,
                    size,
                    cache_key
                );
                return Some(loaded);
            }
        }

        let cache_path = existing_cache_path(&self.cache_dir, uri, size, mtime)
            .ok()
            .flatten()?;
        let pb = match load_pixbuf_sync_or_remove(&cache_path) {
            Ok(pb) => pb,
            Err(e) => {
                warn!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB_LOADER_TRACE try_load_cached_disk_failed uri={} size={:?} cache_path={} error={}",
                    uri,
                    size,
                    cache_path.display(),
                    e
                );
                return None;
            }
        };
        let loaded = LoadedThumb {
            texture: Texture::for_pixbuf(&pb),
            is_light: pixbuf_is_light(&pb),
        };
        if let Ok(mut st) = self.state.lock() {
            st.mem_cache.put(cache_key.clone(), loaded.clone());
        }
        debug!(
            target: crate::core::log_targets::THUMBNAILS,
            "THUMB_LOADER_TRACE try_load_cached_disk_hit uri={} size={:?} cache_path={} cache_key={}",
            uri,
            size,
            cache_path.display(),
            cache_key
        );
        Some(loaded)
    }

    /// Memory-LRU-only lookup — no disk I/O, no main-thread thumbnail decode.
    ///
    /// Used on the GTK main thread while the grid builds tiles: a freshly-built
    /// tile paints instantly only when its thumbnail is already resident in the
    /// in-memory LRU (recently viewed). Everything else is left to the
    /// viewport-driven async request path, whose worker consults the disk cache.
    /// This keeps a rebuild off the disk — building a ~500-tile virtual page no
    /// longer performs ~500 synchronous `try_load_cached` reads + pixbuf decodes
    /// on the main thread, which was the fast-scroll freeze. The async path
    /// still loads those thumbnails (worker disk-cache hit, off the main thread),
    /// so thumbnails appear a few ms later instead of blocking the frame.
    pub fn try_load_mem_cached(
        &self,
        uri: &str,
        size: ThumbnailSize,
        mtime: Option<SystemTime>,
    ) -> Option<LoadedThumb> {
        let cache_key = cache_key_str(uri, size, mtime)?;
        let loaded = self.state.lock().ok()?.mem_cache.get(&cache_key).cloned()?;
        debug!(
            target: crate::core::log_targets::THUMBNAILS,
            "THUMB_LOADER_TRACE try_load_mem_cached_hit uri={} size={:?} cache_key={}",
            uri,
            size,
            cache_key
        );
        Some(loaded)
    }

    fn request_inner(
        &self,
        media_id: i64,
        uri: String,
        size: ThumbnailSize,
        mtime: Option<SystemTime>,
        reply: oneshot::Sender<LoadedThumb>,
        tier: u8,
    ) {
        let requested_at = Instant::now();
        let Some(cache_key) = cache_key_str(&uri, size, mtime) else {
            // 源文件不存在 / 无法 stat：无法去重，按"生成失败"处理。
            warn!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB request_cache_key_failed uri={} size={:?}",
                uri,
                size
            );
            return; // reply 被 drop → 调用方 rx 收到 Err
        };
        debug!(
            target: crate::core::log_targets::THUMBNAILS,
            "THUMB request_start uri={} size={:?} tier={} supplied_mtime={:?} cache_key={}",
            uri,
            size,
            tier,
            mtime,
            cache_key
        );

        let mut st = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return, // poisoned
        };
        // 1) 内存命中
        if let Some(loaded) = st.mem_cache.get(&cache_key).cloned() {
            debug!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB_LOADER_TRACE mem_cache_hit uri={} size={:?} tier={} cache_key={}",
                uri,
                size,
                tier,
                cache_key
            );
            drop(st);
            let _ = reply.send(loaded);
            return;
        }
        // 2) 已在途 → 挂载等待者，不再入队
        if let Some(waiters) = st.in_flight.get_mut(&cache_key) {
            debug!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB_LOADER_TRACE in_flight_join uri={} size={:?} tier={} cache_key={} waiters_before={}",
                uri,
                size,
                tier,
                cache_key,
                waiters.len()
            );
            waiters.push(reply);
            return;
        }
        // 3) 新工作项：先登记在途，再入队
        st.in_flight.insert(cache_key.clone(), vec![reply]);
        drop(st);

        let (lock, cvar) = &*self.queue;
        let enqueued = {
            let mut q = match lock.lock() {
                Ok(q) => q,
                Err(_) => {
                    // poisoned：回滚在途项
                    if let Ok(mut st) = self.state.lock() {
                        st.in_flight.remove(&cache_key);
                    }
                    return;
                }
            };
            if q.queued.len() >= self.queue_capacity {
                false
            } else {
                q.seq += 1;
                let seq = q.seq;
                q.queued.insert(
                    cache_key.clone(),
                    QueuedEntry {
                        tier,
                        uri: uri.clone(),
                        size,
                        mtime,
                        enqueued_at: requested_at,
                        media_id,
                    },
                );
                q.heap.push(Reverse(PriItem {
                    tier,
                    seq,
                    cache_key: cache_key.clone(),
                    uri: uri.clone(),
                    size,
                    mtime,
                    enqueued_at: requested_at,
                    media_id,
                }));
                true
            }
        };
        if enqueued {
            debug!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB_LOADER_TRACE enqueued uri={} size={:?} tier={} queue_len={} in_flight={} cache_key={}",
                uri,
                size,
                tier,
                self.queue_len(),
                self.in_flight_len(),
                cache_key
            );
            cvar.notify_one();
        } else {
            // 队列已满且全是不同的未缓存项：回滚在途项，避免等待者被永久挂起。
            if let Ok(mut st) = self.state.lock() {
                st.in_flight.remove(&cache_key); // drop reply → 调用方 rx.Err
            }
            warn!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB enqueue_failed uri={} size={:?} tier={} queue_capacity={} cache_key={}",
                uri,
                size,
                tier,
                self.queue_capacity,
                cache_key
            );
        }
    }

    /// 把给定（可见）缩略图请求提到队首。
    ///
    /// 仅对**仍在队列里**（未开始、未命中、未在途）的 key 生效：把它们的 tier
    /// 改为 `BOOST` 并用 `queued` 里缓存的真实 uri/size/mtime push 一条新堆项
    /// （旧的 NORMAL 项弹出时惰性丢弃）。已 mem 命中 / 在途 / 已完成的 key 不在
    /// `queued` 中 → 无害跳过，**绝不重复生成**。`notify_all` 唤醒所有睡眠 worker，
    /// 让它们按新优先级取项。
    ///
    /// keys 由 UI 端在建 tile 时用 `cache_key_for` 预算好（带 file_mtime，无主线程
    /// stat），故与 request 端的键天然一致。只动 queue 锁，不碰 state 锁。
    pub fn prioritize_keys(&self, keys: &[String]) {
        let (lock, cvar) = &*self.queue;
        let mut q = match lock.lock() {
            Ok(q) => q,
            Err(_) => return,
        };
        let mut changed = false;
        for key in keys {
            // 先用不可变借用读出 tier 与生成参数并 clone，结束对 q.queued 的借用，
            // 再改 tier、push 堆项（避免 entry 借用与 q.seq/q.heap 的可变借用重叠）。
            let Some(entry) = q.queued.get(key) else {
                continue;
            };
            if entry.tier == TIER_BOOST {
                continue;
            }
            let uri = entry.uri.clone();
            let size = entry.size;
            let mtime = entry.mtime;
            let enqueued_at = entry.enqueued_at;
            let media_id = entry.media_id;
            if let Some(e) = q.queued.get_mut(key) {
                e.tier = TIER_BOOST;
            }
            q.seq += 1;
            let seq = q.seq;
            q.heap.push(Reverse(PriItem {
                tier: TIER_BOOST,
                seq,
                cache_key: key.clone(),
                uri,
                size,
                mtime,
                enqueued_at,
                media_id,
            }));
            changed = true;
        }
        drop(q);
        if changed {
            debug!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB reprioritize changed=true requested_keys={}",
                keys.len()
            );
            cvar.notify_all();
        }
    }

    /// 用与 request/prioritize 一致的方式预算 mem-cache / 去重键
    /// （`{path:?}:{mtime:?}:{size:?}`）。`mtime` 传入可免主线程 stat。
    /// 源文件无法解析（且未给 mtime）时返回 `None`。
    pub fn cache_key_for(
        uri: &str,
        size: ThumbnailSize,
        mtime: Option<SystemTime>,
    ) -> Option<String> {
        cache_key_str(uri, size, mtime)
    }

    /// 清空内存缓存（LRU 缓存和在途去重映射）。
    ///
    /// 用于清理功能，强制后续请求重新从磁盘加载缩略图。
    pub fn clear_mem_cache(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.mem_cache.clear();
            state.in_flight.clear();
        }
    }

    /// 关闭队列：唤醒所有 worker 并让它们退出。生产中 loader 是泄漏单例、永不调用；
    /// 仅供测试收尾，避免 worker 线程跨用例泄漏。
    pub fn shutdown(&self) {
        let (lock, cvar) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            q.closed = true;
        }
        cvar.notify_all();
    }
}

impl Drop for ThumbnailLoader {
    fn drop(&mut self) {
        // worker 阻塞在 std Condvar 上，tokio 无法在 runtime 关闭时强制中止
        // `spawn_blocking` 任务——若不主动唤醒，runtime drop 会因 join worker 线程
        // 而永久挂起（旧的 mpsc 设计靠 channel 关闭让 worker 自然退出）。这里在
        // loader 析构时关闭队列，让 worker 干净退出。生产中 loader 被 Arc 长期持有、
        // 析构不发生，故无副作用；测试里 loader 是局部变量，drop 触发清理。
        self.shutdown();
    }
}

#[cfg(test)]
pub(crate) fn generate_for_tests(
    cache_dir: &std::path::Path,
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> anyhow::Result<gdk_pixbuf::Pixbuf> {
    generate(cache_dir, uri, size, mtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db;
    use crate::core::media::mime_from_extension;
    use crate::core::orientation;
    use crate::core::thumbnails::jpeg_turbo::decode_jpeg_scaled;
    use crate::core::thumbnails::queue::pull_batch_and_enqueue;
    use crate::core::thumbnails::video::extract_video_frame;
    use gtk4::prelude::TextureExt;
    use image::ImageEncoder;
    use std::fs::File;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct CapturedLog(Arc<Mutex<Vec<u8>>>);

    struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedLogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
        type Writer = CapturedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CapturedLogWriter(self.0.clone())
        }
    }

    fn assert_log_message_uses_macro(source: &str, message: &str, expected_macro: &str) {
        let message_index = source
            .find(message)
            .unwrap_or_else(|| panic!("missing log message {message}"));
        let before = &source[..message_index];
        let candidates = ["debug!(", "info!(", "warn!("];
        let actual_macro = candidates
            .iter()
            .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, candidate)| candidate)
            .expect("log message should be inside a tracing macro");
        assert_eq!(
            actual_macro, expected_macro,
            "{message} should use {expected_macro} to stay out of default logs"
        );
    }

    fn thumbnail_production_sources() -> String {
        let root = include_str!("thumbnails.rs");
        let mut production_source = root
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("thumbnails.rs must contain production code")
            .to_string();
        production_source.push_str(include_str!("thumbnails/queue.rs"));
        production_source.push_str(include_str!("thumbnails/decode.rs"));
        production_source.push_str(include_str!("thumbnails/video.rs"));
        production_source
    }

    #[test]
    fn high_frequency_thumbnail_progress_logs_stay_debug() {
        let production_source = thumbnail_production_sources();

        for message in [
            "THUMB disk_cache_hit",
            "THUMB video_generated",
            "THUMB image_generated",
            "VIDEO_THUMB ffmpegthumbnailer 失败，回退 GStreamer",
            "VIDEO_THUMB ffmpegthumbnailer 提取成功",
            "VIDEO_THUMB 提取视频帧(GStreamer)",
            "VIDEO_THUMB 提取成功",
        ] {
            assert_log_message_uses_macro(&production_source, message, "debug!(");
        }
    }

    #[test]
    fn per_thumbnail_trace_spans_stay_debug() {
        let production_source = thumbnail_production_sources();

        for span_name in [
            "thumb:process",
            "thumb:pb_decode",
            "thumb:pb_scale",
            "thumb:pb_save",
        ] {
            let quoted_span_name = format!("\"{span_name}\"");
            let mut search_from = 0;
            let mut found = false;
            while let Some(relative_index) =
                production_source[search_from..].find(&quoted_span_name)
            {
                found = true;
                let span_index = search_from + relative_index;
                let before = &production_source[..span_index];
                let actual_macro = ["tracing::debug_span!(", "tracing::info_span!("]
                    .iter()
                    .filter_map(|candidate| {
                        before.rfind(candidate).map(|index| (index, *candidate))
                    })
                    .max_by_key(|(index, _)| *index)
                    .map(|(_, candidate)| candidate)
                    .expect("span should be inside a tracing span macro");
                assert_eq!(
                    actual_macro, "tracing::debug_span!(",
                    "{span_name} is per-thumbnail tracing and should stay out of default INFO logs"
                );
                search_from = span_index + quoted_span_name.len();
            }
            assert!(found, "missing thumbnail span {span_name}");
        }

        let generate_index = production_source
            .find("name = \"thumb:generate\"")
            .expect("missing thumb:generate instrumentation");
        let attr_start = production_source[..generate_index]
            .rfind("#[tracing::instrument")
            .expect("thumb:generate should use tracing::instrument");
        let attr_end = production_source[generate_index..]
            .find(")]")
            .map(|end| generate_index + end + ")]".len())
            .expect("thumb:generate instrument attribute should close");
        let attr = &production_source[attr_start..attr_end];
        assert!(
            attr.contains("level = \"debug\""),
            "thumb:generate is per-thumbnail tracing and should be debug-level"
        );
    }

    #[test]
    fn request_for_missing_source_drops_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = ThumbnailLoader::new(pool, dir.path().join("cache"));

        let (tx, rx) = oneshot::channel();
        // 源文件不存在（且未给 mtime）→ request 无法计算缓存键 → reply 被 drop
        // → rx 收到 Err，既不 panic 也不让调用方永久挂起。
        loader.request(
            "file:///does/not/exist.jpg".into(),
            ThumbnailSize::Small,
            None,
            tx,
            TIER_NORMAL,
        );

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            rt.block_on(rx).is_err(),
            "missing source should drop the reply, not hang"
        );
    }

    /// 回归 gdk-pixbuf 缩略图生成路径（现为主路径）。
    /// HEIC 在 host 上不一定有 heif loader，故用 PNG（gdk-pixbuf 必带 loader）
    /// 做确定性验证：`generate_via_pixbuf` 解码 → 等比缩放 → 存 JPEG 必须可用。
    #[test]
    fn pixbuf_fallback_generates_jpeg_from_png() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.png");
        let img = image::RgbImage::from_pixel(400, 300, image::Rgb([10, 20, 30]));
        image::DynamicImage::ImageRgb8(img).save(&src).unwrap();

        let out = dir.path().join("out");
        generate_via_pixbuf(&src, 256, &out).expect("gdk-pixbuf 回退应成功");
        let jpeg = out.with_extension("jpg");

        assert!(jpeg.exists(), "应写出 JPEG 缩略图");
        let decoded = image::open(&jpeg).expect("输出的 JPEG 应可被重新解码");
        let (w, h) = (decoded.width(), decoded.height());
        assert!(w <= 256 && h <= 256, "应在 max_dim 内, got {w}x{h}");
        assert_eq!(w.max(h), 256, "长边应正好缩到 max_dim");
    }

    /// 回归：RGBA PNG（截图）必须能生成缩略图,且不能把透明像素合成到白底。
    /// 透明图走 WebP 缓存，避免 viewer / grid 里出现白边。
    #[test]
    fn generate_via_pixbuf_handles_rgba_png() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("rgba.png");
        let img = image::RgbaImage::from_pixel(400, 300, image::Rgba([10, 20, 30, 128]));
        image::DynamicImage::ImageRgba8(img).save(&src).unwrap();

        let out = dir.path().join("out");
        generate_via_pixbuf(&src, 256, &out).expect("RGBA PNG 应能生成 WebP 缩略图");
        assert!(out.with_extension("webp").exists(), "应写出 WebP 缩略图");
    }

    #[test]
    fn gif_content_with_jpg_suffix_skips_turbojpeg_warning() {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/media/gif_with_jpg_extension.jpg");
        assert_eq!(mime_from_extension(&src), Some("image/jpeg"));

        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(CapturedLog(captured.clone()))
            .finish();

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("misnamed-gif");
        let thumb = tracing::subscriber::with_default(subscriber, || {
            generate_via_pixbuf(&src, 256, &out)
                .expect("GIF content with .jpg suffix should generate a thumbnail")
        });

        assert!(thumb.width() > 0);
        let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(
            !logs.contains("THUMB jpeg_shim_decode_failed")
                && !logs.contains("THUMB turbojpeg_fallback"),
            "misnamed GIF should not enter JPEG fast-path fallback, got logs: {logs}"
        );
    }

    #[test]
    fn unavailable_video_thumbnail_is_cached_as_jpeg_without_play_triangle() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("video");

        let thumb = generate_unavailable_placeholder(256, &out, true)
            .expect("video unavailable placeholder should generate");

        assert!(
            out.with_extension("jpg").exists(),
            "placeholder should be cached"
        );
        assert_eq!(thumb.width(), 256);
        assert_eq!(thumb.height(), 144);

        let bytes = thumb.read_pixel_bytes();
        let buf: &[u8] = bytes.as_ref();
        let rowstride = thumb.rowstride() as usize;
        let channels = thumb.n_channels() as usize;
        let center =
            (thumb.height() as usize / 2) * rowstride + (thumb.width() as usize / 2) * channels;
        assert!(
            buf[center] > 180 && buf[center + 1] < 140 && buf[center + 2] < 130,
            "center should carry the unavailable slash accent, not a white play triangle"
        );
    }

    #[test]
    fn image_decode_failure_generates_unavailable_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("broken.jpg");
        std::fs::write(&src, b"not an image").unwrap();
        let cache_dir = dir.path().join("cache");
        let uri = format!("file://{}", src.display());

        let thumb = generate(&cache_dir, &uri, ThumbnailSize::Small, None)
            .expect("broken images should still return a visible unavailable thumbnail");

        assert_eq!(thumb.width(), 256);
        assert_eq!(thumb.height(), 256);
        let cached: Vec<_> = std::fs::read_dir(cache_dir.join("thumbnails/small"))
            .expect("thumbnail cache directory should exist")
            .flat_map(|bucket| {
                let bucket = bucket.expect("bucket entry should read");
                std::fs::read_dir(bucket.path()).expect("bucket should read")
            })
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        assert!(
            cached
                .iter()
                .any(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jpg")),
            "unavailable image thumbnail should be cached as JPEG"
        );
    }

    #[test]
    fn loader_returns_existing_disk_cache_without_queueing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("cached.png");
        let img = image::RgbaImage::from_pixel(20, 20, image::Rgba([10, 20, 30, 255]));
        image::DynamicImage::ImageRgba8(img).save(&src).unwrap();
        let cache_dir = dir.path().join("cache");
        let uri = format!("file://{}", src.display());
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        generate(&cache_dir, &uri, ThumbnailSize::Small, None)
            .expect("test should pre-create a disk thumbnail cache");

        let loader = ThumbnailLoader::new(pool, cache_dir);
        let cached = loader
            .try_load_cached(&uri, ThumbnailSize::Small, None)
            .expect("existing disk cache should load synchronously");

        assert!(cached.texture.width() <= 256);
        assert!(cached.texture.height() <= 256);
        assert_eq!(loader.queue_len(), 0);
        assert_eq!(loader.in_flight_len(), 0);
    }

    #[test]
    fn generate_replaces_empty_disk_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.png");
        let img = image::RgbImage::from_pixel(80, 60, image::Rgb([10, 20, 30]));
        image::DynamicImage::ImageRgb8(img).save(&src).unwrap();
        let cache_dir = dir.path().join("cache");
        let uri = format!("file://{}", src.display());
        let cache_stem = cache_stem_for(&cache_dir, &uri, ThumbnailSize::Small, None).unwrap();
        std::fs::create_dir_all(cache_stem.parent().unwrap()).unwrap();
        let cache_path = cache_stem.with_extension("jpg");
        File::create(&cache_path).unwrap();

        let thumb = generate(&cache_dir, &uri, ThumbnailSize::Small, None)
            .expect("empty cache files should be discarded and regenerated");

        assert!(thumb.width() > 0);
        assert!(
            std::fs::metadata(&cache_path).unwrap().len() > 0,
            "regenerated cache file should not be empty"
        );
    }

    #[test]
    fn background_pull_marks_returned_item_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        let src = dir.path().join("source.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            80,
            60,
            image::Rgb([10, 20, 30]),
        ))
        .save(&src)
        .unwrap();
        let now = chrono::Utc::now();
        let item = crate::core::media::NewMediaItem {
            uri: format!("file://{}", src.display()),
            path: src.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/png".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(80),
            height: Some(60),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: now,
            file_size: std::fs::metadata(&src).unwrap().len(),
            blake3_hash: "hash".into(),
        };
        db::insert_media_item(&pool, &item).unwrap();
        let loader = ThumbnailLoader::new(pool.clone(), dir.path().join("cache"));
        loader
            .background_pull
            .enabled
            .store(true, AtomicOrdering::Relaxed);
        *loader.background_pull.worker_count.lock().unwrap() = 1;

        let first =
            pull_batch_and_enqueue(&pool, &loader.background_pull, &loader.queue, &loader.state)
                .expect("first background pull should return the pending item");
        let second =
            pull_batch_and_enqueue(&pool, &loader.background_pull, &loader.queue, &loader.state);

        assert!(
            second.is_none(),
            "a background item already returned to a worker must be considered in-flight"
        );
        assert!(
            loader
                .state
                .lock()
                .unwrap()
                .in_flight
                .contains_key(&first.cache_key),
            "returned background key should be registered for duplicate suppression"
        );
    }

    /// 重定向预热起点后，下一次后台拉取应从该全局 DESC 偏移取，而非默认 0。
    /// 用户跳到任意区域时，预热要跟随当前浏览位置，而不是一直从最新推进。
    #[test]
    fn redirect_prewarm_to_offset_retargets_pull() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

        // 插入 5 张 taken_at 严格递增的图，使 DESC 全局顺序确定：
        // offset 0 = 最新(i=4)，offset 4 = 最旧(i=0)。
        let base = chrono::Utc::now();
        let mut items = Vec::new();
        for i in 0..5u32 {
            let src = dir.path().join(format!("src{i}.png"));
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                8,
                8,
                image::Rgb([i as u8, 0, 0]),
            ))
            .save(&src)
            .unwrap();
            let len = std::fs::metadata(&src).unwrap().len();
            let item = crate::core::media::NewMediaItem {
                uri: format!("file://{}", src.display()),
                path: src,
                folder_path: dir.path().to_path_buf(),
                mime_type: "image/png".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(8),
                height: Some(8),
                video_duration_secs: None,
                taken_at: Some(base + chrono::Duration::seconds(i as i64)),
                file_mtime: base,
                file_size: len,
                blake3_hash: format!("hash{i}"),
            };
            db::insert_media_item(&pool, &item).unwrap();
            items.push(item);
        }

        let loader = ThumbnailLoader::new(pool.clone(), dir.path().join("cache"));
        loader
            .background_pull
            .enabled
            .store(true, AtomicOrdering::Relaxed);
        *loader.background_pull.worker_count.lock().unwrap() = 1;

        // 重定向到 offset 3（即第 4 新 = i=1）。
        loader.redirect_prewarm_to_offset(3);

        let pulled =
            pull_batch_and_enqueue(&pool, &loader.background_pull, &loader.queue, &loader.state)
                .expect("重定向后应从 offset 3 拉到一条");

        // offset 0 本会返回最新项 items[4]；重定向到 3 应返回 items[1]。
        assert_ne!(
            pulled.uri, items[4].uri,
            "重定向后不应再从 offset 0（最新）拉取"
        );
        assert_eq!(
            pulled.uri, items[1].uri,
            "redirect_prewarm_to_offset(3) 应拉取全局 DESC offset 3 处的项"
        );
    }

    /// 重定向使用的是全局 live-media offset，不应被解释成“待生成缩略图集合”的
    /// offset。当前位置之前如果已有缩略图，预热仍应从当前位置附近的冷项开始。
    #[test]
    fn redirect_prewarm_to_offset_uses_live_media_offset_after_generated_rows() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

        let base = chrono::Utc::now();
        let mut items = Vec::new();
        let mut ids = Vec::new();
        for i in 0..5u32 {
            let src = dir.path().join(format!("src{i}.png"));
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                8,
                8,
                image::Rgb([i as u8, 0, 0]),
            ))
            .save(&src)
            .unwrap();
            let len = std::fs::metadata(&src).unwrap().len();
            let item = crate::core::media::NewMediaItem {
                uri: format!("file://{}", src.display()),
                path: src,
                folder_path: dir.path().to_path_buf(),
                mime_type: "image/png".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(8),
                height: Some(8),
                video_duration_secs: None,
                taken_at: Some(base + chrono::Duration::seconds(i as i64)),
                file_mtime: base,
                file_size: len,
                blake3_hash: format!("hash{i}"),
            };
            let id = db::insert_media_item(&pool, &item).unwrap();
            items.push(item);
            ids.push(id);
        }

        // 全局 DESC 顺序为 i=4,3,2,1,0。把当前位置之前的 4/3/2 标记为已生成，
        // 此时“待生成集合”的 offset 0 是 i=1；但全局 live offset 3 仍是 i=1。
        for id in [ids[4], ids[3], ids[2]] {
            db::set_thumbnail_generated_at_for_tests(&pool, id, base.timestamp() + 1).unwrap();
        }

        let loader = ThumbnailLoader::new(pool.clone(), dir.path().join("cache"));
        loader
            .background_pull
            .enabled
            .store(true, AtomicOrdering::Relaxed);
        *loader.background_pull.worker_count.lock().unwrap() = 1;

        loader.redirect_prewarm_to_offset(3);

        let pulled =
            pull_batch_and_enqueue(&pool, &loader.background_pull, &loader.queue, &loader.state)
                .expect("全局 offset 3 附近仍有待生成缩略图");

        assert_eq!(
            pulled.uri, items[1].uri,
            "redirect offset 应按全局 live-media 顺序定位，而不是按待生成集合重新 offset"
        );
    }

    #[test]
    fn alpha_thumbnail_is_cached_as_webp_without_white_edges() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("transparent-border.png");
        let mut img = image::RgbaImage::from_pixel(400, 300, image::Rgba([0, 0, 0, 0]));
        for y in 40..260 {
            for x in 50..350 {
                img.put_pixel(x, y, image::Rgba([10, 20, 30, 255]));
            }
        }
        image::DynamicImage::ImageRgba8(img).save(&src).unwrap();

        let out = dir.path().join("out");
        let thumb = generate_via_pixbuf(&src, 256, &out).expect("透明 PNG 应能生成 WebP 缩略图");
        let webp = out.with_extension("webp");

        assert!(webp.exists(), "带 alpha 的缩略图应写出 WebP 缓存");
        assert!(thumb.has_alpha(), "内存缩略图应保留 alpha");
        // Use the bundled `image` crate (with `image-webp`) to read the WebP
        // back. The gdk-pixbuf WebP loader is a separate system package and
        // isn't guaranteed to be installed in every headless CI environment;
        // the `image` decoder is linked into the binary and is sufficient to
        // verify the file we wrote round-trips and still preserves alpha.
        let decoded = image::open(&webp).expect("WebP 缓存应能被 image 解码");
        let rgba = decoded.to_rgba8();
        assert_eq!(
            rgba.width() as i32,
            thumb.width(),
            "读回的 WebP 尺寸应与 pixbuf 缩略图一致"
        );
        assert_eq!(rgba.height() as i32, thumb.height());
        let first = rgba.get_pixel(0, 0);
        assert_eq!(
            first.0[3], 0,
            "WebP 解码后透明边缘的 alpha 应仍为 0，不应被合成成白色不透明像素"
        );
    }

    /// `ensure_opaque` 契约：带 alpha 的输入必须返回无 alpha 的等尺寸 pixbuf，
    /// 无 alpha 的输入原样返回。确定性，不依赖具体 JPEG saver 行为。
    #[test]
    fn ensure_opaque_strips_alpha() {
        let rgba = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 40, 30).unwrap();
        let opaque = ensure_opaque(&rgba);
        assert!(!opaque.has_alpha(), "RGBA 经 ensure_opaque 后应无 alpha");
        assert_eq!((opaque.width(), opaque.height()), (40, 30));

        let rgb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 40, 30).unwrap();
        let same = ensure_opaque(&rgb);
        assert!(!same.has_alpha(), "无 alpha 输入应保持无 alpha");
    }

    /// 回归 `scale_pixbuf_to_fit`：缩入 max_dim 内且不放大。
    #[test]
    fn scale_pixbuf_to_fit_fits_and_never_upscales() {
        let big = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 400, 300).unwrap();
        let s = scale_pixbuf_to_fit(&big, 256);
        assert!(s.width() <= 256 && s.height() <= 256);
        assert_eq!(s.width().max(s.height()), 256, "长边应缩到 max_dim");

        let small = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 50, 40).unwrap();
        let s2 = scale_pixbuf_to_fit(&small, 256);
        assert_eq!((s2.width(), s2.height()), (50, 40), "小图不应被放大");
    }

    /// `PriItem` 全序：tier 小者先；同 tier 按 seq 升序（FIFO）。
    #[test]
    fn priitem_orders_by_tier_then_seq() {
        let mk = |tier, seq| PriItem {
            tier,
            seq,
            cache_key: "k".into(),
            uri: "u".into(),
            size: ThumbnailSize::Small,
            mtime: None,
            enqueued_at: Instant::now(),
            media_id: 0,
        };
        // BOOST(tier0) < NORMAL(tier1)
        assert!(mk(TIER_BOOST, 100) < mk(TIER_NORMAL, 1));
        // 同 tier：seq 小者先
        assert!(mk(TIER_NORMAL, 1) < mk(TIER_NORMAL, 2));
        assert!(mk(TIER_BOOST, 1) < mk(TIER_BOOST, 2));
    }

    /// `Reverse<PriItem>` 在 `BinaryHeap` 中弹出最小 (tier, seq)。
    #[test]
    fn heap_pops_highest_priority_first() {
        let mk = |tier, seq, k: &str| {
            Reverse(PriItem {
                tier,
                seq,
                cache_key: k.into(),
                uri: k.into(),
                size: ThumbnailSize::Small,
                mtime: None,
                enqueued_at: Instant::now(),
                media_id: 0,
            })
        };
        let mut heap = BinaryHeap::new();
        heap.push(mk(TIER_NORMAL, 1, "a")); // 先入队 a(NORMAL)
        heap.push(mk(TIER_NORMAL, 2, "b")); // 后入队 b(NORMAL)
        heap.push(mk(TIER_BOOST, 3, "b")); // b 被提权（新堆项）
        assert_eq!(heap.pop().unwrap().0.cache_key, "b", "BOOST 的 b 应先出");
        assert_eq!(heap.pop().unwrap().0.cache_key, "a", "再出 NORMAL 的 a");
        assert_eq!(
            heap.pop().unwrap().0.cache_key,
            "b",
            "最后弹出 b 的过期 NORMAL 项"
        );
    }

    /// `overlay_play_icon` 在 pixbuf 左下角绘制半透明背景 + 白色三角形。
    #[test]
    fn overlay_play_icon_modifies_pixels() {
        let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 200, 150).unwrap();
        pb.fill(0x808080ff); // 灰色填充
        let result = overlay_play_icon(&pb);
        // overlay 后尺寸不变
        assert_eq!(result.width(), 200);
        assert_eq!(result.height(), 150);
        // 左下角区域像素应被修改（不再是纯灰）
        let bytes = result.read_pixel_bytes();
        let buf: &[u8] = bytes.as_ref();
        let rowstride = result.rowstride() as usize;
        // 采样左下角附近一点
        let sample_y = 150 - 20;
        let sample_x = 20;
        let i = sample_y * rowstride + sample_x * 3;
        assert!(i + 2 < buf.len());
        // 像素值应与原始灰色 (128,128,128) 不同
        assert!(
            buf[i] != 128 || buf[i + 1] != 128 || buf[i + 2] != 128,
            "左下角像素应被 overlay 修改"
        );
    }

    /// `overlay_play_icon` 对过小的 pixbuf 不做修改。
    #[test]
    fn overlay_play_icon_skips_tiny_pixbuf() {
        let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 10, 10).unwrap();
        pb.fill(0x808080ff);
        let result = overlay_play_icon(&pb);
        let bytes_orig = pb.read_pixel_bytes();
        let bytes_result = result.read_pixel_bytes();
        assert_eq!(
            bytes_orig.as_ref(),
            bytes_result.as_ref(),
            "10x10 pixbuf 不应被修改"
        );
    }

    #[test]
    fn ffmpeg_thumbnail_temp_path_includes_requested_size() {
        let path = std::path::Path::new("/tmp/video.mp4");

        assert_ne!(
            ffmpeg_thumbnail_temp_path(path, 256),
            ffmpeg_thumbnail_temp_path(path, 1024),
            "parallel video thumbnail requests for different buckets must not share one temp output"
        );
    }

    /// 回归：`ffmpegthumbnailer -f` 会给缩略图加一层"电影胶片"装饰（左右白齿孔 +
    /// 上下黑边），让所有视频缩略图都长成胶片框。修复后 `-f` 已移除，抽出的帧应直接
    /// 是视频内容本身——左侧第一列应该出现视频颜色，而不是一整列纯黑。
    ///
    /// 跑法：`cargo test -p photo-viewer extract_video_frame_ffmpeg_does_not_add_movie_strip_overlay -- --nocapture`
    /// 覆盖路径：`VIDEO_TEST_FILE=/path/to/some.mp4 cargo test ...`
    #[test]
    fn extract_video_frame_ffmpeg_does_not_add_movie_strip_overlay() {
        let path = video_fixture_path();
        if !path.exists() {
            eprintln!("跳过：找不到测试视频 {}", path.display());
            return;
        }
        let pb = match extract_video_frame_ffmpeg(&path, 256) {
            Ok(pb) => pb,
            Err(e) => {
                // 主机若没装 ffmpegthumbnailer 也无法验证：跳过而不是失败。
                eprintln!("跳过：extract_video_frame_ffmpeg 失败 {e}");
                return;
            }
        };

        let bytes = pb.read_pixel_bytes();
        let buf: &[u8] = bytes.as_ref();
        let rowstride = pb.rowstride() as usize;
        let channels = pb.n_channels() as usize;
        let w = pb.width() as usize;
        let h = pb.height() as usize;
        assert!(channels >= 3, "pixbuf 至少需要 RGB 三通道");

        // `-f` 装饰的胶片框会让图像左侧整列（外加右侧对称列）变成纯黑 0,0,0。
        // 统计 x=0 这一列的纯黑像素占比：装饰模式下应 ≈100%；正常视频帧应远低于此。
        let mut black_count = 0usize;
        for y in 0..h {
            let i = y * rowstride;
            if i + 2 < buf.len() && buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 0 {
                black_count += 1;
            }
        }
        eprintln!(
            "左侧 (x=0) 列像素统计: {}/{} 为纯黑 ({:.1}%)",
            black_count,
            h,
            100.0 * black_count as f64 / h as f64
        );
        assert!(
            black_count * 2 < h,
            "左侧整列几乎全是纯黑 ({} / {}) ——ffmpegthumbnailer 又被传入了 -f (胶片装饰) 选项",
            black_count,
            h
        );

        // 同时校验顶部一行也不该被胶片框的黑色横条占满：抽样中间一段像素，
        // 至少应有多种非黑颜色（真实视频帧的顶部有树叶/天空/物件等）。
        let mid = h / 2;
        let mut distinct = std::collections::HashSet::new();
        for x in (w / 4)..(3 * w / 4) {
            let i = mid * rowstride + x * channels;
            if i + 2 < buf.len() {
                distinct.insert((buf[i], buf[i + 1], buf[i + 2]));
            }
        }
        assert!(
            distinct.len() >= 4,
            "图像中段颜色种类过少 ({} 种)，缩略图可能仍被胶片框覆盖",
            distinct.len()
        );
        // Sanity: image must have non-trivial size.
        let _ = w;
    }

    /// 视频帧提取端到端测试。默认使用仓库内真实视频；设置
    /// `VIDEO_TEST_FILE` 可覆盖为其他视频。
    #[test]
    fn extract_video_frame_from_file() {
        let path = video_fixture_path();
        let result = extract_video_frame(&path, 256);
        match result {
            Ok(pb) => {
                eprintln!("成功! 帧尺寸: {}x{}", pb.width(), pb.height());
                assert!(pb.width() > 0 && pb.height() > 0);
            }
            Err(e) => {
                panic!("extract_video_frame 失败: {e}");
            }
        }
    }

    /// 保存视频帧到文件以便对比。默认使用仓库内真实视频；设置
    /// `VIDEO_TEST_FILE` 可覆盖为其他视频。
    #[test]
    fn save_video_frame() {
        let path = video_fixture_path();
        let result = extract_video_frame(&path, 1024);
        match result {
            Ok(pb) => {
                pb.savev("/tmp/test_video_frame.jpg", "jpeg", &[("quality", "90")])
                    .expect("保存帧失败");
                eprintln!("帧已保存到 /tmp/test_video_frame.jpg");
                eprintln!("帧尺寸: {}x{}", pb.width(), pb.height());
            }
            Err(e) => {
                panic!("extract_video_frame 失败: {e}");
            }
        }
    }

    /// 测试从 MP4 文件读取旋转信息。默认使用仓库内真实视频；设置
    /// `VIDEO_TEST_FILE` 可覆盖为其他视频。
    #[test]
    fn read_video_rotation_from_mp4() {
        let path = video_fixture_path();
        let rotation = read_video_rotation(&path);
        eprintln!("视频旋转: {} rotation={}", path.display(), rotation);
        assert!(rotation == 0 || rotation == 90 || rotation == 180 || rotation == 270);
    }

    fn video_fixture_path() -> std::path::PathBuf {
        std::env::var("VIDEO_TEST_FILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests")
                    .join("fixtures")
                    .join("media")
                    .join("real_phone_video.mp4")
            })
    }

    /// 缩略图生成性能分析：对模拟真实手机照片（12MP / 24MP / 48MP）的图像，
    /// 分别测量各阶段耗时以判断瓶颈在 IO 还是 CPU。
    ///
    /// 输出到 stderr，用 `cargo test profile_generate_phases -- --nocapture` 查看。
    #[test]
    fn profile_generate_phases() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();

        let resolutions: &[(&str, u32, u32)] = &[
            ("12MP", 4032, 3024),
            ("24MP", 6048, 4032),
            ("48MP", 8000, 6000),
        ];

        for (label, w, h) in resolutions {
            let t_gen = Instant::now();

            // 生成模拟 JPEG 源文件
            let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(*w, *h, |x, y| {
                let r = ((x.wrapping_mul(y).wrapping_add(x)) % 251) as u8;
                let g = ((y.wrapping_mul(3).wrapping_add(x)) % 241) as u8;
                let b = ((x.wrapping_add(y).wrapping_mul(2)) % 231) as u8;
                image::Rgb([r, g, b])
            }));
            let src_path = dir.path().join(format!("test_{}x{}.jpg", w, h));
            let mut buf = std::io::Cursor::new(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92)
                .write_image(img.as_bytes(), *w, *h, image::ExtendedColorType::Rgb8)
                .unwrap();
            std::fs::write(&src_path, buf.into_inner()).unwrap();
            let src_mb = std::fs::metadata(&src_path).unwrap().len() as f64 / 1_048_576.0;
            let t_gen_done = Instant::now();
            let gen_file_ms = t_gen_done.duration_since(t_gen).as_millis();
            eprintln!("{label} 生成测试文件 {w}x{h} {src_mb:.1}MB: {gen_file_ms}ms");

            // 预热
            let warmup_stem = cache_dir.join("warmup");
            let _ = generate_via_pixbuf(&src_path, 512, &warmup_stem);

            // ── 冷路径：完整生成（读源文件 → 解码 → 缩放 → 编码写缓存）──
            let stem = cache_dir.join(format!("profile_{}x{}", w, h));
            let _ = std::fs::remove_file(stem.with_extension("jpg"));
            let _ = std::fs::remove_file(stem.with_extension("webp"));

            // 阶段 1a: 纯 IO（读源文件到内存，不解码）
            let t_io0 = Instant::now();
            let raw_bytes = std::fs::read(&src_path).expect("should read source file");
            let pure_read_ms = t_io0.elapsed().as_millis();
            let pure_read_mbps =
                (raw_bytes.len() as f64 / 1_048_576.0) / (pure_read_ms as f64 / 1000.0);
            eprintln!(
                "{label} 纯IO读 {:.1}MB: {pure_read_ms}ms ({pure_read_mbps:.0}MB/s)",
                raw_bytes.len() as f64 / 1_048_576.0
            );

            // 阶段 1b: 读源文件 + gdk-pixbuf 解码
            let t0 = Instant::now();
            let pb = orientation::load_oriented_pixbuf(&src_path)
                .expect("gdk-pixbuf should decode source");
            let decode_ms = t0.elapsed().as_millis();
            let jpeg_decode_cpu_ms = decode_ms.saturating_sub(pure_read_ms);
            eprintln!(
                "{label} 读+解码: {decode_ms}ms (其中IO≈{pure_read_ms}ms, JPEG解码≈{jpeg_decode_cpu_ms}ms)",
            );

            // 阶段 2: 缩放到目标尺寸
            let t1 = Instant::now();
            let scaled = scale_pixbuf_to_fit(&pb, 512);
            let scale_ms = t1.elapsed().as_millis();

            // 阶段 3: 编码 JPEG + 写入磁盘缓存
            let t2 = Instant::now();
            let cache_path = stem.with_extension("jpg");
            let thumb = ensure_opaque(&scaled);
            thumb
                .savev(&cache_path, "jpeg", &[])
                .expect("should save JPEG cache");
            let save_ms = t2.elapsed().as_millis();

            let total_ms = t0.elapsed().as_millis();
            let src_size = std::fs::metadata(&src_path).unwrap().len();
            let cache_size = std::fs::metadata(&cache_path).unwrap().len();
            let read_mbps = (src_size as f64 / 1_048_576.0) / (decode_ms as f64 / 1000.0);
            let write_mbps = (cache_size as f64 / 1_048_576.0) / (save_ms as f64 / 1000.0);

            eprintln!(
                "{label} [旧路径全分辨率] {w}x{h} src={src_mb:.1}MB → {tx}x{th}: \
                 decode={decode_ms}ms ({read_mbps:.0}MB/s) \
                 scale={scale_ms}ms \
                 save={save_ms}ms ({write_mbps:.0}MB/s) \
                 total={total_ms}ms",
                tx = scaled.width(),
                th = scaled.height()
            );

            // ── 新路径：generate_via_pixbuf（JPEG 走 turbojpeg IDCT 缩放）──
            let stem_tj = cache_dir.join(format!("profile_tj_{}x{}", w, h));
            let _ = std::fs::remove_file(stem_tj.with_extension("jpg"));
            let _ = std::fs::remove_file(stem_tj.with_extension("webp"));
            let t_tj = Instant::now();
            let tj_thumb =
                generate_via_pixbuf(&src_path, 512, &stem_tj).expect("turbojpeg 路径应成功");
            let tj_ms = t_tj.elapsed().as_millis();
            let speedup = total_ms as f64 / tj_ms.max(1) as f64;
            eprintln!(
                "{label} [新路径turbojpeg]  → {}x{}: total={}ms  (相对旧路径 {speedup:.1}×)",
                tj_thumb.width(),
                tj_thumb.height(),
                tj_ms
            );

            // 阶段 4: 热路径（读磁盘缓存 → 解码）
            let t3 = Instant::now();
            let _cached = load_pixbuf_sync(&stem_tj.with_extension("jpg"))
                .or_else(|_| load_pixbuf_sync(&stem_tj.with_extension("webp")))
                .expect("should load cached thumb");
            let cached_ms = t3.elapsed().as_millis();
            eprintln!("{label} 热路径 cached_load={cached_ms}ms\n");
        }
    }

    /// 回归：libjpeg IDCT 缩放解码路径必须可用，且输出尺寸正确。
    /// 对一张 2400×1800 的 JPEG，目标 max_dim=512 时应选 1/4 缩放（600×450），
    /// 再由 scale_pixbuf_to_fit 缩到 512px 长边。
    #[test]
    fn decode_jpeg_scaled_produces_valid_pixbuf() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (2400u32, 1800u32);
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            w,
            h,
            image::Rgb([200, 100, 50]),
        ));
        let src = dir.path().join("src.jpg");
        let mut buf = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90)
            .write_image(img.as_bytes(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&src, buf.into_inner()).unwrap();

        let pb = decode_jpeg_scaled(&src, 512, 1).expect("turbojpeg 路径应成功解码 JPEG");

        // 1/4 缩放：2400/4=600, 1800/4=450（无 EXIF 方向，原图）
        assert_eq!(pb.width(), 600, "1/4 IDCT 缩放后宽应为 600");
        assert_eq!(pb.height(), 450, "1/4 IDCT 缩放后高应为 450");
        assert!(!pb.has_alpha(), "JPEG 无 alpha");
        assert_eq!(pb.n_channels(), 3, "应输出 RGB 3 通道");
    }

    /// 回归：turbojpeg 路径的方向处理走 `apply_orientation_to_pixbuf`，
    /// orientation=6（90° 顺时针）应把 600×450 翻成 450×600。
    #[test]
    fn decode_jpeg_scaled_applies_orientation_6() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (2400u32, 1800u32);
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            w,
            h,
            image::Rgb([200, 100, 50]),
        ));
        let src = dir.path().join("src.jpg");
        let mut buf = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90)
            .write_image(img.as_bytes(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&src, buf.into_inner()).unwrap();

        let pb = decode_jpeg_scaled(&src, 512, 6).expect("turbojpeg 路径应成功解码 JPEG");

        // 1/4 缩放得 600×450，再经 orientation 6（90° CW）翻转为 450×600。
        assert_eq!(pb.width(), 450, "orientation 6 后宽高应交换");
        assert_eq!(pb.height(), 600, "orientation 6 后宽高应交换");
    }

    /// 端到端：JPEG 经 generate_via_pixbuf 走 turbojpeg 快速路径生成缩略图，
    /// 尺寸应在 max_dim 内、长边正好为 max_dim，且缓存文件可被重新解码。
    #[test]
    fn jpeg_thumbnail_via_turbojpeg_fast_path() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (4032u32, 3024u32); // 12MP
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 231) as u8])
        }));
        let src = dir.path().join("12mp.jpg");
        let mut buf = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92)
            .write_image(img.as_bytes(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&src, buf.into_inner()).unwrap();

        let stem = dir.path().join("thumb");
        let thumb =
            generate_via_pixbuf(&src, 256, &stem).expect("JPEG 应走 turbojpeg 路径生成缩略图");

        assert!(
            thumb.width() <= 256 && thumb.height() <= 256,
            "应在 max_dim 内"
        );
        assert_eq!(thumb.width().max(thumb.height()), 256, "长边应缩到 max_dim");

        let jpeg = stem.with_extension("jpg");
        assert!(jpeg.exists(), "应写出 JPEG 缓存");
        let decoded = image::open(&jpeg).expect("缓存 JPEG 应可被重新解码");
        assert!(decoded.width() <= 256 && decoded.height() <= 256);
    }

    /// 比较 turbojpeg 缩放解码 vs gdk-pixbuf 全分辨率解码的耗时。
    /// 输出到 stderr，用 `cargo test jpeg_decode_bench -- --nocapture --ignored` 查看。
    #[test]
    #[ignore]
    fn jpeg_decode_bench() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (8000u32, 6000u32); // 48MP
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 231) as u8])
        }));
        let src = dir.path().join("48mp.jpg");
        let mut buf = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92)
            .write_image(img.as_bytes(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&src, buf.into_inner()).unwrap();

        // turbojpeg IDCT 缩放解码
        let t0 = Instant::now();
        let tj_pb = decode_jpeg_scaled(&src, 512, 1).expect("turbojpeg 应成功");
        let tj_ms = t0.elapsed().as_millis();

        // gdk-pixbuf 全分辨率解码
        let t1 = Instant::now();
        let gp_pb = orientation::load_oriented_pixbuf(&src).expect("gdk-pixbuf 应成功");
        let gp_ms = t1.elapsed().as_millis();

        let speedup = gp_ms as f64 / tj_ms.max(1) as f64;
        eprintln!(
            "48MP 解码对比: turbojpeg(1/8缩放)={}ms → {}x{}, gdk-pixbuf(全分辨率)={}ms → {}x{}, 提速 {speedup:.1}×",
            tj_ms, tj_pb.width(), tj_pb.height(),
            gp_ms, gp_pb.width(), gp_pb.height()
        );
    }

    /// 真实库基准：扫描 `~/图片`（或 `PICTURES_BENCH_DIR` 覆盖）取最大的若干 JPEG，
    /// 分别用 turbojpeg 快速路径与旧的全分辨率路径生成 Medium(512) 缩略图，
    /// 汇总总耗时与平均提速。输出到 stderr，用
    /// `cargo test real_library_thumbnail_bench --lib --release -- --nocapture --ignored` 查看。
    #[test]
    #[ignore]
    fn real_library_thumbnail_bench() {
        use std::path::PathBuf;

        let lib_dir: PathBuf = std::env::var("PICTURES_BENCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
                    .join("图片")
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from("/dev/null"))
            });

        // 首选 $HOME/图片
        let home_pictures = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("图片"))
            .filter(|p| p.is_dir());
        let scan_root = home_pictures.unwrap_or(lib_dir);

        if !scan_root.is_dir() {
            eprintln!("跳过：找不到图片库目录 {}", scan_root.display());
            return;
        }

        // 收集 JPEG，按文件大小降序，取最大的 30 张
        let mut files: Vec<(PathBuf, u64)> = Vec::new();
        for entry in walkdir::WalkDir::new(&scan_root)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let is_jpeg = path
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("jpg") || x.eq_ignore_ascii_case("jpeg"))
                .unwrap_or(false);
            if !is_jpeg {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                files.push((path.to_path_buf(), meta.len()));
            }
        }
        files.sort_unstable_by_key(|&(_, size)| std::cmp::Reverse(size));
        let sample: Vec<&PathBuf> = files.iter().take(30).map(|(p, _)| p).collect();
        if sample.is_empty() {
            eprintln!("跳过：{} 下未找到 JPEG", scan_root.display());
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        eprintln!(
            "真实库基准：{} 中最大的 {} 张 JPEG（共 {} 个 JPEG）",
            scan_root.display(),
            sample.len(),
            files.len()
        );

        // 旧路径：全分辨率解码 → 缩放 → 存盘
        let mut old_total = 0u128;
        let mut old_pixels = 0u64;
        for (i, src) in sample.iter().enumerate() {
            let stem = tmp.path().join(format!("old_{i}"));
            let t0 = Instant::now();
            let pb = orientation::load_oriented_pixbuf(src).expect("decode");
            let scaled = scale_pixbuf_to_fit(&pb, 512);
            let thumb = ensure_opaque(&scaled);
            thumb
                .savev(stem.with_extension("jpg"), "jpeg", &[])
                .unwrap();
            let ms = t0.elapsed().as_millis();
            old_total += ms;
            old_pixels += (pb.width() as u64) * (pb.height() as u64);
            if i < 3 {
                eprintln!(
                    "  旧[#{i}] {} → {}x{} {}ms",
                    src.display(),
                    pb.width(),
                    pb.height(),
                    ms
                );
            }
        }

        // 新路径：turbojpeg IDCT 缩放（生产路径 generate_via_pixbuf）
        let mut new_total = 0u128;
        let mut new_pixels = 0u64;
        for (i, src) in sample.iter().enumerate() {
            let stem = tmp.path().join(format!("new_{i}"));
            let t0 = Instant::now();
            let ori = orientation::read_orientation(src).unwrap_or(1);
            let tj_pb = decode_jpeg_scaled(src, 512, ori).expect("turbojpeg decode");
            new_pixels += (tj_pb.width() as u64) * (tj_pb.height() as u64);
            let scaled = scale_pixbuf_to_fit(&tj_pb, 512);
            let thumb = ensure_opaque(&scaled);
            thumb
                .savev(stem.with_extension("jpg"), "jpeg", &[])
                .unwrap();
            let ms = t0.elapsed().as_millis();
            new_total += ms;
            if i < 3 {
                eprintln!(
                    "  新[#{i}] {} → {}x{} {}ms",
                    src.display(),
                    tj_pb.width(),
                    tj_pb.height(),
                    ms
                );
            }
        }

        let n = sample.len() as f64;
        let old_avg = old_total as f64 / n;
        let new_avg = new_total as f64 / n;
        let speedup = old_total as f64 / new_total.max(1) as f64;
        let old_mem_mb = old_pixels as f64 * 3.0 / 1_048_576.0;
        let new_mem_mb = new_pixels as f64 * 3.0 / 1_048_576.0;
        eprintln!(
            "\n汇总（{} 张，每张 Medium/512 缩略图）：\n  旧路径(全分辨率解码) 总 {}ms, 均 {:.0}ms/张, 解码像素总量 {:.0}MP ({:.0}MB)\n  新路径(turbojpeg缩放) 总 {}ms, 均 {:.0}ms/张, 解码像素总量 {:.0}MP ({:.0}MB)\n  提速 {speedup:.2}×, 解码像素缩减 {:.0}×, 内存占用从 {:.0}MB → {:.0}MB",
            sample.len(),
            old_total,
            old_avg,
            old_pixels as f64 / 1_000_000.0,
            old_mem_mb,
            new_total,
            new_avg,
            new_pixels as f64 / 1_000_000.0,
            new_mem_mb,
            old_pixels as f64 / new_pixels.max(1) as f64,
            old_mem_mb,
            new_mem_mb,
        );
    }
}
