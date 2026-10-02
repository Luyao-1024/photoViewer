//! Regression coverage for the album detail view and viewer push.
//!
//! Albums are opened directly from the sidebar now (see `sidebar_navigation`),
//! so this builds an `AlbumDetailPage` the same way the sidebar does — with a
//! pre-filtered media list — and checks the page's structure: template chrome,
//! header styling and the day-grouped virtual grid.
//!
//! This is a structure test, not UX evidence: its media items point at paths that
//! do not exist, so no photo can really be opened here. The album-open-and-view
//! journey lives in `tests/ux_click_flows.rs`.

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
use photo_viewer::ui::virtual_media_grid::VirtualMediaGrid;
use photo_viewer::ui::AlbumDetailPage;

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
        video_duration_secs: None,
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
        is_favorite: false,
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
        .and_downcast::<VirtualMediaGrid>()
        .expect("album detail should use VirtualMediaGrid");
    assert_eq!(grid.mode(), photo_viewer::core::section_model::GroupBy::Day);

    // Stopping here is deliberate. Emitting `activate` on the GridView used to be
    // this file's way of "opening" a photo, but that signal bypasses the hit test
    // a real click runs and the fixture's media files do not exist on disk, so it
    // proved nothing a user could rely on. Opening an album photo by aiming at the
    // tile that paints it is owned by
    // `tests/ux_click_flows.rs::journey_select_copy_to_album_then_open_it`, which
    // also asserts the viewer receives a working pool and loader.
    let _ = nav;
}
