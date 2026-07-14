//! Fixed per-mode metrics for the virtual `GtkGridView` backend.
//!
//! This module deliberately contains no GTK types.  The GridView and layout
//! index must use the same [`VirtualGridModeSpec`] so a physical row maps to
//! the same logical slots that GTK renders.

use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailSize;

/// Fixed spacing between neighboring tiles in the virtual grid.
///
/// Keep enough breathing room for the card outline and hover accent to remain
/// visible on all four sides, matching the album FlowBox spacing.
pub(crate) const VIRTUAL_GRID_TILE_GAP_PX: i32 = 8;

/// The geometry that one allocated `GtkGridView` viewport actually uses.
///
/// [`VirtualGridModeSpec`] supplies the preferred tile size used to choose a
/// column count. Once `GtkGridView` has a real viewport width, it distributes
/// that width evenly across the chosen columns. This type mirrors that
/// allocated geometry, including GTK's integer rounding, so scroll offsets
/// and range residency use the same row stride as the rendered tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VirtualGridViewportMetrics {
    columns: u32,
    tile_size: i32,
}

impl Default for VirtualGridViewportMetrics {
    fn default() -> Self {
        Self {
            columns: 1,
            tile_size: 1,
        }
    }
}

impl VirtualGridViewportMetrics {
    pub(crate) const fn columns(self) -> u32 {
        self.columns
    }

    /// The square side that an allocated GridView cell gives its tile.
    pub(crate) const fn tile_size(self) -> i32 {
        self.tile_size
    }

    /// Distance between adjacent row starts in the allocated viewport.
    pub(crate) const fn row_extent(self) -> i32 {
        self.tile_size().saturating_add(VIRTUAL_GRID_TILE_GAP_PX)
    }

    /// Converts a vertical adjustment offset into the visible row.
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
    /// slot for this viewport's fixed column count.
    pub(crate) fn top_slot_for_scroll_offset(self, scroll_offset: f64) -> u32 {
        self.top_row_for_scroll_offset(scroll_offset)
            .saturating_mul(self.columns.max(1))
    }
}

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

    pub(crate) fn columns_for_width(self, available_width: i32) -> u32 {
        let width = i64::from(available_width).max(0);
        let extent = i64::from(self.tile_size + VIRTUAL_GRID_TILE_GAP_PX);
        let columns = (width / extent).max(1);
        u32::try_from(columns).unwrap_or(u32::MAX)
    }

    pub(crate) fn viewport_metrics_for_width(
        self,
        available_width: i32,
    ) -> VirtualGridViewportMetrics {
        let columns = self.columns_for_width(available_width);
        let width = i64::from(available_width).max(0);
        let gap = i64::from(VIRTUAL_GRID_TILE_GAP_PX);
        let tile_size = (width / i64::from(columns))
            .saturating_sub(gap)
            .max(i64::from(self.tile_size));
        VirtualGridViewportMetrics {
            columns,
            tile_size: i32::try_from(tile_size).unwrap_or(i32::MAX),
        }
    }

    /// Computes geometry for the user-selected, fixed column count.
    pub(crate) fn viewport_metrics_for_fixed_columns(
        self,
        available_width: i32,
        columns: u32,
    ) -> VirtualGridViewportMetrics {
        let columns = columns.max(1);
        let width = i64::from(available_width).max(0);
        let gap = i64::from(VIRTUAL_GRID_TILE_GAP_PX);
        let allocated_tile = width / i64::from(columns);
        // Day columns are user-configured. Never turn that preference into a
        // thumbnail-quality downgrade just because the current window is too
        // narrow; the host window/content area will request the preferred
        // width and GTK can scroll temporarily while the resize settles.
        let tile_size = allocated_tile
            .saturating_sub(gap)
            .max(i64::from(self.tile_size));

        VirtualGridViewportMetrics {
            columns,
            tile_size: i32::try_from(tile_size).unwrap_or(i32::MAX),
        }
    }

    /// Minimum content width needed to render fixed columns at the product's
    /// preferred tile size without scaling thumbnails down.
    pub(crate) fn preferred_width_for_fixed_columns(self, columns: u32) -> i32 {
        let columns = i64::from(columns.max(1));
        let width = columns.saturating_mul(i64::from(self.tile_size + VIRTUAL_GRID_TILE_GAP_PX));
        i32::try_from(width).unwrap_or(i32::MAX)
    }
}

#[cfg(test)]
mod tests;
