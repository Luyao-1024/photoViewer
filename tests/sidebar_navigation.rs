//! Regression coverage for the tree-shaped sidebar navigation.
//!
//! The sidebar lists albums directly under a collapsible "Albums" group header;
//! selecting an album row schedules its `AlbumDetailPage` push directly (there is no
//! intermediate album-grid page anymore). The album rows live in a dedicated
//! bounded scroll region so the top-level Photos / Albums / Trash rows stay
//! stable even with many albums.
mod common;

use chrono::Utc;
use std::sync::Arc;

use gio::prelude::ListModelExt;
use gtk4 as gtk;
use gtk4::prelude::ObjectExt;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::core::albums;
use photo_viewer::core::db;
use photo_viewer::core::i18n::tr;
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::ui::{MainWindow, PhotosPage};

/// The `visible` *property flag* — i.e. what `set_visible` controls — rather
/// than `is_visible()`, which also walks the ancestor chain and is always
/// `false` in a headless test that never shows the window. Reading the flag
/// lets us assert the collapse/expand toggle actually flips row visibility.
fn visible_flag(w: &gtk::Widget) -> bool {
    w.property::<bool>("visible")
}

fn has_css_class(widget: &gtk::Widget, class_name: &str) -> bool {
    widget.css_classes().iter().any(|class| class == class_name)
}

fn drain_main_context() {
    while glib::MainContext::default().iteration(false) {}
}

fn direct_children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut children = Vec::new();
    let mut child = widget.first_child();
    while let Some(current) = child {
        children.push(current.clone());
        child = current.next_sibling();
    }
    children
}

fn find_label_with_class(widget: &gtk::Widget, class_name: &str) -> Option<gtk::Label> {
    if widget.has_css_class(class_name) {
        if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
            return Some(label);
        }
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_label_with_class(&current, class_name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn photos_count_uses_loaded_model_before_background_refresh() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.TestDeferredPhotosCount")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item("file:///tmp/root/one.jpg", "/tmp/root/one.jpg", "/tmp/root"),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item("file:///tmp/root/two.jpg", "/tmp/root/two.jpg", "/tmp/root"),
    )
    .unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(
        db::list_media_page(&pool, 0, 1).unwrap().remove(0),
    ));

    window.set_resources(pool, loader, media_list);

    let photos_row = window
        .imp()
        .sidebar_list
        .get()
        .row_at_index(0)
        .expect("Photos row exists");
    let photos_count = find_label_with_class(photos_row.upcast_ref(), "photos-sidebar-count")
        .expect("Photos row should expose a media count label");
    assert_eq!(
        photos_count.label(),
        "1",
        "startup count should use the loaded model and avoid a blocking DB total query"
    );
}

fn make_item(uri: &str, path: &str, folder: &str) -> NewMediaItem {
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
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1024,
        blake3_hash: format!("hash-{uri}"),
    }
}

fn make_item_with_subkind(
    uri: &str,
    path: &str,
    folder: &str,
    media_subkind: &str,
) -> NewMediaItem {
    let mut item = make_item(uri, path, folder);
    item.media_subkind = media_subkind.into();
    item
}

fn navigation_view_has_no_touch_swipe_controller() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.TestNoTouchSwipe")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let nav = window.nav_view();
    window.connect_sidebar(&nav);

    let has_swipe = nav
        .observe_controllers()
        .snapshot()
        .into_iter()
        .any(|controller| controller.downcast::<gtk::GestureSwipe>().is_ok());
    assert!(
        !has_swipe,
        "NavigationView should not install a touch swipe controller that can compete with buttons"
    );
}

#[test]
fn sidebar_navigation_suite() {
    gtk::init().expect("GTK init failed");

    photos_count_uses_loaded_model_before_background_refresh();
    navigation_view_has_no_touch_swipe_controller();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.Test")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    // The main sidebar width must stay stable when viewer pages change content
    // sizing (UI invariant — see CLAUDE.md).
    let split_view = find_overlay_split_view(window.upcast_ref())
        .expect("main window should contain an OverlaySplitView");
    assert_eq!(
        split_view.min_sidebar_width(),
        240.0,
        "main sidebar width should stay stable when viewer pages change content sizing"
    );
    assert_eq!(
        split_view.max_sidebar_width(),
        240.0,
        "main sidebar width should stay stable when viewer pages change content sizing"
    );

    // Glass material on the sidebar surface and every row (including the
    // non-selectable Albums header and the album sub-rows).
    {
        let sidebar_page = window.imp().sidebar_page.get();
        let page_classes: Vec<String> = sidebar_page
            .css_classes()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            page_classes.iter().any(|c| c == "glass-sidebar-page"),
            "sidebar page should carry glass-sidebar-page, got {page_classes:?}",
        );

        let top_sidebar = window.imp().sidebar_list.get();
        let trash_list = window.imp().trash_list.get();
        let list_classes: Vec<String> = top_sidebar
            .css_classes()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let trash_list_classes: Vec<String> = trash_list
            .css_classes()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let sidebar_bg_classes: Vec<String> = top_sidebar
            .parent()
            .map(|p| p.css_classes().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        assert!(
            list_classes.iter().any(|c| c == "glass-sidebar"),
            "top sidebar list should carry glass-sidebar, got {list_classes:?}",
        );
        assert_eq!(
            top_sidebar.margin_top(),
            8,
            "Photos row should not sit flush against the top window edge",
        );
        assert!(
            trash_list_classes.iter().any(|c| c == "glass-sidebar"),
            "trash sidebar list should carry glass-sidebar, got {trash_list_classes:?}",
        );
        assert!(
            sidebar_bg_classes.iter().any(|c| c == "glass-base"),
            "sidebar surface should use glass-base material, got list={list_classes:?}",
        );
        assert!(
            sidebar_bg_classes
                .iter()
                .any(|c| c == "glass-sidebar-surface"),
            "sidebar surface should own the shared sidebar background, got {sidebar_bg_classes:?}",
        );

        let n_items = top_sidebar.observe_children().n_items();
        assert!(n_items > 0, "sidebar should have rows");
        for idx in 0..n_items {
            let row = top_sidebar
                .row_at_index(idx as i32)
                .expect("row exists in sidebar");
            let classes: Vec<String> = row.css_classes().iter().map(|s| s.to_string()).collect();
            assert!(
                classes.iter().any(|c| c == "glass-sidebar-row"),
                "row {idx} should carry glass-sidebar-row, got {classes:?}",
            );
        }
        let trash_row = trash_list
            .row_at_index(0)
            .expect("Trash row should exist in trash list");
        let trash_classes: Vec<String> = trash_row
            .css_classes()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            trash_classes.iter().any(|c| c == "glass-sidebar-row"),
            "Trash row should carry glass-sidebar-row, got {trash_classes:?}",
        );

        let surface = top_sidebar
            .parent()
            .expect("sidebar list should be inside sidebar surface");
        let footer = window
            .imp()
            .settings_button
            .get()
            .parent()
            .expect("settings button should live in sidebar footer");
        let children = direct_children(&surface);
        assert!(
            children.len() >= 4,
            "sidebar surface should include top list, album/media/trash wrapper, spacer, and footer",
        );
        assert_eq!(
            children[0],
            top_sidebar.clone().upcast::<gtk::Widget>(),
            "sidebar surface child 0 should be the top navigation list",
        );
        // The wrapper Box groups album_scroll + selection bar +
        // media_type_header_list + media_type_scroll + trash_list.
        let wrapper = &children[1];
        let wrapper_children = direct_children(wrapper);
        assert!(
            wrapper_children.len() >= 5,
            "wrapper should contain album scroll, selection bar, media type header, media type scroll, and trash list",
        );
        assert_eq!(
            wrapper_children[0],
            window.imp().album_scroll.get().upcast::<gtk::Widget>(),
            "wrapper child 0 should be the album scroll region",
        );
        assert_eq!(
            wrapper_children[1],
            window
                .imp()
                .album_selection_bar
                .get()
                .upcast::<gtk::Widget>(),
            "wrapper child 1 should be the album selection action bar",
        );
        assert!(
            !window.imp().album_selection_bar.get().is_revealed(),
            "album selection action bar should start hidden",
        );
        assert_eq!(
            wrapper_children[2],
            window
                .imp()
                .media_type_header_list
                .get()
                .upcast::<gtk::Widget>(),
            "wrapper child 2 should be the media type header list",
        );
        assert_eq!(
            wrapper_children[3],
            window.imp().media_type_scroll.get().upcast::<gtk::Widget>(),
            "wrapper child 3 should be the media type scroll region",
        );
        assert_eq!(
            wrapper_children[4],
            trash_list.clone().upcast::<gtk::Widget>(),
            "wrapper child 4 should be the Trash list",
        );
        assert!(
            has_css_class(&children[2], "glass-sidebar-spacer"),
            "sidebar surface child 2 should be the flexible footer spacer",
        );
        assert!(
            !children[2].property::<bool>("vexpand"),
            "sidebar spacer should not expand since the wrapper already fills space",
        );
        assert_eq!(
            children[3], footer,
            "sidebar surface child 3 should be the settings footer",
        );
    }

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item("file:///tmp/root/one.jpg", "/tmp/root/one.jpg", "/tmp/root"),
    )
    .unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item_with_subkind(
            "file:///tmp/root/two.jpg",
            "/tmp/root/two.jpg",
            "/tmp/root",
            photo_viewer::core::media::MEDIA_SUBKIND_MOTION_PHOTO,
        ),
    )
    .unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));

    let nav = window.nav_view();
    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    window.show_photos_browsing_page(&photos);

    window.set_resources(pool, loader, media_list);
    let (events, _events_receiver) = photo_viewer::core::events::DomainEventSender::new();
    window.set_db_actor(photo_viewer::core::db_actor::start_db_actor(
        window
            .imp()
            .pool
            .borrow()
            .as_ref()
            .expect("pool should be installed")
            .clone(),
        events,
    ));
    // Now that the pool exists, populate the album rows under the header —
    // mirroring app.rs ordering (set_resources → populate_album_rows → connect).
    window.populate_album_rows();
    window.connect_sidebar(&nav);

    let sidebar = window.imp().sidebar_list.get();
    let trash_list = window.imp().trash_list.get();
    let settings_btn = window.imp().settings_button.get();
    let settings_btn_classes: Vec<String> = settings_btn
        .css_classes()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(
        settings_btn_classes
            .iter()
            .any(|c| c == "glass-toolbar-button"),
        "settings button should use glass toolbar button styling, got {settings_btn_classes:?}",
    );
    assert!(
        settings_btn_classes
            .iter()
            .any(|c| c == "sidebar-settings-button"),
        "settings button should include sidebar-settings-button, got {settings_btn_classes:?}",
    );

    assert_eq!(
        sidebar.observe_children().n_items(),
        2,
        "top sidebar list should contain Photos and Albums header only",
    );
    assert_eq!(
        trash_list.observe_children().n_items(),
        1,
        "trash list should contain the stable Trash row",
    );
    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    let photos_count = find_label_with_class(photos_row.upcast_ref(), "photos-sidebar-count")
        .expect("Photos row should expose a media count label");
    assert_eq!(
        photos_count.label(),
        "2",
        "Photos row should show the total live media count"
    );
    assert!(
        !window.imp().album_scroll.property::<bool>("vexpand"),
        "album scroll should not expand — it sizes to content, wrapper handles fill",
    );

    // Row 1 is the Albums group header — non-selectable (it only collapses).
    let header = sidebar.row_at_index(1).expect("Albums header row exists");
    assert!(
        !header.is_selectable(),
        "Albums header should be non-selectable so it never claims the navigation slot",
    );
    let media_type_header = window
        .imp()
        .media_type_header_list
        .get()
        .row_at_index(0)
        .expect("Media Types header row exists");
    assert!(
        !media_type_header.is_selectable(),
        "Media Types header should be non-selectable so it never claims the navigation slot",
    );

    // Album entries are nested under the header. Even with an empty DB the
    // three virtual albums (favorites / images / videos) are present. The
    // ListView realizes their widgets lazily, so assert against its model.
    {
        let album_targets = window.imp().album_targets.borrow();
        assert!(
            album_targets.len() >= 3,
            "sidebar should list the virtual albums, got {}",
            album_targets.len()
        );
        assert_eq!(
            window
                .imp()
                .album_model
                .borrow()
                .as_ref()
                .expect("album ListStore should exist")
                .n_items(),
            album_targets.len() as u32,
            "virtual album model should mirror all album targets",
        );
    }
    {
        let media_type_rows = window.imp().media_type_rows.borrow();
        assert_eq!(
            media_type_rows.len(),
            1,
            "sidebar should list the single non-empty media type"
        );
        let row = media_type_rows[0].clone();
        assert!(
            visible_flag(row.upcast_ref()),
            "media type rows start expanded/visible"
        );
        let classes: Vec<String> = row.css_classes().iter().map(|s| s.to_string()).collect();
        assert!(
            classes.iter().any(|c| c == "glass-sidebar-subrow"),
            "media type row should carry glass-sidebar-subrow, got {classes:?}",
        );
    }
    // Selecting an album row schedules its AlbumDetailPage crossfade directly.
    window
        .imp()
        .album_selection
        .borrow()
        .as_ref()
        .expect("album selection should exist")
        .select_item(0, true);
    drain_main_context();
    assert_eq!(
        window.browsing_stack().visible_child_name().as_deref(),
        Some("album"),
        "selecting an album row should crossfade to the album child after idle dispatch",
    );
    assert_eq!(
        nav.navigation_stack().n_items(),
        1,
        "album selection should not add an outer NavigationView page",
    );

    // Selecting Photos returns to the root Photos page.
    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    sidebar.select_row(Some(&photos_row));
    assert_eq!(
        window.browsing_stack().visible_child_name().as_deref(),
        Some("photos"),
        "selecting Photos should crossfade back to the Photos child",
    );

    // Selecting a media type row uses the same AlbumDetailPage flow.
    let media_type_row = window.imp().media_type_rows.borrow()[0].clone();
    window
        .imp()
        .media_type_list
        .get()
        .select_row(Some(&media_type_row));
    drain_main_context();
    assert_eq!(
        window.browsing_stack().visible_child_name().as_deref(),
        Some("album"),
        "selecting a media type row should crossfade to the album child after idle dispatch",
    );

    // Trash is in its own stable bottom nav list. Selecting it pushes the Trash
    // page on top of the Photos root.
    let trash_row = trash_list.row_at_index(0).expect("Trash row exists");
    trash_list.select_row(Some(&trash_row));
    drain_main_context();
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(tr("page.trash.title").as_str()),
        "selecting Trash should push the Trash page",
    );
    nav.pop();
    drain_main_context();
    assert_eq!(
        window.browsing_stack().visible_child_name().as_deref(),
        Some("album"),
        "returning from Trash should restore the previous album page",
    );
    assert_eq!(
        window
            .imp()
            .media_type_list
            .get()
            .selected_row()
            .map(|row| row.index()),
        Some(0),
        "returning from Trash should restore the previous album sidebar selection",
    );

    // Collapse toggle hides the album scroll region; expanding brings it back.
    window.toggle_albums_expanded();
    assert!(
        !visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "album scroll region should hide when collapsed"
    );
    window.toggle_albums_expanded();
    assert!(
        visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "album scroll region should reappear when expanded",
    );
    window.toggle_media_types_expanded();
    assert!(
        !visible_flag(window.imp().media_type_scroll.get().upcast_ref()),
        "media type scroll region should hide when collapsed"
    );
    window.toggle_media_types_expanded();
    assert!(
        visible_flag(window.imp().media_type_scroll.get().upcast_ref()),
        "media type scroll region should reappear when expanded",
    );

    assert_album_sidebar_scroll_region_contains_all_albums();
    assert_collapsed_album_refresh_restores_active_selection_after_expand();
    assert_media_type_group_hides_when_no_media_type_albums_exist();
}

fn assert_media_type_group_hides_when_no_media_type_albums_exist() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.TestEmptyMediaTypes")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    common::db::insert_media_item(
        &pool,
        &make_item("file:///tmp/root/one.jpg", "/tmp/root/one.jpg", "/tmp/root"),
    )
    .unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();

    window.set_resources(pool, loader, media_list);
    window.populate_album_rows();

    assert!(window.imp().media_type_rows.borrow().is_empty());
    assert!(
        !visible_flag(window.imp().media_type_header_list.get().upcast_ref()),
        "media type header should be hidden when every media type category is empty"
    );
    assert!(
        !visible_flag(window.imp().media_type_scroll.get().upcast_ref()),
        "media type scroll region should be hidden when every media type category is empty"
    );
}

fn assert_album_sidebar_scroll_region_contains_all_albums() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.TestScrollableAlbums")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let nav = window.nav_view();

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    window.show_photos_browsing_page(&photos);

    window.set_resources(pool.clone(), loader, media_list);
    let (events, _events_receiver) = photo_viewer::core::events::DomainEventSender::new();
    window.set_db_actor(photo_viewer::core::db_actor::start_db_actor(
        pool.clone(),
        events,
    ));

    for i in 0..25 {
        let folder = format!("/tmp/album-{i:02}");
        let uri = format!("file://{folder}/cover.jpg");
        let path = format!("{folder}/cover.jpg");
        common::db::insert_media_item(&pool, &make_item(&uri, &path, &folder)).unwrap();
    }
    albums::refresh(&pool).unwrap();
    window.populate_album_rows();
    window.connect_sidebar(&nav);

    assert_eq!(
        window.imp().targets.borrow().len(),
        2,
        "top sidebar targets should contain only Photos and AlbumsHeader",
    );
    assert_eq!(
        window.imp().album_targets.borrow().len(),
        28,
        "sidebar album list should render all 25 folder albums plus 3 virtual albums",
    );
    assert!(
        visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "expanded album section should show its scroll region",
    );

    let sidebar = window.imp().sidebar_list.get();
    let trash_list = window.imp().trash_list.get();
    assert_eq!(
        sidebar.observe_children().n_items(),
        2,
        "top sidebar list should contain only Photos and Albums header",
    );
    assert_eq!(
        trash_list.observe_children().n_items(),
        1,
        "trash list should contain one stable Trash row",
    );
    let trash_row = trash_list
        .row_at_index(0)
        .expect("Trash row remains stable");
    trash_list.select_row(Some(&trash_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(tr("page.trash.title").as_str()),
        "Trash should remain a stable main sidebar row",
    );
}

fn assert_collapsed_album_refresh_restores_active_selection_after_expand() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.TestCollapsedAlbumRefresh")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let nav = window.nav_view();

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    window.show_photos_browsing_page(&photos);

    window.set_resources(pool.clone(), loader, media_list);

    let folder = "/tmp/album-active";
    common::db::insert_media_item(
        &pool,
        &make_item(
            "file:///tmp/album-active/cover.jpg",
            "/tmp/album-active/cover.jpg",
            folder,
        ),
    )
    .unwrap();
    albums::refresh(&pool).unwrap();
    window.populate_album_rows();
    window.connect_sidebar(&nav);

    let target_idx = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .position(|album| album.folder_path == std::path::Path::new(folder))
        .expect("folder album should be rendered");
    let album_selection = window
        .imp()
        .album_selection
        .borrow()
        .as_ref()
        .cloned()
        .expect("virtual album selection should exist");
    album_selection.select_item(target_idx as u32, true);
    drain_main_context();
    assert_eq!(
        window.browsing_stack().visible_child_name().as_deref(),
        Some("album"),
        "selecting the target album should show AlbumDetailPage after idle dispatch",
    );

    window.toggle_albums_expanded();
    window.refresh_album_rows();
    assert!(
        !visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "album section should stay collapsed after refresh",
    );
    window.toggle_albums_expanded();

    assert!(
        album_selection.is_selected(target_idx as u32),
        "restored selection should point at the active album after collapsed refresh",
    );
}

fn find_overlay_split_view(root: &gtk::Widget) -> Option<adw::OverlaySplitView> {
    if let Some(split_view) = root.downcast_ref::<adw::OverlaySplitView>() {
        return Some(split_view.clone());
    }

    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(split_view) = find_overlay_split_view(&widget) {
            return Some(split_view);
        }
        child = widget.next_sibling();
    }

    None
}
