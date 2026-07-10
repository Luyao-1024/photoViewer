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
mod tests;
