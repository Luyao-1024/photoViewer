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

pub fn media_list_contains_id(list: &gio::ListStore, item_id: i64) -> bool {
    (0..list.n_items()).any(|idx| {
        media_item_at(list, idx)
            .map(|item| item.id == item_id)
            .unwrap_or(false)
    })
}

/// Put an item back into a live list at the position its query would have given
/// it (newest first, ties by id) instead of appending it to the end. Restoring
/// a photo from the trash is the only writer, and both the trash page's grid and
/// the viewer's filmstrip share these lists — appending would move a restored
/// photo to the wrong end of the timeline. Duplicate ids are ignored, because
/// re-inserting an item the list never dropped would show it twice.
pub fn insert_media_item_sorted(list: &gio::ListStore, item: MediaItem) {
    if media_list_contains_id(list, item.id) {
        return;
    }
    let insert_at = (0..list.n_items())
        .find(|&idx| {
            let Some(existing) = media_item_at(list, idx) else {
                return false;
            };
            item.sort_datetime() > existing.sort_datetime()
                || (item.sort_datetime() == existing.sort_datetime() && item.id > existing.id)
        })
        .unwrap_or_else(|| list.n_items());
    list.insert(insert_at, &glib::BoxedAnyObject::new(item));
}
