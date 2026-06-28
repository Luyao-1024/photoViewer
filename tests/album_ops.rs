//! album_ops: 核心 — 把媒体项复制 / 移动到目标相册文件夹，并同步更新 DB。
//!
//! 这些测试不依赖 GTK,只验证纯文件 + DB 行为。
//! 用真实磁盘 tmpdir + 真实 JPEG 字节（`common::write_plain_jpeg`）
//! 来保证 `std::fs::copy` / `rename` 路径真实工作。
mod common;
use chrono::Utc;
use common::*;
use photo_viewer::core::album_ops::{add_to_album, AlbumOpMode};
use photo_viewer::core::albums;
use photo_viewer::core::db;
use photo_viewer::core::media::{MediaItem, NewMediaItem};
use std::path::PathBuf;
use tempfile::tempdir;

/// 构造一个新的 `NewMediaItem`（不写入 DB），辅助复用。
fn make_new_item(path: PathBuf, folder: PathBuf, hash: &str) -> NewMediaItem {
    NewMediaItem {
        uri: format!("file://{}", path.display()),
        path,
        folder_path: folder,
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1_024,
        blake3_hash: hash.into(),
    }
}

/// 一组源文件 + DB,返回 (TempDir, pool, source_folder, source_id, dest_folder, file_path)
/// 注意 TempDir 必须保留以维持 tmp 目录存活。
#[allow(clippy::type_complexity)]
fn setup() -> (
    tempfile::TempDir,
    db::DbPool,
    PathBuf,
    i64,
    PathBuf,
    std::path::PathBuf,
) {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let source_folder = root.join("Camera");
    std::fs::create_dir(&source_folder).unwrap();
    let file_path = write_plain_jpeg(&source_folder, "img1.jpg");

    let dest_folder = root.join("Screenshots");
    std::fs::create_dir(&dest_folder).unwrap();

    let pool = db::init_pool(&root.join("test.db")).unwrap();
    let id = db::insert_media_item(
        &pool,
        &make_new_item(file_path.clone(), source_folder.clone(), "h_img1"),
    )
    .unwrap();
    (dir, pool, source_folder, id, dest_folder, file_path)
}

#[test]
fn move_photo_to_album_updates_db_path_and_folder() {
    let (_dir, pool, source_folder, id, dest_folder, original_path) = setup();

    // Sanity: 源在 DB
    let before = db::get_media_item(&pool, id).unwrap();
    assert_eq!(before.folder_path, source_folder);

    let updated = add_to_album(&pool, &[id], &dest_folder, AlbumOpMode::Move).unwrap();
    assert_eq!(updated.len(), 1);
    let moved = &updated[0];
    assert_eq!(moved.id, id, "Move 保留 id");
    assert_eq!(moved.folder_path, dest_folder);
    assert!(moved.path.starts_with(&dest_folder));
    assert!(moved.path.exists(), "目标文件应在磁盘上");
    assert!(!original_path.exists(), "原文件应已被移动");
    assert_eq!(moved.blake3_hash, "h_img1", "blake3_hash 不变");

    // DB 同步
    let fresh = db::get_media_item(&pool, id).unwrap();
    assert_eq!(fresh.folder_path, dest_folder);
    assert_eq!(fresh.path, moved.path);
}

#[test]
fn copy_photo_creates_new_db_row_with_new_id_same_hash() {
    let (_dir, pool, source_folder, id, dest_folder, original_path) = setup();

    let updated = add_to_album(&pool, &[id], &dest_folder, AlbumOpMode::Copy).unwrap();
    assert_eq!(updated.len(), 1);
    let copied = &updated[0];
    assert_ne!(copied.id, id, "Copy 产生新 id");
    assert_eq!(copied.folder_path, dest_folder);
    assert!(copied.path.exists());
    assert!(original_path.exists(), "Copy 保留原文件");
    assert_eq!(copied.blake3_hash, "h_img1", "blake3_hash 保持");

    // 原行 path/folder 不变
    let orig = db::get_media_item(&pool, id).unwrap();
    assert_eq!(orig.folder_path, source_folder);
    assert_eq!(orig.path, original_path);

    // 新行能在 DB 里查到
    let fresh = db::get_media_item(&pool, copied.id).unwrap();
    assert_eq!(fresh.path, copied.path);
}

#[test]
fn conflict_in_target_folder_renames_with_underscore_n_suffix() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let source = root.join("Camera");
    std::fs::create_dir(&source).unwrap();
    let source_file = write_plain_jpeg(&source, "img.jpg");

    let dest = root.join("Screenshots");
    std::fs::create_dir(&dest).unwrap();
    // 目标里已有一个同名文件
    let existing = write_plain_jpeg(&dest, "img.jpg");

    let pool = db::init_pool(&root.join("test.db")).unwrap();
    let id = db::insert_media_item(
        &pool,
        &make_new_item(source_file.clone(), source.clone(), "h_img"),
    )
    .unwrap();

    let updated = add_to_album(&pool, &[id], &dest, AlbumOpMode::Move).unwrap();
    let moved = &updated[0];
    assert_ne!(moved.path, existing, "冲突时不能覆盖已存在文件");
    assert_eq!(moved.path.file_name().unwrap(), "img_1.jpg");
    assert!(moved.path.exists());
    assert!(existing.exists(), "原 existing 不应被改");
}

#[test]
fn move_refreshes_album_counts_so_old_folder_decrements_new_increments() {
    let (_dir, pool, source_folder, id, dest_folder, _) = setup();

    albums::refresh(&pool).unwrap();
    let list = albums::list(&pool).unwrap();
    let cam_before = list
        .iter()
        .find(|a| a.folder_path == source_folder)
        .map(|a| a.photo_count)
        .unwrap_or(0);
    let scr_before = list
        .iter()
        .find(|a| a.folder_path == dest_folder)
        .map(|a| a.photo_count)
        .unwrap_or(0);
    assert_eq!(cam_before, 1);
    assert_eq!(scr_before, 0);

    add_to_album(&pool, &[id], &dest_folder, AlbumOpMode::Move).unwrap();

    let list2 = albums::list(&pool).unwrap();
    let cam_after = list2
        .iter()
        .find(|a| a.folder_path == source_folder)
        .map(|a| a.photo_count)
        .unwrap_or(0);
    let scr_after = list2
        .iter()
        .find(|a| a.folder_path == dest_folder)
        .map(|a| a.photo_count)
        .unwrap_or(0);
    assert_eq!(cam_after, 0, "源相册计数应减 1");
    assert_eq!(scr_after, 1, "目标相册计数应加 1");
}

// 把 MediaItem 引入以防未使用警告
#[allow(dead_code)]
fn _check_type(_: MediaItem) {}
