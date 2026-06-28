//! SQLite 连接池与迁移管理
use crate::core::error::{AppError, Result};
use crate::core::media::{media_kind_from_mime, MediaItem, MediaKind, NewMediaItem};
use chrono::{DateTime, TimeZone, Utc};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::types::Type;
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub type DbPool = Pool<SqliteConnectionManager>;

const SCHEMA_SQL: &str = include_str!("schema.sql");
const REQUIRED_MEDIA_ITEM_COLUMNS: &[&str] = &[
    "id",
    "uri",
    "path",
    "folder_path",
    "mime_type",
    "media_kind",
    "media_subkind",
    "media_attributes",
    "width",
    "height",
    "video_duration_secs",
    "taken_at",
    "file_mtime",
    "file_size",
    "blake3_hash",
    "is_favorite",
    "trashed_at",
    "indexed_at",
];

/// 初始化数据库连接池；如不存在则创建并运行迁移。
///
/// 若打开或迁移失败（DB 文件损坏 / 迁移 SQL 因不兼容报错），
/// 删除 `.db` / `.db-wal` / `.db-shm` 后重新创建一次。应用尚未对外
/// 发布，允许通过删库换取自愈。默认只做「必要列」一致性校验：若缺关键列，
/// 则触发重建。
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
    ensure_thumbnail_generated_column(&pool)?;
    validate_media_schema(&pool)?;
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

fn validate_media_schema(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("PRAGMA table_info(media_items)")?;
    let existing: HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;

    let mut missing = Vec::new();
    for col in REQUIRED_MEDIA_ITEM_COLUMNS {
        if !existing.contains(*col) {
            missing.push(*col);
        }
    }
    if !missing.is_empty() {
        return Err(AppError::Backend(format!(
            "database schema mismatch: media_items missing required columns: {}",
            missing.join(", ")
        )));
    }

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

pub fn media_kind_db_value(mime_type: &str) -> &'static str {
    media_kind_from_mime(mime_type)
        .unwrap_or(MediaKind::Image)
        .as_db_value()
}

/// 插入新项，返回自增 id
pub fn insert_media_item(pool: &DbPool, item: &NewMediaItem) -> Result<i64> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO media_items
            (uri, path, folder_path, mime_type, media_kind, media_subkind,
             media_attributes, width, height, video_duration_secs, taken_at,
             file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, unixepoch())",
        rusqlite::params![
            item.uri,
            item.path.to_string_lossy(),
            item.folder_path.to_string_lossy(),
            item.mime_type,
            media_kind_db_value(&item.mime_type),
            item.media_subkind,
            item.media_attributes,
            item.width,
            item.height,
            item.video_duration_secs,
            item.taken_at.map(ts),
            ts(item.file_mtime),
            item.file_size as i64,
            item.blake3_hash,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 批量 upsert：把 `items` 全部写进**同一个事务**后一次性提交。
///
/// 这是冷扫描（十万级文件）的关键：autocommit 下每行 INSERT 各自 fsync 一次，
/// 数万次 fsync 就是数十秒；放进一个事务则整批只付一次提交开销，把 DB 写入从
/// 「秒级每万行」降到「毫秒级每万行」。逐项按 uri 冲突 → UPDATE（并清 trashed_at，
/// 与单行 [`crate::core::backend::local::LocalBackend::upsert`] 同口径），其余
/// INSERT。返回每个成功写入行的完整物化视图（顺序与输入一致）；单行出错只跳过该行、
/// 计入返回外的差异，不影响同批其余行的提交。
pub fn upsert_media_items_batch(pool: &DbPool, items: &[NewMediaItem]) -> Result<Vec<MediaItem>> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM media_items WHERE uri = ?1",
                [&item.uri],
                |row| row.get(0),
            )
            .optional()?;
        let id = if let Some(id) = existing {
            tx.execute(
                "UPDATE media_items
                 SET path=?2, folder_path=?3, mime_type=?4, media_kind=?5,
                     media_subkind=?6, media_attributes=?7, width=?8, height=?9,
                     video_duration_secs=?10, taken_at=?11, file_mtime=?12,
                     file_size=?13, blake3_hash=?14,
                     trashed_at=NULL, indexed_at=unixepoch()
                 WHERE id=?1",
                rusqlite::params![
                    id,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
                    media_kind_db_value(&item.mime_type),
                    item.media_subkind,
                    item.media_attributes,
                    item.width,
                    item.height,
                    item.video_duration_secs,
                    item.taken_at.map(ts),
                    ts(item.file_mtime),
                    item.file_size as i64,
                    item.blake3_hash,
                ],
            )?;
            id
        } else {
            tx.execute(
                "INSERT INTO media_items
                    (uri, path, folder_path, mime_type, media_kind, media_subkind,
                     media_attributes, width, height, video_duration_secs, taken_at,
                     file_mtime, file_size, blake3_hash, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, unixepoch())",
                rusqlite::params![
                    item.uri,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
                    media_kind_db_value(&item.mime_type),
                    item.media_subkind,
                    item.media_attributes,
                    item.width,
                    item.height,
                    item.video_duration_secs,
                    item.taken_at.map(ts),
                    ts(item.file_mtime),
                    item.file_size as i64,
                    item.blake3_hash,
                ],
            )?;
            tx.last_insert_rowid()
        };
        out.push(tx.query_row(
            "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at
             FROM media_items WHERE id = ?1",
            [id],
            row_to_media_item,
        )?);
    }
    tx.commit()?;
    Ok(out)
}

/// 根据 id 查询
pub fn get_media_item(pool: &DbPool, id: i64) -> Result<MediaItem> {
    let conn = pool.get()?;
    let item = conn.query_row(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items WHERE id = ?1",
        [id],
        row_to_media_item,
    )?;
    Ok(item)
}

/// 按 `uri` 查询；找不到返回 `Ok(None)`。供回收站对账等"按原 uri 定位行"的场景使用。
pub fn get_media_item_by_uri(pool: &DbPool, uri: &str) -> Result<Option<MediaItem>> {
    let conn = pool.get()?;
    let item = conn
        .query_row(
            "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at
             FROM media_items WHERE uri = ?1",
            [uri],
            row_to_media_item,
        )
        .optional()?;
    Ok(item)
}

/// 列出所有非回收站项，按照片排序时间 DESC 排序：
/// EXIF 拍摄时间优先；没有 EXIF 时使用文件侧时间（created/mtime fallback）。
pub fn list_all_media(pool: &DbPool) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC",
    )?;
    let rows = stmt.query_map([], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// 为现有 DB 补齐 `thumbnail_generated_at` 列（忽略已存在错误）。
pub fn ensure_thumbnail_generated_column(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    let _ = conn.execute(
        "ALTER TABLE media_items ADD COLUMN thumbnail_generated_at INTEGER",
        [],
    );
    Ok(())
}

/// 非回收站媒体项总数。
pub fn count_live_media(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_items WHERE trashed_at IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

/// 已生成缩略图的媒体项总数。
pub fn count_thumbnail_generated(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_items WHERE trashed_at IS NULL AND thumbnail_generated_at IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
    Ok(count as usize)
}

/// 分页列出需要生成缩略图的非回收站项。
/// 条件：`thumbnail_generated_at IS NULL OR thumbnail_generated_at < file_mtime`
/// ——即从未生成、或文件 mtime 已变更（源文件被修改后过期）。
pub fn list_media_needing_thumbnail(
    pool: &DbPool,
    offset: u32,
    limit: u32,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL
           AND (thumbnail_generated_at IS NULL OR thumbnail_generated_at < file_mtime)
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map([limit as i64, offset as i64], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// 批量标记已生成缩略图的 media_items：写入 `thumbnail_generated_at = unixepoch()`。
pub fn mark_thumbnails_generated(pool: &DbPool, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let conn = pool.get()?;
    let mut stmt =
        conn.prepare("UPDATE media_items SET thumbnail_generated_at = unixepoch() WHERE id = ?1")?;
    for id in ids {
        stmt.execute([*id])?;
    }
    Ok(())
}

/// 分页列出非回收站项，排序语义与 [`list_all_media`] 一致。
pub fn list_media_page(pool: &DbPool, offset: u32, limit: u32) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
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

/// 一次性载入所有「已索引且非回收站」行的 `(uri → (file_mtime 秒, file_size))` 快照，
/// 供启动扫描做未改动短路。
///
/// 替代逐文件 `SELECT`：扫描线程据此在内存里按 uri 查表，命中且 `(mtime, size)` 完全
/// 一致即视为未改动——逐文件零 DB 往返，也不与消费者的写事务争 WAL（此前十万级图库
/// 扫描的 ~20s 读写竞争主要来源就是这条逐行只读查询并发了消费者的批量写）。一次顺序
/// 扫描全表即可，内存开销与行数线性（约 ~140 B/行），本轮扫描结束即丢弃。
///
/// 注意：被标记为回收站（`trashed_at IS NOT NULL`）的行**不**入快照，因此对它们不会
/// 短路。回收站行意味着文件本该不在原路径；若启动扫描又能看到它，说明它被外部（文件
/// 管理器）从系统回收站还原了，必须重新 upsert 以清掉 `trashed_at`，否则还原后的图片
/// 不会重新出现在相册里。
pub fn load_unchanged_index(pool: &DbPool) -> Result<HashMap<String, (i64, i64)>> {
    let conn = pool.get()?;
    let mut stmt = conn
        .prepare("SELECT uri, file_mtime, file_size FROM media_items WHERE trashed_at IS NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        ))
    })?;
    let mut map: HashMap<String, (i64, i64)> = HashMap::new();
    for row in rows {
        let (uri, mtime_size) = row?;
        map.insert(uri, mtime_size);
    }
    Ok(map)
}

/// 删除指定本地路径对应的媒体行。返回受影响行数。
///
/// 注意：只有 `trashed_at IS NULL` 的行才会被删。被应用标记为回收站
/// （`mark_trashed`）的行，其原文件已被 gio 移到 `~/.local/share/Trash/`，
/// 原路径消失是预期行为 —— 文件系统监听器看到 Remove 事件时绝不能把这些行
/// 硬删，否则回收站页面会因为 `list_trashed_media` 返回空而"看不见图片"。
/// 该函数目前只被 `notify_watcher` 经由 `backend.delete_path` 调用。
pub fn delete_media_by_path(pool: &DbPool, path: &Path) -> Result<usize> {
    let conn = pool.get()?;
    let uri = format!("file://{}", path.display());
    let changed = conn.execute(
        "DELETE FROM media_items WHERE (path = ?1 OR uri = ?2) AND trashed_at IS NULL",
        rusqlite::params![path.to_string_lossy(), uri],
    )?;
    Ok(changed)
}

/// 清空所有媒体记录。返回删除的记录数。
///
/// 用于重置数据库，不会删除原始文件。
pub fn clear_all_media(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    let count = conn.execute("DELETE FROM media_items", [])?;
    Ok(count)
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
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
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

/// 按文件夹路径列出未删除的媒体，按 `file_mtime` 倒序。
///
/// 用于相册详情页加载完整相册内容，不受 `UI_MEDIA_LIST_CAP` 限制。
pub fn list_media_by_folder(
    pool: &DbPool,
    folder_path: &std::path::Path,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let folder_str = folder_path.to_string_lossy().to_string();
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL AND folder_path = ?1
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC",
    )?;
    let rows = stmt.query_map([folder_str], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
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
    let taken_at: Option<i64> = row.get(10)?;
    let file_mtime: i64 = row.get(11)?;
    let trashed_at: Option<i64> = row.get(15)?;

    Ok(MediaItem {
        id: row.get(0)?,
        uri: row.get(1)?,
        path: std::path::PathBuf::from(row.get::<_, String>(2)?),
        folder_path: std::path::PathBuf::from(row.get::<_, String>(3)?),
        mime_type: row.get(4)?,
        media_subkind: row.get(5)?,
        media_attributes: row.get(6)?,
        width: row.get(7)?,
        height: row.get(8)?,
        video_duration_secs: row.get(9)?,
        taken_at: optional_ts(taken_at, 10)?,
        file_mtime: required_ts(file_mtime, 11)?,
        file_size: row.get::<_, i64>(12)? as u64,
        blake3_hash: row.get(13)?,
        is_favorite: row.get::<_, i64>(14)? == 1,
        trashed_at: optional_ts(trashed_at, 15)?,
    })
}
