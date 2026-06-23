//! SQLite 连接池与迁移管理
use crate::core::error::{AppError, Result};
use crate::core::media::{MediaItem, NewMediaItem};
use chrono::{DateTime, TimeZone, Utc};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::types::Type;
use rusqlite::OptionalExtension;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub type DbPool = Pool<SqliteConnectionManager>;

const SCHEMA_SQL: &str = include_str!("schema.sql");

/// 初始化数据库连接池；如不存在则创建并运行迁移。
///
/// 若打开或迁移失败（DB 文件损坏 / 迁移 SQL 因不兼容报错），
/// 删除 `.db` / `.db-wal` / `.db-shm` 后重新创建一次。应用尚未对外
/// 发布，允许通过删库换取自愈。注意：本函数**不**主动比对列名/类型，
/// 不会因"字段看上去不一致"就误删库——只有 SQLite 自身真正失败才会触发。
pub fn init_pool(path: &Path) -> Result<DbPool> {
    match try_open_and_migrate(path) {
        Ok(pool) => Ok(pool),
        Err(err) => {
            tracing::warn!(
                "DB at {} failed to open/migrate ({}); deleting and regenerating.",
                path.display(),
                err
            );
            remove_db_files(path)?;
            try_open_and_migrate(path).map_err(|e| {
                AppError::Backend(format!(
                    "failed to regenerate DB at {}: {e}",
                    path.display()
                ))
            })
        }
    }
}

/// 打开连接池 + 跑 schema 迁移。任一步出错都会让上层走重建分支。
fn try_open_and_migrate(path: &Path) -> Result<DbPool> {
    let manager = SqliteConnectionManager::file(path).with_init(|c| {
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA synchronous = NORMAL;",
        )
    });
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(AppError::from)?;
    run_migrations(&pool)?;
    Ok(pool)
}

/// 删除 `path` 对应的 SQLite 主文件 + WAL/SHM 副本。文件不存在视为成功。
fn remove_db_files(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let candidate: PathBuf = if suffix.is_empty() {
            path.to_path_buf()
        } else {
            let mut name: OsString = path.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        };
        match std::fs::remove_file(&candidate) {
            Ok(()) => tracing::info!("removed legacy DB file: {}", candidate.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::Io(e)),
        }
    }
    Ok(())
}

/// 执行 schema.sql 迁移（幂等）
pub fn run_migrations(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(())
}

fn ts(dt: DateTime<Utc>) -> i64 {
    dt.timestamp()
}

fn from_ts(ts: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(ts, 0).single()
}

fn required_ts(ts: i64, col: usize) -> rusqlite::Result<DateTime<Utc>> {
    from_ts(ts).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            col,
            Type::Integer,
            format!("invalid timestamp: {ts}").into(),
        )
    })
}

fn optional_ts(ts: Option<i64>, col: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    ts.map(|value| required_ts(value, col)).transpose()
}

/// 插入新项，返回自增 id
pub fn insert_media_item(pool: &DbPool, item: &NewMediaItem) -> Result<i64> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO media_items
            (uri, path, folder_path, mime_type, width, height,
             taken_at, file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, unixepoch())",
        rusqlite::params![
            item.uri,
            item.path.to_string_lossy(),
            item.folder_path.to_string_lossy(),
            item.mime_type,
            item.width,
            item.height,
            item.taken_at.map(ts),
            ts(item.file_mtime),
            item.file_size as i64,
            item.blake3_hash,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 根据 id 查询
pub fn get_media_item(pool: &DbPool, id: i64) -> Result<MediaItem> {
    let conn = pool.get()?;
    let item = conn.query_row(
        "SELECT id, uri, path, folder_path, mime_type, width, height,
                taken_at, file_mtime, file_size, blake3_hash, trashed_at
         FROM media_items WHERE id = ?1",
        [id],
        row_to_media_item,
    )?;
    Ok(item)
}

/// 列出所有非回收站项，按照片排序时间 DESC 排序：
/// EXIF 拍摄时间优先；没有 EXIF 时使用文件侧时间（created/mtime fallback）。
pub fn list_all_media(pool: &DbPool) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, width, height,
                taken_at, file_mtime, file_size, blake3_hash, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC",
    )?;
    let rows = stmt.query_map([], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// 分页列出非回收站项，排序语义与 [`list_all_media`] 一致。
pub fn list_media_page(pool: &DbPool, offset: u32, limit: u32) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, width, height,
                taken_at, file_mtime, file_size, blake3_hash, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map([limit as i64, offset as i64], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// 删除单行
pub fn delete_media_item(pool: &DbPool, id: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute("DELETE FROM media_items WHERE id = ?1", [id])?;
    Ok(())
}

/// 删除指定本地路径对应的媒体行。返回受影响行数。
pub fn delete_media_by_path(pool: &DbPool, path: &Path) -> Result<usize> {
    let conn = pool.get()?;
    let uri = format!("file://{}", path.display());
    let changed = conn.execute(
        "DELETE FROM media_items WHERE path = ?1 OR uri = ?2",
        rusqlite::params![path.to_string_lossy(), uri],
    )?;
    Ok(changed)
}

/// 标记为已删除（不立即物理删除）
pub fn mark_trashed(pool: &DbPool, id: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE media_items SET trashed_at = unixepoch() WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// 取消回收站标记
pub fn unmark_trashed(pool: &DbPool, id: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE media_items SET trashed_at = NULL WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// 移动语义下原地更新 path / folder_path / uri / file_mtime。
///
/// `id` 与 `blake3_hash` 保持不变（仍是同一张照片）;只把磁盘位置同步到
/// `media_items` 行,以便随后的 `list_all_media` / `albums::refresh` 看见
/// 新位置。`file_mtime` 保存文件侧排序时间（created 优先, modified
/// fallback）,失败时回退当前时间,避免出现 NULL。
pub fn update_media_location(
    pool: &DbPool,
    id: i64,
    new_path: &Path,
    new_folder: &Path,
) -> Result<()> {
    let conn = pool.get()?;
    let uri = format!("file://{}", new_path.display());
    let file_time = std::fs::metadata(new_path)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    conn.execute(
        "UPDATE media_items
         SET path = ?2, folder_path = ?3, uri = ?4, file_mtime = ?5
         WHERE id = ?1",
        rusqlite::params![
            id,
            new_path.to_string_lossy(),
            new_folder.to_string_lossy(),
            uri,
            file_time
        ],
    )?;
    Ok(())
}

/// 列出所有回收站中项
pub fn list_trashed_media(pool: &DbPool) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, width, height,
                taken_at, file_mtime, file_size, blake3_hash, trashed_at
         FROM media_items
         WHERE trashed_at IS NOT NULL
         ORDER BY trashed_at DESC",
    )?;
    let rows = stmt.query_map([], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// 查询媒体是否已收藏。找不到 id 则返回错误。
pub fn is_media_favorite(pool: &DbPool, media_id: i64) -> Result<bool> {
    let conn = pool.get()?;
    let value: Option<i64> = conn
        .query_row(
            "SELECT is_favorite FROM media_items WHERE id = ?1",
            [media_id],
            |row| row.get(0),
        )
        .optional()?;
    value
        .map(|v| v == 1)
        .ok_or_else(|| AppError::Backend(format!("media item not found: {media_id}")))
}

/// 设置单张媒体收藏状态。
pub fn set_media_favorite(pool: &DbPool, media_id: i64, is_favorite: bool) -> Result<()> {
    let conn = pool.get()?;
    let changed = conn.execute(
        "UPDATE media_items SET is_favorite = ?2 WHERE id = ?1",
        rusqlite::params![media_id, if is_favorite { 1 } else { 0 }],
    )?;
    if changed == 0 {
        return Err(AppError::Backend(format!(
            "failed to update favorite flag: media item not found {media_id}"
        )));
    }
    Ok(())
}

/// 列出所有未删除的收藏媒体 ID，按 `file_mtime` 倒序。
pub fn list_favorite_media_ids(pool: &DbPool) -> Result<Vec<i64>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id
         FROM media_items
         WHERE trashed_at IS NULL
           AND is_favorite = 1
         ORDER BY file_mtime DESC",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

fn row_to_media_item(row: &rusqlite::Row) -> rusqlite::Result<MediaItem> {
    let taken_at: Option<i64> = row.get(7)?;
    let file_mtime: i64 = row.get(8)?;
    let trashed_at: Option<i64> = row.get(11)?;

    Ok(MediaItem {
        id: row.get(0)?,
        uri: row.get(1)?,
        path: std::path::PathBuf::from(row.get::<_, String>(2)?),
        folder_path: std::path::PathBuf::from(row.get::<_, String>(3)?),
        mime_type: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        taken_at: optional_ts(taken_at, 7)?,
        file_mtime: required_ts(file_mtime, 8)?,
        file_size: row.get::<_, i64>(9)? as u64,
        blake3_hash: row.get(10)?,
        trashed_at: optional_ts(trashed_at, 11)?,
    })
}
