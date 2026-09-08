//! SQLite 连接池与当前 schema 初始化。
use crate::core::error::{AppError, Result};
use crate::core::media::{media_kind_from_mime, MediaItem, MediaKind, NewMediaItem};

/// Which fields to search in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchField {
    /// Search both filename and date (default).
    #[default]
    All,
    /// Search only by file name (path).
    Name,
    /// Search only by date (taken_at or file_mtime).
    Date,
}
use chrono::{DateTime, TimeZone, Utc};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::types::{Type, Value};
use rusqlite::{params_from_iter, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub type DbPool = Pool<SqliteConnectionManager>;

const SCHEMA_SQL: &str = include_str!("schema.sql");
const SQLITE_BUSY_TIMEOUT_MS: u64 = 10_000;
const UPSERT_RETRY_LIMIT: usize = 3;
/// Initialize or transactionally migrate the library without discarding user data.
pub fn init_pool(path: &Path) -> Result<DbPool> {
    let manager = SqliteConnectionManager::file(path).with_init(|c| {
        c.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = {SQLITE_BUSY_TIMEOUT_MS};
             PRAGMA synchronous = NORMAL;"
        ))
    });
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(AppError::from)?;
    let mut conn = pool.get()?;
    migrate_schema(&mut conn)?;
    Ok(pool)
}

const SCHEMA_VERSION: i64 = 1;

fn migrate_schema(conn: &mut rusqlite::Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(AppError::Backend(format!(
            "library schema {version} requires a newer Photo Viewer (supported: {SCHEMA_VERSION})"
        )));
    }
    let columns = {
        let mut stmt = tx.prepare("PRAGMA table_info(media_items)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?
    };
    if version == 0 && !columns.is_empty() {
        // Historical unversioned libraries share the same identity/location
        // columns. Only add derived fields; never rebuild media or user tables.
        for (name, definition) in [
            ("media_kind", "TEXT NOT NULL DEFAULT 'image'"),
            ("media_subkind", "TEXT NOT NULL DEFAULT 'standard'"),
            ("media_attributes", "TEXT NOT NULL DEFAULT '{}'"),
            ("media_type_flags", "INTEGER NOT NULL DEFAULT 0"),
            ("video_duration_secs", "REAL"),
            ("thumbnail_generated_at", "INTEGER"),
            ("is_favorite", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !columns.contains(name) {
                tx.execute_batch(&format!(
                    "ALTER TABLE media_items ADD COLUMN {name} {definition}"
                ))?;
            }
        }
        tx.execute_batch(
            "UPDATE media_items SET media_kind = CASE WHEN mime_type LIKE 'video/%' THEN 'video' ELSE 'image' END;
             UPDATE media_items SET media_type_flags =
                CASE WHEN media_subkind = 'motion_photo' THEN 1 ELSE 0 END |
                CASE WHEN mime_type = 'image/gif' OR json_extract(CASE WHEN json_valid(media_attributes) THEN media_attributes ELSE '{}' END, '$.animated') = 1 THEN 2 ELSE 0 END |
                CASE WHEN json_extract(CASE WHEN json_valid(media_attributes) THEN media_attributes ELSE '{}' END, '$.hdr') = 1 THEN 4 ELSE 0 END;"
        )?;
    }
    tx.execute_batch(SCHEMA_SQL)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
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
pub(crate) fn insert_media_item(pool: &DbPool, item: &NewMediaItem) -> Result<i64> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO media_items
            (uri, path, folder_path, mime_type, media_kind, media_subkind,
             media_attributes, media_type_flags, width, height, video_duration_secs, taken_at,
             file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, unixepoch())",
        rusqlite::params![
            item.uri,
            item.path.to_string_lossy(),
            item.folder_path.to_string_lossy(),
            item.mime_type,
            media_kind_db_value(&item.mime_type),
            item.media_subkind,
            item.media_attributes,
            crate::core::media::media_type_flags(&item.media_subkind, &item.media_attributes),
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
    for attempt in 0..UPSERT_RETRY_LIMIT {
        match upsert_media_items_batch_once(pool, items) {
            Ok(items) => return Ok(items),
            Err(err) if is_retryable_lock_error(&err) && attempt + 1 < UPSERT_RETRY_LIMIT => {
                let delay_ms = 25_u64 << attempt;
                tracing::warn!(
                    target: crate::core::log_targets::STORAGE,
                    "SQL_FLOW op=upsert_media_items_batch phase=retry attempt={} delay_ms={} error={}",
                    attempt + 1,
                    delay_ms,
                    err
                );
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("upsert retry loop must return within its attempt limit")
}

fn is_retryable_lock_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Db(rusqlite::Error::SqliteFailure(err, _))
            if matches!(
                err.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

fn upsert_media_items_batch_once(pool: &DbPool, items: &[NewMediaItem]) -> Result<Vec<MediaItem>> {
    let started = Instant::now();
    tracing::trace!(
        target: crate::core::log_targets::STORAGE,
        "SQL_FLOW op=upsert_media_items_batch phase=begin item_count={}",
        items.len()
    );
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    tracing::trace!(
        target: crate::core::log_targets::STORAGE,
        "SQL_FLOW op=upsert_media_items_batch phase=transaction_started item_count={}",
        items.len()
    );
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
                     media_subkind=?6, media_attributes=?7, media_type_flags=?8, width=?9, height=?10,
                     video_duration_secs=?11, taken_at=?12, file_mtime=?13,
                     file_size=?14, blake3_hash=?15,
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
                    crate::core::media::media_type_flags(&item.media_subkind, &item.media_attributes),
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
                     media_attributes, media_type_flags, width, height, video_duration_secs, taken_at,
                     file_mtime, file_size, blake3_hash, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, unixepoch())",
                rusqlite::params![
                    item.uri,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
                    media_kind_db_value(&item.mime_type),
                    item.media_subkind,
                    item.media_attributes,
                    crate::core::media::media_type_flags(&item.media_subkind, &item.media_attributes),
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
    tracing::trace!(
        target: crate::core::log_targets::STORAGE,
        "SQL_FLOW op=upsert_media_items_batch phase=commit_begin item_count={} changed_count={}",
        items.len(),
        out.len()
    );
    tx.commit()?;
    tracing::trace!(
        target: crate::core::log_targets::STORAGE,
        "SQL_FLOW op=upsert_media_items_batch phase=commit_done item_count={} changed_count={} elapsed_ms={}",
        items.len(),
        out.len(),
        started.elapsed().as_millis()
    );
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

/// 返回所有 live 行的 `(id, uri, path, folder_path)`（已过滤 `trashed_at IS NULL`）。
/// `folder_path` 取自存储列，供启动对账按目录分桶做批量清理，而非依赖
/// 「`folder_path == path.parent()`」不变量（move 路径并非处处成立）。
pub fn list_live_media_locations(pool: &DbPool) -> Result<Vec<(i64, String, PathBuf, PathBuf)>> {
    let conn = pool.get()?;
    let mut stmt = conn
        .prepare("SELECT id, uri, path, folder_path FROM media_items WHERE trashed_at IS NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            PathBuf::from(row.get::<_, String>(2)?),
            PathBuf::from(row.get::<_, String>(3)?),
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
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

pub(crate) fn search_like_pattern(term: &str) -> String {
    let mut pattern = String::from("%");
    let normalized = term
        .trim()
        .replace(['年', '月'], "-")
        .replace('日', "")
        .replace('/', "-"); // Normalize slash to dash for date matching.
    let normalized = normalized.trim_end_matches('-');
    for ch in normalized.chars() {
        match ch {
            '%' | '_' | '\\' => {
                pattern.push('\\');
                pattern.push(ch);
            }
            _ => pattern.push(ch),
        }
    }
    pattern.push('%');
    pattern
}

pub fn count_live_media_search(
    pool: &DbPool,
    term: &str,
    media_kind: Option<&str>,
    field: SearchField,
) -> Result<usize> {
    let conn = pool.get()?;
    let pattern = search_like_pattern(term);
    let field_clause = match field {
        SearchField::All => {
            "(lower(path) LIKE lower(?1) ESCAPE '\\' \
             OR strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ?1 ESCAPE '\\')"
        }
        SearchField::Name => "lower(path) LIKE lower(?1) ESCAPE '\\'",
        SearchField::Date => {
            "strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ?1 ESCAPE '\\'"
        }
    };
    let count: i64 = if let Some(media_kind) = media_kind {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM media_items
                 WHERE trashed_at IS NULL
                   AND media_kind = ?2
                   AND {field_clause}"
            ),
            rusqlite::params![pattern, media_kind],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM media_items
                 WHERE trashed_at IS NULL
                   AND {field_clause}"
            ),
            [pattern],
            |row| row.get(0),
        )?
    };
    Ok(count as usize)
}

pub fn count_media_by_folder(pool: &DbPool, folder_path: &std::path::Path) -> Result<usize> {
    let conn = pool.get()?;
    let folder_str = folder_path.to_string_lossy().to_string();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_items
         WHERE trashed_at IS NULL AND folder_path = ?1",
        [folder_str],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn count_favorite_media(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_items
         WHERE trashed_at IS NULL AND is_favorite = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn count_media_by_kind(pool: &DbPool, media_kind: &str) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_items
         WHERE trashed_at IS NULL AND media_kind = ?1",
        [media_kind],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn count_media_by_subkind(pool: &DbPool, media_subkind: &str) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_items
         WHERE trashed_at IS NULL AND media_subkind = ?1",
        [media_subkind],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn count_media_by_logical_type(
    pool: &DbPool,
    media_type: crate::core::media::LogicalMediaType,
) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM media_items WHERE trashed_at IS NULL AND {}",
            media_type.sql_predicate()
        ),
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

/// 按完整日期(年-月-日)分组统计非回收站媒体数量。
///
/// 日期取自 `COALESCE(taken_at, file_mtime)`（与列表排序/分组的基准一致），
/// 以 UTC 解析，与 `MediaItem::sort_datetime()`（`DateTime<Utc>`）对齐，不会跨年错位。
/// `file_mtime` 非空，故每条 live 行都会落入某个日期组。返回 `(年, 月, 日, 计数)`，
/// 供 `section_model::counts_from_date_groups` 按 Year/Month/Day 折叠成 section 真实计数。
pub fn count_live_media_by_date(pool: &DbPool) -> Result<Vec<(i32, u32, u32, u32)>> {
    count_media_by_date_for_filter(pool, "trashed_at IS NULL", Vec::new())
}

pub fn count_media_by_date_for_filter(
    pool: &DbPool,
    where_clause: &str,
    params: Vec<Value>,
) -> Result<Vec<(i32, u32, u32, u32)>> {
    let conn = pool.get()?;
    let sql = format!(
        "SELECT
                CAST(strftime('%Y', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) AS INTEGER),
                CAST(strftime('%m', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) AS INTEGER),
                CAST(strftime('%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) AS INTEGER),
                COUNT(*)
         FROM media_items
         WHERE {where_clause}
         GROUP BY 1, 2, 3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        let year: i64 = row.get(0)?;
        let month: i64 = row.get(1)?;
        let day: i64 = row.get(2)?;
        let count: i64 = row.get(3)?;
        Ok((year as i32, month as u32, day as u32, count as u32))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// 已生成缩略图的媒体项总数。
pub fn count_thumbnail_generated(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_items
             WHERE trashed_at IS NULL
               AND thumbnail_generated_at IS NOT NULL
               AND thumbnail_generated_at >= file_mtime",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn set_thumbnail_generated_at_for_tests(pool: &DbPool, id: i64, timestamp: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE media_items SET thumbnail_generated_at = ?1 WHERE id = ?2",
        rusqlite::params![timestamp, id],
    )?;
    Ok(())
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

/// 从全局 live-media DESC offset 附近列出需要生成缩略图的非回收站项。
///
/// `live_offset` 按完整 live-media 排序解释，而不是按“待生成缩略图”过滤后的集合
/// 解释。这样用户跳到一个部分已预热的区域时，后台预热仍会从当前浏览位置附近的
/// 冷项开始，而不会因为当前位置之前已有缩略图而被过滤集合的 OFFSET 推偏。
pub fn list_media_needing_thumbnail_from_live_offset(
    pool: &DbPool,
    live_offset: u32,
    limit: u32,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let anchor = conn
        .query_row(
            "SELECT COALESCE(taken_at, file_mtime), id
             FROM media_items
             WHERE trashed_at IS NULL
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT 1 OFFSET ?1",
            [live_offset as i64],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;

    let Some((anchor_sort, anchor_id)) = anchor else {
        return Ok(Vec::new());
    };

    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL
           AND (thumbnail_generated_at IS NULL OR thumbnail_generated_at < file_mtime)
           AND (
                COALESCE(taken_at, file_mtime) < ?1
                OR (COALESCE(taken_at, file_mtime) = ?1 AND id <= ?2)
           )
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map([anchor_sort, anchor_id, limit as i64], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// 批量标记已生成缩略图的 media_items：写入 `thumbnail_generated_at = unixepoch()`。
pub(crate) fn mark_thumbnails_generated(pool: &DbPool, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let started = Instant::now();
    tracing::trace!(
        target: crate::core::log_targets::STORAGE,
        "SQL_FLOW op=mark_thumbnails_generated phase=begin id_count={} ids={:?}",
        ids.len(),
        ids
    );
    let conn = pool.get()?;
    let mut stmt =
        conn.prepare("UPDATE media_items SET thumbnail_generated_at = unixepoch() WHERE id = ?1")?;
    for id in ids {
        tracing::trace!(
            target: crate::core::log_targets::STORAGE,
            "SQL_FLOW op=mark_thumbnails_generated phase=execute media_id={}",
            id
        );
        stmt.execute([*id])?;
    }
    tracing::trace!(
        target: crate::core::log_targets::STORAGE,
        "SQL_FLOW op=mark_thumbnails_generated phase=done id_count={} elapsed_ms={}",
        ids.len(),
        started.elapsed().as_millis()
    );
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

pub fn list_media_search_page(
    pool: &DbPool,
    term: &str,
    media_kind: Option<&str>,
    field: SearchField,
    offset: u32,
    limit: u32,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let pattern = search_like_pattern(term);
    let field_clause = match field {
        SearchField::All => {
            "(lower(path) LIKE lower(?1) ESCAPE '\\' \
             OR strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ?1 ESCAPE '\\')"
        }
        SearchField::Name => "lower(path) LIKE lower(?1) ESCAPE '\\'",
        SearchField::Date => {
            "strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ?1 ESCAPE '\\'"
        }
    };
    let columns = "id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at";
    if let Some(media_kind) = media_kind {
        let mut stmt = conn.prepare(&format!(
            "SELECT {columns}
             FROM media_items
             WHERE trashed_at IS NULL
               AND media_kind = ?2
               AND {field_clause}
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT ?3 OFFSET ?4"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![pattern, media_kind, limit as i64, offset as i64],
            row_to_media_item,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    } else {
        let mut stmt = conn.prepare(&format!(
            "SELECT {columns}
             FROM media_items
             WHERE trashed_at IS NULL
               AND {field_clause}
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![pattern, limit as i64, offset as i64],
            row_to_media_item,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }
}

pub fn list_media_by_folder_page(
    pool: &DbPool,
    folder_path: &std::path::Path,
    offset: u32,
    limit: u32,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let folder_str = folder_path.to_string_lossy().to_string();
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL AND folder_path = ?1
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![folder_str, limit as i64, offset as i64],
        row_to_media_item,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

pub fn list_favorite_media_page(pool: &DbPool, offset: u32, limit: u32) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL AND is_favorite = 1
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map([limit as i64, offset as i64], row_to_media_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

pub fn list_media_by_kind_page(
    pool: &DbPool,
    media_kind: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL AND media_kind = ?1
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![media_kind, limit as i64, offset as i64],
        row_to_media_item,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

pub fn list_media_by_subkind_page(
    pool: &DbPool,
    media_subkind: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL AND media_subkind = ?1
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![media_subkind, limit as i64, offset as i64],
        row_to_media_item,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

pub fn list_media_by_logical_type_page(
    pool: &DbPool,
    media_type: crate::core::media::LogicalMediaType,
    offset: u32,
    limit: u32,
) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL AND {}
         ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
         LIMIT ?1 OFFSET ?2",
        media_type.sql_predicate()
    ))?;
    let rows = stmt.query_map(
        rusqlite::params![limit as i64, offset as i64],
        row_to_media_item,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::from)
}

/// Return the neighbor of a live media item in the canonical live sort order.
///
/// `delta` follows viewer cursor semantics: `1` moves to the next row in the
/// descending live-media order, `-1` moves to the previous row.
pub fn live_media_neighbor(
    pool: &DbPool,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter(pool, current_id, delta, "trashed_at IS NULL", vec![])
}

pub fn favorite_media_neighbor(
    pool: &DbPool,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter(
        pool,
        current_id,
        delta,
        "trashed_at IS NULL AND is_favorite = 1",
        vec![],
    )
}

pub fn folder_media_neighbor(
    pool: &DbPool,
    folder_path: &std::path::Path,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter(
        pool,
        current_id,
        delta,
        "trashed_at IS NULL AND folder_path = ?",
        vec![Value::Text(folder_path.to_string_lossy().to_string())],
    )
}

pub fn kind_media_neighbor(
    pool: &DbPool,
    media_kind: &str,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter(
        pool,
        current_id,
        delta,
        "trashed_at IS NULL AND media_kind = ?",
        vec![Value::Text(media_kind.to_string())],
    )
}

pub fn subkind_media_neighbor(
    pool: &DbPool,
    media_subkind: &str,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter(
        pool,
        current_id,
        delta,
        "trashed_at IS NULL AND media_subkind = ?",
        vec![Value::Text(media_subkind.to_string())],
    )
}

pub fn media_type_media_neighbor(
    pool: &DbPool,
    media_type: crate::core::media::LogicalMediaType,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter(
        pool,
        current_id,
        delta,
        &format!("trashed_at IS NULL AND {}", media_type.sql_predicate()),
        Vec::new(),
    )
}

pub fn search_media_neighbor(
    pool: &DbPool,
    term: &str,
    media_kind: Option<&str>,
    field: SearchField,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    let pattern = search_like_pattern(term);
    let (field_clause, mut params) = match field {
        SearchField::All => (
            "(lower(path) LIKE lower(?) ESCAPE '\\' \
             OR strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ? ESCAPE '\\')",
            vec![Value::Text(pattern.clone()), Value::Text(pattern)],
        ),
        SearchField::Name => (
            "lower(path) LIKE lower(?) ESCAPE '\\'",
            vec![Value::Text(pattern)],
        ),
        SearchField::Date => (
            "strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ? ESCAPE '\\'",
            vec![Value::Text(pattern)],
        ),
    };
    let where_clause = if let Some(media_kind) = media_kind {
        let mut with_kind = vec![Value::Text(media_kind.to_string())];
        with_kind.append(&mut params);
        params = with_kind;
        format!("trashed_at IS NULL AND media_kind = ? AND {field_clause}")
    } else {
        format!("trashed_at IS NULL AND {field_clause}")
    };

    media_neighbor_with_filter(pool, current_id, delta, &where_clause, params)
}

pub fn trashed_media_neighbor(
    pool: &DbPool,
    current_id: i64,
    delta: i32,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter_and_order(
        pool,
        current_id,
        delta,
        "trashed_at IS NOT NULL",
        vec![],
        "trashed_at DESC, id DESC",
    )
}

fn media_neighbor_with_filter(
    pool: &DbPool,
    current_id: i64,
    delta: i32,
    where_clause: &str,
    filter_params: Vec<Value>,
) -> Result<Option<(u32, u32, MediaItem)>> {
    media_neighbor_with_filter_and_order(
        pool,
        current_id,
        delta,
        where_clause,
        filter_params,
        "COALESCE(taken_at, file_mtime) DESC, id DESC",
    )
}

fn media_neighbor_with_filter_and_order(
    pool: &DbPool,
    current_id: i64,
    delta: i32,
    where_clause: &str,
    filter_params: Vec<Value>,
    order_by: &str,
) -> Result<Option<(u32, u32, MediaItem)>> {
    let trashed = order_by.starts_with("trashed_at");
    let Some(item) = seek_media_neighbor(
        pool,
        current_id,
        delta,
        where_clause,
        &filter_params,
        trashed,
    )?
    else {
        return Ok(None);
    };
    // Compatibility projection for consumers that explicitly need a rank.
    // Interactive navigation calls seek_media_neighbor and skips both counts.
    let conn = pool.get()?;
    let total: u32 = conn.query_row(
        &format!("SELECT COUNT(*) FROM media_items WHERE {where_clause}"),
        params_from_iter(filter_params.iter()),
        |r| r.get(0),
    )?;
    let sort = if trashed {
        "trashed_at"
    } else {
        "COALESCE(taken_at, file_mtime)"
    };
    let time = if trashed {
        item.trashed_at.unwrap_or(item.file_mtime)
    } else {
        item.sort_datetime()
    }
    .timestamp();
    let mut params = filter_params;
    params.extend([
        Value::Integer(time),
        Value::Integer(time),
        Value::Integer(item.id),
    ]);
    let index = conn.query_row(&format!("SELECT COUNT(*) FROM media_items WHERE {where_clause} AND {sort} >= ? AND ({sort} > ? OR id > ?)"), params_from_iter(params.iter()), |r| r.get(0))?;
    Ok(Some((index, total, item)))
}

/// Index seek for navigation: no full-result ranking, count, or global offset.
pub(crate) fn seek_media_neighbor(
    pool: &DbPool,
    current_id: i64,
    delta: i32,
    where_clause: &str,
    filter_params: &[Value],
    trashed: bool,
) -> Result<Option<MediaItem>> {
    if delta == 0 {
        return Ok(None);
    }
    let conn = pool.get()?;
    let sort = if trashed {
        "trashed_at"
    } else {
        "COALESCE(taken_at, file_mtime)"
    };
    let mut current_params = vec![Value::Integer(current_id)];
    current_params.extend_from_slice(filter_params);
    let time: Option<i64> = conn
        .query_row(
            &format!("SELECT {sort} FROM media_items WHERE id = ? AND {where_clause}"),
            params_from_iter(current_params.iter()),
            |r| r.get(0),
        )
        .optional()?;
    let Some(time) = time else {
        return Ok(None);
    };
    let (cmp, order) = if delta > 0 {
        ("<", "DESC")
    } else {
        (">", "ASC")
    };
    let sql = format!(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE {where_clause} AND {sort} {cmp}= ? AND ({sort} {cmp} ? OR id {cmp} ?)
         ORDER BY {sort} {order}, id {order}
         LIMIT 1 OFFSET ?"
    );
    let mut params = filter_params.to_vec();
    params.extend([
        Value::Integer(time),
        Value::Integer(time),
        Value::Integer(current_id),
        Value::Integer(i64::from(delta).abs() - 1),
    ]);
    Ok(conn
        .query_row(&sql, params_from_iter(params.iter()), row_to_media_item)
        .optional()?)
}

/// 删除单行
pub(crate) fn delete_media_item(pool: &DbPool, id: i64) -> Result<()> {
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

/// Delete multiple live rows in one transaction and return the exact URIs
/// that were removed. Duplicate paths are harmless; trashed rows remain
/// protected by the same guard as [`delete_media_by_path`].
pub fn delete_live_media_by_paths(pool: &DbPool, paths: &[PathBuf]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let mut removed = Vec::new();
    for path in paths {
        let uri = format!("file://{}", path.display());
        let changed = tx.execute(
            "DELETE FROM media_items WHERE (path = ?1 OR uri = ?2) AND trashed_at IS NULL",
            rusqlite::params![path.to_string_lossy(), uri],
        )?;
        if changed > 0 {
            removed.push(uri);
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// 删除指定文件夹相册下的 live 媒体索引。只删除数据库行，不触碰磁盘文件。
pub fn delete_live_media_by_folder(pool: &DbPool, folder_path: &Path) -> Result<usize> {
    let conn = pool.get()?;
    let changed = conn.execute(
        "DELETE FROM media_items WHERE folder_path = ?1 AND trashed_at IS NULL",
        rusqlite::params![folder_path.to_string_lossy()],
    )?;
    Ok(changed)
}

/// 按 id 批量删除 live 媒体索引（自动分块，避开 SQLite 参数上限）。
/// 只删 `trashed_at IS NULL` 的行，并发 `mark_trashed` 的行不会被误删。
/// 返回实际删除的行数。供启动对账「目录仍在、仅个别文件消失」场景批量清理。
pub fn delete_media_by_ids(pool: &DbPool, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    // SQLite 默认 SQLITE_MAX_VARIABLE_NUMBER=999，留余量按 500 一块。
    const CHUNK: usize = 500;
    let conn = pool.get()?;
    let mut total = 0usize;
    for chunk in ids.chunks(CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql =
            format!("DELETE FROM media_items WHERE id IN ({placeholders}) AND trashed_at IS NULL");
        total += conn.execute(&sql, params_from_iter(chunk.iter()))?;
    }
    Ok(total)
}

/// Delete live rows and return their exact ids. This is used by two-phase
/// filesystem reconciliation: the scan/stat phase runs outside the DB actor,
/// while this short transaction is serialized by the actor.
pub fn delete_media_by_ids_returning(pool: &DbPool, ids: &[i64]) -> Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    const CHUNK: usize = 500;
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let mut removed = Vec::new();
    for chunk in ids.chunks(CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM media_items WHERE id IN ({placeholders}) AND trashed_at IS NULL RETURNING id"
        );
        let mut statement = tx.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(chunk.iter()), |row| row.get(0))?;
        for row in rows {
            removed.push(row?);
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// 重置媒体库数据库内容。返回删除的媒体记录数。
///
/// 不会删除原始文件或用户偏好，但会清空媒体、相册物化视图及相册自定义数据。
pub(crate) fn reset_library_database(pool: &DbPool) -> Result<usize> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let count = tx.execute("DELETE FROM media_items", [])?;
    tx.execute("DELETE FROM albums", [])?;
    tx.execute("DELETE FROM album_order", [])?;
    tx.execute("DELETE FROM album_covers", [])?;
    tx.commit()?;
    Ok(count)
}

/// 标记为已删除（不立即物理删除）
pub(crate) fn mark_trashed(pool: &DbPool, id: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE media_items SET trashed_at = unixepoch() WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// 取消回收站标记
pub(crate) fn unmark_trashed(pool: &DbPool, id: i64) -> Result<()> {
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
pub(crate) fn update_media_location(
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

/// Update metadata after an editor overwrites the source file.
pub(crate) fn update_media_edit_metadata(
    pool: &DbPool,
    id: i64,
    item: &NewMediaItem,
) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE media_items
         SET file_mtime = ?2, file_size = ?3, blake3_hash = ?4,
             thumbnail_generated_at = NULL, width = ?5, height = ?6,
             media_subkind = ?7, media_attributes = ?8, media_type_flags = ?9,
             video_duration_secs = NULL
         WHERE id = ?1",
        rusqlite::params![
            id,
            item.file_mtime.timestamp(),
            item.file_size as i64,
            item.blake3_hash,
            item.width,
            item.height,
            item.media_subkind,
            item.media_attributes,
            crate::core::media::media_type_flags(&item.media_subkind, &item.media_attributes)
        ],
    )?;
    Ok(())
}

/// 列出所有回收站中项
pub fn list_trashed_media(pool: &DbPool) -> Result<Vec<MediaItem>> {
    list_trashed_media_page(pool, 0, u32::MAX)
}

pub fn count_trashed_media(pool: &DbPool) -> Result<usize> {
    let conn = pool.get()?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_items WHERE trashed_at IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn list_trashed_media_page(pool: &DbPool, offset: u32, limit: u32) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, media_subkind,
                media_attributes, width, height, video_duration_secs, taken_at,
                file_mtime, file_size, blake3_hash, is_favorite, trashed_at
         FROM media_items
         WHERE trashed_at IS NOT NULL
         ORDER BY trashed_at DESC, id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map([limit as i64, offset as i64], row_to_media_item)?;
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
pub(crate) fn set_media_favorite(pool: &DbPool, media_id: i64, is_favorite: bool) -> Result<()> {
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
    list_media_by_folder_page(pool, folder_path, 0, u32::MAX)
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
