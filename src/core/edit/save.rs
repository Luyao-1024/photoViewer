//! Save Copy / Save Overwrite 实现
//!
//! `save_as_copy` 渲染当前 `EditState` 到一个新文件
//! `{原名}_edited_{毫秒时间戳}.{ext}`
//! 并在 `media_items` 中插入新行（不破坏原图）。
//!
//! `save_overwrite` 先把原图备份到 `{原名}.jpg.bak`，再渲染并写回原文件，
//! 最后刷新 `media_items` 中对应行的 `file_mtime` / `file_size` /
//! `blake3_hash` 以反映新内容。
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use image::ImageReader;

use crate::core::db::{self, DbPool};
use crate::core::edit::{apply_all, EditRegistry, EditState};
use crate::core::error::Result;
use crate::core::media::{MediaItem, NewMediaItem, MEDIA_SUBKIND_STANDARD};
use crate::core::orientation;

/// 保存为副本：渲染到 `{原名}_edited_{毫秒时间戳}.{ext}`，插入新 DB 行。
/// 返回新行对应的 `MediaItem`（含新分配的 `id`）。
pub fn save_as_copy(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
) -> Result<MediaItem> {
    // 1. 加载原图全分辨率
    let img = load_source_image(&source.path)?;

    // 2. 应用所有编辑
    let rendered = apply_all(registry, img, state).map_err(crate::core::error::AppError::Decode)?;

    // 3. 生成新文件名（避免覆盖同名副本）
    let new_path = generate_edited_path(&source.path);

    // 4. 按目标扩展名保存，避免 `.png` 路径写入 JPEG 字节。
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let format =
        image::ImageFormat::from_path(&new_path).map_err(crate::core::error::AppError::Image)?;
    rendered.save_with_format(&new_path, format)?;

    // 5. 插入新 DB 行
    let new_uri = format!("file://{}", new_path.display());
    let new_item = NewMediaItem {
        uri: new_uri,
        path: new_path.clone(),
        folder_path: source.folder_path.clone(),
        mime_type: source.mime_type.clone(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: Some(rendered.width()),
        height: Some(rendered.height()),
        video_duration_secs: None,
        taken_at: source.taken_at,
        file_mtime: Utc::now(),
        file_size: std::fs::metadata(&new_path).map(|m| m.len()).unwrap_or(0),
        blake3_hash: stream_file_hash(&new_path)?,
    };
    insert_or_update_copy_row(pool, &new_item)
}

/// 覆盖原图：备份到 `.{ext}.bak` → 渲染 → 写回原文件 → 更新 DB 元数据。
pub fn save_overwrite(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
) -> Result<()> {
    // 1. 备份原图
    let backup = backup_path_for(&source.path);
    std::fs::copy(&source.path, &backup)?;

    // 2. 加载原图全分辨率
    let img = load_source_image(&source.path)?;
    let rendered = apply_all(registry, img, state).map_err(crate::core::error::AppError::Decode)?;

    // 3. 按原路径扩展名写回
    let format =
        image::ImageFormat::from_path(&source.path).map_err(crate::core::error::AppError::Image)?;
    rendered.save_with_format(&source.path, format)?;

    // 4. 更新 DB 元数据
    let conn = pool.get()?;
    let new_mtime = Utc::now().timestamp();
    let new_size = std::fs::metadata(&source.path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let new_hash = stream_file_hash(&source.path)?;
    conn.execute(
        "UPDATE media_items SET file_mtime=?2, file_size=?3, blake3_hash=?4 WHERE id=?1",
        rusqlite::params![source.id, new_mtime, new_size, new_hash],
    )?;

    Ok(())
}

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

fn insert_or_update_copy_row(pool: &DbPool, item: &NewMediaItem) -> Result<MediaItem> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO media_items
            (uri, path, folder_path, mime_type, media_kind, media_subkind,
             media_attributes, width, height, video_duration_secs, taken_at,
             file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, unixepoch())
         ON CONFLICT(uri) DO UPDATE SET
             path=excluded.path,
             folder_path=excluded.folder_path,
             mime_type=excluded.mime_type,
             media_kind=excluded.media_kind,
             media_subkind=excluded.media_subkind,
             media_attributes=excluded.media_attributes,
             width=excluded.width,
             height=excluded.height,
             video_duration_secs=excluded.video_duration_secs,
             taken_at=excluded.taken_at,
             file_mtime=excluded.file_mtime,
             file_size=excluded.file_size,
             blake3_hash=excluded.blake3_hash,
             trashed_at=NULL,
             indexed_at=unixepoch()",
        rusqlite::params![
            item.uri,
            item.path.to_string_lossy(),
            item.folder_path.to_string_lossy(),
            item.mime_type,
            db::media_kind_db_value(&item.mime_type),
            item.media_subkind,
            item.media_attributes,
            item.width,
            item.height,
            item.video_duration_secs,
            item.taken_at.map(|t| t.timestamp()),
            item.file_mtime.timestamp(),
            item.file_size as i64,
            item.blake3_hash,
        ],
    )?;
    drop(conn);
    db::get_media_item_by_uri(pool, &item.uri)?
        .ok_or_else(|| crate::core::error::AppError::Backend("saved copy row missing".into()))
}

fn load_source_image(path: &Path) -> Result<image::DynamicImage> {
    let file = std::fs::File::open(path)?;
    let reader = ImageReader::new(std::io::BufReader::new(file)).with_guessed_format()?;
    let img = reader.decode()?;
    let orientation = orientation::read_orientation(path)?;
    Ok(apply_orientation_to_image(img, orientation))
}

fn apply_orientation_to_image(img: image::DynamicImage, orientation: u16) -> image::DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// 构造 `{原名}.{ext}.bak` 路径——在原文件扩展名后再加 `.bak`。
fn backup_path_for(path: &std::path::Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

/// 生成新副本路径：`{stem}_edited_{毫秒时间戳}.{ext}`。
fn generate_edited_path(orig: &std::path::Path) -> PathBuf {
    let stem = orig.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let base_stem = base_stem_without_edited_timestamp(stem);
    let ext = orig.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
    let parent = orig.parent().unwrap_or(std::path::Path::new("."));
    let mut timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    loop {
        let candidate = parent.join(format!("{}_edited_{}.{}", base_stem, timestamp_ms, ext));
        if !candidate.exists() {
            return candidate;
        }
        timestamp_ms += 1;
    }
}

fn base_stem_without_edited_timestamp(stem: &str) -> &str {
    match stem.rsplit_once("_edited_") {
        Some((base, timestamp))
            if !base.is_empty() && timestamp.chars().all(|c| c.is_ascii_digit()) =>
        {
            base
        }
        _ => stem,
    }
}
