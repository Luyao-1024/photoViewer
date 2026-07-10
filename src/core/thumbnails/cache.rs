use super::ThumbnailSize;
use gdk_pixbuf::Pixbuf;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::warn;

/// 解析 uri -> 源路径 + mtime。`mtime` 优先用调用方给的（来自 `MediaItem.file_mtime`，
/// 避免主线程 stat）；否则现场 `metadata` + `modified()` 兜底。
pub(in crate::core::thumbnails) fn resolve_src(
    uri: &str,
    mtime: Option<SystemTime>,
) -> anyhow::Result<(PathBuf, SystemTime)> {
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
pub(in crate::core::thumbnails) fn cache_key_str(
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> Option<String> {
    let (path, mtime) = resolve_src(uri, mtime).ok()?;
    Some(format!("{path:?}:{mtime:?}:{size:?}"))
}

/// 同步加载缩略图缓存文件，确保文件完全写入后再解码。
///
/// 使用 `std::fs::read` 读取整个文件到内存，然后从内存构造 Pixbuf。
/// 这避免了 gdk-pixbuf 直接读取文件时可能遇到的竞态条件（文件被写入一半）。
pub(in crate::core::thumbnails) fn load_pixbuf_sync(path: &Path) -> anyhow::Result<Pixbuf> {
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

pub(in crate::core::thumbnails) fn load_pixbuf_sync_or_remove(
    path: &Path,
) -> anyhow::Result<Pixbuf> {
    match load_pixbuf_sync(path) {
        Ok(pb) => Ok(pb),
        Err(e) => {
            if let Err(remove_err) = std::fs::remove_file(path) {
                warn!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB invalid_cache_remove_failed cache_path={} error={}",
                    path.display(),
                    remove_err
                );
            } else {
                warn!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB invalid_cache_removed cache_path={} error={}",
                    path.display(),
                    e
                );
            }
            Err(e)
        }
    }
}

pub(in crate::core::thumbnails) fn existing_cache_path(
    cache_dir: &Path,
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> anyhow::Result<Option<PathBuf>> {
    let cache_stem = cache_stem_for(cache_dir, uri, size, mtime)?;
    let webp_path = cache_stem.with_extension("webp");
    if webp_path.exists() {
        return Ok(Some(webp_path));
    }
    let jpeg_path = cache_stem.with_extension("jpg");
    if jpeg_path.exists() {
        return Ok(Some(jpeg_path));
    }
    Ok(None)
}

pub(in crate::core::thumbnails) fn cache_stem_for(
    cache_dir: &Path,
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> anyhow::Result<PathBuf> {
    let (src_path, mtime) = resolve_src(uri, mtime)?;
    // v4 invalidates caches generated before video thumbnails stopped embedding
    // the playback triangle and before failure placeholders became memory-only.
    let key = format!("thumb-v4:{}{:?}", src_path.display(), mtime);
    let hash = blake3::hash(key.as_bytes()).to_hex().to_string();
    Ok(cache_dir
        .join("thumbnails")
        .join(size.subdir())
        .join(&hash[..2])
        .join(hash.as_str()))
}
