//! Regression coverage for sidebar-driven top-level navigation.
//!
//! GTK widgets must be created and manipulated on the same thread, so keep
//! these checks in one integration test function.

use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::ui::{MainWindow, PhotosPage};

#[test]
fn sidebar_photos_selection_returns_to_photos_root() {
    gtk::init().expect("GTK init failed");

    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.Test")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));

    let nav = window.nav_view();
    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let photos = PhotosPage::new(media_list, loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    nav.push(&photos);

    window.set_resources(pool, loader);
    window.connect_sidebar(&nav);

    let sidebar = window.imp().sidebar_list.get();
    let albums_row = sidebar.row_at_index(1).expect("Albums row exists");
    sidebar.select_row(Some(&albums_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some("Albums"),
        "selecting Albums should show the Albums page"
    );

    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    sidebar.select_row(Some(&photos_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some("Photos"),
        "selecting Photos after Albums should return to the Photos root page"
    );
}
