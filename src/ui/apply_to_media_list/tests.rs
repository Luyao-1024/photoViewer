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
    let source = include_str!("../apply_to_media_list.rs");
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
