//! Regression coverage for album tile activation and the album detail view.

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
use photo_viewer::ui::{AlbumDetailPage, AlbumsPage, MediaGrid, ViewerPage};

fn item(id: i64, folder: &str, file: &str) -> MediaItem {
    MediaItem {
        id,
        uri: format!("file://{folder}/{file}"),
        path: PathBuf::from(folder).join(file),
        folder_path: PathBuf::from(folder),
        mime_type: "image/jpeg".into(),
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

fn album(folder: &str, count: i64) -> Album {
    Album {
        folder_path: PathBuf::from(folder),
        name: folder.into(),
        cover_uri: None,
        photo_count: count,
        last_modified: chrono::Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap(),
        is_virtual: false,
    }
}

#[test]
fn album_tile_pushes_day_grouped_detail_page() {
    gtk::init().expect("GTK init failed");

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let nav = adw::NavigationView::new();

    let albums = vec![album("/tmp/Camera", 2)];
    let media = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in [
        item(1, "/tmp/Camera", "one.jpg"),
        item(2, "/tmp/Camera", "two.jpg"),
        item(3, "/tmp/Other", "three.jpg"),
    ] {
        media.append(&glib::BoxedAnyObject::new(item));
    }
    let page = AlbumsPage::new(albums, loader.clone());
    let album_header_classes: Vec<String> = page
        .imp()
        .header_bar
        .get()
        .css_classes()
        .iter()
        .map(|class| class.to_string())
        .collect();
    assert!(
        album_header_classes
            .iter()
            .any(|class| class == "glass-header"),
        "AlbumsPage header should carry glass-header, got {album_header_classes:?}",
    );

    page.set_nav_target(&nav, media, pool);
    nav.push(&page);

    let child = page
        .imp()
        .flow_box
        .get()
        .child_at_index(0)
        .expect("album tile exists");
    page.imp()
        .flow_box
        .get()
        .emit_by_name::<()>("child-activated", &[&child]);

    let detail = nav
        .visible_page()
        .and_downcast::<AlbumDetailPage>()
        .expect("activating an album should push AlbumDetailPage");
    assert_eq!(
        detail.title().as_str(),
        "Camera",
        "detail page title should identify the current album"
    );
    let detail_header_classes: Vec<String> = detail
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

    let grid = detail.imp().content_box.get().first_child();
    let grid = grid
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
