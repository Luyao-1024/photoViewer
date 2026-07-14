//! Fixed per-mode metrics for the virtual `GtkGridView` backend.
//!
//! This module deliberately contains no GTK types.  The GridView and layout
//! index must use the same [`VirtualGridModeSpec`] so a physical row maps to
//! the same logical slots that GTK renders.

use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailSize;

/// Fixed spacing between neighboring tiles in the virtual grid.
pub(crate) const VIRTUAL_GRID_TILE_GAP_PX: i32 = 2;

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

    /// Preferred distance between the starts of two adjacent rows, in CSS
    /// pixels, before the grid has a real viewport allocation.
    ///
    /// The rendered viewport may be wider than this preferred target. Runtime
    /// scroll calculations must use [`Self::viewport_metrics_for_width`] once
    /// the scroller has reported its width.
    pub(crate) const fn row_extent(self) -> i32 {
        self.tile_size + VIRTUAL_GRID_TILE_GAP_PX
    }

    /// Geometry used before the scroller's first non-zero allocation.
    pub(crate) const fn initial_viewport_metrics(self) -> VirtualGridViewportMetrics {
        VirtualGridViewportMetrics {
            columns: 1,
            tile_size: self.tile_size,
        }
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

    /// Computes the real cell geometry for a GridView viewport.
    ///
    /// This intentionally follows GTK's `GtkGridView` allocation formula:
    /// `(viewport_width + border_spacing) / columns - border_spacing`, then
    /// clamps to the tile's natural minimum. GTK uses integer division here,
    /// so matching it avoids a one-pixel-per-row drift in virtual-scroll
    /// calculations after a window resize.
    pub(crate) fn viewport_metrics_for_width(
        self,
        available_width: i32,
    ) -> VirtualGridViewportMetrics {
        let columns = self.columns_for_width(available_width).max(1);
        let width = i64::from(available_width).max(0);
        let gap = i64::from(VIRTUAL_GRID_TILE_GAP_PX);
        let allocated_tile = (width + gap) / i64::from(columns);
        let tile_size = allocated_tile
            .saturating_sub(gap)
            .max(i64::from(self.tile_size));

        VirtualGridViewportMetrics {
            columns,
            tile_size: i32::try_from(tile_size).unwrap_or(i32::MAX),
        }
    }
}

#[cfg(test)]
mod tests;
