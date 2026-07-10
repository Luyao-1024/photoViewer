use super::virtual_paging::{
    replace_pending_virtual_page, should_consider_virtual_page_load, virtual_offset_for_ratio,
    virtual_page_start_for_offset,
};
use super::{
    library_stats_text, load_grid_metadata, scroll_ratio_from_adjustment_value,
    should_show_library_stats, MediaGrid,
};
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::runtime_config;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{glib, prelude::*};
use std::sync::Arc;
use std::time::Duration;

impl MediaGrid {
    /// 如果滚动接近底部（距底部 3 屏内），动态扩大 `rendered_limit` 并触发 rebuild。
    pub(super) fn try_expand_render_limit(&self, adj: &gtk::Adjustment) {
        let absolute = runtime_config::grid_render_absolute_cap();
        let limit = self.imp().rendered_limit.get();
        if limit >= absolute {
            return;
        }
        let near_bottom = adj.value() + adj.page_size() * 4.0 >= adj.upper();
        if !near_bottom {
            return;
        }
        let new_limit = limit
            .saturating_add(runtime_config::grid_render_expand_step())
            .min(absolute);
        // 仅在真正扩容时建 span：value_changed 每帧调用，但 no-op（已到上限 /
        // 离底部还远）是常态，逐帧建 span 会淹没 trace。扩容会触发一次 rebuild，
        // 快速连到底部时会连续扩容 + 重建，把它记进时间线才能定位这种链式卡顿。
        let span = tracing::info_span!(
            "grid:expand_render_limit",
            limit = tracing::field::Empty,
            new_limit = tracing::field::Empty,
            scroll_y = tracing::field::Empty,
        );
        let _trace = span.enter();
        span.record("limit", limit);
        span.record("new_limit", new_limit);
        span.record("scroll_y", adj.value());
        self.imp().rendered_limit.set(new_limit);
        // 触发 rebuild，让新 limit 生效
        if let Some(list) = self.imp().media_list.borrow().as_ref().cloned() {
            self.schedule_rebuild(list);
        }
    }

    pub(super) fn try_load_virtual_page(&self, adj: &gtk::Adjustment) {
        let total = self.imp().virtual_total.get();
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let current_len = list.n_items();
        if !should_consider_virtual_page_load(self.imp().restoring_scroll.get(), total, current_len)
        {
            return;
        }
        let virtual_page_size = runtime_config::virtual_media_page_size();
        let ratio = scroll_ratio_from_adjustment_value(adj.value(), adj.upper(), adj.page_size());
        let desired_offset = virtual_offset_for_ratio(ratio, total, virtual_page_size);
        let current_start = self.imp().virtual_window_start.get();
        let Some(target_start) = virtual_page_start_for_offset(
            desired_offset,
            current_start,
            current_len,
            total,
            virtual_page_size,
        ) else {
            return;
        };
        if target_start == current_start {
            return;
        }

        let Some(loader) = self.imp().loader.get().cloned() else {
            return;
        };
        let generation = self.imp().virtual_page_generation.get().saturating_add(1);
        // 真正的虚拟页重定向。上面的 guard 让常态（restoring / 预取带内 / 同一起点）
        // 零成本早退，只有真正换页才建 span。这里只更新窗口状态并发起 DB 查询——
        // 不再同步重建（旧骨架重建是滚动条抖动源，见下方注释）。
        let span = tracing::info_span!(
            "grid:try_virtual_page",
            generation = tracing::field::Empty,
            target_start = tracing::field::Empty,
            current_start = tracing::field::Empty,
            total = tracing::field::Empty,
            ratio = tracing::field::Empty,
            outcome = tracing::field::Empty,
        );
        let _trace = span.enter();
        span.record("generation", generation);
        span.record("target_start", target_start);
        span.record("current_start", current_start);
        span.record("total", total);
        span.record("ratio", ratio);

        self.imp().virtual_page_generation.set(generation);
        self.imp().virtual_window_start.set(target_start);
        self.imp().virtual_page_loading.set(true);
        // 不做骨架重建，也不预置 pending_scroll_ratio（见上方注释）：DB 取页仅
        // ~1.5ms，骨架重建（清空旧 tile + 建占位 + 把滚动恢复到翻页点的旧 ratio）
        // 反而是滚动条抖动源——骨架恢复到翻页点（往上跳），落地又恢复到当前位置
        // （往下跳），每次翻页一上一下。去掉后旧 tile 原地保留至落地，落地重建用
        // saved_scroll（居中翻页下即正确的全局位置）恢复，单次过渡、无抖动。

        if self.imp().virtual_query_in_flight.get() {
            replace_pending_virtual_page(
                &self.imp().pending_virtual_page_start,
                &self.imp().pending_virtual_page_ratio,
                target_start,
                ratio,
            );
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIRTUAL_SCROLL coalesce_page generation={generation} ratio={ratio:.4} desired_offset={desired_offset} current_start={current_start} current_len={current_len} target_start={target_start} total={total}"
            );
            span.record("outcome", "coalesced");
            return;
        }

        self.imp().virtual_query_in_flight.set(true);
        self.spawn_virtual_page_query(
            loader,
            target_start,
            desired_offset,
            virtual_page_size,
            generation,
        );
        span.record("outcome", "triggered");
    }

    fn spawn_virtual_page_query(
        &self,
        loader: Arc<ThumbnailLoader>,
        target_start: u32,
        prewarm_offset: u32,
        virtual_page_size: u32,
        generation: u64,
    ) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "VIRTUAL_SCROLL load_page generation={generation} target_start={target_start} prewarm_offset={prewarm_offset} page_size={virtual_page_size}"
        );

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            // `grid:page_query` spans the async page load; the DB fetch runs in
            // a `spawn_blocking` worker and is nested as `grid:db_page`.
            let page_span = tracing::info_span!(
                "grid:page_query",
                generation,
                target_start,
                prewarm_offset,
                page_size = virtual_page_size
            );
            let _page = page_span.enter();
            let pool = loader.pool().clone();
            let result = gtk::gio::spawn_blocking(move || {
                let db_span = tracing::info_span!("grid:db_page");
                let _db = db_span.enter();
                MediaRepository::new(pool)
                    .page(MediaQuery::LiveAll, target_start, virtual_page_size)
                    .map(|page| page.items)
            })
            .await;
            let items = match result {
                Ok(Ok(items)) => {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "VIRTUAL db_page_loaded generation={} target_start={} rows={}",
                        generation,
                        target_start,
                        items.len()
                    );
                    items
                }
                Ok(Err(err)) => {
                    tracing::warn!(
                        target: crate::core::log_targets::BROWSING,
                        "VIRTUAL db_page_failed generation={} target_start={} error={err}",
                        generation,
                        target_start
                    );
                    Vec::new()
                }
                Err(err) => {
                    tracing::warn!(
                        target: crate::core::log_targets::BROWSING,
                        "VIRTUAL db_page_join_failed generation={} target_start={} error={err:?}",
                        generation,
                        target_start
                    );
                    Vec::new()
                }
            };

            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().virtual_page_generation.get() != generation {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIRTUAL_SCROLL stale_page generation={} current_generation={}",
                    generation,
                    this.imp().virtual_page_generation.get()
                );
                if this.start_pending_virtual_page_query(loader.clone()) {
                    return;
                }
                this.imp().virtual_query_in_flight.set(false);
                return;
            }
            this.imp().virtual_page_loading.set(false);
            if items.is_empty() {
                let list = this.imp().media_list.borrow().as_ref().cloned();
                if let Some(list) = list {
                    this.rebuild_immediately(list);
                }
                this.imp().virtual_query_in_flight.set(false);
                return;
            }
            this.imp().virtual_window_start.set(target_start);
            // 预热跟随当前浏览位置：用户能用滚动条瞬间跳到任意（可能冷的）
            // 区域，落地后把屏外预热起点移到真实滚动落点附近，而不是居中页的
            // 开头，使其先于无关的最新批次被暖。可见 tile 仍走 BOOST 最高优先级。
            loader.redirect_prewarm_to_offset(prewarm_offset);
            let additions: Vec<glib::BoxedAnyObject> =
                items.into_iter().map(glib::BoxedAnyObject::new).collect();
            let list = this.imp().media_list.borrow().as_ref().cloned();
            if let Some(list) = list {
                let old_len = list.n_items();
                let new_len = additions.len();
                this.imp().applying_virtual_page.set(true);
                list.splice(0, list.n_items(), &additions);
                this.imp().applying_virtual_page.set(false);
                this.rebuild_immediately(list);
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIRTUAL page_applied generation={} target_start={} old_len={} new_len={}",
                    generation,
                    target_start,
                    old_len,
                    new_len
                );
            }
            this.imp().virtual_query_in_flight.set(false);
        });
    }

    fn start_pending_virtual_page_query(&self, loader: Arc<ThumbnailLoader>) -> bool {
        let Some(target_start) = self.imp().pending_virtual_page_start.take() else {
            return false;
        };
        let pending_ratio = self.imp().pending_virtual_page_ratio.take();
        if let Some(ratio) = pending_ratio {
            self.imp().pending_scroll_ratio.set(Some(ratio));
        }
        let generation = self.imp().virtual_page_generation.get();
        self.imp().virtual_query_in_flight.set(true);
        let virtual_page_size = runtime_config::virtual_media_page_size();
        let prewarm_offset = pending_ratio
            .map(|ratio| {
                virtual_offset_for_ratio(ratio, self.imp().virtual_total.get(), virtual_page_size)
            })
            .unwrap_or(target_start);
        self.spawn_virtual_page_query(
            loader,
            target_start,
            prewarm_offset,
            virtual_page_size,
            generation,
        );
        true
    }
}

impl MediaGrid {
    /// Pace the remainder of the first page after the seed render: every tick
    /// raise `rendered_limit` by `startup_render_batch` and re-run `rebuild`,
    /// which reuses already-built tiles (keyed by media_id) and only appends the
    /// new chunk, until the whole first page is rendered. Cancels itself when
    /// the grid is dropped or a mode/active/model change bumps the generation.
    pub(super) fn schedule_progressive_render_fill(&self) {
        let weak = self.downgrade();
        let gen = self.imp().progressive_render_gen.get();
        let interval = Duration::from_millis(runtime_config::startup_render_interval_ms());
        // The first tick waits longer so the seed render's thumbnails can
        // generate and deliver (set_paintable runs on the main thread) before
        // the fill starts competing for it — directly lowering time to first
        // thumbnail. Subsequent ticks use the shorter `interval`.
        let first_delay =
            Duration::from_millis(runtime_config::startup_render_first_tick_delay_ms());
        let batch = runtime_config::startup_render_batch();
        let ceiling = runtime_config::max_rendered_grid_items()
            .min(runtime_config::grid_render_absolute_cap());
        glib::spawn_future_local(async move {
            let mut first_tick = true;
            loop {
                let delay = if first_tick { first_delay } else { interval };
                first_tick = false;
                glib::timeout_future(delay).await;
                let Some(this) = weak.upgrade() else { break };
                if this.imp().progressive_render_gen.get() != gen {
                    break;
                }
                let Some(list) = this.imp().media_list.borrow().as_ref().cloned() else {
                    break;
                };
                let source_len = list.n_items() as usize;
                let cap = ceiling.min(source_len);
                let cur = this.imp().rendered_limit.get();
                if cur >= cap {
                    // Restore the steady-state limit so later scroll-expand and
                    // comparisons behave exactly as before this optimization.
                    this.imp().rendered_limit.set(ceiling);
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "PROGRESSIVE_RENDER done rendered_limit={} cap={}",
                        ceiling,
                        cap
                    );
                    break;
                }
                let next = cur.saturating_add(batch).min(cap);
                this.imp().rendered_limit.set(next);
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "PROGRESSIVE_RENDER tick rendered_limit={} cap={}",
                    next,
                    cap
                );
                let mode = this.mode();
                let added = next.saturating_sub(cur) as u32;
                if added > 0 && this.apply_progressive_render_addition(cur as u32, added, &list) {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "PROGRESSIVE_RENDER append rendered_limit={} added={}",
                        next,
                        added
                    );
                } else {
                    this.rebuild(list, mode);
                }
            }
        });
    }

    /// Cancel any in-flight progressive fill by bumping its generation,
    /// so the paced loop bails on its next tick (mode/active change, etc.).
    pub(super) fn invalidate_progressive_render_fill(&self) {
        self.imp()
            .progressive_render_gen
            .set(self.imp().progressive_render_gen.get().wrapping_add(1));
    }

    pub(super) fn ensure_library_metadata_async(
        &self,
        loader: Arc<ThumbnailLoader>,
        mode: GroupBy,
    ) {
        if !self.imp().full_library_context.get() {
            return;
        }
        let has_mode_counts = self
            .imp()
            .section_count_snapshots
            .borrow()
            .contains_key(&mode);
        if self.imp().library_total_snapshot.get().is_some()
            && self.imp().library_stats_snapshot.get().is_some()
            && has_mode_counts
        {
            return;
        }
        if self.imp().library_metadata_loading.get() {
            return;
        }

        self.imp().library_metadata_loading.set(true);
        let pool = loader.pool().clone();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || load_grid_metadata(pool, mode)).await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.imp().library_metadata_loading.set(false);
            if this.imp().library_metadata_dirty_pending.replace(false) {
                this.ensure_library_metadata_async(loader.clone(), mode);
                return;
            }
            let snapshot = match result {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    tracing::warn!("media grid metadata refresh failed: {err:?}");
                    return;
                }
            };
            if let Some(total) = snapshot.live_total {
                this.imp().library_total_snapshot.set(Some(total));
            }
            if let Some(stats) = snapshot.library_stats {
                this.imp().library_stats_snapshot.set(Some(stats));
            }
            if let Some(counts) = snapshot.section_counts {
                this.imp()
                    .section_count_snapshots
                    .borrow_mut()
                    .insert(snapshot.mode, counts);
            }
            if this.imp().active.get() {
                if let Some(list) = this.imp().media_list.borrow().as_ref().cloned() {
                    this.rebuild(list, this.mode());
                }
            }
        });
    }

    pub(super) fn invalidate_library_metadata(&self) {
        if !self.imp().full_library_context.get() {
            return;
        }
        let pending_thumbnail_stats = self
            .imp()
            .stats_label
            .borrow()
            .is_some()
            .then(|| self.imp().library_stats_snapshot.get())
            .flatten()
            .filter(should_show_library_stats);
        if self.imp().library_metadata_loading.get() {
            self.imp().library_metadata_dirty_pending.set(true);
        } else {
            self.imp().library_metadata_dirty_pending.set(false);
        }
        self.imp().library_total_snapshot.set(None);
        self.imp()
            .library_stats_snapshot
            .set(pending_thumbnail_stats);
        self.imp().section_count_snapshots.borrow_mut().clear();
    }

    pub(super) fn start_stats_refresh(&self, loader: Arc<ThumbnailLoader>, total_media: usize) {
        if total_media == 0 {
            return;
        }

        let weak = self.downgrade();
        let source = glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if this.imp().stats_label.borrow().is_none() {
                this.imp().stats_refresh_source.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            if this.imp().stats_refresh_running.replace(true) {
                return glib::ControlFlow::Continue;
            }

            let pool = loader.pool().clone();
            let weak_for_refresh = this.downgrade();
            glib::spawn_future_local(async move {
                let result =
                    gtk::gio::spawn_blocking(move || MediaRepository::new(pool).library_stats())
                        .await;
                let Some(this) = weak_for_refresh.upgrade() else {
                    return;
                };
                this.imp().stats_refresh_running.set(false);
                let stats = match result {
                    Ok(Ok(stats)) => stats,
                    Ok(Err(err)) => {
                        tracing::warn!("media grid stats refresh failed: {err}");
                        return;
                    }
                    Err(err) => {
                        tracing::warn!("media grid stats refresh join failed: {err:?}");
                        return;
                    }
                };
                this.imp().library_stats_snapshot.set(Some(stats));
                if let Some(label) = this.imp().stats_label.borrow().as_ref() {
                    label.set_label(&library_stats_text(
                        stats.live_total,
                        stats.thumbnails_generated,
                    ));
                } else {
                    this.imp().stats_refresh_source.borrow_mut().take();
                    return;
                }
                if stats.thumbnails_generated >= stats.live_total {
                    if let Some(label) = this.imp().stats_label.borrow_mut().take() {
                        if let Some(parent) = label.parent().and_downcast::<gtk::Box>() {
                            parent.remove(&label);
                        }
                    }
                    this.imp().stats_refresh_source.borrow_mut().take();
                }
            });
            glib::ControlFlow::Continue
        });
        *self.imp().stats_refresh_source.borrow_mut() = Some(source);
    }

    pub(super) fn schedule_rebuild(&self, media_list: gtk::gio::ListStore) {
        let mode = self.mode();
        let list_len = media_list.n_items();
        let span = tracing::info_span!(
            target: crate::core::log_targets::BROWSING,
            "grid:schedule_rebuild",
            mode = ?mode,
            list_len,
            delay_ms = 750u64,
            outcome = tracing::field::Empty
        );
        let _trace = span.enter();
        if self.imp().rebuild_debounce.borrow().is_some() {
            span.record("outcome", "coalesced");
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "GRID_MODEL_TRACE schedule_rebuild_coalesced mode={:?} list_len={}",
                mode,
                list_len
            );
            return;
        }

        span.record("outcome", "scheduled");
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "GRID_MODEL_TRACE schedule_rebuild mode={:?} list_len={} delay_ms=750",
            mode,
            list_len
        );
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(Duration::from_millis(750), move || {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let span = tracing::info_span!(
                target: crate::core::log_targets::BROWSING,
                "grid:scheduled_rebuild",
                mode = ?this.mode(),
                list_len = media_list.n_items()
            );
            let _trace = span.enter();
            this.imp().rebuild_debounce.borrow_mut().take();
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "GRID_MODEL_TRACE scheduled_rebuild_fire mode={:?} list_len={}",
                this.mode(),
                media_list.n_items()
            );
            this.clear_selection();
            this.rebuild(media_list, this.mode());
        });
        *self.imp().rebuild_debounce.borrow_mut() = Some(source);
    }

    pub(super) fn rebuild_immediately(&self, media_list: gtk::gio::ListStore) {
        let span = tracing::info_span!(
            target: crate::core::log_targets::BROWSING,
            "grid:rebuild_immediately",
            mode = ?self.mode(),
            list_len = media_list.n_items(),
            canceled_pending = tracing::field::Empty
        );
        let _trace = span.enter();
        let canceled_pending = if let Some(source) = self.imp().rebuild_debounce.borrow_mut().take()
        {
            source.remove();
            true
        } else {
            false
        };
        span.record("canceled_pending", canceled_pending);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "GRID_MODEL_TRACE rebuild_immediately mode={:?} list_len={}",
            self.mode(),
            media_list.n_items()
        );
        self.clear_selection();
        self.rebuild(media_list, self.mode());
    }
}

#[cfg(test)]
mod tests;
