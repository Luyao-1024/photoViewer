//! 本地文件系统扫描后端
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::media::{MediaItem, NewMediaItem};
use crate::core::metadata;
use chrono::Utc;
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;

const SUPPORTED_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "heic", "heif"];

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

pub struct LocalBackend {
    pool: DbPool,
}

impl LocalBackend {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// 递归扫描目录，返回所有支持的图片项
    pub fn scan_dir(&self, root: &Path) -> Result<Vec<NewMediaItem>> {
        let mut items = Vec::new();

        for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_ascii_lowercase(),
                None => continue,
            };
            if !SUPPORTED_EXT.contains(&ext.as_str()) {
                continue;
            }

            match self.process_file(path) {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {} // 不支持的 MIME
                Err(e) => {
                    tracing::warn!("跳过文件 {}: {}", path.display(), e);
                }
            }
        }
        Ok(items)
    }

    fn process_file(&self, path: &Path) -> Result<Option<NewMediaItem>> {
        let meta = metadata::extract(path)?;

        let file_meta = std::fs::metadata(path)?;
        let file_time = file_meta
            .created()
            .or_else(|_| file_meta.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now());
        let file_time_utc: chrono::DateTime<Utc> = file_time.into();

        let uri = format!("file://{}", path.display());
        let folder = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new("/").to_path_buf());

        let hash = stream_file_hash(path)?;

        Ok(Some(NewMediaItem {
            uri,
            path: path.to_path_buf(),
            folder_path: folder,
            mime_type: meta.mime_type,
            width: meta.width,
            height: meta.height,
            taken_at: meta.taken_at,
            file_mtime: file_time_utc,
            file_size: file_meta.len(),
            blake3_hash: hash,
        }))
    }

    /// 从单个文件路径提取元数据并 upsert 到数据库。
    ///
    /// 专为 `notify_watcher` 等增量入口设计：
    ///   - 路径不是文件（目录事件、临时消失等）时静默返回 `Ok(())`；
    ///   - 解析失败时返回错误，调用方负责记录日志。
    pub fn upsert_from_path(&self, path: &Path) -> Result<()> {
        if !path.is_file() {
            return Ok(());
        }
        let item = self
            .process_file(path)?
            .ok_or_else(|| AppError::Decode(format!("not an image: {}", path.display())))?;
        self.upsert(&item).map(|_| ())
    }

    /// 删除指定路径对应的索引行，供文件监听的 remove/rename 事件使用。
    pub fn delete_path(&self, path: &Path) -> Result<usize> {
        db::delete_media_by_path(&self.pool, path)
    }

    /// Insert or update (URI conflict → UPDATE). Returns the fully-materialized
    /// row so callers (notably `notify_watcher`) can forward it to the UI
    /// without a second DB round-trip.
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
                 SET path=?2, folder_path=?3, mime_type=?4, width=?5,
                     height=?6, taken_at=?7, file_mtime=?8, file_size=?9,
                     blake3_hash=?10, indexed_at=unixepoch()
                 WHERE id=?1",
                rusqlite::params![
                    id,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
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
}
