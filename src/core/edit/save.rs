//! Save Copy / Save Overwrite 实现
//!
//! `save_as_copy` 渲染当前 `EditState` 到一个新文件 `{原名}_edited.{ext}`
//! 并在 `media_items` 中插入新行（不破坏原图）。
//!
//! `save_overwrite` 先把原图备份到 `{原名}.jpg.bak`，再渲染并写回原文件，
//! 最后刷新 `media_items` 中对应行的 `file_mtime` / `file_size` /
//! `blake3_hash` 以反映新内容。
use std::path::PathBuf;

use chrono::Utc;

use crate::core::db::{self, DbPool};
use crate::core::edit::{apply_all, EditRegistry, EditState};
use crate::core::error::Result;
use crate::core::media::{MediaItem, NewMediaItem};

/// 保存为副本：渲染到 `{原名}_edited.{ext}`，插入新 DB 行。返回新行对应的
/// `MediaItem`（含新分配的 `id`）。
pub fn save_as_copy(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
) -> Result<MediaItem> {
    // 1. 加载原图全分辨率
    let img = image::open(&source.path)?;

    // 2. 应用所有编辑
    let rendered = apply_all(registry, img, state)
        .map_err(crate::core::error::AppError::Decode)?;

    // 3. 生成新文件名（避免覆盖同名副本）
    let new_path = generate_edited_path(&source.path);

    // 4. 保存为 JPEG quality=95
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    rendered.save_with_format(&new_path, image::ImageFormat::Jpeg)?;

    // 5. 插入新 DB 行
    let new_uri = format!("file://{}", new_path.display());
    let file_bytes = std::fs::read(&new_path)?;
    let new_item = NewMediaItem {
        uri: new_uri,
        path: new_path.clone(),
        folder_path: source.folder_path.clone(),
        mime_type: source.mime_type.clone(),
        width: Some(rendered.width()),
        height: Some(rendered.height()),
        taken_at: source.taken_at,
        file_mtime: Utc::now(),
        file_size: std::fs::metadata(&new_path).map(|m| m.len()).unwrap_or(0),
        blake3_hash: blake3::hash(&file_bytes).to_hex().to_string(),
    };
    let id = db::insert_media_item(pool, &new_item)?;
    Ok(db::get_media_item(pool, id)?)
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
    let img = image::open(&source.path)?;
    let rendered = apply_all(registry, img, state)
        .map_err(crate::core::error::AppError::Decode)?;

    // 3. 写回原路径
    rendered.save_with_format(&source.path, image::ImageFormat::Jpeg)?;

    // 4. 更新 DB 元数据
    let conn = pool.get()?;
    let new_mtime = Utc::now().timestamp();
    let new_size = std::fs::metadata(&source.path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let new_hash = blake3::hash(&std::fs::read(&source.path)?)
        .to_hex()
        .to_string();
    conn.execute(
        "UPDATE media_items SET file_mtime=?2, file_size=?3, blake3_hash=?4 WHERE id=?1",
        rusqlite::params![source.id, new_mtime, new_size, new_hash],
    )?;

    Ok(())
}

/// 构造 `{原名}.{ext}.bak` 路径——在原文件扩展名后再加 `.bak`。
fn backup_path_for(path: &std::path::Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

/// 生成新副本路径：优先 `{stem}_edited.{ext}`，若已存在则用
/// `{stem}_edited_1.{ext}`、`{stem}_edited_2.{ext}` … 直至可用。
fn generate_edited_path(orig: &std::path::Path) -> PathBuf {
    let stem = orig.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let ext = orig.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
    let parent = orig.parent().unwrap_or(std::path::Path::new("."));
    let mut candidate = parent.join(format!("{}_edited.{}", stem, ext));
    if !candidate.exists() {
        return candidate;
    }
    let mut suffix = 1;
    loop {
        candidate = parent.join(format!("{}_edited_{}.{}", stem, suffix, ext));
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
}
