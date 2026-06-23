//! 把媒体项复制 / 移动到目标相册文件夹，并同步更新 DB。
//!
//! 设计要点：
//! - **Copy**: `std::fs::copy` + 插入新 `media_items` 行（新 `id`，同 `blake3_hash`）。
//! - **Move**: `std::fs::rename` + `db::update_media_location` 原地更新
//!   `path` / `folder_path` / `uri` / `file_mtime`（同 `id`）。
//! - 目标文件夹中已存在同名文件时自动重命名为 `<stem>_1.<ext>`、
//!   `<stem>_2.<ext>`……最多尝试 9999 次后报错。
//! - 每次操作完成后调用 `albums::refresh`,相册侧栏计数自动同步。
//!
//! 整个函数 `add_to_album` 应当在调用方的工作线程中运行（`spawn_blocking`），
//! 因为它会做阻塞的 `std::fs` I/O。
use std::path::{Path, PathBuf};

use crate::core::albums;
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::media::{MediaItem, NewMediaItem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumOpMode {
    Copy,
    Move,
}

/// 把一组媒体项加入目标相册文件夹,返回最终生效的 `MediaItem` 列表（按输入顺序）。
///
/// - `media_ids`: 要添加的 `media_items.id` 列表（不会修改入参）
/// - `target_folder`: 目标相册目录（必须已存在）
/// - `mode`: Copy 或 Move
///
/// 错误：DB 错误 / IO 错误 / 目标文件夹不存在 / 重名超过 9999。
pub fn add_to_album(
    pool: &DbPool,
    media_ids: &[i64],
    target_folder: &Path,
    mode: AlbumOpMode,
) -> Result<Vec<MediaItem>> {
    if !target_folder.is_dir() {
        return Err(AppError::Backend(format!(
            "target album folder does not exist: {}",
            target_folder.display()
        )));
    }
    let mut updated = Vec::with_capacity(media_ids.len());
    for &id in media_ids {
        let item = db::get_media_item(pool, id)?;
        let new_item = add_one(pool, item, target_folder, mode)?;
        updated.push(new_item);
    }
    albums::refresh(pool)?;
    Ok(updated)
}

fn add_one(
    pool: &DbPool,
    item: MediaItem,
    target_folder: &Path,
    mode: AlbumOpMode,
) -> Result<MediaItem> {
    let ext = item
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg");
    let stem = item
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("photo");
    let dst = resolve_unique_path(target_folder, stem, ext)?;

    match mode {
        AlbumOpMode::Copy => {
            std::fs::copy(&item.path, &dst)?;
            let mut new_item = item.clone();
            new_item.path = dst.clone();
            new_item.folder_path = target_folder.to_path_buf();
            new_item.uri = format!("file://{}", dst.display());
            // blake3_hash 已不变,file_mtime 取新文件侧排序时间
            let mtime = std::fs::metadata(&dst)
                .and_then(|m| m.created().or_else(|_| m.modified()))
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                })
                .unwrap_or_else(chrono::Utc::now);
            new_item.file_mtime = mtime;
            // 通过 NewMediaItem 走统一 insert 路径（自动生成新 id），
            // 然后把新行 id 写回返回的 new_item,这样调用方能拿到正确 id。
            let new_new = NewMediaItem::from(&new_item);
            let new_id = db::insert_media_item(pool, &new_new)?;
            new_item.id = new_id;
            Ok(new_item)
        }
        AlbumOpMode::Move => {
            std::fs::rename(&item.path, &dst)?;
            db::update_media_location(pool, item.id, &dst, target_folder)?;
            let mut moved = item;
            moved.path = dst;
            moved.folder_path = target_folder.to_path_buf();
            moved.uri = format!("file://{}", moved.path.display());
            Ok(moved)
        }
    }
}

/// 目标文件夹中找一个不存在的文件名：先尝试 `<stem>.<ext>`，再 `<stem>_1.<ext>`……
fn resolve_unique_path(folder: &Path, stem: &str, ext: &str) -> Result<PathBuf> {
    let primary = folder.join(format!("{stem}.{ext}"));
    if !primary.exists() {
        return Ok(primary);
    }
    for n in 1..=9999 {
        let candidate = folder.join(format!("{stem}_{n}.{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::Backend(format!(
        "no available filename for {stem}.{ext} in {}",
        folder.display()
    )))
}
