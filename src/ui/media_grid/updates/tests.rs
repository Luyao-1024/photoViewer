use super::super::test_support::*;
use super::super::*;
use super::*;
use chrono::{TimeZone, Utc};

use std::rc::Rc;
use std::sync::Arc;

#[gtk::test]
fn grid_rebuilds_when_backing_store_removes_item() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    assert_eq!(tile_count(&grid), 2);

    media_list.remove(0);

    assert_eq!(
        tile_count(&grid),
        1,
        "MediaGrid must drop stale thumbnails when the shared ListStore changes"
    );
}

#[gtk::test]
fn grid_removes_backing_store_item_without_replacing_section_flow() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(3, "three.png")));

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    let flow_before = first_section_flow(&grid).expect("grid should render a section flow");

    media_list.remove(1);

    assert_eq!(tile_count(&grid), 2);
    let flow_after = first_section_flow(&grid).expect("section flow should remain");
    assert!(
                flow_before == flow_after,
                "a single backing-store removal should remove the child in place instead of rebuilding the whole section flow"
            );
}

#[gtk::test]
fn grid_inserts_same_section_item_without_replacing_existing_tiles() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let cache_dir = dir.path().join("thumbs");
    let loader = Arc::new(ThumbnailLoader::new(pool, cache_dir.clone()));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let existing_one = sample_item(1, "one.png");
    let existing_two = sample_item(2, "two.png");
    media_list.append(&glib::BoxedAnyObject::new(existing_one.clone()));
    media_list.append(&glib::BoxedAnyObject::new(existing_two.clone()));

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    let flow_before = first_section_flow(&grid).expect("grid should render a section flow");
    let first_child_before = flow_child_at(&flow_before, 0).expect("first tile should be rendered");

    let inserted = sample_item(3, "inserted.png");
    crate::core::thumbnails::generate_for_tests(
        &cache_dir,
        &inserted.uri,
        ThumbnailSize::Medium,
        Some(thumbnail_request_mtime(&inserted)),
    )
    .expect("test should pre-create thumbnail cache for the inserted item");
    media_list.splice(0, 0, &[glib::BoxedAnyObject::new(inserted)]);

    assert_eq!(
        tile_count(&grid),
        3,
        "same-section pure insertion should update the visible grid immediately"
    );
    let flow_after = first_section_flow(&grid).expect("section flow should remain");
    assert!(
        flow_before == flow_after,
        "same-section pure insertion should preserve the existing section flow"
    );
    let shifted_child = flow_child_at(&flow_after, 1).expect("old first tile should shift");
    assert!(
        first_child_before == shifted_child,
        "same-section pure insertion should not recreate existing tile children"
    );
}

#[gtk::test]
fn grid_defers_uncached_incremental_insert_until_thumbnail_ready() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    let flow_before = first_section_flow(&grid).expect("grid should render a section flow");
    let first_child_before = flow_child_at(&flow_before, 0).expect("first tile should be rendered");

    let inserted = sample_item(3, "inserted.png");
    media_list.splice(0, 0, &[glib::BoxedAnyObject::new(inserted)]);

    assert_eq!(
                tile_count(&grid),
                2,
                "uncached incremental inserts should wait for thumbnail success/failure before entering the grid"
            );
    let flow_after = first_section_flow(&grid).expect("section flow should remain");
    assert!(
        flow_before == flow_after,
        "deferring the new tile should still preserve the existing section flow"
    );
    let first_child_after =
        flow_child_at(&flow_after, 0).expect("old first tile should remain first while pending");
    assert!(
                first_child_before == first_child_after,
                "pending uncached insert should not insert a gray/transparent tile before the old first child"
            );
}

#[gtk::test]
fn grid_inserts_deferred_item_after_thumbnail_failure() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader.clone(),
        noop_callbacks(),
        false,
    );
    let inserted = sample_item(3, "inserted.png");
    let inserted_uri = inserted.uri.clone();
    media_list.splice(0, 0, &[glib::BoxedAnyObject::new(inserted)]);
    assert_eq!(tile_count(&grid), 2);

    grid.insert_deferred_incremental_item(
        media_list,
        inserted_uri,
        spec_for_mode(GroupBy::Day),
        loader,
        Rc::new(|| {}),
        None,
    );

    assert_eq!(
                tile_count(&grid),
                3,
                "thumbnail failure should insert the final failure placeholder only after the request completes"
            );
    let first_tile = first_square_tile(&grid).expect("deferred item should be visible");
    assert!(
        !first_tile.has_css_class("thumb-loading"),
        "ready failure placeholder must not be inserted as a loading gray tile"
    );
    assert_eq!(first_tile.opacity(), 1.0);
}

#[gtk::test]
fn rebuild_reuses_loaded_tiles_when_a_new_section_is_added() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let existing = sample_item(1, "one.png");
    media_list.append(&glib::BoxedAnyObject::new(existing));
    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    let existing_tile = first_square_tile(&grid).expect("existing tile should render");
    existing_tile.set_paintable(Some(&gray_placeholder_texture()));

    let mut inserted = sample_item(2, "new-day.png");
    inserted.taken_at = Some(Utc.with_ymd_and_hms(2026, 6, 24, 12, 0, 0).unwrap());
    inserted.file_mtime = inserted.taken_at.unwrap();
    media_list.splice(0, 0, &[glib::BoxedAnyObject::new(inserted)]);
    grid.rebuild(media_list, GroupBy::Day);

    let tiles = square_tiles(&grid);
    assert!(
                tiles.iter().any(|tile| tile == &existing_tile),
                "full rebuild fallback should reuse already-loaded tile widgets instead of making them gray again"
            );
}

#[gtk::test]
fn progressive_render_addition_appends_without_rebuilding_existing_tiles() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    for id in 1..=4 {
        media_list.append(&glib::BoxedAnyObject::new(sample_item(
            id,
            &format!("same-day-{id}.png"),
        )));
    }

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    grid.imp().rendered_limit.set(2);
    grid.rebuild(media_list.clone(), GroupBy::Day);
    let before = square_tiles(&grid);
    assert_eq!(before.len(), 2);
    let first_tile = before[0].clone();

    assert!(
        grid.apply_progressive_render_addition(2, 2, &media_list),
        "same-section progressive fill should append instead of forcing a full rebuild"
    );

    let after = square_tiles(&grid);
    assert_eq!(after.len(), 4);
    assert_eq!(
        after[0], first_tile,
        "progressive append must preserve already-rendered tile widgets"
    );
    assert_eq!(
        grid.imp().virtual_total.get(),
        4,
        "rendering more of the same model must not inflate full-library totals"
    );
}
