//! Apply a `MediaChangeEvent` to the shared `gio::ListStore`.
//!
//! Kept as a tiny free function in its own module so it can be tested
//! headlessly (no GTK window required). The list store is the single
//! data source backing the three `MediaGrid` instances on `PhotosPage`.

use crate::core::media::MediaItem;
use crate::core::media_change_notifier::MediaChangeEvent;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

/// Apply a `MediaChangeEvent` to `list`, keeping the same global ordering as
/// `db::list_all_media`: photo sort time descending, then id descending.
/// The function is panic-free: any unexpected type mismatch in a list item is
/// silently skipped.
pub fn apply_to_media_list(list: &gtk::gio::ListStore, event: MediaChangeEvent) {
    match event {
        MediaChangeEvent::Upserted(item) => {
            let uri = item.uri.clone();
            for i in 0..list.n_items() {
                if let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() {
                    let existing = obj.borrow::<MediaItem>();
                    if existing.uri == uri {
                        if same_sort_position(&existing, &item) {
                            list.splice(i, 1, &[glib::BoxedAnyObject::new(item)]);
                        } else {
                            list.remove(i);
                            insert_sorted(list, item);
                        }
                        return;
                    }
                }
            }
            insert_sorted(list, item);
        }
        MediaChangeEvent::Removed { uri } => {
            for i in 0..list.n_items() {
                if let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() {
                    if obj.borrow::<MediaItem>().uri == uri {
                        list.remove(i);
                        return;
                    }
                }
            }
        }
        // TrashChanged 不影响 live 相册列表（回收站视图自己监听该事件刷新）。
        // 此 arm 仅为穷尽匹配；正常调用方在分发前已把 TrashChanged 单独处理。
        MediaChangeEvent::TrashChanged => {}
    }
}

fn same_sort_position(a: &MediaItem, b: &MediaItem) -> bool {
    a.sort_datetime() == b.sort_datetime() && a.id == b.id
}

fn should_sort_before(candidate: &MediaItem, existing: &MediaItem) -> bool {
    candidate.sort_datetime() > existing.sort_datetime()
        || (candidate.sort_datetime() == existing.sort_datetime() && candidate.id > existing.id)
}

fn insert_sorted(list: &gtk::gio::ListStore, item: MediaItem) {
    let pos = sorted_insert_position(list, &item);
    list.insert(pos, &glib::BoxedAnyObject::new(item));
}

fn sorted_insert_position(list: &gtk::gio::ListStore, item: &MediaItem) -> u32 {
    for i in 0..list.n_items() {
        let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if should_sort_before(item, &obj.borrow::<MediaItem>()) {
            return i;
        }
    }
    list.n_items()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::media::MediaItem;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn item(id: i64, uri: &str) -> MediaItem {
        item_at(id, uri, 2026, 6, 25, 12)
    }

    fn item_at(id: i64, uri: &str, year: i32, month: u32, day: u32, hour: u32) -> MediaItem {
        let dt = Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap();
        MediaItem {
            id,
            uri: uri.into(),
            path: PathBuf::from(uri.trim_start_matches("file://")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            width: Some(64),
            height: Some(48),
            taken_at: None,
            file_mtime: dt,
            file_size: 1,
            blake3_hash: "h".into(),
            trashed_at: None,
        }
    }

    fn list_with(items: Vec<MediaItem>) -> gtk::gio::ListStore {
        let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for it in items {
            list.append(&glib::BoxedAnyObject::new(it));
        }
        list
    }

    fn nth_uri(list: &gtk::gio::ListStore, idx: u32) -> String {
        list.item(idx)
            .and_downcast::<glib::BoxedAnyObject>()
            .unwrap()
            .borrow::<MediaItem>()
            .uri
            .clone()
    }

    #[test]
    fn upserted_places_older_absent_item_at_end() {
        let list = list_with(vec![item_at(1, "file:///tmp/a.jpg", 2026, 6, 25, 12)]);
        apply_to_media_list(
            &list,
            MediaChangeEvent::Upserted(item_at(2, "file:///tmp/b.jpg", 2026, 6, 24, 12)),
        );
        assert_eq!(list.n_items(), 2);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/b.jpg");
    }

    #[test]
    fn upserted_inserts_new_item_by_global_photo_order() {
        let list = list_with(vec![
            item_at(1, "file:///tmp/newer.jpg", 2026, 6, 25, 12),
            item_at(2, "file:///tmp/older.jpg", 2026, 6, 23, 12),
        ]);

        apply_to_media_list(
            &list,
            MediaChangeEvent::Upserted(item_at(3, "file:///tmp/middle.jpg", 2026, 6, 24, 12)),
        );

        assert_eq!(nth_uri(&list, 0), "file:///tmp/newer.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/middle.jpg");
        assert_eq!(nth_uri(&list, 2), "file:///tmp/older.jpg");
    }

    #[test]
    fn upserted_replaces_in_place_when_uri_present() {
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
            item(3, "file:///tmp/c.jpg"),
        ]);
        let mut updated = item(2, "file:///tmp/b.jpg");
        updated.blake3_hash = "new-hash".into();
        apply_to_media_list(&list, MediaChangeEvent::Upserted(updated));
        assert_eq!(list.n_items(), 3, "upsert must not change list length");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/b.jpg");
        // Sanity: the new blake3 hash actually took effect.
        let boxed = list.item(1).and_downcast::<glib::BoxedAnyObject>().unwrap();
        assert_eq!(boxed.borrow::<MediaItem>().blake3_hash, "new-hash");
    }

    #[test]
    fn upserted_moves_existing_item_when_sort_time_changes() {
        let list = list_with(vec![
            item_at(1, "file:///tmp/a.jpg", 2026, 6, 25, 12),
            item_at(2, "file:///tmp/b.jpg", 2026, 6, 24, 12),
            item_at(3, "file:///tmp/c.jpg", 2026, 6, 23, 12),
        ]);
        let mut updated = item_at(3, "file:///tmp/c.jpg", 2026, 6, 26, 12);
        updated.blake3_hash = "new-hash".into();

        apply_to_media_list(&list, MediaChangeEvent::Upserted(updated));

        assert_eq!(list.n_items(), 3);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/c.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/a.jpg");
        assert_eq!(nth_uri(&list, 2), "file:///tmp/b.jpg");
        let boxed = list.item(0).and_downcast::<glib::BoxedAnyObject>().unwrap();
        assert_eq!(boxed.borrow::<MediaItem>().blake3_hash, "new-hash");
    }

    #[test]
    fn removed_deletes_when_uri_present() {
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
        ]);
        apply_to_media_list(
            &list,
            MediaChangeEvent::Removed {
                uri: "file:///tmp/b.jpg".into(),
            },
        );
        assert_eq!(list.n_items(), 1);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
    }

    #[test]
    fn removed_is_noop_when_uri_absent() {
        let list = list_with(vec![item(1, "file:///tmp/a.jpg")]);
        apply_to_media_list(
            &list,
            MediaChangeEvent::Removed {
                uri: "file:///tmp/missing.jpg".into(),
            },
        );
        assert_eq!(list.n_items(), 1);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
    }
}
