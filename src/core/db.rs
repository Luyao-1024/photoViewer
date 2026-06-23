//! SQLite 连接池与迁移管理
use crate::core::error::{AppError, Result};
use crate::core::media::{MediaItem, NewMediaItem};
use chrono::{DateTime, TimeZone, Utc};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::types::Type;
use std::path::Path;

pub type DbPool = Pool<SqliteConnectionManager>;

const SCHEMA_SQL: &str = include_str!("schema.sql");

/// 初始化数据库连接池；如不存在则创建并运行迁移
pub fn init_pool(path: &Path) -> Result<DbPool> {
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
