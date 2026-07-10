mod common;
use chrono::{TimeZone, Utc};
use photo_viewer::core::albums;
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use std::path::Path;
use tempfile::tempdir;

fn make_item(uri: &str, path: &str, folder: &str) -> NewMediaItem {
    make_item_with_mime(uri, path, folder, "image/jpeg")
}

fn make_item_at(uri: &str, path: &str, folder: &str, day: u32) -> NewMediaItem {
    let mtime = Utc.with_ymd_and_hms(2025, 4, day, 12, 0, 0).unwrap();
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(mtime),
        file_mtime: mtime,
        file_size: 1000,
        blake3_hash: format!("h{}", uri),
    }
}

fn make_item_with_mime(uri: &str, path: &str, folder: &str, mime_type: &str) -> NewMediaItem {
    make_item_with_mime_and_subkind(uri, path, folder, mime_type, "standard")
}

fn make_item_with_mime_and_subkind(
    uri: &str,
    path: &str,
    folder: &str,
    mime_type: &str,
    media_subkind: &str,
) -> NewMediaItem {
    make_item_with_mime_subkind_and_attrs(uri, path, folder, mime_type, media_subkind, "{}")
}

fn make_item_with_mime_subkind_and_attrs(
    uri: &str,
    path: &str,
    folder: &str,
    mime_type: &str,
    media_subkind: &str,
    media_attributes: &str,
) -> NewMediaItem {
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: mime_type.into(),
        media_subkind: media_subkind.into(),
        media_attributes: media_attributes.into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: format!("h{}", uri),
    }
}

#[test]
fn media_type_albums_include_only_non_empty_attribute_categories() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    common::db::insert_media_item(
        &pool,
        &make_item_with_mime_and_subkind(
            "file:///Pictures/live.jpg",
            "/Pictures/live.jpg",
            "/Pictures",
            "image/jpeg",
            "motion_photo",
        ),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item_with_mime_subkind_and_attrs(
            "file:///Pictures/anim.gif",
            "/Pictures/anim.gif",
            "/Pictures",
            "image/gif",
            "standard",
            r#"{"animated":true}"#,
        ),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item_with_mime_subkind_and_attrs(
            "file:///Pictures/hdr.heic",
            "/Pictures/hdr.heic",
            "/Pictures",
            "image/heic",
            "standard",
            r#"{"hdr":true}"#,
        ),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item_with_mime(
            "file:///Pictures/still.jpg",
            "/Pictures/still.jpg",
            "/Pictures",
            "image/jpeg",
        ),
    )
    .unwrap();

    let list = albums::list_media_type_albums(&pool).unwrap();
    assert_eq!(list.len(), 3);
    let motion = &list[0];
    assert_eq!(
        motion.folder_path.as_path(),
        Path::new(albums::MOTION_PHOTOS_ALBUM_PATH)
    );
    assert!(motion.is_virtual);
    assert_eq!(motion.photo_count, 1);
    assert_eq!(
        motion.cover_uri.as_deref(),
        Some("file:///Pictures/live.jpg")
    );

    let animated = &list[1];
    assert_eq!(
        animated.folder_path.as_path(),
        Path::new(albums::ANIMATED_ALBUM_PATH)
    );
    assert_eq!(animated.photo_count, 1);
    assert_eq!(
        animated.cover_uri.as_deref(),
        Some("file:///Pictures/anim.gif")
    );

    let hdr = &list[2];
    assert_eq!(hdr.folder_path.as_path(), Path::new(albums::HDR_ALBUM_PATH));
    assert_eq!(hdr.photo_count, 1);
    assert_eq!(hdr.cover_uri.as_deref(), Some("file:///Pictures/hdr.heic"));
}

#[test]
fn media_type_albums_hide_empty_categories() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    common::db::insert_media_item(
        &pool,
        &make_item_with_mime(
            "file:///Pictures/still.jpg",
            "/Pictures/still.jpg",
            "/Pictures",
            "image/jpeg",
        ),
    )
    .unwrap();

    let list = albums::list_media_type_albums(&pool).unwrap();
    assert!(
        list.is_empty(),
        "media type albums should be hidden when no attribute categories have media"
    );
}

#[test]
fn refresh_groups_by_folder() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    common::db::insert_media_item(
        &pool,
        &make_item("file:///p/Camera/a.jpg", "/p/Camera/a.jpg", "/p/Camera"),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item("file:///p/Camera/b.jpg", "/p/Camera/b.jpg", "/p/Camera"),
    )
    .unwrap();
    common::db::insert_media_item(
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
fn folder_album_cover_defaults_to_latest_media() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    common::db::insert_media_item(
        &pool,
        &make_item_at(
            "file:///p/Camera/old.jpg",
            "/p/Camera/old.jpg",
            "/p/Camera",
            1,
        ),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item_at(
            "file:///p/Camera/new.jpg",
            "/p/Camera/new.jpg",
            "/p/Camera",
            2,
        ),
    )
    .unwrap();

    albums::refresh(&pool).unwrap();

    let album = albums::find_by_folder_path(&pool, Path::new("/p/Camera"))
        .unwrap()
        .expect("folder album should exist");
    assert_eq!(
        album.cover_uri.as_deref(),
        Some("file:///p/Camera/new.jpg"),
        "album cover should fall back to the newest media item"
    );
}

#[test]
fn explicit_folder_album_cover_overrides_latest_media_and_survives_refresh() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    common::db::insert_media_item(
        &pool,
        &make_item_at(
            "file:///p/Camera/old.jpg",
            "/p/Camera/old.jpg",
            "/p/Camera",
            1,
        ),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item_at(
            "file:///p/Camera/new.jpg",
            "/p/Camera/new.jpg",
            "/p/Camera",
            2,
        ),
    )
    .unwrap();

    albums::refresh(&pool).unwrap();
    albums::set_album_cover(&pool, Path::new("/p/Camera"), "file:///p/Camera/old.jpg").unwrap();
    albums::refresh(&pool).unwrap();

    let album = albums::find_by_folder_path(&pool, Path::new("/p/Camera"))
        .unwrap()
        .expect("folder album should exist");
    assert_eq!(
        album.cover_uri.as_deref(),
        Some("file:///p/Camera/old.jpg"),
        "manual album cover should have priority over the newest media item"
    );
}

#[test]
fn trashed_items_excluded_from_albums() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let id = common::db::insert_media_item(&pool, &make_item("file:///p/a.jpg", "/p/a.jpg", "/p"))
        .unwrap();
    common::db::mark_trashed(&pool, id).unwrap();

    albums::refresh(&pool).unwrap();
    let list = albums::list(&pool).unwrap();
    assert_eq!(list.len(), 0);
}

#[test]
fn list_with_favorites_includes_type_virtual_albums() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    common::db::insert_media_item(
        &pool,
        &make_item_with_mime(
            "file:///Videos/photo-in-video-dir.jpg",
            "/Videos/photo-in-video-dir.jpg",
            "/Videos",
            "image/jpeg",
        ),
    )
    .unwrap();
    common::db::insert_media_item(
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
