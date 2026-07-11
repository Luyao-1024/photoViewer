use super::render::{
    build_photo_picture, prepare_reused_tile, sync_flow_child_visibility_for_tile,
};
use super::virtual_paging::{
    build_virtual_placeholder_flow, estimated_virtual_columns, virtual_spacer,
    virtual_spacer_height, virtual_window_item_count,
};
use super::{
    build_library_stats_label, extract_items, find_media_item_by_uri, gray_placeholder_texture,
    library_stats_text, media_item_at_displayed_index, runtime_config, section_key_for_item,
    should_show_library_stats, spec_for_mode, thumbnail_request_mtime, uri_index_map,
    DisplayedItem, MediaGrid, ViewSpec,
};
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::section_model::{
    apply_authoritative_counts, group_items, GroupBy, MediaSection, SectionKey,
};
use crate::core::thumbnails::{LoadedThumb, ThumbnailLoader};
use crate::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{gio, glib, prelude::*};
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};
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

    /// Tear down the current sections and rebuild them for `mode`.
    #[tracing::instrument(name = "grid:rebuild", skip(self, media_list), fields(source_len = media_list.n_items()))]
    pub(super) fn rebuild(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        let source_len = media_list.n_items();
        // Progressive first render: on this grid's FIRST rebuild, cap
        // rendered_limit at a viewport-sized seed so the first paint builds only
        // ~seed tiles. The same runtime plan also controls the model-window cap
        // used by album loading.
        let mut progressive_just_seeded = false;
        if self.imp().progressive_render_pending.replace(false) {
            let steady_limit = self.imp().rendered_limit.get();
            let plan = runtime_config::progressive_render_plan(
                runtime_config::startup_progressive_render(),
                runtime_config::startup_render_seed(),
                source_len as usize,
                steady_limit,
            );
            if plan.progressive {
                self.imp().rendered_limit.set(plan.first_render_limit);
                progressive_just_seeded = true;
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "PROGRESSIVE_RENDER seed mode={:?} seed={} source_len={} model_limit={}",
                    mode,
                    plan.first_render_limit,
                    source_len,
                    plan.model_limit
                );
            }
        }
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "GRID_MODEL_TRACE rebuild_start mode={:?} source_len={} active={} dirty={} rendered_limit={}",
            mode,
            source_len,
            self.imp().active.get(),
            self.imp().dirty_model.get(),
            self.imp().rendered_limit.get()
        );
        let loader = self
            .imp()
            .loader
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_activate = self
            .imp()
            .on_activate
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_add_to_album = self
            .imp()
            .on_add_to_album
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_move_to_trash = self
            .imp()
            .on_move_to_trash
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_set_favorite = self
            .imp()
            .on_set_favorite
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_query_favorite_state = self
            .imp()
            .on_query_favorite_state
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_set_album_cover = self
            .imp()
            .on_set_album_cover
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let enable_context_menu = self.imp().enable_context_menu.get();

        let spec = spec_for_mode(mode);

        // 重建前保存滚动位置，避免因清空/重建子 widget 导致 adjustment.value 归零。
        let saved_scroll = self.imp().scroller.get().vadjustment().value();

        let content = self.imp().content.get();
        if let Some(source) = self.imp().stats_refresh_source.borrow_mut().take() {
            source.remove();
        }
        self.imp().stats_label.borrow_mut().take();
        let mut reusable_tiles = self.detach_reusable_loaded_tiles();
        {
            let clear_span = tracing::info_span!(
                target: crate::core::log_targets::BROWSING,
                "grid:clear_content",
                mode = ?mode
            );
            let _clear = clear_span.enter();
            // Clear any previously built sections.
            while let Some(child) = content.first_child() {
                content.remove(&child);
            }
        }
        self.imp().displayed_items.borrow_mut().clear();

        // Extract MediaItems + a uri→global-index lookup from the store.
        let mut items = {
            let extract_span = tracing::info_span!("grid:extract_items");
            let _extract = extract_span.enter();
            extract_items(&media_list)
        };
        let max_items = self.imp().rendered_limit.get();
        if items.len() > max_items {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "MediaGrid::rebuild limiting rendered items mode={:?} total={} rendered={}",
                mode,
                items.len(),
                max_items
            );
            items.truncate(max_items);
        }
        let rendered_item_count = items.len() as u32;
        let uri_to_index = uri_index_map(&media_list);
        let total_media_count = if self.imp().full_library_context.get() {
            self.imp()
                .library_total_snapshot
                .get()
                .unwrap_or(source_len)
                .max(source_len)
        } else {
            source_len
        };
        self.imp().virtual_total.set(total_media_count);
        self.ensure_library_metadata_async(loader.clone(), mode);
        let is_loading_virtual_window = self.imp().virtual_page_loading.get();
        let loading_placeholder_count = if is_loading_virtual_window {
            virtual_window_item_count(
                self.imp().virtual_window_start.get(),
                total_media_count,
                runtime_config::virtual_media_page_size(),
            )
        } else {
            0
        };
        let effective_window_len = if loading_placeholder_count > 0 {
            loading_placeholder_count
        } else {
            items.len() as u32
        };
        let window_start = self
            .imp()
            .virtual_window_start
            .get()
            .min(total_media_count.saturating_sub(effective_window_len));
        self.imp().virtual_window_start.set(window_start);
        let viewport_width = self.imp().scroller.get().allocated_width().max(1) as f64;
        let virtual_columns = estimated_virtual_columns(viewport_width.max(1000.0), spec);
        let top_spacer_height =
            virtual_spacer_height(window_start, virtual_columns, viewport_width, spec);
        if top_spacer_height > 0 {
            content.append(&virtual_spacer(top_spacer_height));
        }

        let total_media = total_media_count as usize;
        if self.imp().full_library_context.get()
            && mode == GroupBy::Day
            && (loading_placeholder_count > 0 || !items.is_empty())
        {
            if let Some(stats) = self.imp().library_stats_snapshot.get() {
                if should_show_library_stats(&stats) {
                    let stats_label = build_library_stats_label(stats);
                    content.append(&stats_label);
                    *self.imp().stats_label.borrow_mut() = Some(stats_label.clone());
                    self.start_stats_refresh(loader.clone(), total_media);
                }
            }
        }

        // Group by year/month/day, then emit header + FlowBox per section.
        let mut section_count = 0u32;
        let mut photo_count = 0u32;
        let mut displayed_items = Vec::new();
        let flat_sections = self.imp().flat_sections.get();
        {
            let build_sections_span = tracing::info_span!(
                target: crate::core::log_targets::BROWSING,
                "grid:build_sections",
                mode = ?mode,
                flat_sections,
                loading_placeholder_count,
                sections = tracing::field::Empty,
                photos = tracing::field::Empty
            );
            let _build_sections = build_sections_span.enter();
            if loading_placeholder_count > 0 {
                content.append(&build_virtual_placeholder_flow(
                    spec,
                    loading_placeholder_count,
                ));
                section_count = 1;
                photo_count = loading_placeholder_count;
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIRTUAL_SCROLL placeholder_window mode={:?} start={} count={} total={}",
                    mode,
                    window_start,
                    loading_placeholder_count,
                    total_media_count
                );
            } else {
                let input_len = items.len();
                let group_span = tracing::info_span!(
                    target: crate::core::log_targets::BROWSING,
                    "grid:group_sections",
                    mode = ?mode,
                    flat_sections,
                    input_len
                );
                let mut sections = {
                    let _group = group_span.enter();
                    if flat_sections {
                        vec![MediaSection {
                            key: SectionKey {
                                year: None,
                                month: None,
                                day: None,
                            },
                            label: String::new(),
                            items,
                        }]
                    } else {
                        group_items(&items, mode)
                    }
                };
                // section 头部计数改用整个库的 DB 聚合，而非当前虚拟分页窗口切片。
                // 窗口受 virtual_media_page_size（默认 500）截断，否则一个实际几千张的
                // 年份只会显示窗口里的 500。窗口只决定渲染哪些缩略图，不影响真实计数。
                if !flat_sections && self.imp().full_library_context.get() {
                    if let Some(counts) = self.imp().section_count_snapshots.borrow().get(&mode) {
                        apply_authoritative_counts(&mut sections, counts);
                    }
                }
                for section in sections {
                    if section.items.is_empty() {
                        continue;
                    }

                    if !flat_sections {
                        // Full-width section header.
                        let header = gtk::Label::builder()
                            .label(&section.label)
                            .halign(gtk::Align::Start)
                            .margin_start(12)
                            .margin_top(12)
                            .margin_bottom(6)
                            .xalign(0.0)
                            .css_classes(["heading"])
                            .build();
                        content.append(&header);
                    }

                    // FlowBox of square thumbnails. `homogeneous` makes every cell the
                    // same size; with each picture's `set_size_request(target)` the
                    // cells become target×target squares. `column/row spacing` is the
                    // thin separator (≤3px); hover styling lives in grid_css.
                    //
                    // `selection_mode` starts at `None` and is flipped to
                    // `Multiple` only while multi-select is active (see
                    // `apply_selection_mode`). `None` stops the FlowBox from
                    // tracking its own selection, so no child can become
                    // `:selected` — and reveal the `.thumb-checkmark` — outside
                    // explicit multi-select. We mirror selection into
                    // `imp.selected` for our own bookkeeping; the focus ring
                    // (driven by `:hover` / `:focus` in grid_css) is unchanged.
                    let flow = gtk::FlowBox::builder()
                        .orientation(gtk::Orientation::Horizontal)
                        .homogeneous(true)
                        .column_spacing(8)
                        .row_spacing(8)
                        .max_children_per_line(100)
                        .selection_mode(gtk::SelectionMode::None)
                        .build();
                    flow.set_activate_on_single_click(true);
                    flow.add_css_class("thumb-grid");
                    // While arrow-keying between tiles, hide the `:hover` hint so the
                    // highlight follows the keyboard focus, not the resting pointer.
                    crate::ui::grid_css::attach_kbd_nav(&flow);

                    // Build tiles and remember each child's stable id/current store
                    // index. Activation and context menus read this live mapping so
                    // incremental child removals cannot leave stale index arrays.
                    for item in &section.items {
                        let gi = uri_to_index.get(&item.uri).copied().unwrap_or(u32::MAX);
                        let media_id = MediaId::from(item.id);
                        let on_bg = self
                            .imp()
                            .on_background_changed
                            .get()
                            .expect("MediaGrid::rebuild called before new()")
                            .clone();
                        // Only reuse a rescued tile if it is fully detached. GTK
                        // toggle-ref finalization of the old FlowBoxChild can lag,
                        // leaving the tile parented when we re-append it — that
                        // trips `gtk_flow_box_child_set_child` and the tile never
                        // attaches (blank), which is the fast-scroll "stuck" bug.
                        // A still-parented tile falls through to a fresh build.
                        let reused = reusable_tiles
                            .remove(&media_id)
                            .filter(|t| t.parent().is_none());
                        let picture = if let Some(tile) = reused {
                            prepare_reused_tile(&tile, spec, item);
                            tile
                        } else {
                            build_photo_picture(
                                spec,
                                item.clone(),
                                media_list.clone(),
                                gi,
                                loader.clone(),
                                on_bg,
                                false,
                            )
                        };
                        flow.append(&picture);
                        if let Some(flow_child) = flow
                            .last_child()
                            .and_then(|w| w.downcast::<gtk::FlowBoxChild>().ok())
                        {
                            sync_flow_child_visibility_for_tile(&picture, &flow_child);
                            if gi != u32::MAX {
                                displayed_items.push(DisplayedItem {
                                    flow_child: flow_child.clone(),
                                    window_index: gi,
                                    media_id,
                                    section_key: section.key.clone(),
                                });
                            }
                        }
                        photo_count += 1;
                    }

                    // Activation: FlowBox child-activated → look up stable media id.
                    // Only explicit multi-select mode (entered via right-click “Multi-select”)
                    // toggles selection; otherwise the item opens in viewer.
                    let on_act = on_activate.clone();
                    let weak = self.downgrade();
                    let section_label_for_activation = section.label.clone();
                    flow.connect_child_activated(move |flow, child| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let idx = child.index();
                let Some(displayed_item) = this.displayed_item_for_child(child) else {
                    return;
                };
                let media_id = displayed_item.media_id;
                let gi = displayed_item.window_index;
                let current_item = this.media_item_for_id(media_id);
                let item_id = current_item.as_ref().map(|item| item.id).unwrap_or(-1);
                let item_name = current_item
                    .as_ref()
                    .map(|item| item.display_name().to_string())
                    .unwrap_or_else(|| "(missing)".into());
                let item_uri = current_item
                    .as_ref()
                    .map(|item| item.uri.clone())
                    .unwrap_or_else(|| "(missing)".into());
                let is_multi = this.is_multi_select_mode();
                let displayed_indices = this.displayed_indices();
                let displayed_pos = displayed_indices.iter().position(|index| *index == gi);
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIEWER_TRACE grid_activate mode={:?} section={} section_child_index={} global_index={} displayed_pos={:?} displayed_len={} displayed_first={:?} displayed_last={:?} item_id={} item_name={} item_uri={} multi_select={}",
                    this.mode(),
                    section_label_for_activation,
                    idx,
                    gi,
                    displayed_pos,
                    displayed_indices.len(),
                    displayed_indices.first(),
                    displayed_indices.last(),
                    item_id,
                    item_name.as_str(),
                    item_uri.as_str(),
                    is_multi
                );
                if is_multi {
                    // `flow` comes from the signal arg, no extra upgrade needed.
                    this.toggle_selection(media_id, child, flow);
                } else {
                    on_act(media_id);
                }
            });

                    if enable_context_menu {
                        let weak_for_context = self.downgrade();
                        let flow_for_ctx = flow.clone();
                        let on_add_to_album_ctx = on_add_to_album.clone();
                        let on_move_to_trash_ctx = on_move_to_trash.clone();
                        let on_set_favorite_ctx = on_set_favorite.clone();
                        let on_query_favorite_state_ctx = on_query_favorite_state.clone();
                        let on_set_album_cover_ctx = on_set_album_cover.clone();
                        let gesture = gtk::GestureClick::new();
                        gesture.set_button(3);
                        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

                        gesture.connect_released(move |gesture, _n_press, x, y| {
                            if gesture.current_button() != 3 {
                                return;
                            }
                            let Some(this) = weak_for_context.upgrade() else {
                                return;
                            };

                            let Some(flow_child_for_ctx) =
                                flow_for_ctx.child_at_pos(x as i32, y as i32)
                            else {
                                return;
                            };

                            let Some(displayed_item) =
                                this.displayed_item_for_child(&flow_child_for_ctx)
                            else {
                                return;
                            };
                            let media_id = displayed_item.media_id;

                            let in_multi_mode = this.is_multi_select_mode();
                            let target_indices = if in_multi_mode {
                                this.ensure_context_selection(
                                    &flow_for_ctx,
                                    &flow_child_for_ctx,
                                    media_id,
                                )
                            } else {
                                vec![media_id]
                            };
                            let favorite_state =
                                (on_query_favorite_state_ctx)(target_indices.clone());

                            let Some(context_overlay) =
                                this.imp().context_menu_overlay.borrow().clone()
                            else {
                                return;
                            };
                            let mut items = Vec::new();

                            // Multi-select / Exit Multi-select.
                            if in_multi_mode {
                                let weak_exit = weak_for_context.clone();
                                items.push(GlassMenuItem::new(
                                    tr("photos.batch.exit_multi_select"),
                                    GlassMenuItemKind::Danger,
                                    move || {
                                        if let Some(this) = weak_exit.upgrade() {
                                            this.set_multi_select_mode(false);
                                        }
                                    },
                                ));
                            } else {
                                let weak_enter = weak_for_context.clone();
                                let flow_for_ctx_enter = flow_for_ctx.clone();
                                let flow_child_for_ctx_enter = flow_child_for_ctx.clone();
                                items.push(GlassMenuItem::new(
                                    tr("photos.batch.multi_select"),
                                    GlassMenuItemKind::Suggested,
                                    move || {
                                        if let Some(this) = weak_enter.upgrade() {
                                            this.set_multi_select_mode(true);
                                            this.ensure_context_selection(
                                                &flow_for_ctx_enter,
                                                &flow_child_for_ctx_enter,
                                                media_id,
                                            );
                                        }
                                    },
                                ));
                            }

                            // Favorite / Unfavorite (single and batch context).
                            if favorite_state.can_favorite {
                                let indices_for_fav = target_indices.clone();
                                let on_set_favorite_fav = on_set_favorite_ctx.clone();
                                items.push(GlassMenuItem::new(
                                    tr("photos.batch.favorite"),
                                    GlassMenuItemKind::Normal,
                                    move || {
                                        on_set_favorite_fav(indices_for_fav.clone(), true);
                                    },
                                ));
                            }
                            if favorite_state.can_unfavorite {
                                let indices_for_unfav = target_indices.clone();
                                let on_set_favorite_unfav = on_set_favorite_ctx.clone();
                                items.push(GlassMenuItem::new(
                                    tr("photos.batch.unfavorite"),
                                    GlassMenuItemKind::Normal,
                                    move || {
                                        on_set_favorite_unfav(indices_for_unfav.clone(), false);
                                    },
                                ));
                            }

                            if !target_indices.is_empty() {
                                if !in_multi_mode {
                                    if let Some(on_set_album_cover_ctx) =
                                        on_set_album_cover_ctx.clone()
                                    {
                                        items.push(GlassMenuItem::new(
                                            tr("album.context.set_cover"),
                                            GlassMenuItemKind::Normal,
                                            move || {
                                                on_set_album_cover_ctx(media_id);
                                            },
                                        ));
                                    }
                                }

                                let indices_for_album = target_indices.clone();
                                let on_add_to_album_ctx = on_add_to_album_ctx.clone();
                                items.push(GlassMenuItem::new(
                                    tr("photos.batch.move_to_album"),
                                    GlassMenuItemKind::Normal,
                                    move || {
                                        on_add_to_album_ctx(indices_for_album.clone());
                                    },
                                ));

                                let indices_for_trash = target_indices.clone();
                                let on_move_to_trash_ctx = on_move_to_trash_ctx.clone();
                                let grid_weak = this.downgrade();
                                items.push(GlassMenuItem::new(
                                    tr("viewer.tooltip.move_to_trash"),
                                    GlassMenuItemKind::Danger,
                                    move || {
                                        let count = indices_for_trash.len();
                                        let body = if count == 1 {
                                            tr("trash.confirm_body_one")
                                        } else {
                                            tr("trash.confirm_body_many")
                                                .replace("{count}", &count.to_string())
                                        };
                                        let dialog = adw::AlertDialog::builder()
                                            .heading(tr("trash.confirm_title"))
                                            .body(body)
                                            .build();
                                        dialog.add_css_class("glass-alert-dialog");
                                        dialog.add_response("cancel", &tr("dialog.cancel"));
                                        dialog.add_response("trash", &tr("dialog.trash"));
                                        dialog.set_response_appearance(
                                            "trash",
                                            adw::ResponseAppearance::Destructive,
                                        );
                                        dialog.set_default_response(Some("cancel"));
                                        dialog.set_close_response("cancel");

                                        let indices2 = indices_for_trash.clone();
                                        let on_move2 = on_move_to_trash_ctx.clone();
                                        dialog.connect_response(None, move |_, response| {
                                            if response == "trash" {
                                                on_move2(indices2.clone());
                                            }
                                        });

                                        if let Some(grid) = grid_weak.upgrade() {
                                            dialog.present(&grid);
                                        }
                                    },
                                ));
                            }

                            glass_context_menu::show(
                                &context_overlay,
                                flow_for_ctx.upcast_ref(),
                                x,
                                y,
                                items,
                            );
                        });

                        flow.add_controller(gesture);
                    }

                    content.append(&flow);
                    section_count += 1;
                }
            }
            build_sections_span.record("sections", section_count);
            build_sections_span.record("photos", photo_count);
        }
        let rendered_window_len = if loading_placeholder_count > 0 {
            loading_placeholder_count
        } else {
            rendered_item_count
        };
        let loaded_end = window_start.saturating_add(rendered_window_len);
        let bottom_unloaded = total_media_count.saturating_sub(loaded_end);
        let bottom_spacer_height =
            virtual_spacer_height(bottom_unloaded, virtual_columns, viewport_width, spec);
        if bottom_spacer_height > 0 {
            content.append(&virtual_spacer(bottom_spacer_height));
        }
        *self.imp().displayed_items.borrow_mut() = displayed_items;
        self.sync_visible_selection();

        // 重建后恢复滚动位置：用 idle 回调等下一帧 layout 完成后再设值，
        // 否则 adj.upper 仍为零，会被 clamp 吞掉。
        let pending_ratio = self.imp().pending_scroll_ratio.take();
        if pending_ratio.is_some() || saved_scroll > 0.0 {
            let weak = self.downgrade();
            gtk::glib::idle_add_local_once(move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let adj = this.imp().scroller.get().vadjustment();
                let restored = if let Some(ratio) = pending_ratio {
                    let scrollable = (adj.upper() - adj.page_size()).max(0.0);
                    scrollable * ratio.clamp(0.0, 1.0)
                } else {
                    saved_scroll.min(adj.upper() - adj.page_size())
                };
                if restored >= 0.0 {
                    this.imp().restoring_scroll.set(true);
                    adj.set_value(restored);
                    this.imp().restoring_scroll.set(false);
                }
            });
        }

        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "MediaGrid::rebuild mode={:?} source_len={} sections={} photos={} spec.pixel_size={}",
            mode,
            source_len,
            section_count,
            photo_count,
            spec.pixel_size
        );
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "GRID_MODEL_TRACE rebuild_finish mode={:?} source_len={} sections={} rendered_photos={} pixel_size={}",
            mode,
            source_len,
            section_count,
            photo_count,
            spec.pixel_size
        );

        // 下一帧 layout 完成后立即请求/提权视口附近缩略图；滚动期间仍走去抖路径。
        let weak = self.downgrade();
        gtk::glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "GRID_MODEL_TRACE idle_reprioritize_after_rebuild mode={:?}",
                    this.mode()
                );
                this.reprioritize_visible();
            }
        });

        // The first rebuild just built the viewport seed; pace the rest of the
        // first page in background ticks so the window is interactive long
        // before all tiles exist.
        if progressive_just_seeded {
            self.schedule_progressive_render_fill();
        }
    }
}

#[cfg(test)]
mod tests;
