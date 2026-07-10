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
pub const MOTION_PHOTOS_ALBUM_PATH: &str = "__photo-viewer-motion-photos__";
pub const ANIMATED_ALBUM_PATH: &str = "__photo-viewer-animated__";
pub const HDR_ALBUM_PATH: &str = "__photo-viewer-hdr__";

const VIRTUAL_ALBUM_PATHS: [&str; 6] = [
    FAVORITES_ALBUM_PATH,
    IMAGES_ALBUM_PATH,
    VIDEOS_ALBUM_PATH,
    MOTION_PHOTOS_ALBUM_PATH,
    ANIMATED_ALBUM_PATH,
    HDR_ALBUM_PATH,
];

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
        self.is_virtual && self.folder_path.as_path() == Path::new(FAVORITES_ALBUM_PATH)
    }

    pub fn is_images_album(&self) -> bool {
        self.is_virtual && self.folder_path.as_path() == Path::new(IMAGES_ALBUM_PATH)
    }

    pub fn is_videos_album(&self) -> bool {
        self.is_virtual && self.folder_path.as_path() == Path::new(VIDEOS_ALBUM_PATH)
    }

    pub fn is_motion_photos_album(&self) -> bool {
        self.is_virtual && self.folder_path.as_path() == Path::new(MOTION_PHOTOS_ALBUM_PATH)
    }

    pub fn is_animated_album(&self) -> bool {
        self.is_virtual && self.folder_path.as_path() == Path::new(ANIMATED_ALBUM_PATH)
    }

    pub fn is_hdr_album(&self) -> bool {
        self.is_virtual && self.folder_path.as_path() == Path::new(HDR_ALBUM_PATH)
    }
}

/// 列出实体相册 + 虚拟“收藏/图片/视频”相册（放在列表首位）。
///
/// 若 `album_order` 表中存有用户拖动重排后的顺序，则按该顺序重排结果；
/// 未记录顺序的相册追加到末尾并保留其默认相对顺序（虚拟相册在前、文件夹按
/// 最近修改倒序）。
pub fn list_with_favorites(pool: &DbPool) -> Result<Vec<Album>> {
    let mut list = list(pool)?;
    list.insert(
        0,
        virtual_album_or_compute(pool, FAVORITES_ALBUM_PATH, || favorites_album(pool))?,
    );
    list.insert(
        1,
        virtual_album_or_compute(pool, IMAGES_ALBUM_PATH, || {
            media_kind_album(pool, "image", IMAGES_ALBUM_PATH, tr("album.images.name"))
        })?,
    );
    list.insert(
        2,
        virtual_album_or_compute(pool, VIDEOS_ALBUM_PATH, || {
            media_kind_album(pool, "video", VIDEOS_ALBUM_PATH, tr("album.videos.name"))
        })?,
    );
    apply_saved_order(list, pool)
}

/// Media-type sidebar groups. This is separate from folder albums so new
/// subkind-based categories can sit below Albums without changing folder
/// ordering. Currently only dynamic/motion photos are exposed.
pub fn list_media_type_albums(pool: &DbPool) -> Result<Vec<Album>> {
    let candidates = vec![
        virtual_album_or_compute(pool, MOTION_PHOTOS_ALBUM_PATH, || {
            media_subkind_album(
                pool,
                crate::core::media::MEDIA_SUBKIND_MOTION_PHOTO,
                MOTION_PHOTOS_ALBUM_PATH,
                tr("album.motion_photos.name"),
            )
        })?,
        virtual_album_or_compute(pool, ANIMATED_ALBUM_PATH, || {
            media_attribute_album(
                pool,
                crate::core::media::MEDIA_ATTRIBUTE_ANIMATED,
                ANIMATED_ALBUM_PATH,
                tr("album.animated.name"),
            )
        })?,
        virtual_album_or_compute(pool, HDR_ALBUM_PATH, || {
            media_attribute_album(
                pool,
                crate::core::media::MEDIA_ATTRIBUTE_HDR,
                HDR_ALBUM_PATH,
                tr("album.hdr.name"),
            )
        })?,
    ];
    Ok(candidates
        .into_iter()
        .filter(|album| album.photo_count > 0)
        .collect())
}

/// 按 `album_order` 表中保存的顺序重排相册列表。
///
/// - 有记录 `sort_order` 的相册按该值升序排前；
/// - 没有记录的相册按它们在传入列表中的相对顺序追加在末尾；
/// - 表为空时原样返回（全新库 / 从未拖动过 → 维持虚拟相册置顶的默认行为）。
fn apply_saved_order(albums: Vec<Album>, pool: &DbPool) -> Result<Vec<Album>> {
    let order = album_order_map(pool)?;
    if order.is_empty() {
        return Ok(albums);
    }
    // (保存的序号 or None, 传入时的原索引, 相册) —— 用原索引做稳定 tiebreaker。
    let mut keyed: Vec<(Option<i64>, usize, Album)> = albums
        .into_iter()
        .enumerate()
        .map(|(i, album)| {
            let key = album.folder_path.to_string_lossy();
            (order.get(key.as_ref()).copied(), i, album)
        })
        .collect();
    keyed.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    Ok(keyed.into_iter().map(|(_, _, album)| album).collect())
}

/// 读取 `album_order` 表为 `folder_path → sort_order` 映射。
fn album_order_map(pool: &DbPool) -> Result<std::collections::HashMap<String, i64>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("SELECT folder_path, sort_order FROM album_order")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (path, order) = row?;
        map.insert(path, order);
    }
    Ok(map)
}

fn cached_virtual_album(pool: &DbPool, virtual_path: &str) -> Result<Option<Album>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT folder_path, name, cover_uri, photo_count, last_modified
         FROM albums WHERE folder_path = ?1",
    )?;
    let result = stmt.query_row([virtual_path], |row| {
        let path: String = row.get(0)?;
        let last_modified: i64 = row.get(4)?;
        Ok(Album {
            folder_path: PathBuf::from(path),
            name: row.get(1)?,
            cover_uri: row.get(2)?,
            photo_count: row.get(3)?,
            last_modified: chrono::DateTime::from_timestamp(last_modified, 0)
                .unwrap_or_else(Utc::now),
            is_virtual: true,
        })
    });
    Ok(result.ok())
}

fn virtual_album_or_compute<F>(pool: &DbPool, virtual_path: &str, compute: F) -> Result<Album>
where
    F: FnOnce() -> Result<Album>,
{
    match cached_virtual_album(pool, virtual_path)? {
        Some(album) => Ok(album),
        None => compute(),
    }
}

/// 持久化侧栏相册的完整顺序。`ordered` 为从上到下的 `folder_path` 列表。
///
/// 先 `DELETE` 再逐条写入，保证表内容与当前显示顺序完全一致；扫描中新出现的、
/// 尚未被拖动过的相册不会出现在此列表中，会按 `apply_saved_order` 的回退规则
/// 落到末尾。重排动作本身不频繁，列表也不大，整表重写可接受。
pub fn set_album_order(pool: &DbPool, ordered: &[String]) -> Result<()> {
    let conn = pool.get()?;
    conn.execute("DELETE FROM album_order", [])?;
    for (i, path) in ordered.iter().enumerate() {
        conn.execute(
            "INSERT INTO album_order (folder_path, sort_order) VALUES (?1, ?2)",
            rusqlite::params![path, i as i64],
        )?;
    }
    Ok(())
}

pub fn set_album_cover(pool: &DbPool, folder: &Path, cover_uri: &str) -> Result<()> {
    let conn = pool.get()?;
    let folder_path = folder.to_string_lossy();
    conn.execute(
        "INSERT INTO album_covers (folder_path, cover_uri)
         VALUES (?1, ?2)
         ON CONFLICT(folder_path) DO UPDATE SET cover_uri = excluded.cover_uri",
        rusqlite::params![folder_path.as_ref(), cover_uri],
    )?;
    conn.execute(
        "UPDATE albums SET cover_uri = ?2 WHERE folder_path = ?1",
        rusqlite::params![folder_path.as_ref(), cover_uri],
    )?;
    Ok(())
}

fn favorites_album(pool: &DbPool) -> Result<Album> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*),
            COALESCE(
            (SELECT cover_uri FROM album_covers c
             WHERE c.folder_path = ?1),
            (SELECT uri FROM media_items m2
             WHERE m2.trashed_at IS NULL AND m2.is_favorite = 1
             ORDER BY m2.file_mtime DESC LIMIT 1)),
            COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND is_favorite = 1",
    )?;
    let album = stmt.query_row([FAVORITES_ALBUM_PATH], |row| {
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
            COALESCE(
            (SELECT cover_uri FROM album_covers c
             WHERE c.folder_path = ?2),
            (SELECT uri FROM media_items m2
             WHERE m2.trashed_at IS NULL AND m2.media_kind = ?1
             ORDER BY m2.file_mtime DESC LIMIT 1)),
            COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND media_kind = ?1",
    )?;
    let album = stmt.query_row([media_kind, virtual_path], |row| {
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

fn media_subkind_album(
    pool: &DbPool,
    media_subkind: &str,
    virtual_path: &str,
    name: String,
) -> Result<Album> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*),
            COALESCE(
            (SELECT cover_uri FROM album_covers c
             WHERE c.folder_path = ?2),
            (SELECT uri FROM media_items m2
             WHERE m2.trashed_at IS NULL AND m2.media_subkind = ?1
             ORDER BY m2.file_mtime DESC LIMIT 1)),
            COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND media_subkind = ?1",
    )?;
    let album = stmt.query_row([media_subkind, virtual_path], |row| {
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

fn media_attribute_album(
    pool: &DbPool,
    attribute: &str,
    virtual_path: &str,
    name: String,
) -> Result<Album> {
    let conn = pool.get()?;
    let json_path = format!("$.{attribute}");
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*),
            COALESCE(
            (SELECT cover_uri FROM album_covers c
             WHERE c.folder_path = ?2),
            (SELECT uri FROM media_items m2
             WHERE m2.trashed_at IS NULL AND json_extract(m2.media_attributes, ?1) = 1
             ORDER BY m2.file_mtime DESC LIMIT 1)),
            COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND json_extract(media_attributes, ?1) = 1",
    )?;
    let album = stmt.query_row(rusqlite::params![json_path, virtual_path], |row| {
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
    let started = std::time::Instant::now();
    tracing::trace!(
        target: crate::core::log_targets::STORAGE,
        "SQL_FLOW op=refresh_albums phase=begin"
    );
    refresh_with_observer(pool, || Ok(())).inspect(|_| {
        tracing::trace!(
            target: crate::core::log_targets::STORAGE,
            "SQL_FLOW op=refresh_albums phase=done elapsed_ms={}",
            started.elapsed().as_millis()
        );
    })
}

fn refresh_with_observer<F>(pool: &DbPool, after_clear: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM albums", [])?;
    after_clear()?;
    tx.execute(
        "INSERT INTO albums (folder_path, name, cover_uri, photo_count, last_modified)
         SELECT
             folder_path,
             folder_path,
             COALESCE(
             (SELECT cover_uri FROM album_covers c
              WHERE c.folder_path = m.folder_path),
             (SELECT uri FROM media_items m2
              WHERE m2.folder_path = m.folder_path AND m2.trashed_at IS NULL
              ORDER BY m2.file_mtime DESC LIMIT 1)),
             COUNT(*),
             MAX(file_mtime)
         FROM media_items m
         WHERE trashed_at IS NULL
         GROUP BY folder_path",
        [],
    )?;
    refresh_virtual_album_rows(&tx)?;
    tx.commit()?;
    Ok(())
}

fn refresh_virtual_album_rows(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    insert_favorites_album_row(tx)?;
    insert_media_kind_album_row(tx, "image", IMAGES_ALBUM_PATH, &tr("album.images.name"))?;
    insert_media_kind_album_row(tx, "video", VIDEOS_ALBUM_PATH, &tr("album.videos.name"))?;
    insert_media_subkind_album_row(
        tx,
        crate::core::media::MEDIA_SUBKIND_MOTION_PHOTO,
        MOTION_PHOTOS_ALBUM_PATH,
        &tr("album.motion_photos.name"),
    )?;
    insert_media_attribute_album_row(
        tx,
        crate::core::media::MEDIA_ATTRIBUTE_ANIMATED,
        ANIMATED_ALBUM_PATH,
        &tr("album.animated.name"),
    )?;
    insert_media_attribute_album_row(
        tx,
        crate::core::media::MEDIA_ATTRIBUTE_HDR,
        HDR_ALBUM_PATH,
        &tr("album.hdr.name"),
    )?;
    Ok(())
}

fn insert_favorites_album_row(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute(
        "INSERT INTO albums (folder_path, name, cover_uri, photo_count, last_modified)
         SELECT
             ?1,
             ?2,
             COALESCE(
                 (SELECT cover_uri FROM album_covers c WHERE c.folder_path = ?1),
                 (SELECT uri FROM media_items m2
                  WHERE m2.trashed_at IS NULL AND m2.is_favorite = 1
                  ORDER BY COALESCE(m2.taken_at, m2.file_mtime) DESC, m2.id DESC
                  LIMIT 1)
             ),
             COUNT(*),
             COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND is_favorite = 1",
        rusqlite::params![FAVORITES_ALBUM_PATH, tr("album.favorites.name")],
    )?;
    Ok(())
}

fn insert_media_kind_album_row(
    tx: &rusqlite::Transaction<'_>,
    media_kind: &str,
    virtual_path: &str,
    name: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO albums (folder_path, name, cover_uri, photo_count, last_modified)
         SELECT
             ?1,
             ?2,
             COALESCE(
                 (SELECT cover_uri FROM album_covers c WHERE c.folder_path = ?1),
                 (SELECT uri FROM media_items m2
                  WHERE m2.trashed_at IS NULL AND m2.media_kind = ?3
                  ORDER BY COALESCE(m2.taken_at, m2.file_mtime) DESC, m2.id DESC
                  LIMIT 1)
             ),
             COUNT(*),
             COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND media_kind = ?3",
        rusqlite::params![virtual_path, name, media_kind],
    )?;
    Ok(())
}

fn insert_media_subkind_album_row(
    tx: &rusqlite::Transaction<'_>,
    media_subkind: &str,
    virtual_path: &str,
    name: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO albums (folder_path, name, cover_uri, photo_count, last_modified)
         SELECT
             ?1,
             ?2,
             COALESCE(
                 (SELECT cover_uri FROM album_covers c WHERE c.folder_path = ?1),
                 (SELECT uri FROM media_items m2
                  WHERE m2.trashed_at IS NULL AND m2.media_subkind = ?3
                  ORDER BY COALESCE(m2.taken_at, m2.file_mtime) DESC, m2.id DESC
                  LIMIT 1)
             ),
             COUNT(*),
             COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND media_subkind = ?3",
        rusqlite::params![virtual_path, name, media_subkind],
    )?;
    Ok(())
}

fn insert_media_attribute_album_row(
    tx: &rusqlite::Transaction<'_>,
    attribute: &str,
    virtual_path: &str,
    name: &str,
) -> Result<()> {
    let json_path = format!("$.{attribute}");
    tx.execute(
        "INSERT INTO albums (folder_path, name, cover_uri, photo_count, last_modified)
         SELECT
             ?1,
             ?2,
             COALESCE(
                 (SELECT cover_uri FROM album_covers c WHERE c.folder_path = ?1),
                 (SELECT uri FROM media_items m2
                  WHERE m2.trashed_at IS NULL AND json_extract(m2.media_attributes, ?3) = 1
                  ORDER BY COALESCE(m2.taken_at, m2.file_mtime) DESC, m2.id DESC
                  LIMIT 1)
             ),
             COUNT(*),
             COALESCE(MAX(file_mtime), 0)
         FROM media_items
         WHERE trashed_at IS NULL AND json_extract(media_attributes, ?3) = 1",
        rusqlite::params![virtual_path, name, json_path],
    )?;
    Ok(())
}

#[cfg(test)]
fn refresh_with_observer_for_tests<F>(pool: &DbPool, after_clear: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    refresh_with_observer(pool, after_clear)
}

/// 列出所有相册，按最近修改排序
pub fn list(pool: &DbPool) -> Result<Vec<Album>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT folder_path, name, cover_uri, photo_count, last_modified
         FROM albums
         WHERE folder_path NOT IN (?1, ?2, ?3, ?4, ?5, ?6)
         ORDER BY last_modified DESC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            VIRTUAL_ALBUM_PATHS[0],
            VIRTUAL_ALBUM_PATHS[1],
            VIRTUAL_ALBUM_PATHS[2],
            VIRTUAL_ALBUM_PATHS[3],
            VIRTUAL_ALBUM_PATHS[4],
            VIRTUAL_ALBUM_PATHS[5]
        ],
        |row| {
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
        },
    )?;
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

#[cfg(test)]
mod tests;
