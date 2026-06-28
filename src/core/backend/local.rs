//! 本地文件系统扫描后端
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::media::{
    is_supported_media_path, MediaItem, NewMediaItem, MEDIA_SUBKIND_MOTION_PHOTO,
    MEDIA_SUBKIND_STANDARD,
};
use crate::core::metadata;
use crate::core::motion_photo::{self, MediaAttributes};
use chrono::Utc;
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;

fn stream_file_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 文件的索引时间信号：created 优先，失败回退 modified。
///
/// `process_file` 存入 DB 的 `file_mtime` 与 `scan_and_upsert_dir` 的跳过
/// 判断**必须**用同一套逻辑，否则会出现「存的与比的口径不一致」导致永不命中。
fn file_index_time(meta: &std::fs::Metadata) -> Option<std::time::SystemTime> {
    meta.created().or_else(|_| meta.modified()).ok()
}

pub struct LocalBackend {
    pool: DbPool,
}

impl LocalBackend {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// 返回内部连接池的引用，供 `notify_watcher` 在事件处理中调用
    /// `albums::refresh(&pool)` 同步刷新物化视图。
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// 递归扫描目录，返回所有支持的媒体项（**不做跳过**，逐个全量提取 + 全文件
    /// 哈希）。供需要完整 `NewMediaItem` 列表的场景（测试、相册预处理）使用。
    /// 启动扫描请用 [`Self::scan_and_upsert_dir`]，它会跳过未改动文件以避免
    /// 重复哈希。
    pub fn scan_dir(&self, root: &Path) -> Result<Vec<NewMediaItem>> {
        let mut items = Vec::new();
        for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() || !is_supported_media_path(path) {
                continue;
            }
            match self.process_file(path) {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {} // 不支持的 MIME
                Err(e) => tracing::warn!("跳过文件 {}: {}", path.display(), e),
            }
        }
        Ok(items)
    }

    /// 启动扫描入口：遍历 `root`，对每个媒体文件 upsert，但**先用 `(uri, file_mtime,
    /// file_size)` 查库短路**——未改动的文件直接跳过，不读全文件做 blake3、不重提
    /// EXIF。返回实际（重新）索引的文件数。
    ///
    /// 这把「每次启动对整个图库重复哈希」（1.8GB → 数秒）降到「逐文件 stat + 一次
    /// 索引查询」（毫秒级），除非文件真的新增/改动。
    pub fn scan_and_upsert_dir(&self, root: &Path) -> Result<usize> {
        self.scan_and_upsert_dir_with(root, |_| {})
    }

    /// 与 [`Self::scan_and_upsert_dir`] 相同，但每个实际 upsert 的项目都会传给
    /// `on_upserted`。用于启动后台扫描把新增/变更项增量推给 UI，同时仍保留
    /// `(uri, file_mtime, file_size)` 未改动短路。
    pub fn scan_and_upsert_dir_notify<F>(&self, root: &Path, mut on_upserted: F) -> Result<usize>
    where
        F: FnMut(MediaItem),
    {
        self.scan_and_upsert_dir_with(root, |item| on_upserted(item))
    }

    fn scan_and_upsert_dir_with<F>(&self, root: &Path, mut on_upserted: F) -> Result<usize>
    where
        F: FnMut(MediaItem),
    {
        let mut indexed = 0usize;
        for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() || !is_supported_media_path(path) {
                continue;
            }

            let file_meta = match std::fs::metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("跳过文件 {}: {}", path.display(), e);
                    continue;
                }
            };
            // 廉价的改动检测：uri + mtime(秒) + size 全部一致即视为未改动。
            if let Some(mtime) = file_index_time(&file_meta).and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs() as i64)
            }) {
                let uri = format!("file://{}", path.display());
                if db::is_media_unchanged(&self.pool, &uri, mtime, file_meta.len() as i64)? {
                    continue;
                }
            }

            match self.process_file(path) {
                Ok(Some(item)) => {
                    let upserted = self.upsert(&item)?;
                    on_upserted(upserted);
                    indexed += 1;
                }
                Ok(None) => {} // 不支持的 MIME
                Err(e) => tracing::warn!("跳过文件 {}: {}", path.display(), e),
            }
        }
        Ok(indexed)
    }

    fn process_file(&self, path: &Path) -> Result<Option<NewMediaItem>> {
        let meta = metadata::extract(path)?;

        let file_meta = std::fs::metadata(path)?;
        let file_time = file_index_time(&file_meta).unwrap_or_else(std::time::SystemTime::now);
        let file_time_utc: chrono::DateTime<Utc> = file_time.into();

        let uri = format!("file://{}", path.display());
        let folder = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new("/").to_path_buf());

        let hash = stream_file_hash(path)?;
        let motion_photo = motion_photo::detect(path);

        Ok(Some(NewMediaItem {
            uri,
            path: path.to_path_buf(),
            folder_path: folder,
            mime_type: meta.mime_type,
            media_subkind: if motion_photo.is_some() {
                MEDIA_SUBKIND_MOTION_PHOTO.into()
            } else {
                MEDIA_SUBKIND_STANDARD.into()
            },
            media_attributes: motion_photo
                .map(MediaAttributes::motion_photo_json)
                .unwrap_or_else(MediaAttributes::standard_json),
            width: meta.width,
            height: meta.height,
            taken_at: meta.taken_at,
            file_mtime: file_time_utc,
            file_size: file_meta.len(),
            blake3_hash: hash,
        }))
    }

    /// 与 [`Self::process_file`] 同样的元数据/哈希提取，但把结果的 `uri` / `path` /
    /// `folder_path` 覆盖成 `uri` / `path` / `folder`，而非 `source` 自身的路径。
    ///
    /// 供回收站对账使用：被外部删除的图片物理上在 `Trash/files/<name>`，但其 DB 行
    /// 必须记原始位置（缩略图解析、还原都靠原始 `uri` 找 `.trashinfo`），所以从
    /// 回收站副本读元数据、写到原始路径下。
    pub fn process_file_at(
        &self,
        source: &Path,
        uri: &str,
        path: &Path,
        folder: &Path,
    ) -> Result<NewMediaItem> {
        let meta = metadata::extract(source)?;
        let file_meta = std::fs::metadata(source)?;
        let file_time = file_index_time(&file_meta).unwrap_or_else(std::time::SystemTime::now);
        let file_time_utc: chrono::DateTime<Utc> = file_time.into();
        let hash = stream_file_hash(source)?;
        let motion_photo = motion_photo::detect(source);
        Ok(NewMediaItem {
            uri: uri.to_string(),
            path: path.to_path_buf(),
            folder_path: folder.to_path_buf(),
            mime_type: meta.mime_type,
            media_subkind: if motion_photo.is_some() {
                MEDIA_SUBKIND_MOTION_PHOTO.into()
            } else {
                MEDIA_SUBKIND_STANDARD.into()
            },
            media_attributes: motion_photo
                .map(MediaAttributes::motion_photo_json)
                .unwrap_or_else(MediaAttributes::standard_json),
            width: meta.width,
            height: meta.height,
            taken_at: meta.taken_at,
            file_mtime: file_time_utc,
            file_size: file_meta.len(),
            blake3_hash: hash,
        })
    }

    /// 从单个文件路径提取元数据并 upsert 到数据库。
    ///
    /// 专为 `notify_watcher` 等增量入口设计：
    ///   - 路径不是文件（目录事件、临时消失等）时返回 `Ok(None)`；
    ///   - 解析失败时返回错误，调用方负责记录日志；
    ///   - upsert 成功时返回 `Ok(Some(MediaItem))`，调用方可以直接转发给
    ///     `MediaChangeNotifier` 而无需再次查询 DB。
    pub fn upsert_from_path(&self, path: &Path) -> Result<Option<MediaItem>> {
        if !path.is_file() {
            return Ok(None);
        }
        let item = self.process_file(path)?.ok_or_else(|| {
            AppError::Decode(format!("not a supported media: {}", path.display()))
        })?;
        self.upsert(&item).map(Some)
    }

    /// 删除指定路径对应的索引行，供文件监听的 remove/rename 事件使用。
    pub fn delete_path(&self, path: &Path) -> Result<usize> {
        db::delete_media_by_path(&self.pool, path)
    }

    /// Insert or update (URI conflict → UPDATE). Returns the fully-materialized
    /// row so callers (notably `notify_watcher`) can forward it to the UI
    /// without a second DB round-trip.
    ///
    /// 更新既有行时一并清空 `trashed_at`：upsert 只在文件确实存在于（原）路径时
    /// 被调用——一个仍标记为回收站的行此刻文件却在原路径，只可能是被外部从系统
    /// 回收站还原了，应重新视为 live，否则还原后的图片不会回到相册、也不会从
    /// 回收站视图消失。
    pub fn upsert(&self, item: &NewMediaItem) -> Result<MediaItem> {
        let conn = self.pool.get()?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM media_items WHERE uri = ?1",
                [&item.uri],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            conn.execute(
                "UPDATE media_items
                 SET path=?2, folder_path=?3, mime_type=?4, media_kind=?5,
                     media_subkind=?6, media_attributes=?7, width=?8, height=?9,
                     taken_at=?10, file_mtime=?11, file_size=?12, blake3_hash=?13,
                     trashed_at=NULL, indexed_at=unixepoch()
                 WHERE id=?1",
                rusqlite::params![
                    id,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
                    db::media_kind_db_value(&item.mime_type),
                    item.media_subkind,
                    item.media_attributes,
                    item.width,
                    item.height,
                    item.taken_at.map(|t| t.timestamp()),
                    item.file_mtime.timestamp(),
                    item.file_size as i64,
                    item.blake3_hash,
                ],
            )?;
            drop(conn);
            Ok(db::get_media_item(&self.pool, id)?)
        } else {
            let id = db::insert_media_item(&self.pool, item)?;
            Ok(db::get_media_item(&self.pool, id)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn stream_file_hash_matches_blake3_hash_for_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large-ish.bin");
        let mut file = std::fs::File::create(&path).unwrap();
        for i in 0..4096_u32 {
            file.write_all(&i.to_le_bytes()).unwrap();
        }
        drop(file);

        let bytes = std::fs::read(&path).unwrap();
        let expected = blake3::hash(&bytes).to_hex().to_string();

        assert_eq!(stream_file_hash(&path).unwrap(), expected);
    }

    #[test]
    fn upsert_returns_inserted_media_item_with_populated_id() {
        use crate::core::media::NewMediaItem;
        use chrono::Utc;

        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "x.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let new_item = NewMediaItem {
            uri: format!("file://{}", path.display()),
            path: path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(64),
            height: Some(48),
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: std::fs::metadata(&path).unwrap().len(),
            blake3_hash: "placeholder".into(),
        };

        let returned = backend.upsert(&new_item).expect("upsert should succeed");
        assert!(
            returned.id > 0,
            "returned MediaItem must have a populated id"
        );
        assert_eq!(returned.uri, new_item.uri);
        assert_eq!(returned.blake3_hash, "placeholder");
    }

    #[test]
    fn upsert_from_path_returns_inserted_media_item() {
        let dir = tempfile::tempdir().unwrap();
        let _path = write_plain_jpeg_in(dir.path(), "new.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let returned = backend
            .upsert_from_path(&dir.path().join("new.jpg"))
            .expect("upsert_from_path should succeed");
        let item = returned.expect("expected Some(MediaItem) for a valid jpeg");
        assert!(item.id > 0);
        assert!(item.path.ends_with("new.jpg"));
    }

    #[test]
    fn upsert_from_path_returns_none_for_directory_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let returned = backend
            .upsert_from_path(&dir.path().join("subdir"))
            .expect("directory path should not error");
        assert!(
            returned.is_none(),
            "directory path must yield None, not Some"
        );
    }

    #[test]
    fn upsert_from_path_returns_updated_item_for_existing_uri() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "dup.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let first = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("first upsert must yield Some");
        // Re-write the file with different (still-valid) image content to
        // change the blake3 hash while preserving EXIF-decodable bytes.
        write_distinct_jpeg_in(&path, 32, 32, [255, 0, 0]);
        let second = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("second upsert must yield Some");
        assert_eq!(first.id, second.id, "upsert must reuse the same id");
        assert_ne!(
            first.blake3_hash, second.blake3_hash,
            "second upsert must reflect new content"
        );
    }

    /// Test-only helper: write a 64x48 plain JPEG (mirrors
    /// `tests/common/mod.rs::write_plain_jpeg` without requiring that
    /// module to be in scope for the lib's own test binary).
    fn write_plain_jpeg_in(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        use image::{ImageBuffer, Rgb};
        let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| Rgb([128, 128, 128]));
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    /// Test-only helper: overwrite an existing JPEG with a different-sized,
    /// different-colored image so its blake3 hash differs but EXIF decoding
    /// still succeeds.
    fn write_distinct_jpeg_in(path: &std::path::Path, w: u32, h: u32, color: [u8; 3]) {
        use image::{ImageBuffer, Rgb};
        let img = ImageBuffer::<Rgb<u8>, _>::from_fn(w, h, |_, _| Rgb(color));
        img.save(path).unwrap();
    }

    fn append_micro_video_tail(path: &std::path::Path, video_len: usize) {
        let mut bytes = std::fs::read(path).unwrap();
        let xmp = format!(
            r#"<x:xmpmeta><rdf:Description GCamera:MicroVideo="1" GCamera:MicroVideoOffset="{video_len}" GCamera:MicroVideoPresentationTimestampUs="123456"/></x:xmpmeta>"#
        );
        bytes.extend_from_slice(xmp.as_bytes());
        let mut video = vec![0_u8; video_len];
        video[0..4].copy_from_slice(&(24_u32.to_be_bytes()));
        video[4..8].copy_from_slice(b"ftyp");
        video[8..12].copy_from_slice(b"mp42");
        bytes.extend_from_slice(&video);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn upsert_from_path_persists_motion_photo_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "motion.jpg");
        append_micro_video_tail(&path, 96);
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool);

        let item = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("motion photo jpeg should be indexed");

        assert_eq!(item.media_subkind, MEDIA_SUBKIND_MOTION_PHOTO);
        let attrs = motion_photo::MediaAttributes::from_json(&item.media_attributes);
        let info = attrs
            .motion_photo
            .expect("motion photo attributes should be persisted");
        assert_eq!(info.video_length, 96);
        assert_eq!(info.presentation_timestamp_us, Some(123_456));
    }

    /// 文件监听器看到被外部还原的文件重新出现在原路径 → `upsert_from_path`。
    /// 此刻行仍是 trashed，upsert 必须清掉 `trashed_at`，否则图片既不回相册、
    /// 也赖在回收站视图里。
    #[test]
    fn upsert_clears_trashed_at_when_file_reappears() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "restored.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let item = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("initial upsert must yield Some");
        crate::core::db::mark_trashed(&pool, item.id).unwrap();
        assert!(
            crate::core::db::get_media_item(&pool, item.id)
                .unwrap()
                .trashed_at
                .is_some(),
            "precondition: row must be trashed"
        );

        // 模拟文件被外部还原后监听器收到的 Create 事件
        let restored = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("re-upsert must yield Some");
        assert_eq!(restored.id, item.id);
        assert!(
            restored.trashed_at.is_none(),
            "upsert of a reappearing file must clear trashed_at (external restore)"
        );
        assert!(
            crate::core::db::list_trashed_media(&pool)
                .unwrap()
                .is_empty(),
            "restored item must no longer be in the trash list"
        );
    }

    /// 启动扫描路径：文件在应用关闭期间被外部还原。即便 mtime/size 与索引时
    /// 完全一致，`is_media_unchanged` 也不能对 trashed 行短路——否则扫描会跳过
    /// 它，`trashed_at` 永远清不掉。
    #[test]
    fn scan_reindexes_restored_file_clearing_trashed_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "scan-restored.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let item = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("initial upsert must yield Some");
        crate::core::db::mark_trashed(&pool, item.id).unwrap();

        // 文件仍在原路径（被外部还原），mtime/size 未变
        backend.scan_and_upsert_dir(dir.path()).unwrap();

        let after = crate::core::db::get_media_item(&pool, item.id).unwrap();
        assert!(
            after.trashed_at.is_none(),
            "startup scan must re-index a restored (present) file and clear trashed_at"
        );
    }
}
