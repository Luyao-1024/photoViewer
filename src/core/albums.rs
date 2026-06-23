//! 相册聚合（按 folder_path 分组）
use crate::core::db::DbPool;
use crate::core::error::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Album {
    pub folder_path: PathBuf,
    pub name: String,
    pub cover_uri: Option<String>,
    pub photo_count: i64,
    pub last_modified: DateTime<Utc>,
}

impl Album {
    /// Basename of `folder_path` (e.g. `Pictures/Vacation` → `Vacation`).
    /// Falls back to the full path string if no basename component exists.
    pub fn display_name(&self) -> String {
        self.folder_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.folder_path.display().to_string())
    }
}

/// 重新计算 albums 表（启动时 + 索引完成后调用）
pub fn refresh(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    conn.execute("DELETE FROM albums", [])?;
    conn.execute(
        "INSERT INTO albums (folder_path, name, cover_uri, photo_count, last_modified)
         SELECT
             folder_path,
             folder_path,
             (SELECT uri FROM media_items m2
              WHERE m2.folder_path = m.folder_path AND m2.trashed_at IS NULL
              ORDER BY m2.file_mtime DESC LIMIT 1),
             COUNT(*),
             MAX(file_mtime)
         FROM media_items m
         WHERE trashed_at IS NULL
         GROUP BY folder_path",
        [],
    )?;
    Ok(())
}

/// 列出所有相册，按最近修改排序
pub fn list(pool: &DbPool) -> Result<Vec<Album>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT folder_path, name, cover_uri, photo_count, last_modified
         FROM albums ORDER BY last_modified DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let last_modified: i64 = row.get(4)?;
        Ok(Album {
            folder_path: PathBuf::from(path),
            name: row.get(1)?,
            cover_uri: row.get(2)?,
            photo_count: row.get(3)?,
            last_modified: chrono::DateTime::from_timestamp(last_modified, 0)
                .unwrap_or_else(Utc::now),
        })
    })?;
    Ok(rows.filter_map(std::result::Result::ok).collect())
}

/// 按 `folder_path` 查单个 album;未找到时返回 `Ok(None)`。
///
/// 比 `list` 后再过滤更轻量,适合 picker / 单目标选择场景。
/// 注意 `folder_path` 在 schema 中是 `TEXT PRIMARY KEY` —— 直接等值查找走主键索引。
pub fn find_by_folder_path(pool: &DbPool, folder: &Path) -> Result<Option<Album>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT folder_path, name, cover_uri, photo_count, last_modified
         FROM albums WHERE folder_path = ?1",
    )?;
    let result = stmt.query_row([folder.to_string_lossy()], |row| {
        let path: String = row.get(0)?;
        let last_modified: i64 = row.get(4)?;
        Ok(Album {
            folder_path: PathBuf::from(path),
            name: row.get(1)?,
            cover_uri: row.get(2)?,
            photo_count: row.get(3)?,
            last_modified: chrono::DateTime::from_timestamp(last_modified, 0)
                .unwrap_or_else(Utc::now),
        })
    });
    // `QueryReturnedNoRows` → Ok(None),其它错误照旧上抛
    Ok(result.ok())
}
