//! GridView row model.
//!
//! The `MediaGrid` is backed by a single `GListStore` of `BoxedAnyObject`
//! values, where each value is a `GridRow` enum — either a section header
//! or a photo. The GridView's factory inspects the variant to decide
//! which child widget to show inside the list item.

use crate::core::media::MediaItem;

#[derive(Debug, Clone)]
pub enum GridRow {
    Header {
        label: String,
    },
    Photo {
        item: MediaItem,
        global_index: u32,
    },
}
