//! Regression coverage for the tree-shaped sidebar navigation.
//!
//! The sidebar lists albums directly under a collapsible "Albums" group header;
//! selecting an album row pushes its `AlbumDetailPage` immediately (there is no
//! intermediate album-grid page anymore). GTK widgets must be created and
//! manipulated on the same thread, so keep these checks in one function.

use std::sync::Arc;

use gio::prelude::ListModelExt;
use gtk4 as gtk;
use gtk4::prelude::ObjectExt;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::core::i18n::tr;
use photo_viewer::ui::{MainWindow, PhotosPage};

/// The `visible` *property flag* — i.e. what `set_visible` controls — rather
/// than `is_visible()`, which also walks the ancestor chain and is always
/// `false` in a headless test that never shows the window. Reading the flag
/// lets us assert the collapse/expand toggle actually flips row visibility.
fn visible_flag(w: &gtk::Widget) -> bool {
    w.property::<bool>("visible")
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
        assert!(
            list_classes.iter().any(|c| c == "glass-sidebar"),
            "sidebar list should carry glass-sidebar, got {list_classes:?}",
        );
        assert!(
            list_classes.iter().any(|c| c == "glass-base"),
            "sidebar list should carry the shared glass-base material, got {list_classes:?}",
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

    // Trash sits right before Settings (last two rows). Selecting it pushes the
    // Trash page on top of the Photos root.
    let n_items = sidebar.observe_children().n_items() as i32;
    let trash_row = sidebar.row_at_index(n_items - 2).expect("Trash row exists");
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
