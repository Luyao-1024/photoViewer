//! 缩略图加载器：worker pool + 优先级队列 + 分桶磁盘缓存
//!
//! - 多个 tokio blocking worker 并行处理缩略图生成/读取
//! - 按 `path + mtime` 计算 blake3 哈希作为缓存键（mtime 变了自动失效）
//! - 缓存目录按 `thumbnails/{small|medium|large}/<hash 前两位>/<hash>.(jpg|webp)` 分桶
//! - 内存 LRU 缓存已加载的 `Texture`，避免重复解码
//! - **优先级队列**：可见 tile 可经 `prioritize_keys` 提到队首（BOOST），
//!   先于普通（NORMAL）请求被 worker 取走，消除分页 rebuild / 滚动时的优先级倒置。
use crate::core::db::DbPool;
use crate::core::media::{media_kind_from_mime, mime_from_extension, MediaKind};
use crate::core::orientation;
use gdk_pixbuf::Pixbuf;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gtk4::gdk::Texture;
use image::ImageEncoder;
use lru::LruCache;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::fs::File;
use std::io::BufWriter;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

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
struct PriItem {
    tier: u8,
    seq: u64,
    cache_key: String,
    uri: String,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
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
struct QueuedEntry {
    tier: u8,
    uri: String,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
}

/// 优先级队列的可变状态。
struct QueueState {
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
type SharedQueue = Arc<(Mutex<QueueState>, Condvar)>;

/// 加载器的可变缓存状态，用单一 Mutex 保护。
///
/// 把 `mem_cache` 与 `in_flight` 放在同一把锁后，request 端的
/// "查 mem_cache → 查 in_flight → 登记并入队" 与 worker 端的
/// "写 mem_cache → 取走等待者" 互斥执行，杜绝二者之间的竞态窗口
/// （否则一个刚完成的 key 可能被新请求当作未生成而重复入队）。
struct LoaderState {
    mem_cache: LruCache<String, LoadedThumb>,
    /// `cache_key` → 正在生成的请求的等待者列表。
    ///
    /// 同 key 的后续 request 直接 append 到这里、**不再单独入队**，因此：
    ///   - 同一张缩略图永远不会被重复生成；
    ///   - 重复请求永远不会因为队列满而被丢弃。
    in_flight: HashMap<String, Vec<oneshot::Sender<LoadedThumb>>>,
}

/// 后台预热拉取状态：worker 在队列为空时据此从 DB 拉取下一个需生成的项。
struct BackgroundPullState {
    enabled: AtomicBool,
    offset: Mutex<u32>,
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
    queue: SharedQueue,
    state: Arc<Mutex<LoaderState>>,
    background_pull: Arc<BackgroundPullState>,
}

/// 内存 LRU 容量。Large 档单张 texture 可达数 MB；过大的常驻缓存会在大图库滚动
/// 时把进程推到 GB 级内存。磁盘缓存仍保留 2GB 上限，内存只保留近期视口附近。
const MEM_CACHE_CAP: usize = 16;

impl ThumbnailLoader {
    /// 工作项队列容量。配合在途去重后，这里只存「彼此不同且未缓存」的项；
    /// 取一个充裕的值，使得在加入视口级虚拟化之前，单库数万张也能容纳。
    pub const QUEUE_CAPACITY: usize = 8192;

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
        let state = Arc::new(Mutex::new(LoaderState {
            mem_cache: LruCache::new(NonZeroUsize::new(MEM_CACHE_CAP).unwrap()),
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
        // 启动时按 mtime LRU 清理超限缓存（2GB 上限）。
        // 用裸线程异步执行，避免在首绘前于主线程上 walkdir 整个缓存目录 +
        // 逐文件 stat + 全量排序（数千文件时是可观的启动延迟）。这里不需要
        // tokio 运行时上下文，所以用 std 线程，在测试中 `new()` 也能安全调用。
        let cleanup_dir = cache_dir.clone();
        std::thread::spawn(move || {
            let _ = crate::core::cache::enforce_size_limit(
                &cleanup_dir.join("thumbnails"),
                2 * 1024 * 1024 * 1024,
            );
        });
        Self {
            pool,
            cache_dir,
            queue,
            state,
            background_pull: Arc::new(BackgroundPullState {
                enabled: AtomicBool::new(false),
                offset: Mutex::new(0),
            }),
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

    /// 启动 n 个 worker 消费请求
    pub fn spawn_workers(&self, n: usize) {
        if n == 0 {
            return;
        }
        for _ in 0..n {
            let pool = self.pool.clone();
            let cache_dir = self.cache_dir.clone();
            let queue = self.queue.clone();
            let state = self.state.clone();
            let bg = self.background_pull.clone();
            tokio::task::spawn_blocking(move || {
                worker_loop(queue, pool, cache_dir, state, bg);
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
        let Some(cache_key) = cache_key_str(&uri, size, mtime) else {
            // 源文件不存在 / 无法 stat：无法去重，按"生成失败"处理。
            warn!("缩略图请求无法计算缓存键（源文件缺失?）: {}", uri);
            return; // reply 被 drop → 调用方 rx 收到 Err
        };
        debug!(
            "THUMB_TRACE request uri={} size={:?} supplied_mtime={:?} cache_key={}",
            uri, size, mtime, cache_key
        );

        let mut st = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return, // poisoned
        };
        // 1) 内存命中
        if let Some(loaded) = st.mem_cache.get(&cache_key).cloned() {
            debug!(
                "THUMB_TRACE mem_cache_hit uri={} size={:?} cache_key={}",
                uri, size, cache_key
            );
            drop(st);
            let _ = reply.send(loaded);
            return;
        }
        // 2) 已在途 → 挂载等待者，不再入队
        if let Some(waiters) = st.in_flight.get_mut(&cache_key) {
            debug!(
                "THUMB_TRACE in_flight_join uri={} size={:?} cache_key={} waiters_before={}",
                uri,
                size,
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
            if q.queued.len() >= Self::QUEUE_CAPACITY {
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
                    },
                );
                q.heap.push(Reverse(PriItem {
                    tier,
                    seq,
                    cache_key: cache_key.clone(),
                    uri: uri.clone(),
                    size,
                    mtime,
                    media_id: 0,
                }));
                true
            }
        };
        if enqueued {
            debug!(
                "THUMB_TRACE enqueued uri={} size={:?} cache_key={}",
                uri, size, cache_key
            );
            cvar.notify_one();
        } else {
            // 队列已满且全是不同的未缓存项：回滚在途项，避免等待者被永久挂起。
            if let Ok(mut st) = self.state.lock() {
                st.in_flight.remove(&cache_key); // drop reply → 调用方 rx.Err
            }
            warn!("缩略图请求入队失败（队列已满于不同项）: {}", cache_key);
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
                media_id: 0,
            }));
            changed = true;
        }
        drop(q);
        if changed {
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

/// 每积攒多少条 BACKGROUND 生成的项就批量刷新一次 DB。
const BACKGROUND_DB_FLUSH_INTERVAL: usize = 100;

fn worker_loop(
    queue: SharedQueue,
    pool: DbPool,
    cache_dir: PathBuf,
    state: Arc<Mutex<LoaderState>>,
    bg: Arc<BackgroundPullState>,
) {
    let mut flush_ids: Vec<i64> = Vec::with_capacity(BACKGROUND_DB_FLUSH_INTERVAL);
    while let Some(req) = next_request_or_pull(&queue, &pool, &bg) {
        match generate(&cache_dir, &req.uri, req.size, req.mtime) {
            Ok(pb) => {
                // BACKGROUND 拉取项：记录 DB id 待批量刷新。
                if req.tier >= TIER_BACKGROUND && req.media_id != 0 {
                    flush_ids.push(req.media_id);
                }
                debug!(
                    "THUMB_TRACE worker_generated uri={} size={:?} tier={} media_id={} cache_key={} texture={}x{}",
                    req.uri,
                    req.size,
                    req.tier,
                    req.media_id,
                    req.cache_key,
                    pb.width(),
                    pb.height()
                );
                let is_light = pixbuf_is_light(&pb);
                let texture = Texture::for_pixbuf(&pb);
                let loaded = LoadedThumb {
                    texture: texture.clone(),
                    is_light,
                };
                let is_bg = req.tier >= TIER_BACKGROUND;
                let waiters = {
                    let mut st = match state.lock() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    if !is_bg {
                        st.mem_cache.put(req.cache_key.clone(), loaded.clone());
                    }
                    st.in_flight.remove(&req.cache_key).unwrap_or_default()
                };
                for w in waiters {
                    let _ = w.send(loaded.clone());
                }
            }
            Err(e) => {
                if req.tier < TIER_BACKGROUND {
                    drop_in_flight(&state, &req.cache_key);
                }
                warn!("缩略图生成失败 {}: {}", req.uri, e);
            }
        }
        // 批量刷 DB
        if flush_ids.len() >= BACKGROUND_DB_FLUSH_INTERVAL {
            if let Err(e) = crate::core::db::mark_thumbnails_generated(&pool, &flush_ids) {
                warn!("批量更新缩略图状态失败: {}", e);
            }
            flush_ids.clear();
        }
    }
    // worker 退出前尾量 flush
    if !flush_ids.is_empty() {
        if let Err(e) = crate::core::db::mark_thumbnails_generated(&pool, &flush_ids) {
            warn!("批量更新缩略图状态失败(退出): {}", e);
        }
    }
}

/// 取下一个工作项：优先队列（网格请求），队列空时从 DB 拉取下一个需生成的项。
fn next_request_or_pull(
    queue: &SharedQueue,
    pool: &DbPool,
    bg: &Arc<BackgroundPullState>,
) -> Option<PriItem> {
    let (lock, cvar) = &**queue;
    loop {
        // 1) 优先从队列弹（BOOST/NORMAL，网格可见请求）
        let mut q = lock.lock().ok()?;
        loop {
            if q.closed {
                return None;
            }
            if let Some(Reverse(item)) = q.heap.pop() {
                if q.queued.get(&item.cache_key).map(|e| e.tier) == Some(item.tier) {
                    q.queued.remove(&item.cache_key);
                    return Some(item);
                }
                continue; // 过期项
            }
            break; // 堆空
        }
        drop(q);

        // 2) 队列空，从 DB 拉取一条需要生成缩略图的项
        if bg.enabled.load(AtomicOrdering::Relaxed) {
            if let Some(item) = pull_next_needing_thumbnail(pool, bg) {
                return Some(item);
            }
        }

        // 3) 无可做，阻塞等待。
        let q = lock.lock().ok()?;
        if q.closed {
            return None;
        }
        if !q.heap.is_empty() {
            continue;
        }
        let wait_dur = if bg.enabled.load(AtomicOrdering::Relaxed) {
            std::time::Duration::from_millis(500)
        } else {
            std::time::Duration::from_secs(30)
        };
        let (q2, _timed_out) = cvar.wait_timeout(q, wait_dur).ok()?;
        drop(q2);
    }
}

/// 从 DB 拉取一条 `thumbnail_generated_at IS NULL OR thumbnail_generated_at < file_mtime`
/// 的项（即从未生成或文件变更后过期），交 worker 直接生成。
///
/// 已缓存（`thumbnail_generated_at >= file_mtime`）的项由 DB 查询自动过滤，
/// 不再需要磁盘 stat。拉取到末尾返回 `None`；下次超时重试时会因为已缓存项增加
/// 而自然收敛。
fn pull_next_needing_thumbnail(pool: &DbPool, bg: &BackgroundPullState) -> Option<PriItem> {
    let mut off = bg.offset.lock().ok()?;
    let page = crate::core::db::list_media_needing_thumbnail(pool, *off, 1).ok()?;
    if page.is_empty() {
        // 当前偏移后无更多需生成的项。重置偏移以便重扫（有新文件加入时能发现）。
        *off = 0;
        return None;
    }
    let item = &page[0];
    *off += 1;
    drop(off);

    let mtime = Some(std::time::SystemTime::from(item.file_mtime));
    let cache_key = cache_key_str(&item.uri, ThumbnailSize::Medium, mtime)
        .unwrap_or_else(|| format!("bg:{}", item.uri));
    Some(PriItem {
        tier: TIER_BACKGROUND,
        seq: 0,
        cache_key,
        uri: item.uri.clone(),
        size: ThumbnailSize::Medium,
        mtime,
        media_id: item.id,
    })
}

/// 生成失败时移除在途项，让等待者的 `rx` 收到 `Err` 而非永久挂起。
fn drop_in_flight(state: &Mutex<LoaderState>, cache_key: &str) {
    if let Ok(mut st) = state.lock() {
        st.in_flight.remove(cache_key);
    }
}

/// 解析 uri → 源路径 + mtime。`mtime` 优先用调用方给的（来自 `MediaItem.file_mtime`，
/// 避免主线程 stat）；否则现场 `metadata` + `modified()` 兜底。
fn resolve_src(uri: &str, mtime: Option<SystemTime>) -> anyhow::Result<(PathBuf, SystemTime)> {
    let path_str = uri.strip_prefix("file://").unwrap_or(uri);
    let src_path = PathBuf::from(path_str);
    let mtime = match mtime {
        Some(m) => m,
        None => std::fs::metadata(&src_path)?.modified()?,
    };
    Ok((src_path, mtime))
}

/// 与 worker 端一致的 mem-cache 键字符串（`{path:?}:{mtime:?}:{size:?}`）。
/// 在 request 端提前算好，用于 mem_cache 查询与在途去重；`mtime=None` 且源文件
/// 无法 stat 时返回 `None`（调用方据此把请求当作生成失败处理）。
fn cache_key_str(uri: &str, size: ThumbnailSize, mtime: Option<SystemTime>) -> Option<String> {
    let (path, mtime) = resolve_src(uri, mtime).ok()?;
    Some(format!("{path:?}:{mtime:?}:{size:?}"))
}

/// 同步加载缩略图缓存文件，确保文件完全写入后再解码。
///
/// 使用 `std::fs::read` 读取整个文件到内存，然后从内存构造 Pixbuf。
/// 这避免了 gdk-pixbuf 直接读取文件时可能遇到的竞态条件（文件被写入一半）。
fn load_pixbuf_sync(path: &Path) -> anyhow::Result<Pixbuf> {
    let data =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("读取缓存文件失败 {:?}: {}", path, e))?;
    if data.is_empty() {
        anyhow::bail!("缓存文件为空: {:?}", path);
    }
    let bytes = glib::Bytes::from(&data);
    let stream = gtk4::gio::MemoryInputStream::from_bytes(&bytes);
    Pixbuf::from_stream(&stream, None::<&gtk4::gio::Cancellable>)
        .map_err(|e| anyhow::anyhow!("缓存缩略图解码失败 {:?}: {}", path, e))
}

fn generate(
    cache_dir: &Path,
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> anyhow::Result<Pixbuf> {
    let (src_path, mtime) = resolve_src(uri, mtime)?;
    let key = format!("thumb-v2:{}{:?}", src_path.display(), mtime);
    let hash = blake3::hash(key.as_bytes()).to_hex().to_string();

    let cache_stem = cache_dir
        .join("thumbnails")
        .join(size.subdir())
        .join(&hash[..2])
        .join(hash.as_str());
    let jpeg_path = cache_stem.with_extension("jpg");
    let webp_path = cache_stem.with_extension("webp");

    for cache_path in [&webp_path, &jpeg_path] {
        if !cache_path.exists() {
            continue;
        }
        info!(
            "VIEWER_DEBUG thumb disk_cache_hit source_uri={} source_path={} size={:?} cache_path={}",
            uri,
            src_path.display(),
            size,
            cache_path.display()
        );
        // 磁盘命中：必须解码一次才能拿到像素做 Texture（不可避免）。
        // 使用同步读取确保文件完全写入后再解码。
        return load_pixbuf_sync(cache_path);
    }

    if let Some(parent) = cache_stem.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if mime_from_extension(&src_path).and_then(media_kind_from_mime) == Some(MediaKind::Video) {
        match extract_video_frame(&src_path, size.max_dim()) {
            Ok(pb) => {
                let scaled = scale_pixbuf_to_fit(&pb, size.max_dim());
                let cache_path = cache_stem.with_extension("jpg");
                let thumb = ensure_opaque(&scaled);
                thumb
                    .savev(&cache_path, "jpeg", &[])
                    .map_err(|e| anyhow::anyhow!("视频缩略图保存失败 {:?}: {}", cache_path, e))?;
                return Ok(thumb);
            }
            Err(e) => {
                warn!("视频帧提取失败，回退占位图 {}: {}", src_path.display(), e);
                return generate_video_placeholder(size.max_dim(), &cache_stem);
            }
        }
    }

    info!(
        "VIEWER_DEBUG thumb generate source_uri={} source_path={} size={:?} cache_stem={}",
        uri,
        src_path.display(),
        size,
        cache_stem.display()
    );
    // 统一用 gdk-pixbuf 解码 + 缩放：覆盖面广（JPEG/PNG/WebP/TIFF，flatpak
    // GNOME 50 runtime 还自带 libheif，能解 HEIC/AVIF），且其双线性缩放与 image
    // crate 的面积滤波在缩略图尺寸下肉眼无差（已 A/B 对照确认），故走单一路径。
    // 直接把缩放好的 pixbuf 返回给 worker 复用，省掉"写盘后再解码一次"的冗余。
    generate_via_pixbuf(&src_path, size.max_dim(), &cache_stem)
}

fn generate_video_placeholder(max_dim: u32, cache_stem: &Path) -> anyhow::Result<Pixbuf> {
    let width = max_dim as i32;
    let height = ((max_dim as f64) * 9.0 / 16.0).round().max(1.0) as i32;
    let pb = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, width, height)
        .ok_or_else(|| anyhow::anyhow!("failed to allocate video thumbnail"))?;
    pb.fill(0x20242cff);

    let rowstride = pb.rowstride() as usize;
    let channels = pb.n_channels() as usize;
    let cx = width / 2;
    let cy = height / 2;
    let tri_w = (width / 5).max(18);
    let tri_h = (height / 3).max(18);
    unsafe {
        let pixels = pb.pixels();
        for y in (cy - tri_h / 2).max(0)..(cy + tri_h / 2).min(height) {
            let rel_y = y - (cy - tri_h / 2);
            let half_height = tri_h.max(1);
            let right = cx - tri_w / 3 + (tri_w * rel_y / half_height);
            let left = cx - tri_w / 3;
            for x in left.max(0)..right.min(width) {
                let i = y as usize * rowstride + x as usize * channels;
                if i + 2 < pixels.len() {
                    pixels[i] = 238;
                    pixels[i + 1] = 242;
                    pixels[i + 2] = 247;
                }
            }
        }
    }

    let cache_path = cache_stem.with_extension("jpg");
    pb.savev(cache_path, "jpeg", &[]).map_err(|e| {
        anyhow::anyhow!(
            "video placeholder thumbnail save failed {:?}: {}",
            cache_stem.with_extension("jpg"),
            e
        )
    })?;
    Ok(pb)
}

/// 用 GStreamer 从视频文件中提取一帧作为缩略图。
///
/// 提取视频封面帧：优先调用 [`extract_video_frame_ffmpeg`]（基于 libav，正确处理
/// limited→full 色彩范围、HDR→SDR 色调映射与旋转），失败时回退到内置 GStreamer
/// 管线 [`extract_video_frame_gst`]。两条路径返回的帧均已在左下角叠加播放图标。
fn extract_video_frame(path: &Path, max_dim: u32) -> anyhow::Result<Pixbuf> {
    match extract_video_frame_ffmpeg(path, max_dim) {
        Ok(pb) => Ok(pb),
        Err(e) => {
            warn!(
                "VIDEO_THUMB ffmpegthumbnailer 失败，回退 GStreamer {}: {}",
                path.display(),
                e
            );
            extract_video_frame_gst(path, max_dim)
        }
    }
}

/// 用外部 `ffmpegthumbnailer` 生成封面帧。它内部走 libav，会正确扩展 limited
/// range（YUV 16–235 → RGB 0–255）并做 HDR→SDR 与旋转，避免手写管线把窄范围
/// 原样塞进 RGB 导致缩略图发灰、低饱和。输出 PNG（无损，避免二次 JPEG 压缩），
/// 解码后在左下角叠加播放图标。
fn extract_video_frame_ffmpeg(path: &Path, max_dim: u32) -> anyhow::Result<Pixbuf> {
    // 临时输出文件：复用 PID + 路径哈希命名（PID 惯例见 prefs.rs），落在系统 tmp。
    let tmp = std::env::temp_dir().join(format!(
        "pvthumb-{}-{}.png",
        std::process::id(),
        blake3::hash(path.to_string_lossy().as_bytes()).to_hex()
    ));

    let out = Command::new("ffmpegthumbnailer")
        .args([
            "-i",
            &path.to_string_lossy(),
            "-o",
            &tmp.to_string_lossy(),
            "-s",
            &max_dim.to_string(),
            "-t",
            "10%",
            "-c",
            "png",
            "-f",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("启动 ffmpegthumbnailer 失败: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!(
            "ffmpegthumbnailer 退出码 {:?}: {}",
            out.status.code(),
            stderr.trim()
        );
    }

    let pb = load_pixbuf_sync(&tmp);
    let _ = std::fs::remove_file(&tmp);
    let pb = pb?;
    let pb = overlay_play_icon(&pb);
    info!(
        "VIDEO_THUMB ffmpegthumbnailer 提取成功 {}x{}",
        pb.width(),
        pb.height()
    );
    Ok(pb)
}

/// GStreamer fallback：`uridecodebin → videoflip(auto) → videoconvert → appsink`，
/// seek 到约 1 秒（或总时长 10%）处拉取一帧。输出 caps 显式指定 `colorimetry=sRGB`
/// 以强制 videoconvert 做 limited→full 色彩范围扩展，修复 TV-range 视频发灰。
fn extract_video_frame_gst(path: &Path, _max_dim: u32) -> anyhow::Result<Pixbuf> {
    info!("VIDEO_THUMB 提取视频帧(GStreamer): {}", path.display());
    gst::init().map_err(|e| anyhow::anyhow!("GStreamer 初始化失败: {e}"))?;

    let uri =
        glib::filename_to_uri(path, None).map_err(|e| anyhow::anyhow!("路径转 URI 失败: {e}"))?;

    // 用 uridecodebin 构建管线：自动处理 decodebin 动态 pad 链接。
    // videoflip video-direction=auto 从所有来源（容器 tkhd、编码 SEI、tags）自动检测并应用旋转。
    // videoconvert 负责 YUV→RGB；显式 colorimetry=sRGB 强制输出 full-range sRGB，
    // 避免 limited-range(TV) 视频黑/白点被压在 16/235 导致缩略图发灰低饱和。
    let desc = format!(
        "uridecodebin uri={} ! videoflip video-direction=auto ! videoconvert ! video/x-raw,format=RGB,colorimetry=sRGB ! appsink name=sink",
        uri
    );
    let pipeline =
        gst::parse::launch(&desc).map_err(|e| anyhow::anyhow!("创建 pipeline 失败: {e}"))?;
    let pipeline = pipeline
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("pipeline 类型转换失败"))?;

    // 获取 appsink 元素。
    let appsink_el = pipeline
        .by_name("sink")
        .ok_or_else(|| anyhow::anyhow!("找不到 appsink 元素"))?;
    let appsink = appsink_el
        .downcast_ref::<gst_app::AppSink>()
        .ok_or_else(|| anyhow::anyhow!("appsink 类型转换失败"))?;

    appsink.set_max_buffers(1);
    appsink.set_drop(true);

    // 启动 pipeline。
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| anyhow::anyhow!("设置 Playing 失败: {e}"))?;

    // 等待 pipeline 进入 Playing（带超时）。
    let bus = pipeline
        .bus()
        .ok_or_else(|| anyhow::anyhow!("pipeline 无 bus"))?;
    let start = std::time::Instant::now();
    loop {
        let (_, state, _) = pipeline.state(gst::ClockTime::from_mseconds(100));
        if state == gst::State::Playing {
            break;
        }
        if start.elapsed().as_secs() >= 5 {
            let (_, cur_state, _) = pipeline.state(gst::ClockTime::ZERO);
            pipeline.set_state(gst::State::Null).ok();
            anyhow::bail!("等待 Playing 超时，当前状态: {:?}", cur_state);
        }
        while let Some(msg) = bus.pop() {
            if let gst::MessageView::Error(e) = msg.view() {
                pipeline.set_state(gst::State::Null).ok();
                anyhow::bail!("GStreamer 错误: {}", e.error().message());
            }
        }
    }

    // 查询时长并 seek 到合适位置。
    let seek_pos = if let Some(duration) = pipeline.query_duration::<gst::ClockTime>() {
        let one_sec = gst::ClockTime::from_seconds(1);
        let ten_pct = duration
            .nseconds()
            .checked_mul(10)
            .and_then(|n| n.checked_div(100))
            .map(gst::ClockTime::from_nseconds)
            .unwrap_or(gst::ClockTime::ZERO);
        let target = std::cmp::max(one_sec, ten_pct);
        if target >= duration {
            gst::ClockTime::ZERO
        } else {
            target
        }
    } else {
        gst::ClockTime::from_seconds(1)
    };

    if !seek_pos.is_none() {
        let _ = pipeline.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, seek_pos);
        // 等待 seek 完成。
        let _ = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(3),
            &[gst::MessageType::AsyncDone, gst::MessageType::Error],
        );
    }

    // 拉取一帧。
    let sample = match appsink.try_pull_sample(gst::ClockTime::from_seconds(3)) {
        Some(s) => s,
        None => {
            pipeline.set_state(gst::State::Null).ok();
            anyhow::bail!("拉取视频帧超时或无数据");
        }
    };

    let buffer = sample
        .buffer()
        .ok_or_else(|| anyhow::anyhow!("sample 无 buffer"))?;
    let caps = sample
        .caps()
        .ok_or_else(|| anyhow::anyhow!("sample 无 caps"))?;
    let vinfo = gst_video::VideoInfo::from_caps(caps)
        .map_err(|e| anyhow::anyhow!("解析视频 caps 失败: {e}"))?;

    let width = vinfo.width() as i32;
    let height = vinfo.height() as i32;
    let stride = vinfo.stride()[0] as i32;

    let map = buffer
        .map_readable()
        .map_err(|e| anyhow::anyhow!("buffer 映射失败: {e}"))?;
    let data = map.as_slice();

    // 构造 Pixbuf（RGB，无 alpha，3 通道）。
    let pb = Pixbuf::from_mut_slice(
        data.to_vec().into_boxed_slice(),
        gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        width,
        height,
        stride,
    );

    pipeline.set_state(gst::State::Null).ok();

    // 旋转已由 GStreamer pipeline 中的 videoflip video-direction=auto 自动处理，
    // 无需手动读取容器元数据并应用方向校正。

    info!("VIDEO_THUMB 提取成功 {}x{}", pb.width(), pb.height());

    // 在左下角叠加半透明播放图标。
    let pb = overlay_play_icon(&pb);

    Ok(pb)
}

/// 从 MP4/MOV 容器的 tkhd atom 中读取视频旋转角度（0/90/180/270）。
///
/// MP4 容器在 track header (tkhd) 中存储一个 3×3 仿射矩阵。
/// 旋转信息编码在矩阵的 a,b,c,d 分量中（16.16 定点数）：
///   - 0°:   a=1, b=0, c=0, d=1
///   - 90°:  a=0, b=1, c=-1, d=0
///   - 180°: a=-1, b=0, c=0, d=-1
///   - 270°: a=0, b=-1, c=1, d=0
///
/// 递归搜索 tkhd atom 以处理嵌套的 box 结构（moov → trak → tkhd）。
#[allow(dead_code)] // used in tests
fn read_video_rotation(path: &Path) -> i32 {
    let Ok(data) = std::fs::read(path) else {
        return 0;
    };

    // 递归搜索 tkhd atom。
    fn find_tkhd_rotation(data: &[u8], start: usize, end: usize) -> i32 {
        let mut pos = start;
        while pos + 8 <= end && pos + 8 <= data.len() {
            let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
            if size < 8 {
                break;
            }
            let typ = &data[pos + 4..pos + 8];

            // 递归进入容器 atom（moov, trak 等）。
            if typ == b"moov" || typ == b"trak" {
                let child_start = pos + 8;
                let child_end = pos + size;
                if child_end <= data.len() {
                    let result = find_tkhd_rotation(data, child_start, child_end);
                    if result != 0 {
                        return result;
                    }
                }
            }

            // 找到 tkhd atom，解析旋转矩阵。
            if typ == b"tkhd" && size >= 84 {
                let version = data[pos + 8];
                // 矩阵偏移量：version + flags + creation_time + modification_time +
                // track_ID + reserved + duration + reserved + layer + alternate_group +
                // volume + reserved = 40 bytes for version 0, 52 for version 1
                let matrix_offset = if version == 0 {
                    pos + 8 + 40 // 4 + 4 + 4 + 4 + 4 + 4 + 8 + 2 + 2 + 2 + 2
                } else {
                    pos + 8 + 52 // 4 + 8 + 8 + 4 + 4 + 8 + 8 + 2 + 2 + 2 + 2
                };

                if matrix_offset + 36 <= data.len() {
                    let a = i32::from_be_bytes(
                        data[matrix_offset..matrix_offset + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );
                    let b = i32::from_be_bytes(
                        data[matrix_offset + 4..matrix_offset + 8]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );

                    let a_f = a as f64 / 65536.0;
                    let b_f = b as f64 / 65536.0;

                    if a_f.abs() < 0.01 && (b_f - 1.0).abs() < 0.01 {
                        return 90;
                    }
                    if (a_f - (-1.0)).abs() < 0.01 && b_f.abs() < 0.01 {
                        return 180;
                    }
                    if a_f.abs() < 0.01 && (b_f - (-1.0)).abs() < 0.01 {
                        return 270;
                    }
                }
            }

            pos += size;
        }
        0
    }

    find_tkhd_rotation(&data, 0, data.len())
}

/// 在 pixbuf 左下角叠加一个半透明播放三角形，标记为视频缩略图。
fn overlay_play_icon(pb: &Pixbuf) -> Pixbuf {
    let pb = pb.clone();
    let w = pb.width();
    let h = pb.height();
    if w < 20 || h < 20 {
        return pb;
    }

    // 图标尺寸：约 1/6 宽度，最小 16px，最大 48px。
    let icon_size = (w / 6).clamp(16, 48);
    let margin = icon_size / 4;

    // 三角形参数：指向右方的等腰三角形。
    let tri_h = icon_size;
    let tri_w = (icon_size as f64 * 0.86) as i32; // 等边三角形比例
    let ox = margin;
    let oy = h - margin - tri_h;

    let rowstride = pb.rowstride() as usize;
    let channels = pb.n_channels() as usize;
    let has_alpha = pb.has_alpha();

    unsafe {
        let pixels = pb.pixels();
        // 半透明深色圆形背景。
        let bg_r: u8 = 0;
        let bg_g: u8 = 0;
        let bg_b: u8 = 0;
        let bg_a: u8 = 140;
        let cx = ox + tri_w / 2;
        let cy = oy + tri_h / 2;
        let radius = (tri_h / 2 + 4) as f64;

        for y in (oy - 4).max(0)..(oy + tri_h + 4).min(h) {
            for x in (ox - 4).max(0)..(ox + tri_w + 4).min(w) {
                let dx = (x - cx) as f64;
                let dy = (y - cy) as f64;
                if dx * dx + dy * dy <= radius * radius {
                    let i = y as usize * rowstride + x as usize * channels;
                    if i + 2 < pixels.len() {
                        let alpha = bg_a as f64 / 255.0;
                        let inv = 1.0 - alpha;
                        pixels[i] = (bg_r as f64 * alpha + pixels[i] as f64 * inv) as u8;
                        pixels[i + 1] = (bg_g as f64 * alpha + pixels[i + 1] as f64 * inv) as u8;
                        pixels[i + 2] = (bg_b as f64 * alpha + pixels[i + 2] as f64 * inv) as u8;
                        if has_alpha && i + 3 < pixels.len() {
                            pixels[i + 3] = 255;
                        }
                    }
                }
            }
        }

        // 白色三角形（指向右方）。
        for dy in 0..tri_h {
            let row_half = (dy as f64 / tri_h as f64 * tri_w as f64 / 2.0) as i32;
            let left = cx - row_half;
            let right = cx + row_half;
            for x in left.max(0)..right.min(w) {
                let y = oy + dy;
                if y < 0 || y >= h {
                    continue;
                }
                let i = y as usize * rowstride + x as usize * channels;
                if i + 2 < pixels.len() {
                    pixels[i] = 255;
                    pixels[i + 1] = 255;
                    pixels[i + 2] = 255;
                    if has_alpha && i + 3 < pixels.len() {
                        pixels[i + 3] = 255;
                    }
                }
            }
        }
    }

    pb
}

/// `image` crate 解不了的格式（HEIC/AVIF 等）走 gdk-pixbuf：解码 → 等比缩放 → 存磁盘缓存。
/// 返回内存里已缩放好的 pixbuf，让调用方直接做成 Texture，省掉读盘重解码。
fn generate_via_pixbuf(src_path: &Path, max_dim: u32, cache_stem: &Path) -> anyhow::Result<Pixbuf> {
    let orientation_value = orientation::read_orientation(src_path).ok();
    let pb = orientation::load_oriented_pixbuf(src_path)
        .map_err(|e| anyhow::anyhow!("gdk-pixbuf 解码失败: {e}"))?;
    debug!(
        "THUMB_TRACE decode_source path={} orientation={:?} decoded={}x{} max_dim={}",
        src_path.display(),
        orientation_value,
        pb.width(),
        pb.height(),
        max_dim
    );
    let scaled = scale_pixbuf_to_fit(&pb, max_dim);
    if pixbuf_has_transparency(&scaled) {
        let cache_path = cache_stem.with_extension("webp");
        save_pixbuf_as_webp(&scaled, &cache_path)?;
        return Ok(scaled);
    }

    let cache_path = cache_stem.with_extension("jpg");
    let thumb = ensure_opaque(&scaled);
    thumb.savev(cache_path, "jpeg", &[]).map_err(|e| {
        anyhow::anyhow!(
            "gdk-pixbuf JPEG 保存失败 {:?}: {}",
            cache_stem.with_extension("jpg"),
            e
        )
    })?;
    Ok(thumb)
}

fn pixbuf_has_transparency(pb: &Pixbuf) -> bool {
    if !pb.has_alpha() {
        return false;
    }
    let bytes = pb.read_pixel_bytes();
    let buf: &[u8] = bytes.as_ref();
    let n_channels = pb.n_channels() as usize;
    let rowstride = pb.rowstride() as usize;
    if n_channels < 4 {
        return false;
    }
    for y in 0..pb.height() as usize {
        for x in 0..pb.width() as usize {
            let i = y * rowstride + x * n_channels + 3;
            if i < buf.len() && buf[i] < 255 {
                return true;
            }
        }
    }
    false
}

fn save_pixbuf_as_webp(pb: &Pixbuf, cache_path: &Path) -> anyhow::Result<()> {
    let rgba = pixbuf_to_rgba_bytes(pb)?;
    let file = File::create(cache_path)?;
    let writer = BufWriter::new(file);
    image::codecs::webp::WebPEncoder::new_lossless(writer)
        .write_image(
            &rgba,
            pb.width() as u32,
            pb.height() as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| anyhow::anyhow!("WebP 缩略图保存失败 {:?}: {}", cache_path, e))
}

fn pixbuf_to_rgba_bytes(pb: &Pixbuf) -> anyhow::Result<Vec<u8>> {
    let width = pb.width() as usize;
    let height = pb.height() as usize;
    let n_channels = pb.n_channels() as usize;
    let rowstride = pb.rowstride() as usize;
    if n_channels != 3 && n_channels != 4 {
        anyhow::bail!("不支持的 pixbuf 通道数: {}", n_channels);
    }

    let bytes = pb.read_pixel_bytes();
    let buf: &[u8] = bytes.as_ref();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let i = y * rowstride + x * n_channels;
            if i + n_channels > buf.len() {
                anyhow::bail!("pixbuf 像素缓冲区越界");
            }
            rgba.extend_from_slice(&buf[i..i + 3]);
            rgba.push(if n_channels == 4 { buf[i + 3] } else { 255 });
        }
    }
    Ok(rgba)
}

/// 返回等尺寸的**不透明**（无 alpha）pixbuf：有 alpha 时合成到不透明白底上，
/// 无 alpha 时原样克隆。供 JPEG 保存前使用（JPEG 无 alpha 通道）。
fn ensure_opaque(pb: &Pixbuf) -> Pixbuf {
    if !pb.has_alpha() {
        return pb.clone();
    }
    let (w, h) = (pb.width(), pb.height());
    let bg =
        Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, w, h).expect("分配不透明背景 pixbuf");
    bg.fill(0xFFFFFFFF); // 不透明白
    pb.composite(
        &bg,
        0,
        0,
        w,
        h,
        0.0,
        0.0,
        1.0,
        1.0,
        gdk_pixbuf::InterpType::Bilinear,
        255,
    );
    bg
}

/// 等比缩放到 `max_dim` 内（不放大），行为对齐 `image::DynamicImage::thumbnail`。
fn scale_pixbuf_to_fit(pb: &Pixbuf, max_dim: u32) -> Pixbuf {
    let (w, h) = (pb.width(), pb.height());
    let longest = (w.max(h).max(1)) as f64;
    let scale = ((max_dim as f64) / longest).min(1.0);
    let nw = ((w as f64) * scale).round().max(1.0) as i32;
    let nh = ((h as f64) * scale).round().max(1.0) as i32;
    pb.scale_simple(nw, nh, gdk_pixbuf::InterpType::Bilinear)
        .unwrap_or_else(|| pb.clone())
}

/// 采样 pixbuf 像素估算平均亮度（>=160 视为"亮"背景），用于 tile 文字配色。
///
/// 在 worker 线程就地读取像素缓冲（零拷贝借用），替代原来在主线程对每张
/// texture 做 `Texture::download` + 大 buffer 分配的做法。RGB(3 通道)/RGBA(4 通道)
/// 均适用：`x * n_channels` 自动按实际通道数定位。
fn pixbuf_is_light(pb: &Pixbuf) -> Option<bool> {
    let width = pb.width();
    let height = pb.height();
    if width <= 0 || height <= 0 {
        return None;
    }
    let bytes = pb.read_pixel_bytes();
    let buf: &[u8] = bytes.as_ref();
    let n_channels = pb.n_channels() as usize;
    let rowstride = pb.rowstride() as usize;
    let step_x = (width / 24).max(1) as usize;
    let step_y = (height / 24).max(1) as usize;
    let mut total = 0.0f64;
    let mut count = 0.0f64;
    for y in (0..height as usize).step_by(step_y) {
        for x in (0..width as usize).step_by(step_x) {
            let i = y * rowstride + x * n_channels;
            if i + 2 < buf.len() {
                total += (buf[i] as f64 + buf[i + 1] as f64 + buf[i + 2] as f64) / 3.0;
                count += 1.0;
            }
        }
    }
    if count == 0.0 {
        return None;
    }
    Some(total / count >= 160.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db;

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
    fn video_placeholder_thumbnail_is_cached_as_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("video");

        let thumb =
            generate_video_placeholder(256, &out).expect("video placeholder should generate");

        assert!(
            out.with_extension("jpg").exists(),
            "placeholder should be cached"
        );
        assert_eq!(thumb.width(), 256);
        assert!(thumb.height() > 0);
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

    /// 视频帧提取端到端测试（需要真实视频文件）。
    /// 运行: VIDEO_TEST_FILE=/path/to/video.mp4 cargo test --lib extract_video_frame_from_file -- --ignored --nocapture
    #[test]
    #[ignore]
    fn extract_video_frame_from_file() {
        let path = match std::env::var("VIDEO_TEST_FILE") {
            Ok(p) => std::path::PathBuf::from(p),
            Err(_) => {
                eprintln!("set VIDEO_TEST_FILE to a video file path; skipping");
                return;
            }
        };
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

    /// 保存视频帧到文件以便对比。
    /// 运行: VIDEO_TEST_FILE=/path/to/video.mp4 cargo test --lib save_video_frame -- --ignored --nocapture
    #[test]
    #[ignore]
    fn save_video_frame() {
        let path = match std::env::var("VIDEO_TEST_FILE") {
            Ok(p) => std::path::PathBuf::from(p),
            Err(_) => {
                eprintln!("set VIDEO_TEST_FILE to a video file path; skipping");
                return;
            }
        };
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

    /// 测试从 MP4 文件读取旋转信息。
    /// 运行: VIDEO_TEST_FILE=/path/to/video.mp4 cargo test --lib read_video_rotation_from_mp4 -- --ignored --nocapture
    #[test]
    #[ignore]
    fn read_video_rotation_from_mp4() {
        let path = match std::env::var("VIDEO_TEST_FILE") {
            Ok(p) => std::path::PathBuf::from(p),
            Err(_) => {
                eprintln!("set VIDEO_TEST_FILE to a video file path; skipping");
                return;
            }
        };
        let rotation = read_video_rotation(&path);
        eprintln!("视频旋转: {} rotation={}", path.display(), rotation);
        assert!(rotation == 0 || rotation == 90 || rotation == 180 || rotation == 270);
    }
}
