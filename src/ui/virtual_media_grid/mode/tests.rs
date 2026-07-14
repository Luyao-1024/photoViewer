use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailSize;

use super::*;

#[test]
fn each_mode_has_its_product_tile_size_and_thumbnail_bucket() {
    let year = VirtualGridModeSpec::for_mode(GroupBy::Year);
    assert_eq!(year.mode(), GroupBy::Year);
    assert_eq!(year.tile_size(), 90);
    assert_eq!(year.thumbnail_size(), ThumbnailSize::Small);
    assert_eq!(year.row_extent(), 92);

    let month = VirtualGridModeSpec::for_mode(GroupBy::Month);
    assert_eq!(month.mode(), GroupBy::Month);
    assert_eq!(month.tile_size(), 180);
    assert_eq!(month.thumbnail_size(), ThumbnailSize::Medium);
    assert_eq!(month.row_extent(), 182);

    let day = VirtualGridModeSpec::for_mode(GroupBy::Day);
    assert_eq!(day.mode(), GroupBy::Day);
    assert_eq!(day.tile_size(), 270);
    assert_eq!(day.thumbnail_size(), ThumbnailSize::Medium);
    assert_eq!(day.row_extent(), 272);

    assert_ne!(year.tile_size(), day.tile_size());
    assert_ne!(year.thumbnail_size(), day.thumbnail_size());
    assert_ne!(month.tile_size(), day.tile_size());
    assert_eq!(
        month.thumbnail_size(),
        day.thumbnail_size(),
        "Month intentionally shares Day's Medium thumbnail bucket but not its tile size"
    );
}

#[test]
fn columns_for_width_counts_the_two_pixel_gaps_at_boundaries() {
    let year = VirtualGridModeSpec::for_mode(GroupBy::Year);
    assert_eq!(year.columns_for_width(181), 1);
    assert_eq!(year.columns_for_width(182), 2);
    assert_eq!(year.columns_for_width(273), 2);
    assert_eq!(year.columns_for_width(274), 3);

    let month = VirtualGridModeSpec::for_mode(GroupBy::Month);
    assert_eq!(month.columns_for_width(361), 1);
    assert_eq!(month.columns_for_width(362), 2);

    let day = VirtualGridModeSpec::for_mode(GroupBy::Day);
    assert_eq!(day.columns_for_width(541), 1);
    assert_eq!(day.columns_for_width(542), 2);

    assert_eq!(
        VirtualGridModeSpec::for_mode(GroupBy::Year).columns_for_width(274),
        3
    );
}

#[test]
fn columns_for_width_never_returns_zero() {
    let spec = VirtualGridModeSpec::for_mode(GroupBy::Year);

    for width in [i32::MIN, -1, 0, 1, 89] {
        assert_eq!(spec.columns_for_width(width), 1, "width={width}");
    }
}

#[test]
fn fixed_row_metrics_map_adjustment_offsets_to_top_slots() {
    let day = VirtualGridModeSpec::for_mode(GroupBy::Day);
    assert_eq!(VIRTUAL_GRID_TILE_GAP_PX, 2);
    assert_eq!(day.row_extent(), 272);

    let metrics = day.initial_viewport_metrics();
    assert_eq!(metrics.columns(), 1);
    assert_eq!(metrics.tile_size(), 270);
    assert_eq!(metrics.row_extent(), 272);

    assert_eq!(metrics.top_row_for_scroll_offset(-1.0), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(f64::NAN), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(f64::INFINITY), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(271.999), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(272.0), 1);

    assert_eq!(metrics.top_slot_for_scroll_offset(543.999), 1);
    assert_eq!(metrics.top_slot_for_scroll_offset(544.0), 2);
}

#[test]
fn viewport_metrics_match_gridview_column_allocation_after_resize() {
    let day = VirtualGridModeSpec::for_mode(GroupBy::Day);

    // GtkGridView chooses three Day columns from the 270px preferred tile
    // target, then expands those cells to consume all 900 viewport pixels:
    // (900 + 2) / 3 - 2 = 298.
    let compact = day.viewport_metrics_for_width(900);
    assert_eq!(compact.columns(), 3);
    assert_eq!(compact.tile_size(), 298);
    assert_eq!(compact.row_extent(), 300);
    assert_eq!(compact.top_row_for_scroll_offset(299.999), 0);
    assert_eq!(compact.top_row_for_scroll_offset(300.0), 1);
    assert_eq!(compact.top_slot_for_scroll_offset(600.0), 6);

    // A wider window keeps the same column count but must still update its
    // row stride; otherwise date/range math drifts on every resized row.
    let wide = day.viewport_metrics_for_width(1_018);
    assert_eq!(wide.columns(), 3);
    assert_eq!(wide.tile_size(), 338);
    assert_eq!(wide.row_extent(), 340);

    // Before a real allocation GTK retains the tile's natural minimum rather
    // than creating a zero-sized cell.
    assert_eq!(
        day.viewport_metrics_for_width(0),
        day.initial_viewport_metrics()
    );
}
