//! Regression coverage for the tree-shaped sidebar navigation.
//!
//! The sidebar lists albums directly under a collapsible "Albums" group header;
//! selecting an album row pushes its `AlbumDetailPage` immediately (there is no
//! intermediate album-grid page anymore). The album rows live in a dedicated
//! bounded scroll region so the top-level Photos / Albums / Trash rows stay
//! stable even with many albums.

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

fn direct_children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut children = Vec::new();
    let mut child = widget.first_child();
    while let Some(current) = child {
        children.push(current.clone());
        child = current.next_sibling();
    }
    children
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

#[test]
fn sidebar_navigation_suite() {
    gtk::init().expect("GTK init failed");

    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.Test")
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
            "sidebar surface should include top list, album-trash wrapper, spacer, and footer",
        );
        assert_eq!(
            children[0],
            top_sidebar.clone().upcast::<gtk::Widget>(),
            "sidebar surface child 0 should be the top navigation list",
        );
        // The wrapper Box groups album_scroll + selection bar + trash_list.
        let wrapper = &children[1];
        let wrapper_children = direct_children(wrapper);
        assert!(
            wrapper_children.len() >= 3,
            "wrapper should contain album scroll, selection bar, and trash list",
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
            trash_list.clone().upcast::<gtk::Widget>(),
            "wrapper child 2 should be the Trash list",
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
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));

    let nav = window.nav_view();
    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    nav.push(&photos);

    window.set_resources(pool, loader, media_list);
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

    // Album rows are nested under the header. Even with an empty DB the three
    // virtual albums (favorites / images / videos) are present.
    {
        let album_rows = window.imp().album_rows.borrow();
        assert!(
            album_rows.len() >= 3,
            "sidebar should list the virtual albums, got {}",
            album_rows.len()
        );
        for row in album_rows.iter() {
            assert!(
                visible_flag(row.upcast_ref()),
                "album rows start expanded/visible"
            );
            let classes: Vec<String> = row.css_classes().iter().map(|s| s.to_string()).collect();
            assert!(
                classes.iter().any(|c| c == "glass-sidebar-subrow"),
                "album row should carry glass-sidebar-subrow, got {classes:?}",
            );
        }
    }
    let first_album_row = window.imp().album_rows.borrow()[0].clone();

    // Selecting an album row pushes its AlbumDetailPage directly.
    window
        .imp()
        .album_list
        .get()
        .select_row(Some(&first_album_row));
    assert!(
        nav.visible_page()
            .and_downcast::<photo_viewer::ui::AlbumDetailPage>()
            .is_some(),
        "selecting an album row should push AlbumDetailPage directly",
    );

    // Selecting Photos returns to the root Photos page.
    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    sidebar.select_row(Some(&photos_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(tr("page.photos.title").as_str()),
        "selecting Photos should return to the Photos root page",
    );

    // Trash is in its own stable bottom nav list. Selecting it pushes the Trash
    // page on top of the Photos root.
    let trash_row = trash_list.row_at_index(0).expect("Trash row exists");
    trash_list.select_row(Some(&trash_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(tr("page.trash.title").as_str()),
        "selecting Trash should push the Trash page",
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

    assert_album_sidebar_scroll_region_contains_all_albums();
    assert_collapsed_album_refresh_restores_active_selection_after_expand();
}

fn assert_album_sidebar_scroll_region_contains_all_albums() {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.TestScrollableAlbums")
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
    nav.push(&photos);

    window.set_resources(pool.clone(), loader, media_list);

    for i in 0..25 {
        let folder = format!("/tmp/album-{i:02}");
        let uri = format!("file://{folder}/cover.jpg");
        let path = format!("{folder}/cover.jpg");
        db::insert_media_item(&pool, &make_item(&uri, &path, &folder)).unwrap();
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
        window.imp().album_rows.borrow().len(),
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
        .application_id("org.gnome.PhotoViewer.TestCollapsedAlbumRefresh")
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
    nav.push(&photos);

    window.set_resources(pool.clone(), loader, media_list);

    let folder = "/tmp/album-active";
    db::insert_media_item(
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

    let album_list = window.imp().album_list.get();
    let target_idx = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .position(|album| album.folder_path == std::path::Path::new(folder))
        .expect("folder album should be rendered");
    let row = album_list
        .row_at_index(target_idx as i32)
        .expect("target album row should exist");
    album_list.select_row(Some(&row));
    assert!(
        nav.visible_page()
            .and_downcast::<photo_viewer::ui::AlbumDetailPage>()
            .is_some(),
        "selecting the target album should open AlbumDetailPage",
    );

    window.toggle_albums_expanded();
    window.refresh_album_rows();
    assert!(
        !visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "album section should stay collapsed after refresh",
    );
    window.toggle_albums_expanded();

    let selected = album_list
        .selected_row()
        .expect("expanded list should restore active album selection");
    assert_eq!(
        selected.index(),
        target_idx as i32,
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
