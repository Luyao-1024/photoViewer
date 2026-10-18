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
    let src_path = crate::core::file_uri::path_or_file_uri(uri)?;
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

/// Size-independent mem-cache key for the embedded EXIF thumbnail (the low-res
/// instant placeholder). Mirrors [`cache_key_str`] but drops the size, since a
/// source file has exactly one embedded EXIF thumb regardless of grid mode.
pub(in crate::core::thumbnails) fn exif_cache_key(
    uri: &str,
    mtime: Option<SystemTime>,
) -> Option<String> {
    let (path, mtime) = resolve_src(uri, mtime).ok()?;
    Some(format!("exif:{path:?}:{mtime:?}"))
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
    let hash = disk_cache_stem_name(uri, mtime)?;
    Ok(cache_dir
        .join("thumbnails")
        .join(size.subdir())
        .join(&hash[..2])
        .join(hash.as_str()))
}

/// The size-independent on-disk cache stem for a source: one blake3 of
/// `thumb-v4:{path}{mtime}`, shared by the Small/Medium/Large bucket
/// subdirectories. Orphan purging rebuilds the same stems to decide which
/// cache files no longer belong to any media row.
pub(in crate::core::thumbnails) fn disk_cache_stem_name(
    uri: &str,
    mtime: Option<SystemTime>,
) -> anyhow::Result<String> {
    let (src_path, mtime) = resolve_src(uri, mtime)?;
    // v4 invalidates caches generated before video thumbnails stopped embedding
    // the playback triangle and before failure placeholders became memory-only.
    let key = format!("thumb-v4:{}{:?}", src_path.display(), mtime);
    Ok(blake3::hash(key.as_bytes()).to_hex().to_string())
}

/// Cache files can be evicted independently of SQLite. Reconcile only cache
/// bookkeeping on the background side; sync never decodes originals itself.
pub(super) fn recover_cache_marks(loader: &super::ThumbnailLoader) {
    let pool = loader.pool.clone();
    let cache_dir = loader.cache_dir.clone();
    let actor = loader.db_actor.lock().ok().and_then(|slot| slot.clone());
    let queue = loader.queue.clone();
    std::thread::spawn(move || {
        purge_orphan_thumbnails(&pool, &cache_dir);
        let mut offset = 0;
        loop {
            if queue.0.lock().is_ok_and(|q| q.closed) {
                break;
            }
            let page = match crate::core::db::list_media_page(&pool, offset, 128) {
                Ok(page) if !page.is_empty() => page,
                Ok(_) => break,
                Err(error) => {
                    warn!("thumbnail cache bookkeeping recovery failed: {error}");
                    break;
                }
            };
            offset += page.len() as u32;
            let mut present = Vec::new();
            let mut missing = Vec::new();
            for item in page
                .into_iter()
                .filter(|item| item.storage_location.has_local_original())
            {
                let mtime = SystemTime::from(item.file_mtime);
                // Do not restore a cache for an external edit the scanner has
                // not observed yet. The actor also checks the captured DB row.
                let unchanged = std::fs::metadata(&item.path).is_ok_and(|meta| {
                    meta.is_file()
                        && meta.len() == item.file_size
                        && meta.modified().is_ok_and(|modified| modified == mtime)
                });
                if !unchanged {
                    continue;
                }
                let caches = [
                    ThumbnailSize::Large,
                    ThumbnailSize::Medium,
                    ThumbnailSize::Small,
                ]
                .map(|size| existing_cache_path(&cache_dir, &item.uri, size, Some(mtime)));
                if caches.iter().any(|cache| matches!(cache, Ok(Some(_)))) {
                    if !item.thumbnail_ready {
                        present.push(item);
                    }
                } else if caches.iter().all(|cache| matches!(cache, Ok(None))) {
                    missing.push(item);
                }
            }
            let mut failed = false;
            for (items, ready) in [(present, true), (missing, false)] {
                if items.is_empty() {
                    continue;
                }
                let result = match &actor {
                    Some(actor) => actor
                        .execute_blocking(
                            crate::core::db_actor::DbCommand::ReconcileThumbnailMarks {
                                items,
                                ready,
                            },
                        )
                        .map(|_| ()),
                    None => crate::core::db::reconcile_thumbnail_marks(&pool, &items, ready),
                };
                if let Err(error) = result {
                    warn!("thumbnail cache bookkeeping recovery failed: {error}");
                    failed = true;
                    break;
                }
            }
            if failed {
                break;
            }
            queue.1.notify_all();
        }
    });
}

/// Delete thumbnail cache files whose source media row no longer exists.
///
/// Rows disappear through several paths (startup reconciliation, watcher
/// deletions, trash cleanup, library/account resets) and none of them touch
/// the disk cache; capacity eviction is the only other reclamation and it
/// fires just once the cache outgrows its budget. This one-shot walk runs at
/// background-prewarm startup, before cache mark recovery, and removes every
/// cache file whose `thumb-v4:{path}{mtime}` stem matches no row in
/// `media_items` — keyed off plain table membership, so trashed rows and
/// cloud-only rows keep their (downloaded) thumbnails.
///
/// The stem set is a snapshot: a row indexed after the snapshot whose cache
/// file lands before the walk loses one regenerable file. Prewarm starts
/// after the startup scan, so newly indexed rows are normally in the set
/// already; a missed cache regenerates on the next request.
pub(super) fn purge_orphan_thumbnails(pool: &crate::core::db::DbPool, cache_dir: &Path) {
    let rows = match crate::core::db::list_media_uri_mtimes(pool) {
        Ok(rows) => rows,
        Err(error) => {
            warn!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB orphan_cache_purge_skipped error={error}"
            );
            return;
        }
    };
    let mut live_stems = std::collections::HashSet::with_capacity(rows.len());
    for (uri, mtime) in rows.iter() {
        if let Some(stem) = mtime.and_then(|mtime| disk_cache_stem_name(uri, Some(mtime)).ok()) {
            live_stems.insert(stem);
        }
    }
    let mut removed = 0usize;
    for entry in walkdir::WalkDir::new(cache_dir.join("thumbnails"))
        .min_depth(1)
        .into_iter()
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(stem) = entry.path().file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if live_stems.contains(stem) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(error) => warn!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB orphan_cache_remove_failed cache_path={} error={}",
                entry.path().display(),
                error
            ),
        }
    }
    if removed > 0 {
        tracing::info!(
            target: crate::core::log_targets::THUMBNAILS,
            "THUMB orphan_cache_purged count={removed}"
        );
    }
}
