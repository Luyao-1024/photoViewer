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
        item: Box<MediaItem>,
        global_index: u32,
    },
}

/// Compile-time guard: if `GridRow::Photo` ever drops the `Box<MediaItem>`
/// indirection, this fails to compile and CI's
/// `cargo clippy --all-targets -- -D warnings` step will reject the build
/// for `clippy::large_enum_variant`.
const _: () = {
    assert!(
        std::mem::size_of::<GridRow>() < 256,
        "GridRow grew past 256 bytes; restore the `Box<MediaItem>` indirection \
         on the `Photo` variant to avoid large_enum_variant lint"
    );
};
