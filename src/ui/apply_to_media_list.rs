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

/// Apply a `MediaChangeEvent` to `list`, preserving the position of
/// existing items. The function is panic-free: any unexpected type
/// mismatch in a list item is silently skipped.
pub fn apply_to_media_list(list: &gtk::gio::ListStore, event: MediaChangeEvent) {
    match event {
        MediaChangeEvent::Upserted(item) => {
            let uri = item.uri.clone();
            for i in 0..list.n_items() {
                if let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() {
                    if obj.borrow::<MediaItem>().uri == uri {
                        list.splice(i, 1, &[glib::BoxedAnyObject::new(item)]);
                        return;
                    }
                }
            }
            list.append(&glib::BoxedAnyObject::new(item));
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::media::MediaItem;
    use chrono::Utc;
    use std::path::PathBuf;

    fn item(id: i64, uri: &str) -> MediaItem {
        MediaItem {
            id,
            uri: uri.into(),
            path: PathBuf::from(uri.trim_start_matches("file://")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            width: Some(64),
            height: Some(48),
            taken_at: None,
            file_mtime: Utc::now(),
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
    fn upserted_appends_when_uri_absent() {
        let list = list_with(vec![item(1, "file:///tmp/a.jpg")]);
        apply_to_media_list(
            &list,
            MediaChangeEvent::Upserted(item(2, "file:///tmp/b.jpg")),
        );
        assert_eq!(list.n_items(), 2);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/b.jpg");
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
