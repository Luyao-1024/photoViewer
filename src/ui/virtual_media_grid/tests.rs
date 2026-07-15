use super::*;
use crate::ui::media_grid::test_support::{insert_sample_item, noop_callbacks, sample_item};
use crate::ui::square_tile::SquareTile;

#[gtk::test]
fn active_grid_seeds_the_initial_window_without_waiting_for_metadata() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.jpg")));
    list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.jpg")));

    let grid = VirtualMediaGrid::new(list, GroupBy::Day, loader, noop_callbacks(), true);

    assert_eq!(grid.model().ready_item_count(), 2);
    assert_eq!(grid.mode(), GroupBy::Day);
    let viewer_seed = grid
        .viewer_seed_for(MediaId::from(2))
        .expect("a warm virtual item should seed the viewer without a full GTK list");
    assert_eq!(
        crate::ui::media_list::media_item_at(&viewer_seed, 0).map(|item| item.id),
        Some(2)
    );
}

#[gtk::test]
fn selection_updates_realized_tile_without_replacing_the_list_model() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.jpg")));
    let grid = VirtualMediaGrid::new(list, GroupBy::Day, loader, noop_callbacks(), true);
    let media_id = MediaId::from(1);

    let tile = SquareTile::new();
    grid.register_factory_cell(factory::FactoryCell {
        tile: tile.clone(),
        binding: std::rc::Rc::new(std::cell::RefCell::new(Some(TileBinding::new(
            grid.layout_generation(),
            0,
            media_id,
            None,
        )))),
    });

    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let captured_changes = changes.clone();
    grid.model()
        .connect_items_changed(move |_, position, removed, added| {
            captured_changes
                .borrow_mut()
                .push((position, removed, added));
        });

    grid.set_multi_select_mode(true);
    grid.toggle_selection(media_id);
    assert!(tile.has_css_class("media-selected"));
    assert!(changes.borrow().is_empty());

    grid.toggle_selection(media_id);
    assert!(!tile.has_css_class("media-selected"));
    assert!(changes.borrow().is_empty());
}

#[gtk::test]
fn query_backed_grid_uses_album_counts_instead_of_the_live_library() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));

    let album_path = std::path::PathBuf::from("/tmp/album");
    let mut album_item = sample_item(1, "album/one.jpg");
    album_item.folder_path = album_path.clone();
    album_item.path = album_path.join("one.jpg");
    album_item.uri = "file:///tmp/album/one.jpg".into();
    album_item.id = insert_sample_item(&pool, &album_item);

    let other_item = sample_item(2, "elsewhere.jpg");
    insert_sample_item(&pool, &other_item);

    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    list.append(&glib::BoxedAnyObject::new(album_item));
    let grid = VirtualMediaGrid::new_for_query(
        list,
        MediaQuery::AlbumFolder(album_path),
        GroupBy::Day,
        loader,
        noop_callbacks(),
        true,
    );

    let context = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !grid.imp().metadata_ready.get() && std::time::Instant::now() < deadline {
        context.iteration(true);
    }

    assert!(
        grid.imp().metadata_ready.get(),
        "album metadata should load"
    );
    assert_eq!(grid.imp().live_total.get(), 1);
    assert_eq!(grid.model().layout().media_count(), 1);
}

#[test]
fn authoritative_counts_keep_layout_and_range_total_in_sync_during_a_db_race() {
    let newest = SectionKey {
        year: Some(2026),
        month: Some(7),
        day: Some(13),
    };
    let mut newer_sections = HashMap::new();
    newer_sections.insert(newest.clone(), 3);
    let (total, counts) = normalise_authoritative_counts(newer_sections, 2);
    assert_eq!(total, 3);
    assert_eq!(counts.get(&newest), Some(&3));

    let mut newer_total = HashMap::new();
    newer_total.insert(newest, 2);
    let (total, counts) = normalise_authoritative_counts(newer_total, 3);
    assert_eq!(total, 3);
    assert_eq!(
        counts.get(&SectionKey {
            year: None,
            month: None,
            day: None,
        }),
        Some(&1)
    );
}

#[test]
fn metadata_layout_replacement_preserves_the_current_media_anchor() {
    let newest = SectionKey {
        year: Some(2026),
        month: Some(7),
        day: Some(13),
    };
    let older = SectionKey {
        year: Some(2026),
        month: Some(7),
        day: Some(12),
    };
    let counts = HashMap::from([(newest, 5), (older, 12)]);
    let previous = VirtualGridLayoutIndex::new(&counts, 4);
    let replacement = VirtualGridLayoutIndex::new(&counts, 3);

    // Slot 8 is the first item in the second section of the four-column
    // layout. That section starts at slot 6 after the replacement layout's
    // different row padding.
    // It must remain anchored to the same logical media item after a metadata-driven model
    // replacement, even though its physical slot changes with the layout.
    assert_eq!(
        restored_slot_after_layout_replacement(&previous, 8, &replacement),
        Some(6)
    );
}

#[gtk::test]
fn layout_replacement_restores_scroll_after_the_next_frame() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    for id in 1..=40 {
        list.append(&glib::BoxedAnyObject::new(sample_item(
            id,
            &format!("{id}.jpg"),
        )));
    }
    let grid = VirtualMediaGrid::new(list.clone(), GroupBy::Day, loader, noop_callbacks(), false);
    grid.seed_provisional_items();

    let window = gtk::Window::builder()
        .default_width(900)
        .default_height(500)
        .child(&grid)
        .build();
    window.present();

    let context = glib::MainContext::default();
    let adjustment = grid.imp().scroller.get().vadjustment();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while adjustment.upper() <= adjustment.page_size() && std::time::Instant::now() < deadline {
        context.iteration(true);
    }
    assert!(
        adjustment.upper() > adjustment.page_size(),
        "test grid must have a scrollable allocation"
    );

    // A structural ListModel replacement can temporarily reset GTK's
    // adjustment, then the frame-tick restore must bring the viewport back.
    let layout = grid.model().layout();
    grid.replace_layout_with_initial_items(layout, media_items_from_list(&list));
    grid.restore_top_slot(20);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while adjustment.value() <= 0.0 && std::time::Instant::now() < deadline {
        context.iteration(true);
    }
    assert!(
        adjustment.value() > 0.0,
        "scroll position should be restored after the replacement allocation"
    );

    let focus_scroll_value = adjustment.value();
    assert!(
        grid.focus_visible_tile(),
        "a visible GridView item should accept focus before a toolbar hides"
    );
    assert!(
        (adjustment.value() - focus_scroll_value).abs() <= 0.5,
        "returning focus to a visible GridView item must not move the viewport"
    );

    // A context-menu layer can steal focus, then GTK performs its fallback to
    // the first GridView item one frame later. The bounded frame watcher must
    // restore the pixel offset captured before that delayed reset.
    let captured_scroll_value = adjustment.value();
    grid.restore_scroll_after_transient_reset(captured_scroll_value, 8);
    adjustment.set_value(0.0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while adjustment.value() <= 0.0 && std::time::Instant::now() < deadline {
        context.iteration(true);
    }
    assert!(
        adjustment.value() > 0.0,
        "context-menu focus fallback should restore the captured scroll offset"
    );

    // Favorite mutations register their captured offset before the database
    // event arrives. The adjustment signal, rather than a guessed delay,
    // triggers the same bounded recovery when GTK resets to zero.
    adjustment.set_value(captured_scroll_value);
    grid.arm_favorite_scroll_restore();
    adjustment.set_value(0.0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while adjustment.value() <= 0.0 && std::time::Instant::now() < deadline {
        context.iteration(true);
    }
    assert!(
        adjustment.value() > 0.0,
        "favorite mutation reset should restore the captured scroll offset"
    );

    // The full authoritative replacement can later scroll a remembered
    // GridView focus position into view without passing through zero. Keep
    // the exact captured offset for every favorite mutation, not only ones
    // that happened to reset to zero first.
    let layout = grid.model().layout();
    grid.replace_layout_with_initial_items(layout, media_items_from_list(&list));
    let (favorite_guard_generation, favorite_scroll_value) = grid
        .favorite_scroll_restore_for_layout()
        .expect("favorite mutation should retain its pre-mutation scroll value");
    grid.hold_favorite_scroll_after_layout(favorite_guard_generation, favorite_scroll_value, 8);
    adjustment.set_value(
        (favorite_scroll_value + 278.0).min((adjustment.upper() - adjustment.page_size()).max(0.0)),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while (adjustment.value() - favorite_scroll_value).abs() > 0.5
        && std::time::Instant::now() < deadline
    {
        context.iteration(true);
    }
    assert!(
        (adjustment.value() - favorite_scroll_value).abs() <= 0.5,
        "favorite layout replacement should preserve the captured scroll offset"
    );
    window.close();
}

#[gtk::test]
fn inactive_grid_defers_its_initial_window_until_activated() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.jpg")));

    let grid = VirtualMediaGrid::new(list, GroupBy::Month, loader, noop_callbacks(), false);

    assert_eq!(grid.model().ready_item_count(), 0);
    grid.set_active(true);
    assert_eq!(grid.model().ready_item_count(), 1);
}

#[gtk::test]
fn realized_day_cells_fill_grid_columns_and_match_dynamic_scroll_metrics() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("thumbs"),
    ));
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    for id in 1..=6 {
        let mut item = sample_item(id, &format!("{id}.jpg"));
        item.id = insert_sample_item(&pool, &item);
        list.append(&glib::BoxedAnyObject::new(item));
    }

    let grid = VirtualMediaGrid::new(list, GroupBy::Day, loader, noop_callbacks(), true);
    let window = gtk::Window::builder()
        .default_width(900)
        .default_height(900)
        .child(&grid)
        .build();
    window.present();

    // Widget realization can be delayed while the complete GTK test suite is
    // creating other windows. Wait for the *observed* GridView state rather
    // than treating one arbitrary frame delay as a layout contract.
    let context = glib::MainContext::default();
    let deadline_reached = std::rc::Rc::new(std::cell::Cell::new(false));
    let deadline_callback = deadline_reached.clone();
    glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
        deadline_callback.set(true);
    });
    let has_expected_realization = || {
        let metrics = grid.viewport_metrics();
        if metrics.columns() != 4
            || metrics.tile_size() != 270
            || !grid.imp().factory_cells.borrow().iter().any(|cell| {
                cell.tile.width() == metrics.tile_size()
                    && cell.tile.height() == metrics.tile_size()
            })
        {
            return false;
        }
        let mut row_starts = grid
            .imp()
            .factory_cells
            .borrow()
            .iter()
            .filter_map(|cell| cell.tile.compute_bounds(&grid))
            .map(|bounds| bounds.y().round() as i32)
            .collect::<Vec<_>>();
        row_starts.sort_unstable();
        row_starts.dedup();
        row_starts.len() >= 2
    };
    while !deadline_reached.get() && !has_expected_realization() {
        context.iteration(true);
    }
    let observed_metrics = grid.viewport_metrics();
    let observed_tiles = grid
        .imp()
        .factory_cells
        .borrow()
        .iter()
        .map(|cell| (cell.tile.width(), cell.tile.height()))
        .collect::<Vec<_>>();
    assert!(
        !deadline_reached.get(),
        "the realized GridView did not receive its 900px viewport layout: metrics={observed_metrics:?}, grid_width={}, scroller_width={}, page_size={}, tiles={observed_tiles:?}",
        grid.imp().grid.get().width(),
        grid.imp().scroller.get().width(),
        grid.imp().scroller.get().hadjustment().page_size(),
    );

    let metrics = grid.viewport_metrics();
    assert_eq!(metrics.columns(), 4);
    assert_eq!(metrics.tile_size(), 270);
    assert_eq!(metrics.row_extent(), 278);
    let first_tile = grid
        .imp()
        .factory_cells
        .borrow()
        .iter()
        .find(|cell| cell.tile.width() > 0)
        .map(|cell| cell.tile.clone())
        .expect("the realized GridView should create at least one reusable tile");
    assert_eq!(grid.viewport_metrics().columns(), 4);
    assert_eq!(grid.imp().grid.get().min_columns(), 4);
    assert_eq!(grid.imp().grid.get().max_columns(), 4);
    assert!(
        grid.imp()
            .factory_cells
            .borrow()
            .iter()
            .all(|cell| cell.tile.allows_width_shrink()),
        "GridView factory tiles must not turn the preferred target into a hard measure minimum"
    );
    assert!(
        grid.focus_visible_tile(),
        "a realized GridView item must accept focus before transient toolbar controls hide"
    );
    assert_eq!(first_tile.width(), metrics.tile_size());
    assert_eq!(first_tile.height(), metrics.tile_size());
    let mut row_starts = grid
        .imp()
        .factory_cells
        .borrow()
        .iter()
        .filter_map(|cell| cell.tile.compute_bounds(&grid))
        .map(|bounds| bounds.y().round() as i32)
        .collect::<Vec<_>>();
    row_starts.sort_unstable();
    row_starts.dedup();
    assert!(row_starts.len() >= 2);
    assert_eq!(
        row_starts[1] - row_starts[0],
        metrics.row_extent(),
        "scroll/date calculations must use the realized physical row stride"
    );

    let first_row_y = row_starts[0] as f32;
    let mut first_row_columns = grid
        .imp()
        .factory_cells
        .borrow()
        .iter()
        .filter_map(|cell| cell.tile.compute_bounds(&grid))
        .filter(|bounds| (bounds.y() - first_row_y).abs() < f32::EPSILON)
        .map(|bounds| bounds.x().round() as i32)
        .collect::<Vec<_>>();
    first_row_columns.sort_unstable();
    first_row_columns.dedup();
    assert_eq!(first_row_columns.len(), 4);
    assert_eq!(
        first_row_columns[1] - first_row_columns[0],
        metrics.row_extent(),
        "filled tiles should leave only the configured 2px inter-column gap"
    );
    assert_eq!(
        first_row_columns[2] - first_row_columns[1],
        metrics.row_extent(),
        "filled tiles should leave only the configured 2px inter-column gap"
    );

    // Resize without crossing a column breakpoint. The layout keeps the
    // preferred Day tile size instead of shrinking it with the viewport.
    window.set_default_size(1_018, 900);
    let resize_deadline_reached = std::rc::Rc::new(std::cell::Cell::new(false));
    let resize_deadline_callback = resize_deadline_reached.clone();
    glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
        resize_deadline_callback.set(true);
    });
    let has_resized_layout = || {
        let metrics = grid.viewport_metrics();
        metrics.columns() == 4
            && metrics.tile_size() == 270
            && grid.imp().factory_cells.borrow().iter().any(|cell| {
                cell.tile.width() == metrics.tile_size()
                    && cell.tile.height() == metrics.tile_size()
            })
    };
    while !resize_deadline_reached.get() && !has_resized_layout() {
        context.iteration(true);
    }
    assert!(
        !resize_deadline_reached.get(),
        "resizing the viewport did not refresh filled tile geometry: metrics={:?}, grid_width={}, scroller_width={}, page_size={}",
        grid.viewport_metrics(),
        grid.imp().grid.get().width(),
        grid.imp().scroller.get().width(),
        grid.imp().scroller.get().hadjustment().page_size(),
    );
    let resized_metrics = grid.viewport_metrics();
    assert_eq!(resized_metrics.columns(), 4);
    assert_eq!(resized_metrics.tile_size(), 270);
    assert_eq!(resized_metrics.row_extent(), 278);

    window.close();
}

#[gtk::test]
fn viewport_width_does_not_shrink_day_tiles_when_column_count_is_unchanged() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let grid = VirtualMediaGrid::new(list, GroupBy::Day, loader, noop_callbacks(), false);

    grid.update_columns_for_width(900);
    let compact = grid.viewport_metrics();
    assert_eq!(compact.columns(), 4);
    assert_eq!(compact.row_extent(), 278);

    grid.update_columns_for_width(1_018);
    let wide = grid.viewport_metrics();
    assert_eq!(wide.columns(), 4);
    assert_eq!(wide.row_extent(), 278);
}

/// Pure drain behaviour: bounded batch size, FIFO order, empties exactly.
#[test]
fn thumbnail_batcher_drain_is_bounded_and_fifo() {
    let mut batcher = ThumbnailBatcher::default();
    let order = std::rc::Rc::new(std::cell::RefCell::new(Vec::<usize>::new()));
    for index in 0..50usize {
        let captured = order.clone();
        batcher.enqueue(Box::new(move || captured.borrow_mut().push(index)));
    }
    assert!(!batcher.is_empty());

    let drained = batcher.drain(24);
    assert_eq!(drained.len(), 24, "first drain must respect the batch size");
    assert!(!batcher.is_empty());
    for request in drained {
        request();
    }

    let drained = batcher.drain(24);
    assert_eq!(drained.len(), 24, "second drain takes the next full batch");
    for request in drained {
        request();
    }

    let drained = batcher.drain(24);
    assert_eq!(
        drained.len(),
        2,
        "final drain takes only the remainder, not a full batch"
    );
    for request in drained {
        request();
    }
    assert!(batcher.is_empty());

    assert_eq!(
        *order.borrow(),
        (0..50).collect::<Vec<_>>(),
        "requests must fire in FIFO enqueue order"
    );
}

/// End-to-end: the idle-driven flush drains every enqueued request without
/// losing any and terminates (re-arms while non-empty, releases when empty).
#[gtk::test]
fn thumbnail_batcher_eventually_drains_all_requests() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("grid.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let grid = VirtualMediaGrid::new(list, GroupBy::Day, loader, noop_callbacks(), false);

    let count = std::rc::Rc::new(std::cell::Cell::new(0usize));
    // More than one batch plus a remainder, so re-arming is exercised.
    let total = runtime_config::thumbnail_batch_per_frame() * 5 + 3;
    for _ in 0..total {
        let captured = count.clone();
        grid.enqueue_thumbnail(move || {
            captured.set(captured.get() + 1);
        });
    }

    let context = glib::MainContext::default();
    let deadline_reached = std::rc::Rc::new(std::cell::Cell::new(false));
    let deadline_callback = deadline_reached.clone();
    glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
        deadline_callback.set(true);
    });
    while !deadline_reached.get() && count.get() < total {
        context.iteration(true);
    }
    assert!(
        !deadline_reached.get(),
        "batcher stalled: only {} of {total} requests fired",
        count.get()
    );
    assert_eq!(count.get(), total);
    assert!(
        !grid.imp().thumb_batcher.borrow().scheduled(),
        "the schedule flag must be released once the batcher is empty"
    );
}
