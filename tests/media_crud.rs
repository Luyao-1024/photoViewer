use chrono::Utc;
use photo_viewer::core::db;
use photo_viewer::core::media::{MediaItem, NewMediaItem};
use tempfile::tempdir;

fn fresh_pool() -> db::DbPool {
    let dir = tempdir().unwrap();
    db::init_pool(&dir.path().join("test.db")).unwrap()
}

fn sample_new_item() -> NewMediaItem {
    let now = Utc::now();
    NewMediaItem {
        uri: "file:///test/IMG_001.jpg".into(),
        path: "/test/IMG_001.jpg".into(),
        folder_path: "/test".into(),
        mime_type: "image/jpeg".into(),
        width: Some(1920),
        height: Some(1080),
        taken_at: Some(now),
        file_mtime: now,
        file_size: 100_000,
        blake3_hash: "hash001".into(),
    }
}

#[test]
fn insert_and_get() {
    let pool = fresh_pool();
    let id = db::insert_media_item(&pool, &sample_new_item()).unwrap();
    assert!(id > 0);

    let item = db::get_media_item(&pool, id).unwrap();
    assert_eq!(item.uri, "file:///test/IMG_001.jpg");
    assert_eq!(item.width, Some(1920));
    assert_eq!(item.blake3_hash, "hash001");
}

#[test]
fn list_all_returns_inserted() {
    let pool = fresh_pool();
    db::insert_media_item(&pool, &sample_new_item()).unwrap();

    let mut item2 = sample_new_item();
    item2.uri = "file:///test/IMG_002.jpg".into();
    item2.path = "/test/IMG_002.jpg".into();
    item2.blake3_hash = "hash002".into();
    db::insert_media_item(&pool, &item2).unwrap();

    let all = db::list_all_media(&pool).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn delete_removes_row() {
    let pool = fresh_pool();
    let id = db::insert_media_item(&pool, &sample_new_item()).unwrap();
    db::delete_media_item(&pool, id).unwrap();
    let result = db::get_media_item(&pool, id);
    assert!(result.is_err());
}

#[test]
fn unique_uri_constraint() {
    let pool = fresh_pool();
    db::insert_media_item(&pool, &sample_new_item()).unwrap();
    let result = db::insert_media_item(&pool, &sample_new_item());
    assert!(result.is_err());
}

// Avoid unused import warning when other tests use MediaItem directly.
#[allow(dead_code)]
fn _type_check(_: MediaItem) {}
