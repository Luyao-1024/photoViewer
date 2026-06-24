//! 缩略图加载器：worker pool + 优先级队列 + 分桶磁盘缓存
//!
//! - 多个 tokio blocking worker 并行处理缩略图生成/读取
//! - 按 `path + mtime` 计算 blake3 哈希作为缓存键（mtime 变了自动失效）
//! - 缓存目录按 `thumbnails/{small|medium|large}/<hash 前两位>/<hash>.jpg` 分桶
//! - 内存 LRU 缓存已加载的 `Texture`，避免重复解码
//! - **优先级队列**：可见 tile 可经 `prioritize_keys` 提到队首（BOOST），
//!   先于普通（NORMAL）请求被 worker 取走，消除分页 rebuild / 滚动时的优先级倒置。
use crate::core::db::DbPool;
use gdk_pixbuf::Pixbuf;
use gtk4::gdk::Texture;
use lru::LruCache;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;
use tokio::sync::oneshot;
use tracing::warn;

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
const TIER_BOOST: u8 = 0;
const TIER_NORMAL: u8 = 1;

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

/// 缩略图加载器单例
///
/// 内部用优先级队列把请求分发给一组 worker；worker 在 tokio 阻塞线程上
/// 完成 CPU/IO 密集的解码/编码后通过 oneshot 归还 `LoadedThumb`。request 端
/// 做在途去重，保证同一 (uri, size) 只生成一次、且永不丢请求；可见 tile 可经
/// `prioritize_keys` 提前。
pub struct ThumbnailLoader {
    pool: DbPool,
    cache_dir: PathBuf,
    queue: SharedQueue,
    state: Arc<Mutex<LoaderState>>,
}

/// 内存 LRU 容量。Large 档单张 texture ~3MB（1024×683×4），512 条最坏 ~1.5GB；
/// 滚动回看时命中内存比走磁盘重解码更顺滑。按机器内存酌情调。
const MEM_CACHE_CAP: usize = 512;

impl ThumbnailLoader {
    /// 工作项队列容量。配合在途去重后，这里只存「彼此不同且未缓存」的项；
    /// 取一个充裕的值，使得在加入视口级虚拟化之前，单库数万张也能容纳。
    pub const QUEUE_CAPACITY: usize = 8192;

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
        }
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
            tokio::task::spawn_blocking(move || {
                worker_loop(queue, pool, cache_dir, state);
            });
        }
    }

    /// 提交一个缩略图请求。
    ///
    /// 走三层短路，确保**同一个 (uri, size) 永远不会被重复生成，也永远不会
    /// 因为队列满而被静默丢弃**：
    ///   1. mem-cache 命中 → 立刻同步回送；
    ///   2. 已有在途生成 → 把 reply 挂到该在途请求的等待者列表（不入队）；
    ///   3. 否则登记一条在途项并以 `NORMAL` 优先级入队。
    ///
    /// `mtime`：若调用方已知源文件 mtime（如 `MediaItem.file_mtime`），传入可
    /// **跳过主线程 stat**（B5：mtime 已在扫描/notify 时入库，无需每次请求再 stat）；
    /// 传 `None` 则现场 stat 兜底。只有当队列里**已塞满彼此不同的**未缓存工作项
    /// （远超库规模才会发生）时，第 3 步才会失败；此时回滚在途项，调用方收到 `Err`。
    ///
    /// 锁序：`state{drop}` → `queue{drop}` →（回滚）`state{drop}`。两锁从不嵌套，无死锁。
    pub fn request(
        &self,
        uri: String,
        size: ThumbnailSize,
        mtime: Option<SystemTime>,
        reply: oneshot::Sender<LoadedThumb>,
    ) {
        let Some(cache_key) = cache_key_str(&uri, size, mtime) else {
            // 源文件不存在 / 无法 stat：无法去重，按"生成失败"处理。
            warn!("缩略图请求无法计算缓存键（源文件缺失?）: {}", uri);
            return; // reply 被 drop → 调用方 rx 收到 Err
        };

        let mut st = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return, // poisoned
        };
        // 1) 内存命中
        if let Some(loaded) = st.mem_cache.get(&cache_key).cloned() {
            drop(st);
            let _ = reply.send(loaded);
            return;
        }
        // 2) 已在途 → 挂载等待者，不再入队
        if let Some(waiters) = st.in_flight.get_mut(&cache_key) {
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
                        tier: TIER_NORMAL,
                        uri: uri.clone(),
                        size,
                        mtime,
                    },
                );
                q.heap.push(Reverse(PriItem {
                    tier: TIER_NORMAL,
                    seq,
                    cache_key: cache_key.clone(),
                    uri: uri.clone(),
                    size,
                    mtime,
                }));
                true
            }
        };
        if enqueued {
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

fn worker_loop(
    queue: SharedQueue,
    _pool: DbPool,
    cache_dir: PathBuf,
    state: Arc<Mutex<LoaderState>>,
) {
    while let Some(req) = next_request(&queue) {
        // request 端已保证此 cache_key 此前既不在 mem_cache 也不在 in_flight，
        // 所以这里直接生成即可。生成期间，同 key 的后续 request 会挂到
        // in_flight 等待者列表上，由下面的广播一并唤醒。
        match generate(&cache_dir, &req.uri, req.size, req.mtime) {
            Ok(pb) => {
                // 亮度判定下沉到 worker：直接就地采样 pixbuf 像素，主线程零回读。
                let is_light = pixbuf_is_light(&pb);
                let texture = Texture::for_pixbuf(&pb);
                let loaded = LoadedThumb {
                    texture: texture.clone(),
                    is_light,
                };
                // 写入 mem_cache，并把结果广播给所有等待者。
                let waiters = {
                    let mut st = match state.lock() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    st.mem_cache.put(req.cache_key.clone(), loaded.clone());
                    st.in_flight.remove(&req.cache_key).unwrap_or_default()
                };
                for w in waiters {
                    let _ = w.send(loaded.clone());
                }
            }
            Err(e) => {
                drop_in_flight(&state, &req.cache_key);
                warn!("缩略图生成失败 {}: {}", req.uri, e);
            }
        }
    }
}

/// 阻塞取下一个**有效**工作项：空则 `cvar` 等待，被关闭返回 `None`。
///
/// 弹出时校验堆项的 tier 是否仍等于 `queued_tiers[key]`：不等说明该 key 已被
/// 提权（旧 NORMAL 项过期）或已被处理/取消 → 丢弃继续弹。命中则从 `queued_tiers`
/// 移除该 key（使该 key 的所有过期堆项在后续弹出时一并失效）。
fn next_request(queue: &SharedQueue) -> Option<PriItem> {
    let (lock, cvar) = &**queue;
    loop {
        let mut q = lock.lock().ok()?;
        // 空且未关：等
        while q.heap.is_empty() && !q.closed {
            q = cvar.wait(q).ok()?;
        }
        // 弹到有效项即返回；过期项就地丢弃继续弹；弹空了回外层重等。
        loop {
            if q.closed {
                return None;
            }
            let Some(Reverse(item)) = q.heap.pop() else {
                break; // 堆又空了 → 回外层 while 等待
            };
            if q.queued.get(&item.cache_key).map(|e| e.tier) == Some(item.tier) {
                q.queued.remove(&item.cache_key);
                return Some(item);
            }
            // 过期项（tier 已被提权 / key 已处理），丢弃继续
        }
    }
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

fn generate(
    cache_dir: &Path,
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> anyhow::Result<Pixbuf> {
    let (src_path, mtime) = resolve_src(uri, mtime)?;
    let key = format!("{}{:?}", src_path.display(), mtime);
    let hash = blake3::hash(key.as_bytes()).to_hex().to_string();

    let cache_path = cache_dir
        .join("thumbnails")
        .join(size.subdir())
        .join(&hash[..2])
        .join(format!("{}.jpg", hash));

    if cache_path.exists() {
        tracing::debug!(
            "VIEWER_DEBUG thumb disk_cache_hit source_uri={} source_path={} size={:?} cache_path={}",
            uri,
            src_path.display(),
            size,
            cache_path.display()
        );
        // 磁盘命中：必须解码一次才能拿到像素做 Texture（不可避免）。
        return Pixbuf::from_file(&cache_path)
            .map_err(|e| anyhow::anyhow!("缓存缩略图解码失败 {:?}: {}", cache_path, e));
    }

    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    tracing::debug!(
        "VIEWER_DEBUG thumb generate source_uri={} source_path={} size={:?} cache_path={}",
        uri,
        src_path.display(),
        size,
        cache_path.display()
    );
    // 统一用 gdk-pixbuf 解码 + 缩放：覆盖面广（JPEG/PNG/WebP/TIFF，flatpak
    // GNOME 50 runtime 还自带 libheif，能解 HEIC/AVIF），且其双线性缩放与 image
    // crate 的面积滤波在缩略图尺寸下肉眼无差（已 A/B 对照确认），故走单一路径。
    // 直接把缩放好的 pixbuf 返回给 worker 复用，省掉"写盘后再解码一次"的冗余。
    generate_via_pixbuf(&src_path, size.max_dim(), &cache_path)
}

/// `image` crate 解不了的格式（HEIC/AVIF 等）走 gdk-pixbuf：解码 → 等比缩放 → 存 JPEG。
/// 返回内存里已缩放好的 pixbuf，让调用方直接做成 Texture，省掉读盘重解码。
fn generate_via_pixbuf(src_path: &Path, max_dim: u32, cache_path: &Path) -> anyhow::Result<Pixbuf> {
    let pb =
        Pixbuf::from_file(src_path).map_err(|e| anyhow::anyhow!("gdk-pixbuf 解码失败: {e}"))?;
    let scaled = scale_pixbuf_to_fit(&pb, max_dim);
    // JPEG 不支持 alpha：GNOME 50 的 gdk-pixbuf JPEG 保存走 glycin，会直接拒绝
    // `Rgba8` pixbuf。先把带 alpha 的（PNG 截图等）合成到不透明白底上再存。
    let thumb = ensure_opaque(&scaled);
    thumb
        .savev(cache_path, "jpeg", &[])
        .map_err(|e| anyhow::anyhow!("gdk-pixbuf JPEG 保存失败: {e}"))?;
    Ok(thumb)
}

/// 返回等尺寸的**不透明**（无 alpha）pixbuf：有 alpha 时合成到不透明白底上，
/// 无 alpha 时原样克隆。供 JPEG 保存前使用（JPEG 无 alpha 通道）。
fn ensure_opaque(pb: &Pixbuf) -> Pixbuf {
    if !pb.has_alpha() {
        return pb.clone();
    }
    let (w, h) = (pb.width(), pb.height());
    let bg = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, w, h)
        .expect("分配不透明背景 pixbuf");
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

        let out = dir.path().join("out.jpg");
        generate_via_pixbuf(&src, 256, &out).expect("gdk-pixbuf 回退应成功");

        assert!(out.exists(), "应写出 JPEG 缩略图");
        let decoded = image::open(&out).expect("输出的 JPEG 应可被重新解码");
        let (w, h) = (decoded.width(), decoded.height());
        assert!(w <= 256 && h <= 256, "应在 max_dim 内, got {w}x{h}");
        assert_eq!(w.max(h), 256, "长边应正好缩到 max_dim");
    }

    /// 回归：RGBA PNG（截图）必须能生成 JPEG 缩略图。
    /// GNOME 50 runtime 下 gdk-pixbuf 的 JPEG 保存走 glycin，拒绝 Rgba8；
    /// 修复前此用例失败（缩略图生成失败 → tile 白/灰块）。
    #[test]
    fn generate_via_pixbuf_handles_rgba_png() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("rgba.png");
        let img = image::RgbaImage::from_pixel(400, 300, image::Rgba([10, 20, 30, 128]));
        image::DynamicImage::ImageRgba8(img).save(&src).unwrap();

        let out = dir.path().join("out.jpg");
        generate_via_pixbuf(&src, 256, &out).expect("RGBA PNG 应能生成 JPEG 缩略图");
        assert!(out.exists(), "应写出 JPEG 缩略图");
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
}
