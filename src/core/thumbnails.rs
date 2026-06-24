//! 缩略图加载器：worker pool + 分桶磁盘缓存
//!
//! - 多个 tokio blocking worker 并行处理缩略图生成/读取
//! - 按 `path + mtime` 计算 blake3 哈希作为缓存键（mtime 变了自动失效）
//! - 缓存目录按 `thumbnails/{small|medium|large}/<hash 前两位>/<hash>.jpg` 分桶
//! - 内存 LRU 缓存已加载的 `Texture`，避免重复解码
use crate::core::db::DbPool;
use gdk_pixbuf::Pixbuf;
use gtk4::gdk::Texture;
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

/// 缩略图尺寸档位
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
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

/// 单个缩略图工作项（已去重：同一个 `cache_key` 在途期间只会有一个工作项排队）。
#[derive(Debug)]
pub struct ThumbnailRequest {
    pub uri: String,
    pub size: ThumbnailSize,
    /// request 端预算好的 mem-cache / 去重键（`{path:?}:{mtime:?}:{size:?}`）。
    pub cache_key: String,
}

/// 共享的 receiver 包装，便于多 worker 互斥消费
type SharedRx = Arc<Mutex<mpsc::Receiver<ThumbnailRequest>>>;

/// 加载器的可变缓存状态，用单一 Mutex 保护。
///
/// 把 `mem_cache` 与 `in_flight` 放在同一把锁后，request 端的
/// "查 mem_cache → 查 in_flight → 登记并入队" 与 worker 端的
/// "写 mem_cache → 取走等待者" 互斥执行，杜绝二者之间的竞态窗口
/// （否则一个刚完成的 key 可能被新请求当作未生成而重复入队）。
struct LoaderState {
    mem_cache: LruCache<String, Texture>,
    /// `cache_key` → 正在生成的请求的等待者列表。
    ///
    /// 同 key 的后续 request 直接 append 到这里、**不再单独入队**，因此：
    ///   - 同一张缩略图永远不会被重复生成；
    ///   - 重复请求永远不会因为队列满而被 `try_send` 丢弃（这正是首屏
    ///     "框出来了但缩略图空白" 的根因——三个 grid 各自为每个 tile
    ///     请求，突发量超过队列容量后被静默丢弃）。
    in_flight: HashMap<String, Vec<oneshot::Sender<Texture>>>,
}

/// 缩略图加载器单例
///
/// 内部用 mpsc 队列把请求分发给一组 worker；worker 在 tokio 阻塞线程上
/// 完成 CPU/IO 密集的解码/编码后通过 oneshot 归还 `Texture`。request 端
/// 做在途去重，保证同一 (uri, size) 只生成一次、且永不丢请求。
pub struct ThumbnailLoader {
    pool: DbPool,
    cache_dir: PathBuf,
    tx: mpsc::Sender<ThumbnailRequest>,
    rx: SharedRx,
    state: Arc<Mutex<LoaderState>>,
}

impl ThumbnailLoader {
    /// 工作项队列容量。配合在途去重后，这里只存「彼此不同且未缓存」的项；
    /// 取一个充裕的值，使得在加入视口级虚拟化之前，单库数万张也能容纳。
    pub const QUEUE_CAPACITY: usize = 8192;

    /// 构造加载器（不自动启动 worker；调用 `spawn_workers` 启动）
    pub fn new(pool: DbPool, cache_dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel(Self::QUEUE_CAPACITY);
        std::fs::create_dir_all(&cache_dir).ok();
        let state = Arc::new(Mutex::new(LoaderState {
            mem_cache: LruCache::new(NonZeroUsize::new(256).unwrap()),
            in_flight: HashMap::new(),
        }));
        // 启动时按 mtime LRU 清理超限缓存（2GB 上限）
        let _ = crate::core::cache::enforce_size_limit(
            &cache_dir.join("thumbnails"),
            2 * 1024 * 1024 * 1024,
        );
        Self {
            pool,
            cache_dir,
            tx,
            rx: Arc::new(Mutex::new(rx)),
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
            let rx = self.rx.clone();
            let state = self.state.clone();
            tokio::task::spawn_blocking(move || {
                worker_loop(rx, pool, cache_dir, state);
            });
        }
    }

    /// 提交一个缩略图请求。
    ///
    /// 走三层短路，确保**同一个 (uri, size) 永远不会被重复生成，也永远不会
    /// 因为队列满而被静默丢弃**：
    ///   1. mem-cache 命中 → 立刻同步回送；
    ///   2. 已有在途生成 → 把 reply 挂到该在途请求的等待者列表（不入队）；
    ///   3. 否则登记一条在途项并入队一个工作项。
    ///
    /// 只有当队列里**已塞满彼此不同的**未缓存工作项（远超库规模才会发生）
    /// 时，第 3 步才会失败；此时回滚在途项，调用方的 `rx` 收到 `Err`。
    pub fn request(&self, uri: String, size: ThumbnailSize, reply: oneshot::Sender<Texture>) {
        let Some(cache_key) = cache_key_str(&uri, size) else {
            // 源文件不存在 / 无法 stat：无法去重，按"生成失败"处理。
            warn!("缩略图请求无法计算缓存键（源文件缺失?）: {}", uri);
            return; // reply 被 drop → 调用方 rx 收到 Err
        };

        let mut st = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return, // poisoned
        };
        // 1) 内存命中
        if let Some(tex) = st.mem_cache.get(&cache_key).cloned() {
            drop(st);
            let _ = reply.send(tex);
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

        if self
            .tx
            .try_send(ThumbnailRequest {
                uri,
                size,
                cache_key: cache_key.clone(),
            })
            .is_err()
        {
            // 队列已满且全是不同的未缓存项：回滚在途项，避免等待者被永久挂起。
            if let Ok(mut st) = self.state.lock() {
                st.in_flight.remove(&cache_key); // drop reply → 调用方 rx.Err
            }
            warn!("缩略图请求入队失败（队列已满于不同项）: {}", cache_key);
        }
    }
}

fn worker_loop(rx: SharedRx, _pool: DbPool, cache_dir: PathBuf, state: Arc<Mutex<LoaderState>>) {
    loop {
        // 阻塞锁 + 阻塞 recv；先取出请求再释放锁以减少竞争
        let req = {
            let mut guard = match rx.lock() {
                Ok(g) => g,
                Err(_) => return, // poisoned
            };
            match guard.blocking_recv() {
                Some(r) => r,
                None => return, // channel closed
            }
        };

        // request 端已保证此 cache_key 此前既不在 mem_cache 也不在 in_flight，
        // 所以这里直接生成即可。生成期间，同 key 的后续 request 会挂到
        // in_flight 等待者列表上，由下面的广播一并唤醒。
        match generate(&cache_dir, &req.uri, req.size) {
            Ok(path) => match Pixbuf::from_file(&path) {
                Ok(pb) => {
                    let texture = Texture::for_pixbuf(&pb);
                    // 写入 mem_cache，并把结果广播给所有等待者。
                    let waiters = {
                        let mut st = match state.lock() {
                            Ok(s) => s,
                            Err(_) => return,
                        };
                        st.mem_cache.put(req.cache_key.clone(), texture.clone());
                        st.in_flight.remove(&req.cache_key).unwrap_or_default()
                    };
                    for w in waiters {
                        let _ = w.send(texture.clone());
                    }
                }
                Err(e) => {
                    drop_in_flight(&state, &req.cache_key);
                    warn!("Pixbuf 加载失败 {:?}: {}", path, e);
                }
            },
            Err(e) => {
                drop_in_flight(&state, &req.cache_key);
                warn!("缩略图生成失败 {}: {}", req.uri, e);
            }
        }
    }
}

/// 生成失败时移除在途项，让等待者的 `rx` 收到 `Err` 而非永久挂起。
fn drop_in_flight(state: &Mutex<LoaderState>, cache_key: &str) {
    if let Ok(mut st) = state.lock() {
        st.in_flight.remove(cache_key);
    }
}

/// 缓存键：(源路径, mtime)
type CacheKey = (PathBuf, std::time::SystemTime);

fn cache_key(uri: &str) -> anyhow::Result<CacheKey> {
    let path_str = uri.strip_prefix("file://").unwrap_or(uri);
    let src_path = PathBuf::from(path_str);
    let meta = std::fs::metadata(&src_path)?;
    Ok((src_path, meta.modified()?))
}

/// 与 worker 端一致的 mem-cache 键字符串（`{path:?}:{mtime:?}:{size:?}`）。
/// 在 request 端提前算好，用于 mem_cache 查询与在途去重；源文件无法 stat
/// 时返回 `None`（调用方据此把请求当作生成失败处理）。
fn cache_key_str(uri: &str, size: ThumbnailSize) -> Option<String> {
    let (path, mtime) = cache_key(uri).ok()?;
    Some(format!("{path:?}:{mtime:?}:{size:?}"))
}

fn generate(cache_dir: &Path, uri: &str, size: ThumbnailSize) -> anyhow::Result<PathBuf> {
    let (src_path, mtime) = cache_key(uri)?;
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
        return Ok(cache_path);
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
    generate_via_pixbuf(&src_path, size.max_dim(), &cache_path)?;
    Ok(cache_path)
}

/// `image` crate 解不了的格式（HEIC/AVIF 等）走 gdk-pixbuf：解码 → 等比缩放 → 存 JPEG。
fn generate_via_pixbuf(src_path: &Path, max_dim: u32, cache_path: &Path) -> anyhow::Result<()> {
    let pb =
        Pixbuf::from_file(src_path).map_err(|e| anyhow::anyhow!("gdk-pixbuf 解码失败: {e}"))?;
    let thumb = scale_pixbuf_to_fit(&pb, max_dim);
    thumb
        .savev(cache_path, "jpeg", &[])
        .map_err(|e| anyhow::anyhow!("gdk-pixbuf JPEG 保存失败: {e}"))?;
    Ok(())
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
        // 源文件不存在 → request 无法计算缓存键 → reply 被 drop → rx 收到 Err，
        // 既不 panic 也不让调用方永久挂起。
        loader.request(
            "file:///does/not/exist.jpg".into(),
            ThumbnailSize::Small,
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
}
