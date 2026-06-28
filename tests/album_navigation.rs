//! Regression coverage for the album detail view and viewer push.
//!
//! Albums are opened directly from the sidebar now (see `sidebar_navigation`),
//! so this builds an `AlbumDetailPage` the same way the sidebar does — with a
//! pre-filtered media list — and checks the day-grouped grid + viewer wiring.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::TimeZone;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::core::albums::Album;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::thumbnails::ThumbnailLoader;
use photo_viewer::ui::{AlbumDetailPage, MediaGrid, ViewerPage};

fn item(id: i64, folder: &str, file: &str) -> MediaItem {
    MediaItem {
        id,
        uri: format!("file://{folder}/{file}"),
        path: PathBuf::from(folder).join(file),
        folder_path: PathBuf::from(folder),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        taken_at: Some(
            chrono::Utc
                .with_ymd_and_hms(2025, 3, id as u32, 12, 0, 0)
                .unwrap(),
        ),
        file_mtime: chrono::Utc
            .with_ymd_and_hms(2025, 3, id as u32, 12, 0, 0)
            .unwrap(),
        file_size: 100,
        blake3_hash: format!("hash-{id}"),
        trashed_at: None,
    }
}

fn boxed(items: &[MediaItem]) -> gtk::gio::ListStore {
    let store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for it in items {
        store.append(&glib::BoxedAnyObject::new(it.clone()));
    }
    store
}

#[test]
fn album_detail_pushes_day_grouped_grid_and_viewer() {
    gtk::init().expect("GTK init failed");

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let nav = adw::NavigationView::new();

    let album = Album {
        folder_path: PathBuf::from("/tmp/Camera"),
        name: "/tmp/Camera".into(),
        cover_uri: None,
        photo_count: 2,
        last_modified: chrono::Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap(),
        is_virtual: false,
    };

    // The sidebar builds this pre-filtered list via `filtered_items_for_album`;
    // here we construct it directly to keep that helper crate-private.
    let filtered = boxed(&[
        item(1, "/tmp/Camera", "one.jpg"),
        item(2, "/tmp/Camera", "two.jpg"),
    ]);
    let master = boxed(&[
        item(1, "/tmp/Camera", "one.jpg"),
        item(2, "/tmp/Camera", "two.jpg"),
        item(3, "/tmp/Other", "three.jpg"),
    ]);

    let page = AlbumDetailPage::new(album, filtered, master, pool, loader);
    page.set_nav_target(&nav);
    nav.push(&page);

    assert_eq!(
        page.title().as_str(),
        "Camera",
        "detail page title should identify the current album",
    );
    let detail_header_classes: Vec<String> = page
        .imp()
        .header_bar
        .get()
        .css_classes()
        .iter()
        .map(|class| class.to_string())
        .collect();
    assert!(
        detail_header_classes
            .iter()
            .any(|class| class == "glass-header"),
        "AlbumDetailPage header should carry glass-header, got {detail_header_classes:?}",
    );

    let grid = page
        .imp()
        .content_box
        .get()
        .first_child()
        .and_downcast::<MediaGrid>()
        .expect("album detail should reuse MediaGrid");
    assert_eq!(grid.mode(), photo_viewer::core::section_model::GroupBy::Day);

    let flow = grid
        .imp()
        .content
        .get()
        .last_child()
        .and_downcast::<gtk::FlowBox>()
        .expect("album detail should contain a thumbnail flow box");
    let photo = flow.child_at_index(0).expect("photo tile exists");
    flow.emit_by_name::<()>("child-activated", &[&photo]);

    let viewer = nav
        .visible_page()
        .and_downcast::<ViewerPage>()
        .expect("activating an album photo should push ViewerPage");
    assert!(
        viewer.imp().pool.borrow().is_some(),
        "album detail viewer must receive DbPool so Delete works"
    );
}
