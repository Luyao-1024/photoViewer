//! 相册聚合（按 folder_path 分组）
use crate::core::db;
use crate::core::db::DbPool;
use crate::core::error::Result;
use crate::core::i18n::tr;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

pub const FAVORITES_ALBUM_PATH: &str = "__photo-viewer-favorites__";
pub const IMAGES_ALBUM_PATH: &str = "__photo-viewer-images__";
pub const VIDEOS_ALBUM_PATH: &str = "__photo-viewer-videos__";

#[derive(Debug, Clone)]
pub struct Album {
    pub folder_path: PathBuf,
    pub name: String,
    pub cover_uri: Option<String>,
    pub photo_count: i64,
    pub last_modified: DateTime<Utc>,
    pub is_virtual: bool,
}

impl Album {
    /// Basename of `folder_path` (e.g. `Pictures/Vacation` → `Vacation`).
    /// Falls back to the full path string if no basename component exists.
    pub fn display_name(&self) -> String {
        if self.is_virtual {
            return self.name.clone();
        }
        self.folder_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.folder_path.display().to_string())
    }

    pub fn is_favorites_album(&self) -> bool {
        self.is_virtual && self.folder_path == PathBuf::from(FAVORITES_ALBUM_PATH)
    }

    pub fn is_images_album(&self) -> bool {
        self.is_virtual && self.folder_path == PathBuf::from(IMAGES_ALBUM_PATH)
    }

    pub fn is_videos_album(&self) -> bool {
        self.is_virtual && self.folder_path == PathBuf::from(VIDEOS_ALBUM_PATH)
    }
}

/// 列出实体相册 + 虚拟“收藏/图片/视频”相册（放在列表首位）。
pub fn list_with_favorites(pool: &DbPool) -> Result<Vec<Album>> {
    let mut list = list(pool)?;
    list.insert(0, favorites_album(pool)?);
    list.insert(
        1,
        media_kind_album(pool, "image", IMAGES_ALBUM_PATH, tr("album.images.name"))?,
    );
    list.insert(
        2,
        media_kind_album(pool, "video", VIDEOS_ALBUM_PATH, tr("album.videos.name"))?,
    );
    Ok(list)
}

fn favorites_album(pool: &DbPool) -> Result<Album> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*),
            (SELECT uri FROM media_items m2
             WHERE m2.trashed_at IS NULL AND m2.is_favorite = 1
             ORDER BY m2.file_mtime DESC LIMIT 1),
            COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND is_favorite = 1",
    )?;
    let album = stmt.query_row([], |row| {
        let count: i64 = row.get(0)?;
        let cover_uri: Option<String> = row.get(1)?;
        let last_modified: i64 = row.get(2)?;
        Ok(Album {
            folder_path: PathBuf::from(FAVORITES_ALBUM_PATH),
            name: tr("album.favorites.name"),
            cover_uri,
            photo_count: count,
            last_modified: chrono::DateTime::from_timestamp(last_modified, 0)
                .unwrap_or_else(Utc::now),
            is_virtual: true,
        })
    })?;
    Ok(album)
}

fn media_kind_album(
    pool: &DbPool,
    media_kind: &str,
    virtual_path: &str,
    name: String,
) -> Result<Album> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*),
            (SELECT uri FROM media_items m2
             WHERE m2.trashed_at IS NULL AND m2.media_kind = ?1
             ORDER BY m2.file_mtime DESC LIMIT 1),
            COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND media_kind = ?1",
    )?;
    let album = stmt.query_row([media_kind], |row| {
        let count: i64 = row.get(0)?;
        let cover_uri: Option<String> = row.get(1)?;
        let last_modified: i64 = row.get(2)?;
        Ok(Album {
            folder_path: PathBuf::from(virtual_path),
            name,
            cover_uri,
            photo_count: count,
            last_modified: chrono::DateTime::from_timestamp(last_modified, 0)
                .unwrap_or_else(Utc::now),
            is_virtual: true,
        })
    })?;
    Ok(album)
}

/// 查询收藏媒体 ID，按收藏列表展示顺序返回。
pub fn favorite_media_ids(pool: &DbPool) -> Result<Vec<i64>> {
    db::list_favorite_media_ids(pool)
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
            is_virtual: false,
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
            is_virtual: false,
        })
    });
    // `QueryReturnedNoRows` → Ok(None),其它错误照旧上抛
    Ok(result.ok())
}
