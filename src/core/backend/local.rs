//! 本地文件系统扫描后端
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::media::NewMediaItem;
use crate::core::metadata;
use chrono::Utc;
use std::path::Path;
use walkdir::WalkDir;

const SUPPORTED_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "heic", "heif"];

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

        let hash = blake3::hash(&std::fs::read(path)?).to_hex().to_string();

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

    /// 插入或更新（URI 冲突则 UPDATE）
    pub fn upsert(&self, item: &NewMediaItem) -> Result<i64> {
        let conn = self.pool.get()?;

        // 检查是否存在
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
            Ok(id)
        } else {
            Ok(db::insert_media_item(&self.pool, item)?)
        }
    }
}
