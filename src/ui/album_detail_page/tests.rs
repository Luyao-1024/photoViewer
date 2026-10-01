use super::*;
use chrono::{TimeZone, Utc};
use std::path::PathBuf;

fn pump_until(timeout: std::time::Duration, done: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let context = glib::MainContext::default();
    while std::time::Instant::now() < deadline {
        while context.iteration(false) {}
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    while context.iteration(false) {}
    done()
}

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

    assert!(
        pump_until(std::time::Duration::from_secs(2), || album_store.n_items()
            == 2),
        "background repository refresh should complete within the test timeout"
    );
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

    assert!(
        pump_until(std::time::Duration::from_secs(2), || !changes
            .borrow()
            .is_empty()),
        "background repository refresh should emit a change within the test timeout"
    );
    assert_eq!(
            *changes.borrow(),
            vec![(0, 0, 1)],
            "album refresh should emit one pure-addition change so MediaGrid can use its insertion path"
        );
}

/// Build an album page the way the sidebar does: a real folder album whose
/// store is already populated, so `new()` takes the grid branch and wires the
/// header selection chrome. The database holds `total` photos while the GTK
/// store carries only the first `window` of them, which is how a large album
/// reaches the page: a bounded seed list over an authoritative query. The
/// `TempDir` owns the fixture database, so callers must keep it alive for as
/// long as they use the page.
fn album_page_with_photos(total: i64) -> (AlbumDetailPage, tempfile::TempDir) {
    album_page_in_window(total, total as u32)
}

fn album_page_in_window(total: i64, window: u32) -> (AlbumDetailPage, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("album-chrome.db")).unwrap();
    for id in 1..=total {
        crate::core::db::insert_media_item(&pool, &new_item(id, "image/jpeg")).unwrap();
    }
    let folder = PathBuf::from("/tmp");
    let items = crate::core::db::list_media_by_folder_page(&pool, &folder, 0, 200).unwrap();
    let album = crate::core::albums::Album {
        folder_path: folder,
        name: "tmp".into(),
        cover_uri: None,
        photo_count: items.len() as i64,
        last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
        is_virtual: false,
    };
    let store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in items.into_iter().take(window as usize) {
        store.append(&glib::BoxedAnyObject::new(item.clone()));
        master.append(&glib::BoxedAnyObject::new(item));
    }
    let loader = Arc::new(crate::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let page = AlbumDetailPage::new(album, store, master, pool, loader);
    (page, tmp)
}

fn album_grid(page: &AlbumDetailPage) -> VirtualMediaGrid {
    page.imp()
        .grid
        .borrow()
        .clone()
        .expect("a populated album should build a grid")
}

#[gtk::test]
fn batch_actions_follow_the_album_selection_and_the_entry_button_covers_mode() {
    let (page, _tmp) = album_page_with_photos(3);
    let imp = page.imp();
    let grid = album_grid(&page);

    assert!(
        imp.select_mode_revealer.get().reveals_child(),
        "the multi-select entry must be on screen before the user selects anything"
    );
    assert!(!imp.select_all_revealer.get().reveals_child());
    assert!(!imp.add_to_album_revealer.get().reveals_child());
    assert!(!imp.delete_to_trash_revealer.get().reveals_child());

    grid.select_ids(&[MediaId::from(1)]);

    assert!(imp.select_all_revealer.get().reveals_child());
    assert!(imp.add_to_album_revealer.get().reveals_child());
    assert!(imp.delete_to_trash_revealer.get().reveals_child());
    assert!(imp.exit_multi_select_revealer.get().reveals_child());
    assert!(
        !imp.select_mode_revealer.get().reveals_child(),
        "showing 'enter multi-select' while the user is in it would be a lie"
    );
    assert_eq!(
        imp.select_all_btn.get().label().unwrap().as_str(),
        crate::core::i18n::tr("photos.batch.unselect_all"),
        "the toggle must read as an undo once a tile is selected"
    );

    grid.clear_selection();

    assert!(!imp.add_to_album_revealer.get().reveals_child());
    assert!(!imp.delete_to_trash_revealer.get().reveals_child());
    assert!(
        imp.select_mode_revealer.get().reveals_child(),
        "deselecting everything must hand the entry button back"
    );
}

#[gtk::test]
fn the_entry_button_switches_the_album_grid_into_multi_select() {
    let (page, _tmp) = album_page_with_photos(3);
    let imp = page.imp();
    let grid = album_grid(&page);
    assert!(!grid.is_multi_select_mode());

    imp.select_mode_btn.get().emit_clicked();

    assert!(grid.is_multi_select_mode());
    assert!(imp.exit_multi_select_revealer.get().reveals_child());
    assert!(
        !imp.add_to_album_revealer.get().reveals_child(),
        "entering multi-select with nothing chosen must not fake a batch toolbar"
    );
}

#[gtk::test]
fn the_select_all_button_toggles_between_the_whole_album_and_nothing() {
    // 60 photos in the album, 10 of them in the page's bounded GTK seed list:
    // select-all must follow the album, not the window that reached the page.
    let (page, _tmp) = album_page_in_window(60, 10);
    let imp = page.imp();
    let grid = album_grid(&page);

    imp.select_all_btn.get().emit_clicked();

    assert_eq!(
        grid.selected_ids().len(),
        60,
        "select-all should cover the album, not only the rows the grid has loaded"
    );
    assert_eq!(
        imp.select_all_btn.get().label().unwrap().as_str(),
        crate::core::i18n::tr("photos.batch.unselect_all")
    );

    imp.select_all_btn.get().emit_clicked();

    assert!(grid.selected_ids().is_empty());
    assert_eq!(
        imp.select_all_btn.get().label().unwrap().as_str(),
        crate::core::i18n::tr("photos.batch.select_all")
    );
    assert!(!imp.select_all_revealer.get().reveals_child());
}

#[gtk::test]
fn the_exit_button_leaves_multi_select_even_with_no_selection() {
    let (page, _tmp) = album_page_with_photos(3);
    let imp = page.imp();
    let grid = album_grid(&page);

    imp.select_mode_btn.get().emit_clicked();
    imp.exit_multi_select_btn.get().emit_clicked();

    assert!(!grid.is_multi_select_mode());
    assert!(grid.selected_ids().is_empty());
    assert!(!imp.exit_multi_select_revealer.get().reveals_child());
    assert!(
        imp.select_mode_revealer.get().reveals_child(),
        "the door back into multi-select must reopen on exit"
    );
    assert!(
        !imp.add_to_album_revealer.get().reveals_child(),
        "clicking a tile should open the viewer again once multi-select is off"
    );
}

#[gtk::test]
fn the_empty_album_header_still_wires_its_chrome_without_a_grid() {
    let (page, _tmp) = album_page_with_photos(0);
    let imp = page.imp();
    assert!(
        imp.grid.borrow().is_none(),
        "an empty album shows a status page, not a grid"
    );

    imp.select_mode_btn.get().emit_clicked();
    imp.select_all_btn.get().emit_clicked();
    imp.exit_multi_select_btn.get().emit_clicked();

    assert_eq!(
        imp.select_all_btn.get().label().unwrap().as_str(),
        crate::core::i18n::tr("photos.batch.select_all"),
        "chrome copy must come from the catalogues even with nothing to select"
    );
    assert!(!imp.add_to_album_revealer.get().reveals_child());
    assert!(!imp.delete_to_trash_revealer.get().reveals_child());
}

#[gtk::test]
fn browse_scope_keys_drive_the_album_selection_and_leave_escape_to_navigation() {
    let (page, _tmp) = album_page_with_photos(3);
    let imp = page.imp();
    let grid = album_grid(&page);

    assert!(
        page.handle_keyboard_action(KeyboardAction::SelectAll)
            .is_handled(),
        "Ctrl+A is advertised for any photo list, so an album must answer it"
    );
    assert_eq!(grid.selected_ids().len(), 3);
    assert!(imp.add_to_album_revealer.get().reveals_child());

    assert!(page
        .handle_keyboard_action(KeyboardAction::CancelOrClose)
        .is_handled());
    assert!(grid.selected_ids().is_empty());
    assert!(!imp.add_to_album_revealer.get().reveals_child());

    assert!(
        !page
            .handle_keyboard_action(KeyboardAction::CancelOrClose)
            .is_handled(),
        "with nothing left to deselect, Escape must belong to the navigation stack"
    );
}

#[gtk::test]
fn delete_only_consumes_the_album_keyboard_action_when_something_is_selected() {
    let (page, _tmp) = album_page_with_photos(3);

    assert!(
        !page
            .handle_keyboard_action(KeyboardAction::Delete)
            .is_handled(),
        "Delete with no selection should fall through instead of trashing the album"
    );

    let grid = album_grid(&page);
    grid.select_ids(&[MediaId::from(1)]);
    assert!(page
        .handle_keyboard_action(KeyboardAction::Delete)
        .is_handled());
}
