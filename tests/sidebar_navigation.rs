//! Regression coverage for the tree-shaped sidebar navigation.
//!
//! The sidebar lists albums directly under a collapsible "Albums" group header;
//! selecting an album row pushes its `AlbumDetailPage` immediately (there is no
//! intermediate album-grid page anymore). If there are more than 15 albums, a
//! dedicated "more" row opens an `AlbumBrowserPage` overlay list.

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
use photo_viewer::ui::{AlbumBrowserPage, MainWindow, PhotosPage};

/// The `visible` *property flag* — i.e. what `set_visible` controls — rather
/// than `is_visible()`, which also walks the ancestor chain and is always
/// `false` in a headless test that never shows the window. Reading the flag
/// lets us assert the collapse/expand toggle actually flips row visibility.
fn visible_flag(w: &gtk::Widget) -> bool {
    w.property::<bool>("visible")
}

fn item_label_text(widget: &gtk::Widget) -> Option<String> {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        return Some(label.label().to_string());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(text) = item_label_text(&current) {
            return Some(text);
        }
        child = current.next_sibling();
    }
    None
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

        let sidebar = window.imp().sidebar_list.get();
        let list_classes: Vec<String> = sidebar
            .css_classes()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let sidebar_bg_classes: Vec<String> = sidebar
            .parent()
            .map(|p| p.css_classes().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        assert!(
            list_classes.iter().any(|c| c == "glass-sidebar"),
            "sidebar list should carry glass-sidebar, got {list_classes:?}",
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

        let n_items = sidebar.observe_children().n_items();
        assert!(n_items > 0, "sidebar should have rows");
        for idx in 0..n_items {
            let row = sidebar
                .row_at_index(idx as i32)
                .expect("row exists in sidebar");
            let classes: Vec<String> = row.css_classes().iter().map(|s| s.to_string()).collect();
            assert!(
                classes.iter().any(|c| c == "glass-sidebar-row"),
                "row {idx} should carry glass-sidebar-row, got {classes:?}",
            );
        }
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
    sidebar.select_row(Some(&first_album_row));
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

    // Trash is the last list row. Selecting it pushes the Trash page on top of
    // the Photos root.
    let n_items = sidebar.observe_children().n_items() as i32;
    let trash_row = sidebar.row_at_index(n_items - 1).expect("Trash row exists");
    sidebar.select_row(Some(&trash_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(tr("page.trash.title").as_str()),
        "selecting Trash should push the Trash page",
    );

    // Collapse toggle hides every album row; expanding brings them back.
    window.toggle_albums_expanded();
    {
        let album_rows = window.imp().album_rows.borrow();
        assert!(
            !album_rows.is_empty(),
            "album rows should still exist while collapsed"
        );
        for row in album_rows.iter() {
            assert!(
                !visible_flag(row.upcast_ref()),
                "album rows should hide when collapsed"
            );
        }
    }
    window.toggle_albums_expanded();
    assert!(
        visible_flag(window.imp().album_rows.borrow()[0].upcast_ref()),
        "album rows should reappear when expanded",
    );

    assert_more_albums_row_opens_album_browser_page();
}

fn assert_more_albums_row_opens_album_browser_page() {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.TestMoreAlbums")
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

    let sidebar = window.imp().sidebar_list.get();
    let mut more_row_index = None;
    for (idx, target) in window.imp().targets.borrow().iter().enumerate() {
        if let photo_viewer::ui::window::SidebarTarget::AllAlbums = target {
            more_row_index = Some(idx as i32);
            break;
        }
    }
    let more_row_index = more_row_index.expect("sidebar should contain AllAlbums target");
    let more_row = sidebar
        .row_at_index(more_row_index)
        .expect("more row should exist in sidebar");

    let label = item_label_text(more_row.upcast_ref()).unwrap_or_default();
    assert_eq!(
        label,
        tr("sidebar.albums_more"),
        "more row label should use sidebar.albums_more",
    );
    assert!(
        visible_flag(more_row.upcast_ref()),
        "more row should be visible when albums exceed MAX_VISIBLE_ALBUMS_IN_SIDEBAR",
    );
    const MAX_VISIBLE_ALBUMS_IN_SIDEBAR: usize = 15;
    assert_eq!(
        window.imp().album_rows.borrow().len(),
        MAX_VISIBLE_ALBUMS_IN_SIDEBAR,
        "sidebar should only keep the first 15 albums as dedicated rows",
    );
    let visible_album_rows = window
        .imp()
        .album_rows
        .borrow()
        .iter()
        .filter(|row| visible_flag(row.upcast_ref()))
        .count();
    assert_eq!(
        visible_album_rows, MAX_VISIBLE_ALBUMS_IN_SIDEBAR,
        "exactly 15 album rows should be visible in sidebar when expanded",
    );

    sidebar.select_row(Some(&more_row));
    let page = nav
        .visible_page()
        .and_then(|page| page.downcast::<AlbumBrowserPage>().ok())
        .expect("selecting more row should push AlbumBrowserPage");
    assert!(
        page.album_count() > MAX_VISIBLE_ALBUMS_IN_SIDEBAR,
        "album browser page should show all albums, not only the visible prefix",
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
