use super::super::test_support::*;
use super::super::*;
use super::*;

use std::sync::Arc;

#[gtk::test]
fn album_grid_uses_progressive_first_render_seed() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let seed = crate::core::runtime_config::DEFAULT_STARTUP_RENDER_SEED;
    for id in 1..=(seed + 12) {
        media_list.append(&glib::BoxedAnyObject::new(sample_item(
            id as i64,
            &format!("album-{id}.png"),
        )));
    }

    let grid = MediaGrid::new_for_album_with_context_menu(
        media_list,
        GroupBy::Day,
        loader,
        noop_callbacks(),
    );

    assert_eq!(
        tile_count(&grid),
        seed as u32,
        "album grids should use the same progressive first-render seed as the Photos grid"
    );
}

#[gtk::test]
fn inactive_grid_defers_initial_tile_build_until_activated() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

    let grid = MediaGrid::new_with_initial_active(
        media_list,
        GroupBy::Month,
        loader,
        noop_callbacks(),
        false,
        false,
    );

    assert_eq!(
        tile_count(&grid),
        0,
        "inactive grids should not build hidden FlowBox tiles at startup"
    );

    grid.set_active(true);

    assert_eq!(
        tile_count(&grid),
        2,
        "activating a dirty grid should build tiles from the current model"
    );
}

#[gtk::test]
fn inactive_full_library_grid_uses_progressive_seed_on_first_activation() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let seed = runtime_config::startup_render_seed();
    let expected_seed = seed as u32;
    let source_len = seed + 12;
    for id in 1..=source_len {
        media_list.append(&glib::BoxedAnyObject::new(sample_item(
            id as i64,
            &format!("lazy-full-library-{id}.png"),
        )));
    }

    let grid = MediaGrid::new_with_initial_active(
        media_list,
        GroupBy::Month,
        loader,
        noop_callbacks(),
        false,
        false,
    );

    assert_eq!(
        tile_count(&grid),
        0,
        "inactive full-library grids should not build hidden tiles at startup"
    );

    grid.set_active(true);

    assert_eq!(
        tile_count(&grid),
        expected_seed,
        "first activation should render only the progressive seed, not the full model"
    );
}

#[gtk::test]
fn active_empty_day_grid_rebuilds_when_first_scan_items_arrive() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );

    assert_eq!(tile_count(&grid), 0);
    assert!(
        grid.imp().stats_label.borrow().is_none(),
        "empty Day grid should not render a stats label before media exists"
    );

    let item = sample_item(1, "first-scan.png");
    insert_sample_item(&pool, &item);
    media_list.append(&glib::BoxedAnyObject::new(item));

    assert_eq!(
        tile_count(&grid),
        1,
        "active Day grid must render the first media items delivered by startup scan"
    );
    assert!(
        grid.imp().stats_label.borrow().is_none(),
        "stats should be filled by background metadata, not the first scan-triggered rebuild"
    );
}

#[gtk::test]
fn day_grid_defers_stats_until_background_metadata_refresh() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let one = sample_item(1, "one.png");
    let two = sample_item(2, "two.png");
    let generated_id = insert_sample_item(&pool, &one);
    insert_sample_item(&pool, &two);
    crate::core::db::mark_thumbnails_generated(&pool, &[generated_id]).unwrap();
    media_list.append(&glib::BoxedAnyObject::new(one));
    media_list.append(&glib::BoxedAnyObject::new(two));

    let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);

    assert_eq!(tile_count(&grid), 2);
    assert!(
        grid.imp().stats_label.borrow().is_none(),
        "first rebuild should not block on full-library thumbnail stats"
    );
    assert_eq!(
        grid.imp().virtual_total.get(),
        2,
        "first rebuild should use the loaded window as the temporary total"
    );
}

#[gtk::test]
fn metadata_invalidation_during_load_requests_followup_refresh() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));

    let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);
    grid.imp().library_metadata_loading.set(true);
    grid.imp().library_total_snapshot.set(Some(1));
    grid.imp().library_stats_snapshot.set(Some(LibraryStats {
        live_total: 1,
        thumbnails_generated: 0,
    }));

    grid.invalidate_library_metadata();

    assert!(
        grid.imp().library_metadata_dirty_pending.get(),
        "metadata invalidated while a DB snapshot is loading must request a follow-up refresh"
    );
    assert!(
        grid.imp().library_total_snapshot.get().is_none(),
        "stale total snapshot should be cleared immediately"
    );
    assert!(
        grid.imp().library_stats_snapshot.get().is_none(),
        "stale stats snapshot should be cleared immediately"
    );
}

#[gtk::test]
fn day_grid_stats_are_above_first_section_header() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let one = sample_item(1, "one.png");
    insert_sample_item(&pool, &one);
    media_list.append(&glib::BoxedAnyObject::new(one));

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    grid.imp().library_total_snapshot.set(Some(1));
    grid.imp().library_stats_snapshot.set(Some(LibraryStats {
        live_total: 1,
        thumbnails_generated: 0,
    }));
    grid.rebuild(media_list, GroupBy::Day);
    let content = grid.imp().content.get();
    let first_child = content
        .first_child()
        .expect("Day grid should have a first content child");

    assert!(
        first_child.has_css_class("library-stats"),
        "Day grid stats should be the first content child, above the first date header"
    );
}

#[gtk::test]
fn pending_thumbnail_stats_survive_metadata_invalidation_rebuild() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let one = sample_item(1, "one.png");
    insert_sample_item(&pool, &one);
    media_list.append(&glib::BoxedAnyObject::new(one));

    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        false,
    );
    grid.imp().library_total_snapshot.set(Some(1));
    grid.imp().library_stats_snapshot.set(Some(LibraryStats {
        live_total: 1,
        thumbnails_generated: 0,
    }));
    grid.rebuild(media_list.clone(), GroupBy::Day);
    assert!(
        grid.imp().stats_label.borrow().is_some(),
        "pending thumbnail stats should be visible before invalidation"
    );

    grid.invalidate_library_metadata();
    grid.rebuild(media_list, GroupBy::Day);

    assert!(
                grid.imp().stats_label.borrow().is_some(),
                "metadata invalidation during thumbnail generation must not temporarily remove the stats prompt"
            );
}

#[gtk::test]
fn album_grid_skips_global_library_stats() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let item = sample_item(1, "album-only.png");
    insert_sample_item(&pool, &item);
    media_list.append(&glib::BoxedAnyObject::new(item));

    let grid = MediaGrid::new_for_album(media_list, GroupBy::Day, loader, noop_callbacks());

    assert!(
        grid.imp().stats_label.borrow().is_none(),
        "album grids should not query or render full-library thumbnail stats"
    );
    assert!(
        grid.imp().stats_refresh_source.borrow().is_none(),
        "album grids should not start the full-library stats refresh timer"
    );
}

#[gtk::test]
fn completed_day_grid_stats_are_hidden() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let item = sample_item(1, "complete.png");
    let media_id = insert_sample_item(&pool, &item);
    crate::core::db::mark_thumbnails_generated(&pool, &[media_id]).unwrap();
    media_list.append(&glib::BoxedAnyObject::new(item));

    let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);
    assert!(
        grid.imp().stats_label.borrow().is_none(),
        "completed thumbnail generation should hide the Day grid stats label"
    );
    assert!(
        grid.imp().stats_refresh_source.borrow().is_none(),
        "completed thumbnail generation should not start a stats refresh timeout"
    );
}

#[test]
fn library_stats_text_clamps_generated_to_total() {
    assert_eq!(library_stats_text(20, 7), "媒体 20 项 · 缩略图 7/20");
    assert_eq!(library_stats_text(20, 99), "媒体 20 项 · 缩略图 20/20");
}

#[gtk::test]
fn current_scroll_section_key_resolves_via_injected_counts() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let grid = MediaGrid::new(media_list, GroupBy::Month, loader, noop_callbacks(), false);

    // Empty/counts-less grid degrades safely.
    assert!(grid.current_scroll_section_key().is_none());
    assert_eq!(grid.scroll_fraction(), 0.0);

    // Inject full-library metadata as the background refresh would.
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: Some(7),
            day: None,
        },
        3,
    );
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: Some(6),
            day: None,
        },
        2,
    );
    counts.insert(
        SectionKey {
            year: Some(2025),
            month: None,
            day: None,
        },
        4,
    );
    grid.imp()
        .section_count_snapshots
        .borrow_mut()
        .insert(GroupBy::Month, counts);
    grid.imp().library_total_snapshot.set(Some(9));

    // Drive the scrolled window's adjustment. upper=1000, page=200 → travel=800.
    let adj = grid.imp().scroller.get().vadjustment();
    adj.set_upper(1000.0);
    adj.set_page_size(200.0);

    let jul = SectionKey {
        year: Some(2026),
        month: Some(7),
        day: None,
    };
    let y2025 = SectionKey {
        year: Some(2025),
        month: None,
        day: None,
    };

    // value=0 → ratio 0 → offset 0 → newest section (Jul).
    adj.set_value(0.0);
    assert_eq!(grid.scroll_fraction(), 0.0);
    assert_eq!(grid.current_scroll_section_key(), Some(jul));

    // value=800 → ratio 1.0 → offset clamps to oldest (2025).
    adj.set_value(800.0);
    assert_eq!(grid.scroll_fraction(), 1.0);
    assert_eq!(grid.current_scroll_section_key(), Some(y2025));
}
