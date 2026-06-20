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
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

/// 缩略图尺寸档位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThumbnailSize {
    Small,  // 256
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

/// 单个缩略图请求
#[derive(Debug)]
pub struct ThumbnailRequest {
    pub uri: String,
    pub size: ThumbnailSize,
    pub reply: oneshot::Sender<Texture>,
}

/// 共享的 receiver 包装，便于多 worker 互斥消费
type SharedRx = Arc<Mutex<mpsc::UnboundedReceiver<ThumbnailRequest>>>;

/// 缩略图加载器单例
///
/// 内部用 mpsc 队列把请求分发给一组 worker；worker 在 tokio 阻塞线程上
/// 完成 CPU/IO 密集的解码/编码后通过 oneshot 归还 `Texture`。
pub struct ThumbnailLoader {
    pool: DbPool,
    cache_dir: PathBuf,
    tx: mpsc::UnboundedSender<ThumbnailRequest>,
    rx: SharedRx,
    mem_cache: Arc<Mutex<LruCache<String, Texture>>>,
}

impl ThumbnailLoader {
    /// 构造加载器（不自动启动 worker；调用 `spawn_workers` 启动）
    pub fn new(pool: DbPool, cache_dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        std::fs::create_dir_all(&cache_dir).ok();
        let mem_cache = Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(256).unwrap())));
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
            mem_cache,
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
            let mem_cache = self.mem_cache.clone();
            tokio::task::spawn_blocking(move || {
                worker_loop(rx, pool, cache_dir, mem_cache);
            });
        }
    }

    /// 提交一个缩略图请求
    pub fn request(&self, uri: String, size: ThumbnailSize, reply: oneshot::Sender<Texture>) {
        let _ = self.tx.send(ThumbnailRequest { uri, size, reply });
    }
}

fn worker_loop(
    rx: SharedRx,
    _pool: DbPool,
    cache_dir: PathBuf,
    mem_cache: Arc<Mutex<LruCache<String, Texture>>>,
) {
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

        let key = match cache_key(&req.uri) {
            Ok(k) => k,
            Err(e) => {
                warn!("无效 URI {}: {}", req.uri, e);
                continue;
            }
        };
        let cache_key_str = format!("{:?}:{:?}", key.0, key.1);

        // 1) 内存缓存命中
        if let Some(tex) = mem_cache
            .lock()
            .ok()
            .and_then(|mut c| c.get(&cache_key_str).cloned())
        {
            let _ = req.reply.send(tex);
            continue;
        }

        // 2) 磁盘缓存/重新生成
        match generate(&cache_dir, &req.uri, req.size) {
            Ok(path) => match Pixbuf::from_file(&path) {
                Ok(pb) => {
                    let texture = Texture::for_pixbuf(&pb);
                    if let Ok(mut cache) = mem_cache.lock() {
                        cache.put(cache_key_str, texture.clone());
                    }
                    let _ = req.reply.send(texture);
                }
                Err(e) => warn!("Pixbuf 加载失败 {:?}: {}", path, e),
            },
            Err(e) => warn!("缩略图生成失败 {}: {}", req.uri, e),
        }
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
        return Ok(cache_path);
    }

    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 使用 image crate 解码 + 缩放 + 保存 JPEG
    let img = image::open(&src_path)?;
    let thumb = img.thumbnail(size.max_dim(), size.max_dim());
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&cache_path)?);
    thumb
        .to_rgb8()
        .write_to(&mut writer, image::ImageFormat::Jpeg)?;
    Ok(cache_path)
}
