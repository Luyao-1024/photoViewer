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
mod tests;
