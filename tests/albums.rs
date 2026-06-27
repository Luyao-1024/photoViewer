use chrono::Utc;
use photo_viewer::core::albums;
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use std::path::Path;
use tempfile::tempdir;

fn make_item(uri: &str, path: &str, folder: &str) -> NewMediaItem {
    make_item_with_mime(uri, path, folder, "image/jpeg")
}

fn make_item_with_mime(uri: &str, path: &str, folder: &str, mime_type: &str) -> NewMediaItem {
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: mime_type.into(),
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

#[test]
fn list_with_favorites_includes_type_virtual_albums() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    db::insert_media_item(
        &pool,
        &make_item_with_mime(
            "file:///Videos/photo-in-video-dir.jpg",
            "/Videos/photo-in-video-dir.jpg",
            "/Videos",
            "image/jpeg",
        ),
    )
    .unwrap();
    db::insert_media_item(
        &pool,
        &make_item_with_mime(
            "file:///Pictures/video-in-picture-dir.mp4",
            "/Pictures/video-in-picture-dir.mp4",
            "/Pictures",
            "video/mp4",
        ),
    )
    .unwrap();

    albums::refresh(&pool).unwrap();
    let list = albums::list_with_favorites(&pool).unwrap();

    let images = list
        .iter()
        .find(|album| album.folder_path.as_path() == Path::new(albums::IMAGES_ALBUM_PATH))
        .expect("images virtual album should exist");
    let videos = list
        .iter()
        .find(|album| album.folder_path.as_path() == Path::new(albums::VIDEOS_ALBUM_PATH))
        .expect("videos virtual album should exist");

    assert!(images.is_virtual);
    assert!(videos.is_virtual);
    assert_eq!(
        images.photo_count, 1,
        "image album filters by type, not path"
    );
    assert_eq!(
        videos.photo_count, 1,
        "video album filters by type, not path"
    );
    assert_eq!(
        images.cover_uri.as_deref(),
        Some("file:///Videos/photo-in-video-dir.jpg")
    );
    assert_eq!(
        videos.cover_uri.as_deref(),
        Some("file:///Pictures/video-in-picture-dir.mp4")
    );
}
