use chrono::{TimeZone, Utc};
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
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
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
fn list_all_reports_invalid_timestamp_rows() {
    let pool = fresh_pool();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO media_items
            (uri, path, folder_path, mime_type, width, height,
             taken_at, file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, ?6, ?7, unixepoch())",
        rusqlite::params![
            "file:///test/bad.jpg",
            "/test/bad.jpg",
            "/test",
            "image/jpeg",
            i64::MAX,
            10_i64,
            "bad-hash",
        ],
    )
    .unwrap();

    let err = db::list_all_media(&pool).expect_err("bad row should not be silently dropped");
    assert!(
        err.to_string().contains("timestamp"),
        "error should explain the invalid timestamp, got {err}"
    );
}

#[test]
fn list_all_orders_by_taken_at_then_file_time_fallback() {
    let pool = fresh_pool();

    let mut exif_newest = sample_new_item();
    exif_newest.uri = "file:///test/exif-newest.jpg".into();
    exif_newest.path = "/test/exif-newest.jpg".into();
    exif_newest.taken_at = Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    exif_newest.file_mtime = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    exif_newest.blake3_hash = "exif-newest".into();
    db::insert_media_item(&pool, &exif_newest).unwrap();

    let mut file_time_middle = sample_new_item();
    file_time_middle.uri = "file:///test/file-middle.jpg".into();
    file_time_middle.path = "/test/file-middle.jpg".into();
    file_time_middle.taken_at = None;
    file_time_middle.file_mtime = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
    file_time_middle.blake3_hash = "file-middle".into();
    db::insert_media_item(&pool, &file_time_middle).unwrap();

    let mut oldest = sample_new_item();
    oldest.uri = "file:///test/oldest.jpg".into();
    oldest.path = "/test/oldest.jpg".into();
    oldest.taken_at = None;
    oldest.file_mtime = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    oldest.blake3_hash = "oldest".into();
    db::insert_media_item(&pool, &oldest).unwrap();

    let all = db::list_all_media(&pool).unwrap();
    let names: Vec<_> = all.iter().map(|item| item.display_name()).collect();
    assert_eq!(
        names,
        vec!["exif-newest.jpg", "file-middle.jpg", "oldest.jpg"]
    );
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
