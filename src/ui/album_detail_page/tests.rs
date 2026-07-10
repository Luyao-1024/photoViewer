use super::*;
use chrono::{TimeZone, Utc};
use std::path::PathBuf;

fn sample_item(id: i64) -> crate::core::media::MediaItem {
    let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
    crate::core::media::MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: PathBuf::from(format!("/tmp/{id}.jpg")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 100,
        blake3_hash: format!("hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}

fn new_item(id: i64, mime_type: &str) -> crate::core::media::NewMediaItem {
    let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
    let ext = if mime_type.starts_with("video/") {
        "mp4"
    } else {
        "jpg"
    };
    let path = PathBuf::from(format!("/tmp/{id}.{ext}"));
    crate::core::media::NewMediaItem {
        uri: format!("file:///tmp/{id}.{ext}"),
        path,
        folder_path: PathBuf::from("/tmp"),
        mime_type: mime_type.into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 100,
        blake3_hash: format!("hash-new-{id}"),
    }
}

#[gtk::test]
fn remove_media_item_by_id_updates_shared_master_list() {
    let _ = gtk::init();
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    list.append(&glib::BoxedAnyObject::new(sample_item(1)));
    list.append(&glib::BoxedAnyObject::new(sample_item(2)));

    assert!(remove_media_item_by_id(&list, 1));
    assert_eq!(list.n_items(), 1);
    assert!(!remove_media_item_by_id(&list, 3));
}

#[gtk::test]
fn video_virtual_album_uses_database_when_master_window_has_no_videos() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("videos.db")).unwrap();
    crate::core::db::upsert_media_items_batch(
        &pool,
        &[new_item(1, "image/jpeg"), new_item(2, "video/mp4")],
    )
    .unwrap();
    let album = crate::core::albums::Album {
        folder_path: PathBuf::from(crate::core::albums::VIDEOS_ALBUM_PATH),
        name: "Videos".into(),
        cover_uri: None,
        photo_count: 1,
        last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
        is_virtual: true,
    };
    let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    master.append(&glib::BoxedAnyObject::new(sample_item(1)));

    let items = filtered_items_for_album_limited(&album, &master, &pool, u32::MAX);

    assert_eq!(items.len(), 1);
    assert!(items[0].is_video());
}

#[gtk::test]
fn virtual_album_filter_can_limit_database_membership() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("limited-videos.db")).unwrap();
    crate::core::db::upsert_media_items_batch(
        &pool,
        &[
            new_item(1, "video/mp4"),
            new_item(2, "video/mp4"),
            new_item(3, "video/mp4"),
        ],
    )
    .unwrap();
    let album = crate::core::albums::Album {
        folder_path: PathBuf::from(crate::core::albums::VIDEOS_ALBUM_PATH),
        name: "Videos".into(),
        cover_uri: None,
        photo_count: 3,
        last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
        is_virtual: true,
    };
    let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();

    let items = filtered_items_for_album_limited(&album, &master, &pool, 2);

    assert_eq!(items.len(), 2);
}

#[gtk::test]
fn visible_real_album_refresh_loads_new_database_items() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("real-album-refresh.db")).unwrap();
    let first = new_item(1, "image/jpeg");
    crate::core::db::insert_media_item(&pool, &first).unwrap();
    let initial =
        crate::core::db::list_media_by_folder_page(&pool, &first.folder_path, 0, 10).unwrap();
    assert_eq!(initial.len(), 1);

    let album = crate::core::albums::Album {
        folder_path: first.folder_path.clone(),
        name: "tmp".into(),
        cover_uri: None,
        photo_count: 1,
        last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
        is_virtual: false,
    };
    let album_store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    album_store.append(&glib::BoxedAnyObject::new(initial[0].clone()));
    let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    master.append(&glib::BoxedAnyObject::new(initial[0].clone()));
    let loader = Arc::new(crate::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let page = AlbumDetailPage::new(album, album_store.clone(), master, pool.clone(), loader);

    crate::core::db::insert_media_item(&pool, &new_item(2, "image/jpeg")).unwrap();
    page.refresh_media_list_from_repository();

    assert_eq!(
        album_store.n_items(),
        2,
        "refreshing a visible real album should use the database, not the stale Photos window"
    );
}

#[gtk::test]
fn visible_real_album_refresh_emits_single_addition_change() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool =
        crate::core::db::init_pool(&tmp.path().join("real-album-refresh-single.db")).unwrap();
    let first = new_item(1, "image/jpeg");
    crate::core::db::insert_media_item(&pool, &first).unwrap();
    let initial =
        crate::core::db::list_media_by_folder_page(&pool, &first.folder_path, 0, 10).unwrap();
    let album = crate::core::albums::Album {
        folder_path: first.folder_path.clone(),
        name: "tmp".into(),
        cover_uri: None,
        photo_count: 1,
        last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
        is_virtual: false,
    };
    let album_store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    album_store.append(&glib::BoxedAnyObject::new(initial[0].clone()));
    let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    master.append(&glib::BoxedAnyObject::new(initial[0].clone()));
    let loader = Arc::new(crate::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let page = AlbumDetailPage::new(album, album_store.clone(), master, pool.clone(), loader);
    let changes = Rc::new(RefCell::new(Vec::<(u32, u32, u32)>::new()));
    let changes_for_signal = changes.clone();
    album_store.connect_items_changed(move |_, position, removed, added| {
        changes_for_signal
            .borrow_mut()
            .push((position, removed, added));
    });

    crate::core::db::insert_media_item(&pool, &new_item(2, "image/jpeg")).unwrap();
    page.refresh_media_list_from_repository();

    assert_eq!(
            *changes.borrow(),
            vec![(0, 0, 1)],
            "album refresh should emit one pure-addition change so MediaGrid can use its insertion path"
        );
}
