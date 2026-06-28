//! Integration tests for the photo grid canvas — Task 4 of the liquid-glass
//! UX adaptation.
//!
//! Verifies that a real `MediaGrid` actually builds inner `GtkFlowBox`es with
//! the 8 px column / row spacing introduced for the glass-style grid canvas
//! (previously 2 px). We construct `MediaGrid` against a seeded `ListStore`
//! (one section worth of items) so `rebuild` emits at least one FlowBox,
//! then dig into `imp().content` to find the first `FlowBox`.
//!
//! GTK is single-threaded, so all checks live in a single `#[test]`
//! function. See `tests/ui_mode_selector.rs` for the same pattern.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use chrono::TimeZone;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::section_model::GroupBy;
use photo_viewer::core::thumbnails::ThumbnailLoader;
use photo_viewer::ui::media_grid::{FavoriteMenuState, MediaGrid};

/// Build a minimal `MediaItem` good enough to pass through grouping.
fn sample_item(id: i64, name: &str) -> MediaItem {
    let dt = chrono::Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
    MediaItem {
        id,
        uri: format!("file:///tmp/{name}"),
        path: PathBuf::from(format!("/tmp/{name}")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/png".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 100,
        blake3_hash: format!("hash-{id}"),
        trashed_at: None,
    }
}

#[test]
fn media_grid_flowbox_uses_8px_gaps() {
    gtk::init().expect("GTK init failed");

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    // Seed two items so `rebuild` actually emits a section header + FlowBox.
    // An empty store would still emit the section header but with no flow box,
    // making the assertion below meaningless.
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

    let grid = MediaGrid::new(
        media_list,
        GroupBy::Year,
        loader,
        Rc::new(|_| {}),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_, _| {}),
        Rc::new(|_| FavoriteMenuState::default()),
        false,
    );

    assert!(
        !grid
            .imp()
            .scroller
            .get()
            .has_css_class("content-safe-bottom"),
        "MediaGrid must not reserve a fixed bottom padding behind the floating mode selector"
    );

    // `rebuild` appends a `GtkLabel` header followed by a `GtkFlowBox` per
    // section, stacked inside `imp().content` (a vertical `GtkBox`). Find
    // the first FlowBox among the children and assert its spacing.
    let content = grid.imp().content.get();
    let mut flow: Option<gtk::FlowBox> = None;
    let mut child = content.first_child();
    while let Some(c) = child {
        if let Some(f) = c.downcast_ref::<gtk::FlowBox>() {
            flow = Some(f.clone());
            break;
        }
        child = c.next_sibling();
    }
    let flow = flow.expect("MediaGrid should have built at least one FlowBox section");
    assert_eq!(
        flow.column_spacing(),
        8,
        "MediaGrid's inner FlowBox must use 8 px column spacing"
    );
    assert_eq!(
        flow.row_spacing(),
        8,
        "MediaGrid's inner FlowBox must use 8 px row spacing"
    );
}
