//! Shared helpers for `gio::ListStore<BoxedAnyObject<MediaItem>>` — the
//! canonical "list of media items" used across the viewer, grid, and trash
//! pages. Centralising the `downcast + borrow + clone` boilerplate keeps
//! `BoxedAnyObject` knowledge in one place and gives us a single place to
//! add new accessors (e.g. `media_item_by_id`).

use crate::core::media::MediaItem;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::{Cast, ListModelExt};

/// Read a `MediaItem` by position. Returns `None` when the position is
/// out of bounds or the wrapped object isn't a `BoxedAnyObject<MediaItem>`
/// (which would be a programmer error — every item is wrapped that way).
pub fn media_item_at(list: &gio::ListStore, index: u32) -> Option<MediaItem> {
    let obj = list.item(index)?;
    let boxed = obj.downcast::<glib::BoxedAnyObject>().ok()?;
    let item = (*boxed.borrow::<MediaItem>()).clone();
    Some(item)
}
