use super::*;
use crate::ui::media_grid::test_support::{insert_sample_item, noop_callbacks, sample_item};

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
    let allows_width_shrink = grid
        .imp()
        .factory_cells
        .borrow()
        .iter()
        .all(|cell| cell.tile.allows_width_shrink());
    assert_eq!(
        allows_width_shrink,
        true,
        "GridView factory tiles must accept the final cell width"
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
