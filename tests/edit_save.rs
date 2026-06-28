mod common;

use chrono::Utc;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::edit::destructive_rotate;
use photo_viewer::core::edit::{save, EditRegistry, EditState};
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::orientation::read_orientation;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::tempdir;

fn make_test_item(dir: &std::path::Path) -> NewMediaItem {
    write_plain_jpeg(dir, "src.jpg");
    NewMediaItem {
        uri: format!("file://{}/src.jpg", dir.display()),
        path: format!("{}/src.jpg", dir.display()).into(),
        folder_path: dir.to_path_buf(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: "test_hash".into(),
    }
}

fn make_edited_test_item(dir: &std::path::Path) -> NewMediaItem {
    write_plain_jpeg(dir, "src_edited_1234567890.jpg");
    NewMediaItem {
        uri: format!("file://{}/src_edited_1234567890.jpg", dir.display()),
        path: format!("{}/src_edited_1234567890.jpg", dir.display()).into(),
        folder_path: dir.to_path_buf(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: "test_hash".into(),
    }
}

fn make_png_test_item(dir: &std::path::Path) -> NewMediaItem {
    write_plain_png(dir, "src.png");
    NewMediaItem {
        uri: format!("file://{}/src.png", dir.display()),
        path: format!("{}/src.png", dir.display()).into(),
        folder_path: dir.to_path_buf(),
        mime_type: "image/png".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: "test_hash".into(),
    }
}

fn make_mismatched_png_path_with_jpeg_bytes_item(dir: &std::path::Path) -> NewMediaItem {
    let path = dir.join("src.png");
    let img = image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(64, 48, |_, _| {
        image::Rgb([128, 128, 128])
    });
    img.save_with_format(&path, image::ImageFormat::Jpeg)
        .unwrap();
    NewMediaItem {
        uri: format!("file://{}", path.display()),
        path,
        folder_path: dir.to_path_buf(),
        mime_type: "image/png".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: "test_hash".into(),
    }
}

#[test]
fn save_as_copy_creates_new_file() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_test_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();

    let new_item = save::save_as_copy(&media_item, &state, &pool, &registry).unwrap();

    // 新文件存在
    assert!(new_item.path.exists());
    assert!(new_item.path.to_string_lossy().contains("_edited"));

    // DB 新行已插入
    let all = db::list_all_media(&pool).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn save_as_copy_names_file_with_edited_millisecond_timestamp() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_test_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();
    let before_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let new_item = save::save_as_copy(&media_item, &state, &pool, &registry).unwrap();

    let after_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let file_name = new_item.path.file_name().unwrap().to_string_lossy();
    let timestamp = file_name
        .strip_prefix("src_edited_")
        .and_then(|name| name.strip_suffix(".jpg"))
        .expect("saved copy name should be src_edited_<milliseconds>.jpg")
        .parse::<u128>()
        .expect("edited suffix should be a millisecond timestamp");

    assert!((before_ms..=after_ms).contains(&timestamp));
}

#[test]
fn save_as_copy_replaces_existing_edited_timestamp_suffix() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_edited_test_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();
    let new_item = save::save_as_copy(&media_item, &state, &pool, &registry).unwrap();

    let file_name = new_item.path.file_name().unwrap().to_string_lossy();
    assert!(
        file_name.starts_with("src_edited_"),
        "existing edited timestamp should be replaced, got {file_name}"
    );
    assert!(
        !file_name.strip_prefix("src_edited_").unwrap().contains('_'),
        "saved copy name should not append another edited segment, got {file_name}"
    );
    assert!(file_name.ends_with(".jpg"));
}

#[test]
fn save_overwrite_replaces_original() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_test_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();
    let _orig_size = std::fs::metadata(&media_item.path).unwrap().len();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();
    save::save_overwrite(&media_item, &state, &pool, &registry).unwrap();

    // 备份存在
    let backup = media_item.path.with_extension("jpg.bak");
    assert!(backup.exists());

    // 原文件大小非空（即使是 identity 编辑也可能因重新编码而变化）
    let new_size = std::fs::metadata(&media_item.path).unwrap().len();
    assert!(new_size > 0);
}

#[test]
fn save_png_as_copy_preserves_png_file_format() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_png_test_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();

    let new_item = save::save_as_copy(&media_item, &state, &pool, &registry).unwrap();

    let copied_bytes = std::fs::read(&new_item.path).unwrap();
    assert!(
        copied_bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Save Copy for .png should write PNG bytes"
    );
}

#[test]
fn save_as_copy_bakes_source_orientation_into_saved_pixels() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_png_test_item(dir.path());
    destructive_rotate::rotate_in_place(&item.path, 90).unwrap();
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();
    let new_item = save::save_as_copy(&media_item, &state, &pool, &registry).unwrap();

    let saved = image::open(&new_item.path).unwrap();
    assert_eq!((saved.width(), saved.height()), (48, 64));
    assert_eq!(read_orientation(&new_item.path).unwrap(), 1);
}

#[test]
fn save_as_copy_creates_distinct_timestamped_files() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_png_test_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();
    let first = save::save_as_copy(&media_item, &state, &pool, &registry).unwrap();
    let second = save::save_as_copy(&media_item, &state, &pool, &registry).unwrap();

    assert_ne!(first.path, second.path);
    assert!(first.path.exists());
    assert!(second.path.exists());
    assert!(first
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("src_edited_"));
    assert!(second
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("src_edited_"));
    assert_eq!(db::list_all_media(&pool).unwrap().len(), 3);
}

#[test]
fn save_png_overwrite_preserves_png_file_format() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_png_test_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();
    save::save_overwrite(&media_item, &state, &pool, &registry).unwrap();

    let overwritten_bytes = std::fs::read(&media_item.path).unwrap();
    assert!(
        overwritten_bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Save Overwrite for .png should write PNG bytes"
    );
}

#[test]
fn save_overwrite_recovers_png_path_that_contains_jpeg_bytes() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let item = make_mismatched_png_path_with_jpeg_bytes_item(dir.path());
    let id = db::insert_media_item(&pool, &item).unwrap();
    let media_item = db::get_media_item(&pool, id).unwrap();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();
    save::save_overwrite(&media_item, &state, &pool, &registry).unwrap();

    let overwritten_bytes = std::fs::read(&media_item.path).unwrap();
    assert!(
        overwritten_bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Save Overwrite should rewrite mismatched .png paths as valid PNG"
    );
}
