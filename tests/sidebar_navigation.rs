//! Regression coverage for sidebar-driven top-level navigation.
//!
//! GTK widgets must be created and manipulated on the same thread, so keep
//! these checks in one integration test function.

use std::sync::Arc;

use gio::prelude::ListModelExt;
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::core::i18n::tr;
use photo_viewer::ui::{MainWindow, PhotosPage};

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

    // Glass material: each sidebar row should carry the glass-sidebar-row class.
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
    window.connect_sidebar(&nav);

    let sidebar = window.imp().sidebar_list.get();
    let expected_albums = tr("page.albums.title");
    let expected_photos = tr("page.photos.title");
    let expected_trash = tr("page.trash.title");

    let albums_row = sidebar.row_at_index(1).expect("Albums row exists");
    sidebar.select_row(Some(&albums_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(expected_albums.as_str()),
        "selecting Albums should show the Albums page"
    );

    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    sidebar.select_row(Some(&photos_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(expected_photos.as_str()),
        "selecting Photos after Albums should return to the Photos root page"
    );

    let trash_row = sidebar.row_at_index(2).expect("Trash row exists");

    sidebar.select_row(Some(&albums_row));
    let albums_page = nav
        .visible_page()
        .expect("Albums should be visible after selecting Albums");
    assert_eq!(albums_page.title().as_str(), expected_albums);

    sidebar.select_row(Some(&trash_row));
    let trash_page = nav
        .visible_page()
        .expect("Trash should be visible after selecting Trash");
    assert_eq!(trash_page.title().as_str(), expected_trash);
    assert_eq!(nav.navigation_stack().n_items(), 3);
    assert_eq!(
        nav.previous_page(&trash_page)
            .map(|page| page.title())
            .as_deref(),
        Some(expected_albums.as_str()),
        "Trash should be stacked on top of Albums without revealing Photos first"
    );

    sidebar.select_row(Some(&albums_row));
    let visible = nav
        .visible_page()
        .expect("Albums should be visible after returning from Trash");
    assert_eq!(visible.title().as_str(), expected_albums);
    assert_eq!(
        visible, albums_page,
        "selecting Albums from Trash should reveal the existing Albums page"
    );
    assert_eq!(nav.navigation_stack().n_items(), 2);

    sidebar.select_row(Some(&trash_row));
    assert_eq!(nav.navigation_stack().n_items(), 3);
    sidebar.select_row(Some(&photos_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(expected_photos.as_str())
    );
    assert_eq!(
        nav.navigation_stack().n_items(),
        1,
        "selecting Photos should remove both Trash and Albums pages"
    );
}
