use super::render::{build_photo_picture, sync_flow_child_visibility_for_tile};
use super::{
    extract_items, find_media_item_by_uri, gray_placeholder_texture, library_stats_text,
    media_item_at_displayed_index, section_key_for_item, should_show_library_stats, spec_for_mode,
    thumbnail_request_mtime, DisplayedItem, MediaGrid, ViewSpec,
};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::section_model::{apply_authoritative_counts, group_items, SectionKey};
use crate::core::thumbnails::{LoadedThumb, ThumbnailLoader};
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{gio, glib, prelude::*};
use std::rc::Rc;
use std::sync::Arc;

impl MediaGrid {
    pub(super) fn connect_model_changes(&self, media_list: &gio::ListStore) {
        let weak = self.downgrade();
        media_list.connect_items_changed(move |list, position, removed, added| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.handle_items_changed(list, position, removed, added);
        });
    }

    fn handle_items_changed(&self, list: &gio::ListStore, position: u32, removed: u32, added: u32) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "GRID_MODEL_TRACE model_changed mode={:?} active={} dirty={} applying_virtual_page={} position={} removed={} added={} list_len={}",
            self.mode(),
            self.imp().active.get(),
            self.imp().dirty_model.get(),
            self.imp().applying_virtual_page.get(),
            position,
            removed,
            added,
            list.n_items()
        );
        *self.imp().media_list.borrow_mut() = Some(list.clone());
        if self.imp().active.get() {
            if self.imp().applying_virtual_page.get() {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "GRID_MODEL_TRACE action=ignore_virtual_page mode={:?} position={} removed={} added={} list_len={}",
                    self.mode(),
                    position,
                    removed,
                    added,
                    list.n_items()
                );
                return;
            }
            let was_empty = list.n_items().saturating_sub(added) == 0;
            if removed > 0 && added == 0 {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "GRID_MODEL_TRACE action=incremental_removal mode={:?} position={} removed={} list_len={}",
                    self.mode(),
                    position,
                    removed,
                    list.n_items()
                );
                self.apply_incremental_removal(position, removed, list);
            } else if removed > 0 {
                self.invalidate_library_metadata();
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "GRID_MODEL_TRACE action=rebuild_immediate reason=replacement mode={:?} position={} removed={} added={} list_len={}",
                    self.mode(),
                    position,
                    removed,
                    added,
                    list.n_items()
                );
                // Replacements can reorder or remap existing children, so rebuild.
                self.rebuild_immediately(list.clone());
            } else if added > 0 && was_empty {
                self.invalidate_library_metadata();
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "GRID_MODEL_TRACE action=rebuild_immediate reason=first_non_empty mode={:?} position={} added={} list_len={}",
                    self.mode(),
                    position,
                    added,
                    list.n_items()
                );
                // 首次启动扫描从空库追加第一批媒体时，Day grid 已经按空列表
                // 构建过；必须立即重建，否则空态切回 Day 后 tile/统计仍为空。
                self.rebuild_immediately(list.clone());
            } else if added > 0 {
                self.invalidate_library_metadata();
                if self.apply_incremental_addition(position, added, list) {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "GRID_MODEL_TRACE action=incremental_addition mode={:?} position={} added={} list_len={}",
                        self.mode(),
                        position,
                        added,
                        list.n_items()
                    );
                } else {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "GRID_MODEL_TRACE action=schedule_rebuild reason=pure_add_fallback mode={:?} position={} added={} list_len={}",
                        self.mode(),
                        position,
                        added,
                        list.n_items()
                    );
                    // 启动扫描和 watcher 的纯新增事件可能把较新的项目插入到当前窗口
                    // 前部。去抖重建以吸收批量扫描突发，同时避免每个新增信号都同步
                    // 拆/建 FlowBox。
                    self.schedule_rebuild(list.clone());
                }
            }
        } else {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "GRID_MODEL_TRACE action=mark_dirty_inactive mode={:?} position={} removed={} added={} list_len={}",
                self.mode(),
                position,
                removed,
                added,
                list.n_items()
            );
            self.imp().dirty_model.set(true);
        }
    }

    pub(super) fn apply_incremental_removal(
        &self,
        position: u32,
        removed: u32,
        media_list: &gio::ListStore,
    ) {
        if removed == 0 {
            return;
        }

        let removed_end = position.saturating_add(removed);
        let mut removed_items = Vec::new();
        {
            let mut displayed = self.imp().displayed_items.borrow_mut();
            let mut kept = Vec::with_capacity(displayed.len());
            for mut item in displayed.drain(..) {
                if item.window_index >= position && item.window_index < removed_end {
                    removed_items.push(item);
                } else {
                    if item.window_index >= removed_end {
                        item.window_index = item.window_index.saturating_sub(removed);
                    }
                    kept.push(item);
                }
            }
            *displayed = kept;
        }

        let mut selection_changed = false;
        {
            let mut selected = self.imp().selected.borrow_mut();
            for item in &removed_items {
                selection_changed |= selected.remove(&item.media_id);
            }
        }

        self.decrement_cached_metadata_after_removal(removed, &removed_items);

        for item in &removed_items {
            self.remove_displayed_child(&item.flow_child);
        }
        self.refresh_section_header_labels(media_list);
        self.refresh_stats_label_after_cached_change();

        if selection_changed {
            self.fire_selection_changed();
        }
        self.reprioritize_visible();
    }

    pub(super) fn apply_incremental_addition(
        &self,
        position: u32,
        added: u32,
        media_list: &gio::ListStore,
    ) -> bool {
        self.apply_incremental_addition_inner(position, added, media_list, true, true)
    }

    pub(super) fn apply_progressive_render_addition(
        &self,
        position: u32,
        added: u32,
        media_list: &gio::ListStore,
    ) -> bool {
        self.apply_incremental_addition_inner(position, added, media_list, false, false)
    }

    fn apply_incremental_addition_inner(
        &self,
        position: u32,
        added: u32,
        media_list: &gio::ListStore,
        update_cached_metadata: bool,
        defer_uncached: bool,
    ) -> bool {
        if added == 0 || self.imp().flat_sections.get() {
            return false;
        }

        let mut inserted_items = Vec::with_capacity(added as usize);
        for offset in 0..added {
            let Some(obj) = media_list.item(position + offset) else {
                return false;
            };
            let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
                return false;
            };
            inserted_items.push((*boxed.borrow::<MediaItem>()).clone());
        }
        if inserted_items.is_empty() {
            return false;
        }

        let mode = self.mode();
        let section_key = section_key_for_item(&inserted_items[0], mode);
        if inserted_items
            .iter()
            .any(|item| section_key_for_item(item, mode) != section_key)
        {
            return false;
        }

        let flow = {
            let displayed = self.imp().displayed_items.borrow();
            displayed
                .iter()
                .find(|item| item.section_key == section_key)
                .and_then(|item| item.flow_child.parent())
                .and_downcast::<gtk::FlowBox>()
        };
        let Some(flow) = flow else {
            return false;
        };

        let insert_child_index = {
            let displayed = self.imp().displayed_items.borrow();
            displayed
                .iter()
                .filter(|item| item.section_key == section_key)
                .filter(|item| item.window_index >= position)
                .filter_map(|item| item.flow_child.index().try_into().ok())
                .min()
                .unwrap_or_else(|| flow.observe_children().n_items())
        };

        {
            let mut displayed = self.imp().displayed_items.borrow_mut();
            for item in displayed.iter_mut() {
                if item.window_index >= position {
                    item.window_index = item.window_index.saturating_add(added);
                }
            }
        }

        let spec = spec_for_mode(mode);
        let loader = self
            .imp()
            .loader
            .get()
            .expect("MediaGrid::apply_incremental_addition called before new()")
            .clone();
        let on_bg = self
            .imp()
            .on_background_changed
            .get()
            .expect("MediaGrid::apply_incremental_addition called before new()")
            .clone();

        let mut new_displayed = Vec::with_capacity(inserted_items.len());
        let mut inserted_ready = 0u32;
        for (offset, item) in inserted_items.into_iter().enumerate() {
            let window_index = position + offset as u32;
            let media_id = MediaId::from(item.id);
            let item_mtime = thumbnail_request_mtime(&item);
            if defer_uncached
                && loader
                    .try_load_cached(&item.uri, spec.thumb_size, Some(item_mtime))
                    .is_none()
            {
                self.defer_incremental_addition_until_thumbnail_ready(
                    item,
                    media_list.clone(),
                    spec,
                    loader.clone(),
                    on_bg.clone(),
                );
                continue;
            }
            let picture = build_photo_picture(
                spec,
                item,
                media_list.clone(),
                window_index,
                loader.clone(),
                on_bg.clone(),
                true,
            );
            flow.insert(&picture, insert_child_index as i32 + inserted_ready as i32);
            let Some(flow_child) = picture
                .parent()
                .and_then(|w| w.downcast::<gtk::FlowBoxChild>().ok())
            else {
                return false;
            };
            sync_flow_child_visibility_for_tile(&picture, &flow_child);
            new_displayed.push(DisplayedItem {
                flow_child,
                window_index,
                media_id,
                section_key: section_key.clone(),
            });
            inserted_ready = inserted_ready.saturating_add(1);
        }

        {
            let mut displayed = self.imp().displayed_items.borrow_mut();
            displayed.extend(new_displayed);
            displayed.sort_by_key(|item| item.window_index);
        }

        if update_cached_metadata {
            self.increment_cached_metadata_after_addition(added, &section_key);
        }
        self.refresh_section_header_labels(media_list);
        self.refresh_stats_label_after_cached_change();
        self.apply_selection_mode();
        self.reprioritize_visible();
        true
    }

    fn defer_incremental_addition_until_thumbnail_ready(
        &self,
        item: MediaItem,
        media_list: gio::ListStore,
        spec: ViewSpec,
        loader: Arc<ThumbnailLoader>,
        on_background_changed: Rc<dyn Fn()>,
    ) {
        let item_uri = item.uri.clone();
        let item_name = item.display_name().to_string();
        let item_mtime = thumbnail_request_mtime(&item);
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request_for_media(
            item.id,
            item_uri.clone(),
            spec.thumb_size,
            Some(item_mtime),
            tx,
            crate::core::thumbnails::TIER_BOOST,
        );
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "THUMB_TILE_TRACE deferred_incremental_insert_wait item_id={} item_name={} uri={} mode={:?} size={:?}",
            item.id,
            item_name,
            item_uri,
            spec.mode,
            spec.thumb_size
        );

        let weak = self.downgrade();
        gtk::glib::spawn_future_local(async move {
            let ready = rx.await.ok();
            let Some(grid) = weak.upgrade() else {
                return;
            };
            grid.insert_deferred_incremental_item(
                media_list,
                item_uri,
                spec,
                loader,
                on_background_changed,
                ready,
            );
        });
    }

    pub(super) fn insert_deferred_incremental_item(
        &self,
        media_list: gio::ListStore,
        item_uri: String,
        spec: ViewSpec,
        loader: Arc<ThumbnailLoader>,
        on_background_changed: Rc<dyn Fn()>,
        ready: Option<LoadedThumb>,
    ) {
        if !self.imp().active.get() {
            self.imp().dirty_model.set(true);
            return;
        }
        if self.imp().displayed_items.borrow().iter().any(|displayed| {
            media_item_at_displayed_index(&media_list, displayed.window_index)
                .is_some_and(|item| item.uri == item_uri)
        }) {
            return;
        }

        let Some((window_index, item)) = find_media_item_by_uri(&media_list, &item_uri) else {
            return;
        };
        let section_key = section_key_for_item(&item, spec.mode);
        let flow = {
            let displayed = self.imp().displayed_items.borrow();
            displayed
                .iter()
                .find(|item| item.section_key == section_key)
                .and_then(|item| item.flow_child.parent())
                .and_downcast::<gtk::FlowBox>()
        };
        let Some(flow) = flow else {
            self.schedule_rebuild(media_list);
            return;
        };
        let insert_child_index = {
            let displayed = self.imp().displayed_items.borrow();
            displayed
                .iter()
                .filter(|item| item.section_key == section_key)
                .filter(|item| item.window_index >= window_index)
                .filter_map(|item| item.flow_child.index().try_into().ok())
                .min()
                .unwrap_or_else(|| flow.observe_children().n_items())
        };

        let media_id = MediaId::from(item.id);
        let picture = build_photo_picture(
            spec,
            item.clone(),
            media_list.clone(),
            window_index,
            loader,
            on_background_changed.clone(),
            true,
        );
        let insert_succeeded = ready.is_some();
        match ready {
            Some(loaded) => {
                if let Some(is_light) = loaded.is_light {
                    picture.set_background_is_light(is_light);
                    on_background_changed();
                }
                picture.set_paintable(Some(&loaded.texture));
            }
            None => {
                picture.set_paintable(Some(&gray_placeholder_texture()));
            }
        }
        flow.insert(&picture, insert_child_index as i32);
        let Some(flow_child) = picture
            .parent()
            .and_then(|w| w.downcast::<gtk::FlowBoxChild>().ok())
        else {
            return;
        };
        sync_flow_child_visibility_for_tile(&picture, &flow_child);
        {
            let mut displayed = self.imp().displayed_items.borrow_mut();
            if displayed.iter().any(|item| item.media_id == media_id) {
                flow.remove(&flow_child);
                return;
            }
            displayed.push(DisplayedItem {
                flow_child,
                window_index,
                media_id,
                section_key,
            });
            displayed.sort_by_key(|item| item.window_index);
        }
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "THUMB_TILE_TRACE deferred_incremental_insert_ready item_id={} uri={} mode={:?} global_index={} success={}",
            item.id,
            item_uri,
            spec.mode,
            window_index,
            insert_succeeded
        );
        self.apply_selection_mode();
        self.refresh_stats_label_after_cached_change();
        self.reprioritize_visible();
    }

    fn increment_cached_metadata_after_addition(&self, added: u32, section_key: &SectionKey) {
        self.imp()
            .virtual_total
            .set(self.imp().virtual_total.get().saturating_add(added));
        if let Some(total) = self.imp().library_total_snapshot.get() {
            self.imp()
                .library_total_snapshot
                .set(Some(total.saturating_add(added)));
        }
        if let Some(mut stats) = self.imp().library_stats_snapshot.get() {
            stats.live_total = stats.live_total.saturating_add(added as usize);
            self.imp().library_stats_snapshot.set(Some(stats));
        }

        let mode = self.mode();
        if let Some(counts) = self
            .imp()
            .section_count_snapshots
            .borrow_mut()
            .get_mut(&mode)
        {
            let next = counts
                .get(section_key)
                .copied()
                .unwrap_or_default()
                .saturating_add(added);
            counts.insert(section_key.clone(), next);
        }
    }

    fn decrement_cached_metadata_after_removal(
        &self,
        removed: u32,
        removed_items: &[DisplayedItem],
    ) {
        self.imp()
            .virtual_total
            .set(self.imp().virtual_total.get().saturating_sub(removed));
        if let Some(total) = self.imp().library_total_snapshot.get() {
            self.imp()
                .library_total_snapshot
                .set(Some(total.saturating_sub(removed)));
        }
        if let Some(mut stats) = self.imp().library_stats_snapshot.get() {
            stats.live_total = stats.live_total.saturating_sub(removed as usize);
            stats.thumbnails_generated = stats.thumbnails_generated.min(stats.live_total);
            self.imp().library_stats_snapshot.set(Some(stats));
        }

        let mode = self.mode();
        if let Some(counts) = self
            .imp()
            .section_count_snapshots
            .borrow_mut()
            .get_mut(&mode)
        {
            for item in removed_items {
                let next = counts
                    .get(&item.section_key)
                    .copied()
                    .unwrap_or_default()
                    .saturating_sub(1);
                if next == 0 {
                    counts.remove(&item.section_key);
                } else {
                    counts.insert(item.section_key.clone(), next);
                }
            }
        }
    }

    fn remove_displayed_child(&self, child: &gtk::FlowBoxChild) {
        let Some(flow) = child.parent().and_downcast::<gtk::FlowBox>() else {
            return;
        };
        flow.remove(child);
        if flow.observe_children().n_items() > 0 {
            return;
        }

        let header = flow.prev_sibling().and_then(|widget| {
            widget
                .downcast::<gtk::Label>()
                .ok()
                .filter(|label| label.has_css_class("heading"))
                .map(|label| label.upcast::<gtk::Widget>())
        });
        let Some(content) = flow.parent().and_downcast::<gtk::Box>() else {
            return;
        };
        content.remove(&flow);
        if let Some(header) = header {
            content.remove(&header);
        }
    }

    fn refresh_section_header_labels(&self, media_list: &gio::ListStore) {
        if self.imp().flat_sections.get() {
            return;
        }

        let mut items = extract_items(media_list);
        let max_items = self.imp().rendered_limit.get();
        if items.len() > max_items {
            items.truncate(max_items);
        }
        let mut sections = group_items(&items, self.mode());
        if self.imp().full_library_context.get() {
            if let Some(counts) = self
                .imp()
                .section_count_snapshots
                .borrow()
                .get(&self.mode())
            {
                apply_authoritative_counts(&mut sections, counts);
            }
        }

        let mut section_iter = sections.into_iter();
        let content = self.imp().content.get();
        let mut child = content.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                if label.has_css_class("heading") {
                    if let Some(section) = section_iter.next() {
                        label.set_label(&section.label);
                    }
                }
            }
            child = next;
        }
    }

    pub(super) fn refresh_stats_label_after_cached_change(&self) {
        let Some(stats) = self.imp().library_stats_snapshot.get() else {
            return;
        };
        let Some(label) = self.imp().stats_label.borrow().as_ref().cloned() else {
            return;
        };
        if should_show_library_stats(&stats) {
            label.set_label(&library_stats_text(
                stats.live_total,
                stats.thumbnails_generated,
            ));
            return;
        }
        if let Some(parent) = label.parent().and_downcast::<gtk::Box>() {
            parent.remove(&label);
        }
        self.imp().stats_label.borrow_mut().take();
        if let Some(source) = self.imp().stats_refresh_source.borrow_mut().take() {
            source.remove();
        }
    }
}

#[cfg(test)]
mod tests;
