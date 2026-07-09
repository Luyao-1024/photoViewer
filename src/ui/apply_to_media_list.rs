//! Apply a `DomainEvent` to the shared `gio::ListStore`.
//!
//! Kept as a tiny free function in its own module so it can be tested
//! headlessly (no GTK window required). The list store is the single
//! data source backing the three `MediaGrid` instances on `PhotosPage`.

use crate::core::events::{ChangeSource, DomainEvent};
use crate::core::media::MediaItem;
use crate::core::runtime_config;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

/// Get the current UI media list cap from runtime configuration.
pub fn ui_media_list_cap() -> usize {
    runtime_config::ui_media_list_cap()
}

/// Apply a `DomainEvent` to `list`, keeping the same global ordering as
/// `db::list_all_media`: photo sort time descending, then id descending.
/// The function is panic-free: any unexpected type mismatch in a list item is
/// silently skipped.
pub fn apply_to_media_list(list: &gtk::gio::ListStore, event: &DomainEvent) {
    match event {
        DomainEvent::MediaUpserted { source, items } => {
            apply_upserted_batch(list, *source, items.clone());
        }
        DomainEvent::MediaUpdated { items, .. } => {
            apply_upserted_batch(list, ChangeSource::UserInteractive, items.clone());
        }
        DomainEvent::MediaRemoved { uris, .. } => {
            remove_uris_batch(list, uris);
        }
        DomainEvent::MediaMovedToTrash { items, .. } => {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "TRASH_TRACE ui_apply_moved_to_trash_begin list_len={} count={} ids={:?}",
                list.n_items(),
                items.len(),
                items.iter().map(|item| item.id).collect::<Vec<_>>()
            );
            let uris: Vec<String> = items.iter().map(|item| item.uri.clone()).collect();
            remove_uris_batch(list, &uris);
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "TRASH_TRACE ui_apply_moved_to_trash_done list_len={}",
                list.n_items()
            );
        }
        DomainEvent::TrashChanged { .. }
        | DomainEvent::MediaRestored { .. }
        | DomainEvent::AlbumsChanged { .. }
        | DomainEvent::AlbumCoverChanged { .. }
        | DomainEvent::AlbumsDirty { .. }
        | DomainEvent::ThumbnailStatsDirty
        | DomainEvent::LiveCountDirty => {}
    }
}

/// 一次性从 `list` 移除所有 uri 命中 `uris` 的项。
///
/// 旧实现逐 uri 线性扫描整个 list（O(uris × list_len)），并每次 miss 都打一条
/// WARN：一个携带数万 uri 的 `MediaRemoved`（如整个相册被删后启动 prune）会触发
/// 数千万次比较 + 海量 WARN。这里改成一趟扫描：先把 `uris` 装进 HashSet，再对
/// list 做一次遍历收集命中位置，最后把**连续位置合并成区间**逆序 `splice` 删除——
/// 复杂度 O(list_len + removed)，连续块只发一条 items-changed，且整批只打一条汇总日志。
fn remove_uris_batch(list: &gtk::gio::ListStore, uris: &[String]) {
    if uris.is_empty() || list.n_items() == 0 {
        return;
    }
    let uri_set: std::collections::HashSet<&str> = uris.iter().map(String::as_str).collect();
    let n = list.n_items();
    let positions: Vec<u32> = (0..n)
        .filter(|&i| {
            list.item(i)
                .and_downcast::<glib::BoxedAnyObject>()
                .map(|obj| uri_set.contains(obj.borrow::<MediaItem>().uri.as_str()))
                .unwrap_or(false)
        })
        .collect();
    if positions.is_empty() {
        return;
    }
    let before = n;
    let removed = positions.len() as u32;
    // 合并连续位置为 (start, len) 区间，逆序 splice 删除以保持索引有效，
    // 并把 items-changed 通知压到「每个连续块一条」。
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut start = positions[0];
    let mut end = positions[0];
    for &pos in positions.iter().skip(1) {
        if pos == end + 1 {
            end = pos;
        } else {
            ranges.push((start, end - start + 1));
            start = pos;
            end = pos;
        }
    }
    ranges.push((start, end - start + 1));
    let empty: &[glib::BoxedAnyObject] = &[];
    for (position, n_removals) in ranges.into_iter().rev() {
        list.splice(position, n_removals, empty);
    }
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "TRASH_TRACE ui_remove_uris_batch removed={} before={} after={} requested={}",
        removed,
        before,
        list.n_items(),
        uris.len()
    );
}

#[tracing::instrument(
    name = "ui:apply_upserted_batch",
    skip(list, items),
    fields(source = ?source, incoming = items.len()),
    level = "debug"
)]
fn apply_upserted_batch(list: &gtk::gio::ListStore, source: ChangeSource, items: Vec<MediaItem>) {
    if items.is_empty() {
        return;
    }
    if source == ChangeSource::StartupScan && list.n_items() as usize >= ui_media_list_cap() {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "UI_LIST_BATCH_MERGE skipped_startup_after_cap incoming_len={} list_len={}",
            items.len(),
            list.n_items()
        );
        return;
    }
    if apply_absent_item_insertions(list, source, &items) {
        return;
    }
    if apply_targeted_upserts(list, source, &items) {
        return;
    }

    let incoming_len = items.len();
    let list_len_before = list.n_items();
    let mut by_uri = std::collections::HashMap::with_capacity(list.n_items() as usize);
    for i in 0..list.n_items() {
        if let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() {
            by_uri.insert(
                obj.borrow::<MediaItem>().uri.clone(),
                (*obj.borrow::<MediaItem>()).clone(),
            );
        }
    }

    for item in items {
        by_uri.insert(item.uri.clone(), item);
    }

    let mut merged: Vec<MediaItem> = by_uri.into_values().collect();
    merged.sort_by(|a, b| {
        b.sort_datetime()
            .cmp(&a.sort_datetime())
            .then_with(|| b.id.cmp(&a.id))
    });

    if merged.len() > ui_media_list_cap() {
        merged.truncate(ui_media_list_cap());
    }

    let additions: Vec<glib::BoxedAnyObject> =
        merged.into_iter().map(glib::BoxedAnyObject::new).collect();
    list.splice(0, list.n_items(), &additions);
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "UI_LIST_BATCH_MERGE incoming_len={} list_len_before={} list_len_after={}",
        incoming_len,
        list_len_before,
        list.n_items()
    );
}

fn compare_media_order(a: &MediaItem, b: &MediaItem) -> std::cmp::Ordering {
    b.sort_datetime()
        .cmp(&a.sort_datetime())
        .then_with(|| b.id.cmp(&a.id))
}

fn item_at(list: &gtk::gio::ListStore, index: u32) -> Option<MediaItem> {
    list.item(index)
        .and_downcast::<glib::BoxedAnyObject>()
        .map(|obj| (*obj.borrow::<MediaItem>()).clone())
}

fn sorted_insert_position(list: &gtk::gio::ListStore, item: &MediaItem) -> u32 {
    for i in 0..list.n_items() {
        let Some(existing) = item_at(list, i) else {
            continue;
        };
        if compare_media_order(item, &existing) == std::cmp::Ordering::Less {
            return i;
        }
    }
    list.n_items()
}

#[tracing::instrument(
    name = "ui:apply_absent_insertions",
    skip(list, items),
    fields(source = ?source, incoming = items.len()),
    level = "debug"
)]
fn apply_absent_item_insertions(
    list: &gtk::gio::ListStore,
    source: ChangeSource,
    items: &[MediaItem],
) -> bool {
    let mut existing_uris = std::collections::HashSet::with_capacity(list.n_items() as usize);
    for i in 0..list.n_items() {
        if let Some(item) = item_at(list, i) {
            existing_uris.insert(item.uri);
        }
    }
    if items.iter().any(|item| existing_uris.contains(&item.uri)) {
        return false;
    }

    let mut incoming = items.to_vec();
    incoming.sort_by(compare_media_order);
    let cap = ui_media_list_cap() as u32;
    let mut inserted = 0u32;
    for item in incoming {
        let position = sorted_insert_position(list, &item);
        if position >= cap {
            continue;
        }
        let boxed = glib::BoxedAnyObject::new(item);
        list.insert(position, &boxed);
        inserted += 1;
        if list.n_items() > cap {
            list.remove(list.n_items() - 1);
        }
    }

    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "UI_LIST_ABSENT_INSERT source={:?} incoming_len={} inserted={} list_len_after={}",
        source,
        items.len(),
        inserted,
        list.n_items()
    );
    true
}

#[tracing::instrument(
    name = "ui:apply_targeted_upserts",
    skip(list, items),
    fields(source = ?source, incoming = items.len()),
    level = "debug"
)]
fn apply_targeted_upserts(
    list: &gtk::gio::ListStore,
    source: ChangeSource,
    items: &[MediaItem],
) -> bool {
    if source == ChangeSource::StartupScan {
        return false;
    }

    let cap = ui_media_list_cap() as u32;
    let mut incoming = items.to_vec();
    incoming.sort_by(compare_media_order);
    let mut removed = 0u32;
    let mut inserted = 0u32;

    for item in incoming {
        let mut existing_position = None;
        let mut preserves_sort_position = false;
        for index in 0..list.n_items() {
            let Some(existing) = item_at(list, index) else {
                continue;
            };
            if existing.uri == item.uri {
                existing_position = Some(index);
                preserves_sort_position = compare_media_order(&item, &existing).is_eq();
                list.remove(index);
                removed = removed.saturating_add(1);
                break;
            }
        }

        let position = if preserves_sort_position {
            existing_position
                .unwrap_or_else(|| list.n_items())
                .min(list.n_items())
        } else {
            sorted_insert_position(list, &item)
        };
        if position >= cap {
            continue;
        }
        list.insert(position, &glib::BoxedAnyObject::new(item));
        inserted = inserted.saturating_add(1);
        if list.n_items() > cap {
            list.remove(list.n_items() - 1);
            removed = removed.saturating_add(1);
        }
    }

    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "UI_LIST_TARGETED_UPSERT source={:?} incoming_len={} removed={} inserted={} list_len_after={}",
        source,
        items.len(),
        removed,
        inserted,
        list.n_items()
    );
    true
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
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(64),
            height: Some(48),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: dt,
            file_size: 1,
            blake3_hash: "h".into(),
            is_favorite: false,
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
    fn high_volume_apply_spans_stay_debug() {
        let source = include_str!("apply_to_media_list.rs");
        let production_source = source
            .split("\n#[cfg(test)]")
            .next()
            .expect("apply_to_media_list.rs must contain production code");

        for span_name in [
            "ui:apply_upserted_batch",
            "ui:apply_absent_insertions",
            "ui:apply_targeted_upserts",
        ] {
            let span_index = production_source
                .find(&format!("name = \"{span_name}\""))
                .unwrap_or_else(|| panic!("missing instrument span {span_name}"));
            let attr_start = production_source[..span_index]
                .rfind("#[tracing::instrument")
                .expect("span should use tracing::instrument");
            let attr_end = production_source[span_index..]
                .find(")]")
                .map(|end| span_index + end + ")]".len())
                .expect("instrument attribute should close");
            let attr = &production_source[attr_start..attr_end];
            assert!(
                attr.contains("level = \"debug\""),
                "{span_name} scales with media-list batches and should stay out of default INFO logs"
            );
        }
    }

    #[test]
    fn upserted_places_older_absent_item_at_end() {
        let list = list_with(vec![item_at(1, "file:///tmp/a.jpg", 2026, 6, 25, 12)]);
        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::FilesystemWatcher,
                items: vec![item_at(2, "file:///tmp/b.jpg", 2026, 6, 24, 12)],
            },
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
            &DomainEvent::MediaUpserted {
                source: ChangeSource::FilesystemWatcher,
                items: vec![item_at(3, "file:///tmp/middle.jpg", 2026, 6, 24, 12)],
            },
        );

        assert_eq!(nth_uri(&list, 0), "file:///tmp/newer.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/middle.jpg");
        assert_eq!(nth_uri(&list, 2), "file:///tmp/older.jpg");
    }

    #[test]
    fn upserted_batch_merges_replaces_and_sorts_with_one_splice() {
        let list = list_with(vec![
            item_at(1, "file:///tmp/newer.jpg", 2026, 6, 25, 12),
            item_at(2, "file:///tmp/older.jpg", 2026, 6, 23, 12),
        ]);
        let mut updated = item_at(2, "file:///tmp/older.jpg", 2026, 6, 26, 12);
        updated.blake3_hash = "updated".into();

        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::UserInteractive,
                items: vec![
                    item_at(3, "file:///tmp/middle.jpg", 2026, 6, 24, 12),
                    updated,
                ],
            },
        );

        assert_eq!(list.n_items(), 3);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/older.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/newer.jpg");
        assert_eq!(nth_uri(&list, 2), "file:///tmp/middle.jpg");
        let boxed = list.item(0).and_downcast::<glib::BoxedAnyObject>().unwrap();
        assert_eq!(boxed.borrow::<MediaItem>().blake3_hash, "updated");
    }

    #[test]
    fn upserted_batch_caps_ui_model_to_recent_items() {
        let cap = ui_media_list_cap();
        let list = list_with(Vec::new());
        let mut items = Vec::new();
        for id in 0..(cap as i64 + 10) {
            items.push(item_at(
                id,
                &format!("file:///tmp/{id}.jpg"),
                2026,
                6,
                25,
                12,
            ));
        }

        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::StartupScan,
                items,
            },
        );

        assert_eq!(list.n_items() as usize, cap);
    }

    #[test]
    fn startup_scan_batch_after_cap_does_not_rebuild_ui_model() {
        // 此测试验证当列表未满 cap 时 StartupScan 批次会正常合并。
        let cap = ui_media_list_cap();
        let initial_len = cap.saturating_sub(1).min(200);
        let list = list_with(
            (0..initial_len as i64)
                .map(|id| item_at(id, &format!("file:///tmp/{id}.jpg"), 2026, 6, 24, 12))
                .collect(),
        );

        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::StartupScan,
                items: vec![item_at(
                    10_000,
                    "file:///tmp/newest-from-scan.jpg",
                    2026,
                    6,
                    26,
                    12,
                )],
            },
        );

        assert_eq!(list.n_items() as usize, initial_len + 1);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/newest-from-scan.jpg");
    }

    #[test]
    fn startup_scan_new_items_insert_without_removing_existing_rows() {
        let list = list_with(vec![item_at(
            1,
            "file:///tmp/existing.jpg",
            2026,
            6,
            24,
            12,
        )]);
        let removed_total = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let removed_total_for_signal = removed_total.clone();
        list.connect_items_changed(move |_, _, removed, _| {
            removed_total_for_signal.set(removed_total_for_signal.get() + removed);
        });

        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::StartupScan,
                items: vec![item_at(2, "file:///tmp/newer.jpg", 2026, 6, 25, 12)],
            },
        );

        assert_eq!(nth_uri(&list, 0), "file:///tmp/newer.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/existing.jpg");
        assert_eq!(
            removed_total.get(),
            0,
            "startup scan insertions should not look like a full model replacement to MediaGrid"
        );
    }

    #[test]
    fn filesystem_watcher_new_items_insert_without_full_model_replacement() {
        let list = list_with(vec![
            item_at(1, "file:///tmp/existing-newer.jpg", 2026, 6, 25, 12),
            item_at(2, "file:///tmp/existing-older.jpg", 2026, 6, 23, 12),
        ]);
        let signal = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let signal_for_cb = signal.clone();
        list.connect_items_changed(move |_, position, removed, added| {
            signal_for_cb.borrow_mut().push((position, removed, added));
        });

        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::FilesystemWatcher,
                items: vec![item_at(3, "file:///tmp/newest.jpg", 2026, 6, 26, 12)],
            },
        );

        assert_eq!(list.n_items(), 3);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/newest.jpg");
        assert_eq!(
            signal.borrow().as_slice(),
            &[(0, 0, 1)],
            "a watcher-only insert must not emit the full replacement that makes MediaGrid rebuild all tiles"
        );
    }

    #[test]
    fn upserted_item_trims_ui_model_after_insert() {
        // cap 已提高至 10000；此测试验证在列表未满 cap 时插入新项不会被截断。
        let list = list_with(
            (0..200)
                .map(|id| item_at(id, &format!("file:///tmp/{id}.jpg"), 2026, 6, 24, 12))
                .collect(),
        );

        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::FilesystemWatcher,
                items: vec![item_at(10_000, "file:///tmp/newest.jpg", 2026, 6, 26, 12)],
            },
        );

        assert_eq!(list.n_items(), 201);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/newest.jpg");
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
        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::FilesystemWatcher,
                items: vec![updated],
            },
        );
        assert_eq!(list.n_items(), 3, "upsert must not change list length");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/b.jpg");
        // Sanity: the new blake3 hash actually took effect.
        let boxed = list.item(1).and_downcast::<glib::BoxedAnyObject>().unwrap();
        assert_eq!(boxed.borrow::<MediaItem>().blake3_hash, "new-hash");
    }

    #[test]
    fn upserted_existing_item_does_not_emit_full_model_replacement() {
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
            item(3, "file:///tmp/c.jpg"),
        ]);
        let signal = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let signal_for_cb = signal.clone();
        list.connect_items_changed(move |_, position, removed, added| {
            signal_for_cb.borrow_mut().push((position, removed, added));
        });

        let mut updated = item(2, "file:///tmp/b.jpg");
        updated.blake3_hash = "new-hash".into();
        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::FilesystemWatcher,
                items: vec![updated],
            },
        );

        assert_eq!(
            signal.borrow().as_slice(),
            &[(1, 1, 0), (1, 0, 1)],
            "updating one existing URI should remove/insert that row only, not replace the full visible model"
        );
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

        apply_to_media_list(
            &list,
            &DomainEvent::MediaUpserted {
                source: ChangeSource::FilesystemWatcher,
                items: vec![updated],
            },
        );

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
            &DomainEvent::MediaRemoved {
                source: ChangeSource::FilesystemWatcher,
                ids: Vec::new(),
                uris: vec!["file:///tmp/b.jpg".into()],
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
            &DomainEvent::MediaRemoved {
                source: ChangeSource::FilesystemWatcher,
                ids: Vec::new(),
                uris: vec!["file:///tmp/missing.jpg".into()],
            },
        );
        assert_eq!(list.n_items(), 1);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
    }

    #[test]
    fn removed_batch_deletes_contiguous_block_with_one_splice() {
        // 整块连续命中应合并成一次 splice：items-changed 只发一条，
        // 而不是每个 uri 一条（这是「整相册被删」时避免刷屏/重建的关键）。
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
            item(3, "file:///tmp/c.jpg"),
            item(4, "file:///tmp/d.jpg"),
        ]);
        let signal = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let signal_for_cb = signal.clone();
        list.connect_items_changed(move |_, position, removed, added| {
            signal_for_cb.borrow_mut().push((position, removed, added));
        });

        apply_to_media_list(
            &list,
            &DomainEvent::MediaRemoved {
                source: ChangeSource::StartupScan,
                ids: Vec::new(),
                uris: vec![
                    "file:///tmp/b.jpg".into(),
                    "file:///tmp/c.jpg".into(),
                    // 一个未命中项，确保它不造成额外扫描或日志
                    "file:///tmp/elsewhere.jpg".into(),
                ],
            },
        );

        assert_eq!(list.n_items(), 2);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/d.jpg");
        assert_eq!(
            signal.borrow().as_slice(),
            &[(1, 2, 0)],
            "a contiguous removal block must coalesce into one items-changed emission"
        );
    }

    #[test]
    fn removed_batch_deletes_scattered_items_preserving_order() {
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
            item(3, "file:///tmp/c.jpg"),
            item(4, "file:///tmp/d.jpg"),
            item(5, "file:///tmp/e.jpg"),
        ]);

        apply_to_media_list(
            &list,
            &DomainEvent::MediaRemoved {
                source: ChangeSource::FilesystemWatcher,
                ids: Vec::new(),
                uris: vec![
                    "file:///tmp/a.jpg".into(),
                    "file:///tmp/c.jpg".into(),
                    "file:///tmp/e.jpg".into(),
                ],
            },
        );

        assert_eq!(list.n_items(), 2);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/b.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/d.jpg");
    }

    #[test]
    fn removed_batch_is_noop_and_emits_nothing_when_none_match() {
        // 大批量 MediaRemoved 但可见 list 里一个都不命中：必须零修改、零 items-changed。
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
        ]);
        let signal = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let signal_for_cb = signal.clone();
        list.connect_items_changed(move |_, position, removed, added| {
            signal_for_cb.borrow_mut().push((position, removed, added));
        });

        let uris: Vec<String> = (0..50_000)
            .map(|i| format!("file:///tmp/gone/{i}.jpg"))
            .collect();
        apply_to_media_list(
            &list,
            &DomainEvent::MediaRemoved {
                source: ChangeSource::StartupScan,
                ids: Vec::new(),
                uris,
            },
        );

        assert_eq!(list.n_items(), 2, "no visible item matched, list unchanged");
        assert!(
            signal.borrow().is_empty(),
            "a no-op removal must not emit any items-changed (no per-uri churn)"
        );
    }
}
