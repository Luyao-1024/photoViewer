use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailSize;

use super::*;

#[test]
fn each_mode_has_its_product_tile_size_and_thumbnail_bucket() {
    let year = VirtualGridModeSpec::for_mode(GroupBy::Year);
    assert_eq!(year.mode(), GroupBy::Year);
    assert_eq!(year.tile_size(), 90);
    assert_eq!(year.thumbnail_size(), ThumbnailSize::Small);

    let month = VirtualGridModeSpec::for_mode(GroupBy::Month);
    assert_eq!(month.mode(), GroupBy::Month);
    assert_eq!(month.tile_size(), 180);
    assert_eq!(month.thumbnail_size(), ThumbnailSize::Medium);

    let day = VirtualGridModeSpec::for_mode(GroupBy::Day);
    assert_eq!(day.mode(), GroupBy::Day);
    assert_eq!(day.tile_size(), 270);
    assert_eq!(day.thumbnail_size(), ThumbnailSize::Medium);

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
fn fixed_row_metrics_map_adjustment_offsets_to_top_slots() {
    let day = VirtualGridModeSpec::for_mode(GroupBy::Day);
    assert_eq!(VIRTUAL_GRID_TILE_GAP_PX, 2);
    let metrics = day.viewport_metrics_for_fixed_columns(900, 4);
    assert_eq!(metrics.columns(), 4);
    assert_eq!(metrics.tile_size(), 223);
    assert_eq!(metrics.row_extent(), 225);

    assert_eq!(metrics.top_row_for_scroll_offset(-1.0), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(f64::NAN), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(f64::INFINITY), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(224.999), 0);
    assert_eq!(metrics.top_row_for_scroll_offset(225.0), 1);

    assert_eq!(metrics.top_slot_for_scroll_offset(449.999), 4);
    assert_eq!(metrics.top_slot_for_scroll_offset(450.0), 8);
}

#[test]
fn fixed_columns_only_change_cell_geometry_after_resize() {
    let day = VirtualGridModeSpec::for_mode(GroupBy::Day);
    let compact = day.viewport_metrics_for_fixed_columns(900, 4);
    let wide = day.viewport_metrics_for_fixed_columns(1_018, 4);
    assert_eq!(compact.columns(), wide.columns());
    assert_eq!(compact.tile_size(), 223);
    assert_eq!(wide.tile_size(), 253);
}
