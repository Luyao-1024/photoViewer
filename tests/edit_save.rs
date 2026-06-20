mod common;

use chrono::Utc;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::edit::{save, EditRegistry, EditState};
use photo_viewer::core::media::NewMediaItem;
use tempfile::tempdir;

fn make_test_item(dir: &std::path::Path) -> NewMediaItem {
    write_plain_jpeg(dir, "src.jpg");
    NewMediaItem {
        uri: format!("file://{}/src.jpg", dir.display()),
        path: format!("{}/src.jpg", dir.display()).into(),
        folder_path: dir.to_path_buf(),
        mime_type: "image/jpeg".into(),
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
