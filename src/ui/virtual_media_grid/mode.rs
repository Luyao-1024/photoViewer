//! Fixed per-mode metrics for the virtual `GtkGridView` backend.
//!
//! This module deliberately contains no GTK types.  The GridView and layout
//! index must use the same [`VirtualGridModeSpec`] so a physical row maps to
//! the same logical slots that GTK renders.

use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailSize;

/// Fixed spacing between neighboring tiles in the virtual grid.
pub(crate) const VIRTUAL_GRID_TILE_GAP_PX: i32 = 2;

/// Product sizing and thumbnail policy for one virtual-grid grouping mode.
///
/// Keep this as the single source of truth for both GridView configuration and
/// layout-index calculations.  In particular, Month intentionally shares the
/// Medium thumbnail bucket with Day while retaining its own, smaller tile size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VirtualGridModeSpec {
    mode: GroupBy,
    tile_size: i32,
    thumbnail_size: ThumbnailSize,
}

impl VirtualGridModeSpec {
    /// Returns the immutable display and thumbnail policy for `mode`.
    pub(crate) const fn for_mode(mode: GroupBy) -> Self {
        match mode {
            GroupBy::Year => Self {
                mode,
                tile_size: 90,
                thumbnail_size: ThumbnailSize::Small,
            },
            GroupBy::Month => Self {
                mode,
                tile_size: 180,
                thumbnail_size: ThumbnailSize::Medium,
            },
            GroupBy::Day => Self {
                mode,
                tile_size: 270,
                thumbnail_size: ThumbnailSize::Medium,
            },
        }
    }

    /// The grouping mode represented by this spec.
    pub(crate) const fn mode(self) -> GroupBy {
        self.mode
    }

    /// Fixed square side length allocated to each tile, in CSS pixels.
    pub(crate) const fn tile_size(self) -> i32 {
        self.tile_size
    }

    /// Thumbnail cache bucket used for this mode.
    pub(crate) const fn thumbnail_size(self) -> ThumbnailSize {
        self.thumbnail_size
    }

    /// Distance between the starts of two adjacent rows, in CSS pixels.
    ///
    /// The final row does not need trailing visual spacing, but its start still
    /// follows this fixed stride.  Use this for adjustment-to-row calculations.
    pub(crate) const fn row_extent(self) -> i32 {
        self.tile_size + VIRTUAL_GRID_TILE_GAP_PX
    }

    /// Computes the one fixed GridView column count for `available_width`.
    ///
    /// The caller passes the GridView's usable content width after any outer
    /// margins.  `n` columns require `n * tile_size + (n - 1) * gap` pixels,
    /// which is equivalently calculated as `(width + gap) / row_extent`.
    /// A transient zero or negative allocation still yields one column so the
    /// layout index never diverges into an invalid zero-column state.
    pub(crate) fn columns_for_width(self, available_width: i32) -> u32 {
        let width = i64::from(available_width).max(0);
        let extent = i64::from(self.row_extent());
        let columns = ((width + i64::from(VIRTUAL_GRID_TILE_GAP_PX)) / extent).max(1);

        u32::try_from(columns).unwrap_or(u32::MAX)
    }

    /// Converts a vertical adjustment offset into the row at the viewport top.
    ///
    /// GTK adjustments are finite and non-negative in normal use.  Treating
    /// invalid values as the first row makes the helper deterministic during
    /// initialization and teardown.
    pub(crate) fn top_row_for_scroll_offset(self, scroll_offset: f64) -> u32 {
        if !scroll_offset.is_finite() || scroll_offset <= 0.0 {
            return 0;
        }

        let row = (scroll_offset / f64::from(self.row_extent())).floor();
        if row >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            row as u32
        }
    }

    /// Converts a vertical adjustment offset directly into the top physical
    /// slot.  The supplied column count is clamped to one for the same reason
    /// as [`Self::columns_for_width`].
    pub(crate) fn top_slot_for_scroll_offset(self, scroll_offset: f64, columns: u32) -> u32 {
        self.top_row_for_scroll_offset(scroll_offset)
            .saturating_mul(columns.max(1))
    }
}

/// Convenience form for code that has a `GroupBy` rather than a cached spec.
pub(crate) fn columns_for_width(mode: GroupBy, available_width: i32) -> u32 {
    VirtualGridModeSpec::for_mode(mode).columns_for_width(available_width)
}

#[cfg(test)]
mod tests;
