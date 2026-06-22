use chrono::Utc;
use photo_viewer::core::albums;
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use tempfile::tempdir;

fn make_item(uri: &str, path: &str, folder: &str) -> NewMediaItem {
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: "image/jpeg".into(),
        width: Some(100),
        height: Some(100),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: format!("h{}", uri),
    }
}

#[test]
fn refresh_groups_by_folder() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    db::insert_media_item(
        &pool,
        &make_item("file:///p/Camera/a.jpg", "/p/Camera/a.jpg", "/p/Camera"),
    )
    .unwrap();
    db::insert_media_item(
        &pool,
        &make_item("file:///p/Camera/b.jpg", "/p/Camera/b.jpg", "/p/Camera"),
    )
    .unwrap();
    db::insert_media_item(
        &pool,
        &make_item(
            "file:///p/Screenshots/c.jpg",
            "/p/Screenshots/c.jpg",
            "/p/Screenshots",
        ),
    )
    .unwrap();

    albums::refresh(&pool).unwrap();

    let list = albums::list(&pool).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "/p/Camera"); // 最近修改，应排第一；name 与 folder_path 相同
    assert_eq!(list[0].photo_count, 2);
}

#[test]
fn trashed_items_excluded_from_albums() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let id = db::insert_media_item(&pool, &make_item("file:///p/a.jpg", "/p/a.jpg", "/p")).unwrap();
    db::mark_trashed(&pool, id).unwrap();

    albums::refresh(&pool).unwrap();
    let list = albums::list(&pool).unwrap();
    assert_eq!(list.len(), 0);
}
