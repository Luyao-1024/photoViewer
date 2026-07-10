use super::ViewSpec;
use crate::ui::square_tile::SquareTile;
use gtk4 as gtk;
use gtk4::prelude::*;

const VIRTUAL_TILE_GAP: i32 = 2;
const VIRTUAL_PREFETCH_LOW_NUM: u32 = 1;
const VIRTUAL_PREFETCH_HIGH_NUM: u32 = 4;
const VIRTUAL_PREFETCH_DEN: u32 = 5;

pub(super) fn virtual_offset_for_ratio(ratio: f64, total: u32, page_size: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    let max_start = total.saturating_sub(page_size);
    let ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((total as f64 * ratio).floor() as u32).min(max_start)
}

pub(super) fn virtual_page_start_for_offset(
    desired_offset: u32,
    current_start: u32,
    current_len: u32,
    total: u32,
    page_size: u32,
) -> Option<u32> {
    if total == 0 || current_len == 0 || current_len >= total {
        return None;
    }
    let low =
        current_start + current_len.saturating_mul(VIRTUAL_PREFETCH_LOW_NUM) / VIRTUAL_PREFETCH_DEN;
    let high = current_start
        + current_len.saturating_mul(VIRTUAL_PREFETCH_HIGH_NUM) / VIRTUAL_PREFETCH_DEN;
    if desired_offset >= low && desired_offset <= high {
        return None;
    }

    let max_start = total.saturating_sub(page_size);
    if desired_offset >= max_start {
        return Some(max_start);
    }

    let centered = desired_offset.saturating_sub(page_size / 2);
    Some(centered.min(max_start))
}

pub(super) fn estimated_virtual_columns(viewport_width: f64, spec: ViewSpec) -> u32 {
    let tile = (spec.pixel_size + VIRTUAL_TILE_GAP).max(1) as f64;
    (viewport_width / tile).floor().max(1.0) as u32
}

pub(super) fn virtual_spacer_height(
    unloaded_items: u32,
    columns: u32,
    _viewport_width: f64,
    spec: ViewSpec,
) -> i32 {
    if unloaded_items == 0 {
        return 0;
    }
    let rows = unloaded_items.div_ceil(columns.max(1));
    let row_height = (spec.pixel_size + VIRTUAL_TILE_GAP).max(1) as u32;
    rows.saturating_mul(row_height).min(i32::MAX as u32) as i32
}

pub(super) fn virtual_spacer(height: i32) -> gtk::Box {
    let spacer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .can_focus(false)
        .build();
    spacer.set_size_request(-1, height.max(0));
    spacer
}

pub(super) fn virtual_window_item_count(start: u32, total: u32, page_size: u32) -> u32 {
    if start >= total {
        0
    } else {
        total.saturating_sub(start).min(page_size)
    }
}

pub(super) fn should_consider_virtual_page_load(
    restoring_scroll: bool,
    total: u32,
    current_len: u32,
) -> bool {
    !restoring_scroll && total > current_len && current_len > 0
}

pub(super) fn replace_pending_virtual_page(
    pending_start: &std::cell::Cell<Option<u32>>,
    pending_ratio: &std::cell::Cell<Option<f64>>,
    target_start: u32,
    ratio: f64,
) {
    pending_start.set(Some(target_start));
    pending_ratio.set(Some(ratio));
}

pub(super) fn build_virtual_placeholder_flow(spec: ViewSpec, count: u32) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .column_spacing(8)
        .row_spacing(8)
        .max_children_per_line(100)
        .selection_mode(gtk::SelectionMode::None)
        .build();
    flow.add_css_class("thumb-grid");
    flow.add_css_class("virtual-placeholder-grid");

    for _ in 0..count {
        let tile = SquareTile::new();
        tile.set_target(spec.pixel_size);
        tile.add_css_class("thumb-loading");
        tile.add_css_class("thumb-placeholder");
        flow.append(&tile);
    }

    flow
}

#[cfg(test)]
mod tests;
