#![allow(dead_code)]

use super::ViewerPage;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

pub(super) const THUMB_HEIGHT: i32 = 56;
pub(super) const THUMB_MIN_WIDTH: i32 = 36;
pub(super) const THUMB_MIN_ASPECT: f64 = 9.0 / 21.0;
pub(super) const THUMB_MAX_ASPECT: f64 = 21.0 / 9.0;
pub(super) const THUMB_INITIAL_HALF: u32 = 5;
pub(super) const THUMB_DEFAULT_WINDOW_LEN: u32 = 2 * THUMB_INITIAL_HALF + 1;
pub(super) const THUMB_STRIP_SPACING: f64 = 6.0;
pub(super) const THUMB_EDGE_INSET: f64 = 24.0;
pub(super) const THUMB_MIN_STABLE_ALLOC_WIDTH: f64 = (THUMB_MIN_WIDTH as f64) * 0.75;
pub(super) const THUMB_LAZY_HALF: u32 = 4;
pub(super) const THUMB_WINDOW_MAX: u32 = 40;
pub(super) const THUMB_CENTER_RETRY_FRAMES: u8 = 8;
pub(super) const THUMB_SCROLL_ANIMATION_MS: f64 = 140.0;

pub(super) fn should_retry_thumb_centering(applied: bool, attempts_remaining: u8) -> bool {
    !applied && attempts_remaining > 0
}

pub(super) fn compute_thumb_scroll_and_residual(
    btn_x: f64,
    btn_w: f64,
    page_size: f64,
    upper: f64,
) -> (f64, f64) {
    let raw = btn_x + btn_w / 2.0 - page_size / 2.0;
    let max_value = (upper - page_size).max(0.0);
    let value = raw.clamp(0.0, max_value);
    let residual = clamp_thumb_residual(value - raw, upper, page_size);
    (value, residual)
}

pub(super) fn compute_thumb_positioning(
    btn_x: f64,
    btn_w: f64,
    page_size: f64,
    adjustment_upper: f64,
    content_width: f64,
) -> (f64, f64, f64) {
    if content_width <= page_size {
        let transform = page_size / 2.0 - (btn_x + btn_w / 2.0);
        return (0.0, transform, transform);
    }

    let effective_upper = adjustment_upper.max(content_width);
    let (target, residual) =
        compute_thumb_scroll_and_residual(btn_x, btn_w, page_size, effective_upper);
    let transform = compute_thumb_visual_transform(target, residual, adjustment_upper, page_size);
    (target, residual, transform)
}

pub(super) fn compute_thumb_animated_scroll_value(start: f64, target: f64, progress: f64) -> f64 {
    let t = progress.clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    start + (target - start) * eased
}

pub(super) fn thumb_item_widths(items: &[gtk::Button]) -> Vec<f64> {
    items
        .iter()
        .map(|item| item.allocation().width() as f64)
        .collect()
}

pub(super) fn clamped_thumb_width_for_texture(tex_w: i32, tex_h: i32) -> i32 {
    if tex_w <= 0 || tex_h <= 0 {
        return THUMB_MIN_WIDTH;
    }

    let aspect = (tex_w as f64 / tex_h as f64).clamp(THUMB_MIN_ASPECT, THUMB_MAX_ASPECT);
    (((THUMB_HEIGHT as f64) * aspect).round() as i32).max(THUMB_MIN_WIDTH)
}

pub(super) fn thumb_width_for_media_dimensions(width: Option<u32>, height: Option<u32>) -> i32 {
    let (Some(width), Some(height)) = (width, height) else {
        return THUMB_MIN_WIDTH;
    };

    clamped_thumb_width_for_texture(width as i32, height as i32)
}

pub(super) fn thumb_item_content_geometry(
    widths: &[f64],
    offset: usize,
    spacing: f64,
) -> Option<(f64, f64, f64)> {
    let width = *widths.get(offset)?;
    if width < THUMB_MIN_STABLE_ALLOC_WIDTH
        || widths
            .iter()
            .any(|width| *width < THUMB_MIN_STABLE_ALLOC_WIDTH)
    {
        return None;
    }

    let x = THUMB_EDGE_INSET
        + widths
            .iter()
            .take(offset)
            .fold(0.0, |acc, width| acc + width + spacing);
    let content_width = THUMB_EDGE_INSET * 2.0
        + widths.iter().sum::<f64>()
        + widths.len().saturating_sub(1) as f64 * spacing;
    Some((x, width, content_width))
}

pub(super) fn clamp_thumb_residual(residual: f64, upper: f64, page_size: f64) -> f64 {
    let scrollable = (upper - page_size).max(0.0);
    if scrollable <= 0.0 {
        0.0
    } else {
        residual.clamp(-scrollable, scrollable)
    }
}

pub(super) fn compute_thumb_visual_transform(
    target: f64,
    residual: f64,
    adjustment_upper: f64,
    page_size: f64,
) -> f64 {
    if adjustment_upper - page_size > 0.5 {
        residual
    } else {
        residual - target
    }
}

pub(super) fn compute_initial_thumb_window(current: u32, n_items: u32) -> (u32, u32) {
    compute_initial_thumb_window_for_len(current, n_items, THUMB_DEFAULT_WINDOW_LEN)
}

pub(super) fn compute_initial_thumb_window_for_len(
    current: u32,
    n_items: u32,
    target_len: u32,
) -> (u32, u32) {
    if n_items == 0 {
        return (0, 0);
    }
    let target_len = target_len.clamp(1, THUMB_WINDOW_MAX).min(n_items);
    let left_half = target_len / 2;
    let mut start = current.saturating_sub(left_half);
    let mut end = start.saturating_add(target_len).min(n_items);
    start = end.saturating_sub(target_len);
    end = start.saturating_add(target_len).min(n_items);
    (start, end)
}

pub(super) fn compute_extended_thumb_window(
    direction: i8,
    current_start: u32,
    current_end: u32,
    n_items: u32,
    current_items_len: usize,
) -> Option<(u32, u32)> {
    debug_assert!(
        direction == -1 || direction == 1,
        "compute_extended_thumb_window: direction must be -1 or 1, got {direction}"
    );
    let at_cap = current_items_len >= THUMB_WINDOW_MAX as usize;

    if direction < 0 {
        let new_start = current_start.saturating_sub(THUMB_LAZY_HALF);
        if new_start == current_start {
            return None;
        }
        let new_end = if at_cap {
            current_end.saturating_sub(current_start - new_start)
        } else {
            current_end
        };
        Some((new_start, new_end))
    } else {
        let new_end = current_end.saturating_add(THUMB_LAZY_HALF).min(n_items);
        if new_end == current_end {
            return None;
        }
        let new_start = if at_cap {
            current_start.saturating_add(new_end - current_end)
        } else {
            current_start
        };
        Some((new_start, new_end))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThumbWindowUpdateKind {
    AppendRight,
    PrependLeft,
    Rebuild,
}

pub(super) fn classify_thumb_window_update(
    old_start: u32,
    old_end: u32,
    new_start: u32,
    new_end: u32,
) -> ThumbWindowUpdateKind {
    if old_start == new_start && new_end > old_end {
        ThumbWindowUpdateKind::AppendRight
    } else if old_end == new_end && new_start < old_start {
        ThumbWindowUpdateKind::PrependLeft
    } else {
        ThumbWindowUpdateKind::Rebuild
    }
}

pub(super) fn compute_current_thumb_extend_direction(
    current: u32,
    start: u32,
    end: u32,
    n_items: u32,
    _current_items_len: usize,
) -> Option<i8> {
    if current < start || current >= end || end <= start {
        return None;
    }

    let left_remaining = current.saturating_sub(start);
    let right_remaining = end.saturating_sub(current).saturating_sub(1);

    let wants_left = left_remaining <= THUMB_LAZY_HALF && start > 0;
    let wants_right = right_remaining <= THUMB_LAZY_HALF && end < n_items;

    match (wants_left, wants_right) {
        (true, true) if left_remaining <= right_remaining => Some(-1),
        (true, true) => Some(1),
        (true, false) => Some(-1),
        (false, true) => Some(1),
        (false, false) => None,
    }
}

impl ViewerPage {
    /// Rebuild or update the filmstrip for the current index. Called from
    /// `show_at`. When the current index is still inside the existing window,
    /// only the highlight is toggled and the strip scrolls to reveal the
    /// current item; otherwise the strip is rebuilt with an initial window
    /// (±THUMB_INITIAL_HALF) centred on the current index.
    pub(super) fn refresh_thumb_strip(&self) {
        let current = self.imp().current_index.get();
        let start = self.imp().thumb_window_start.get();
        let end = self.imp().thumb_window_end.get();
        let list_len = self.list_n_items().unwrap_or(0);

        let in_window = end > start && current >= start && current < end;
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_refresh current={} list_len={} existing_window=[{}, {}) in_window={} current_item={}",
            current,
            list_len,
            start,
            end,
            in_window,
            self.media_item_summary_at(current)
        );

        if in_window {
            self.update_thumb_highlight(current);
        } else {
            self.load_initial_thumb_window(current);
        }
        self.try_extend_thumb_window_for_current();
        self.schedule_scroll_thumb_to_current();
    }

    /// First-time load: centre a small bounded window around `current`.
    /// The visible strip is positioned later by CSS transform so ultrawide
    /// viewports do not force the viewer to load the whole album.
    fn load_initial_thumb_window(&self, current: u32) {
        let Some(n_items) = self.list_n_items() else {
            return;
        };
        if n_items == 0 {
            return;
        }
        let (start, end) =
            compute_initial_thumb_window_for_len(current, n_items, THUMB_DEFAULT_WINDOW_LEN);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_initial_window current={} n_items={} target_len={} computed_window=[{}, {}) current_item={}",
            current,
            n_items,
            THUMB_DEFAULT_WINDOW_LEN,
            start,
            end,
            self.media_item_summary_at(current)
        );
        self.rebuild_thumb_strip(start, end, current);
    }

    /// Lazy extend the loaded window by `THUMB_LAZY_HALF` items in the given
    /// direction (`-1` = prepend on the left, `+1` = append on the right).
    /// Bounded by `[0, n_items)` and the `THUMB_WINDOW_MAX` cap.
    fn try_extend_thumb_window(&self, direction: i8) {
        let imp = self.imp();
        if imp.thumb_pending_extend.get() == Some(direction) {
            // Debounce: rebuild itself can fire value-changed; suppress
            // cascading extends until the next idle clears this flag.
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_extend_skip reason=pending_same_direction direction={} window=[{}, {}) current={} pending={:?}",
                direction,
                imp.thumb_window_start.get(),
                imp.thumb_window_end.get(),
                imp.current_index.get(),
                imp.thumb_pending_extend.get()
            );
            return;
        }
        let Some(n_items) = self.list_n_items() else {
            return;
        };
        let start = imp.thumb_window_start.get();
        let end = imp.thumb_window_end.get();
        let items_len = imp.thumb_items.borrow().len();

        let Some((new_start, new_end)) =
            compute_extended_thumb_window(direction, start, end, n_items, items_len)
        else {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_extend_skip reason=no_new_window direction={} old_window=[{}, {}) current={} items_len={} list_len={}",
                direction,
                start,
                end,
                imp.current_index.get(),
                items_len,
                n_items
            );
            return;
        };

        let current = imp.current_index.get();
        imp.thumb_pending_extend.set(Some(direction));
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_extend direction={} old_window=[{}, {}) new_window=[{}, {}) current={} items_len={} list_len={} at_cap={} slide_delta={}",
            direction,
            start,
            end,
            new_start,
            new_end,
            current,
            items_len,
            n_items,
            items_len >= THUMB_WINDOW_MAX as usize,
            new_start as i64 - start as i64
        );
        match classify_thumb_window_update(start, end, new_start, new_end) {
            ThumbWindowUpdateKind::AppendRight => {
                self.append_thumb_strip_items(end, new_end, current);
            }
            ThumbWindowUpdateKind::PrependLeft => {
                self.prepend_thumb_strip_items(new_start, start, current);
            }
            ThumbWindowUpdateKind::Rebuild => {
                self.rebuild_thumb_strip(new_start, new_end, current);
            }
        }
        self.schedule_scroll_thumb_to_current();

        // Clear the debounce flag on next idle so a subsequent scroll
        // past the new edge can extend again.
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.imp().thumb_pending_extend.set(None);
            }
        });
    }

    fn try_extend_thumb_window_for_current(&self) {
        let imp = self.imp();
        let Some(n_items) = self.list_n_items() else {
            return;
        };
        let current = imp.current_index.get();
        let start = imp.thumb_window_start.get();
        let end = imp.thumb_window_end.get();
        let items_len = imp.thumb_items.borrow().len();

        let Some(direction) =
            compute_current_thumb_extend_direction(current, start, end, n_items, items_len)
        else {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_current_extend_none current={} window=[{}, {}) list_len={} items_len={} left_remaining={} right_remaining={}",
                current,
                start,
                end,
                n_items,
                items_len,
                current.saturating_sub(start),
                end.saturating_sub(current).saturating_sub(1)
            );
            return;
        };
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_current_extend_request current={} window=[{}, {}) list_len={} items_len={} direction={}",
            current,
            start,
            end,
            n_items,
            items_len,
            direction
        );
        self.try_extend_thumb_window(direction);
    }

    /// Tear down the existing strip and rebuild with `[start, end)`.
    /// Each item is a frame-less `GtkButton` wrapping a `GtkPicture` with
    /// `content-fit: cover`. After the thumbnail texture arrives,
    /// `width-request` is set from the image aspect ratio, clamped to
    /// 21:9 / 9:21 so extreme panoramas do not dominate the filmstrip.
    fn rebuild_thumb_strip(&self, start: u32, end: u32, current: u32) {
        let imp = self.imp();
        let strip = imp.thumb_strip.get();
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_rebuild start={} end={} current={} current_offset={:?} list_len={} old_window=[{}, {}) old_items_len={} current_item={}",
            start,
            end,
            current,
            current.checked_sub(start),
            self.list_n_items().unwrap_or(0),
            imp.thumb_window_start.get(),
            imp.thumb_window_end.get(),
            imp.thumb_items.borrow().len(),
            self.media_item_summary_at(current)
        );

        // Clear old buttons.
        while let Some(child) = strip.first_child() {
            strip.remove(&child);
        }
        imp.thumb_items.borrow_mut().clear();

        let mut new_items = Vec::with_capacity((end - start) as usize);
        for idx in start..end {
            let Some(btn) = self.make_thumb_button(idx, current) else {
                continue;
            };
            strip.append(&btn);
            new_items.push(btn);
        }

        imp.thumb_window_start.set(start);
        imp.thumb_window_end.set(end);
        *imp.thumb_items.borrow_mut() = new_items;
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_rebuild_done window=[{}, {}) current={} items_len={} child_count={} strip_alloc={}x{} scrolled_alloc={}x{}",
            start,
            end,
            current,
            imp.thumb_items.borrow().len(),
            end.saturating_sub(start),
            strip.allocation().width(),
            strip.allocation().height(),
            imp.thumb_scrolled.get().allocation().width(),
            imp.thumb_scrolled.get().allocation().height()
        );
    }

    fn append_thumb_strip_items(&self, append_start: u32, append_end: u32, current: u32) {
        let imp = self.imp();
        let old_start = imp.thumb_window_start.get();
        let old_end = imp.thumb_window_end.get();
        let strip = imp.thumb_strip.get();
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_append start={} end={} current={} old_window=[{}, {}) old_items_len={} current_item={}",
            append_start,
            append_end,
            current,
            old_start,
            old_end,
            imp.thumb_items.borrow().len(),
            self.media_item_summary_at(current)
        );

        if append_start != old_end || append_end <= append_start {
            self.rebuild_thumb_strip(old_start, append_end.max(old_end), current);
            return;
        }

        let mut appended = Vec::with_capacity((append_end - append_start) as usize);
        for idx in append_start..append_end {
            let Some(btn) = self.make_thumb_button(idx, current) else {
                continue;
            };
            strip.append(&btn);
            appended.push(btn);
        }

        imp.thumb_window_end.set(append_end);
        imp.thumb_items.borrow_mut().extend(appended);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_append_done window=[{}, {}) current={} items_len={} appended={} strip_alloc={}x{} scrolled_alloc={}x{}",
            old_start,
            append_end,
            current,
            imp.thumb_items.borrow().len(),
            append_end.saturating_sub(append_start),
            strip.allocation().width(),
            strip.allocation().height(),
            imp.thumb_scrolled.get().allocation().width(),
            imp.thumb_scrolled.get().allocation().height()
        );
    }

    fn prepend_thumb_strip_items(&self, prepend_start: u32, prepend_end: u32, current: u32) {
        let imp = self.imp();
        let old_start = imp.thumb_window_start.get();
        let old_end = imp.thumb_window_end.get();
        let strip = imp.thumb_strip.get();
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_prepend start={} end={} current={} old_window=[{}, {}) old_items_len={} current_item={}",
            prepend_start,
            prepend_end,
            current,
            old_start,
            old_end,
            imp.thumb_items.borrow().len(),
            self.media_item_summary_at(current)
        );

        if prepend_end != old_start || prepend_end <= prepend_start {
            self.rebuild_thumb_strip(prepend_start.min(old_start), old_end, current);
            return;
        }

        let mut prepended = Vec::with_capacity((prepend_end - prepend_start) as usize);
        for idx in prepend_start..prepend_end {
            let Some(btn) = self.make_thumb_button(idx, current) else {
                continue;
            };
            prepended.push(btn);
        }
        for btn in prepended.iter().rev() {
            strip.prepend(btn);
        }

        imp.thumb_window_start.set(prepend_start);
        let old_items = {
            let mut items = imp.thumb_items.borrow_mut();
            std::mem::take(&mut *items)
        };
        prepended.extend(old_items);
        *imp.thumb_items.borrow_mut() = prepended;
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_prepend_done window=[{}, {}) current={} items_len={} prepended={} strip_alloc={}x{} scrolled_alloc={}x{}",
            prepend_start,
            old_end,
            current,
            imp.thumb_items.borrow().len(),
            prepend_end.saturating_sub(prepend_start),
            strip.allocation().width(),
            strip.allocation().height(),
            imp.thumb_scrolled.get().allocation().width(),
            imp.thumb_scrolled.get().allocation().height()
        );
    }

    /// Construct one filmstrip button + async thumbnail request. Shared by
    /// initial load and lazy extend so both code paths render identically.
    /// Returns `None` only when the media list / loader hasn't been injected
    /// yet (early construction), which the caller treats as a no-op.
    fn make_thumb_button(&self, idx: u32, current: u32) -> Option<gtk::Button> {
        let loader = self.imp().loader.borrow().as_ref()?.clone();
        let item = {
            let media_guard = self.imp().media_list.borrow();
            let list = media_guard.as_ref()?;
            crate::ui::media_list::media_item_at(list, idx)?
        };

        let button = gtk::Button::new();
        button.set_has_frame(false);
        button.add_css_class("viewer-thumb-item");
        if idx == current {
            button.add_css_class("viewer-thumb-current");
        }
        let fs_mtime = std::fs::metadata(&item.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let item_mtime = fs_mtime.unwrap_or_else(|| std::time::SystemTime::from(item.file_mtime));
        let cache_key =
            ThumbnailLoader::cache_key_for(&item.uri, ThumbnailSize::Small, Some(item_mtime));
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_button idx={} is_current={} item_id={} item_name={} item_uri={} media_item_mtime={} request_mtime={:?} fs_mtime={:?} cache_key={:?}",
            idx,
            idx == current,
            item.id,
            item.display_name(),
            item.uri,
            item.file_mtime,
            item_mtime,
            fs_mtime,
            cache_key
        );

        let initial_width = thumb_width_for_media_dimensions(item.width, item.height);
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .width_request(initial_width)
            .height_request(THUMB_HEIGHT)
            .can_shrink(true)
            .build();
        button.set_child(Some(&picture));

        // Request thumbnail. The ThumbnailLoader caches by `path + mtime`, so
        // extending the strip after the items were already requested once is a
        // cache hit (no extra decode).
        let item_uri = item.uri.clone();
        let item_id = item.id;
        let item_name = item.display_name().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(
            item_uri,
            ThumbnailSize::Small,
            Some(item_mtime),
            tx,
            crate::core::thumbnails::TIER_NORMAL,
        );

        let pic_weak = picture.downgrade();
        let viewer_weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(loaded) = rx.await else {
                return;
            };
            let Some(pic) = pic_weak.upgrade() else {
                return;
            };
            let tex = loaded.texture;
            let tex_w = tex.width();
            let tex_h = tex.height();
            let new_width = clamped_thumb_width_for_texture(tex_w, tex_h);
            let old_width_request = pic.width_request();
            let old_alloc = pic.allocation();
            pic.set_paintable(Some(&tex));
            if old_width_request != new_width {
                pic.set_width_request(new_width);
            }
            if let Some(this) = viewer_weak.upgrade() {
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_JITTER thumb_texture_loaded idx={} item_id={} item_name={} tex={}x{} old_width_request={} new_width_request={} width_changed={} old_alloc={}x{} current={} window=[{}, {})",
                    idx,
                    item_id,
                    item_name,
                    tex_w,
                    tex_h,
                    old_width_request,
                    new_width,
                    old_width_request != new_width,
                    old_alloc.width(),
                    old_alloc.height(),
                    this.imp().current_index.get(),
                    this.imp().thumb_window_start.get(),
                    this.imp().thumb_window_end.get()
                );
                if old_width_request != new_width {
                    this.schedule_scroll_thumb_to_current();
                }
            }
        });

        // Click → navigate to this index.
        let weak = self.downgrade();
        button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let delta = idx as i32 - this.current_index() as i32;
                if delta != 0 {
                    this.navigate_by_delta(delta);
                }
            }
        });

        Some(button)
    }

    /// Toggle the `.viewer-thumb-current` class so only the current item is
    /// highlighted, without rebuilding the strip.
    fn update_thumb_highlight(&self, current: u32) {
        let start = self.imp().thumb_window_start.get();
        let items = self.imp().thumb_items.borrow();
        for (i, btn) in items.iter().enumerate() {
            let idx = start + i as u32;
            if idx == current {
                btn.add_css_class("viewer-thumb-current");
            } else {
                btn.remove_css_class("viewer-thumb-current");
            }
        }
    }

    fn update_thumb_scroll_position(&self) -> bool {
        let hadj = self.imp().thumb_scrolled.get().hadjustment();
        let page_size = hadj.page_size();
        let upper = hadj.upper();
        if page_size <= 0.0 {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_scroll_skip reason=no_page_size page_size={} upper={} current={} window=[{}, {})",
                page_size,
                upper,
                self.imp().current_index.get(),
                self.imp().thumb_window_start.get(),
                self.imp().thumb_window_end.get()
            );
            return false;
        }

        let start = self.imp().thumb_window_start.get();
        let current = self.imp().current_index.get();
        let Some(offset) = current.checked_sub(start).map(|v| v as usize) else {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_scroll_skip reason=current_before_window current={} window=[{}, {}) page_size={} upper={}",
                current,
                start,
                self.imp().thumb_window_end.get(),
                page_size,
                upper
            );
            return false;
        };
        let items = self.imp().thumb_items.borrow();
        let Some(btn) = items.get(offset) else {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_scroll_skip reason=offset_missing current={} start={} offset={} items_len={} page_size={} upper={}",
                current,
                start,
                offset,
                items.len(),
                page_size,
                upper
            );
            return false;
        };

        let alloc = btn.allocation();
        if alloc.width() <= 0 {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_scroll_skip reason=current_unallocated current={} start={} offset={} alloc={}x{} page_size={} upper={}",
                current,
                start,
                offset,
                alloc.width(),
                alloc.height(),
                page_size,
                upper
            );
            return false;
        }

        let item_widths = thumb_item_widths(&items);
        let Some((button_x, button_w, content_width)) =
            thumb_item_content_geometry(&item_widths, offset, THUMB_STRIP_SPACING)
        else {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_scroll_skip reason=unstable_widths current={} start={} offset={} items_len={} alloc={}x{} page_size={} upper={} widths={:?}",
                current,
                start,
                offset,
                items.len(),
                alloc.width(),
                alloc.height(),
                page_size,
                upper,
                item_widths
            );
            return false;
        };
        let (target, residual, visual_transform) =
            compute_thumb_positioning(button_x, button_w, page_size, upper, content_width);
        let imp = self.imp();
        let old_value = hadj.value();
        let old_transform = imp.thumb_last_transform.get();
        self.animate_thumb_scroll_adjustment_to(target);
        self.apply_thumb_strip_transform(visual_transform);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_scroll current={} start={} offset={} items_len={} button_x={} content_x={} button_w={} page_size={} upper={} content_width={} old_value={} target={} residual={} old_transform={} transform={} delta_transform={} widths={:?}",
            current,
            start,
            offset,
            items.len(),
            alloc.x(),
            button_x,
            button_w,
            page_size,
            upper,
            content_width,
            old_value,
            target,
            residual,
            old_transform,
            visual_transform,
            visual_transform - old_transform,
            item_widths
        );
        true
    }

    fn set_thumb_scroll_adjustment_value(&self, value: f64) {
        let imp = self.imp();
        let hadj = imp.thumb_scrolled.get().hadjustment();
        imp.thumb_programmatic_scroll.set(true);
        hadj.set_value(value);
        imp.thumb_programmatic_scroll.set(false);
    }

    fn animate_thumb_scroll_adjustment_to(&self, target: f64) {
        let imp = self.imp();
        let hadj = imp.thumb_scrolled.get().hadjustment();
        let start = hadj.value();
        let clamped_target = target.clamp(0.0, (hadj.upper() - hadj.page_size()).max(0.0));
        let distance = (clamped_target - start).abs();
        let animation_id = imp.thumb_scroll_animation_seq.get() + 1;
        imp.thumb_scroll_animation_seq.set(animation_id);

        if distance < 0.5 {
            self.set_thumb_scroll_adjustment_value(clamped_target);
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_scroll_anim_skip id={} reason=small_distance start={} target={}",
                animation_id,
                start,
                clamped_target
            );
            return;
        }

        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_scroll_anim_start id={} start={} target={} distance={} duration_ms={}",
            animation_id,
            start,
            clamped_target,
            distance,
            THUMB_SCROLL_ANIMATION_MS
        );
        let weak = self.downgrade();
        let start_time_us = Rc::new(Cell::new(None::<i64>));
        self.imp()
            .thumb_scrolled
            .get()
            .add_tick_callback(move |_, clock| {
                let Some(this) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if this.imp().thumb_scroll_animation_seq.get() != animation_id {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_JITTER thumb_scroll_anim_cancel id={} current_id={}",
                        animation_id,
                        this.imp().thumb_scroll_animation_seq.get()
                    );
                    return glib::ControlFlow::Break;
                }

                let frame_time = clock.frame_time();
                let start_time = match start_time_us.get() {
                    Some(value) => value,
                    None => {
                        start_time_us.set(Some(frame_time));
                        frame_time
                    }
                };
                let elapsed_ms = (frame_time - start_time).max(0) as f64 / 1000.0;
                let progress = (elapsed_ms / THUMB_SCROLL_ANIMATION_MS).clamp(0.0, 1.0);
                let value = compute_thumb_animated_scroll_value(start, clamped_target, progress);
                this.set_thumb_scroll_adjustment_value(value);

                if progress >= 1.0 {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_JITTER thumb_scroll_anim_done id={} target={}",
                        animation_id,
                        clamped_target
                    );
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
    }

    fn apply_thumb_strip_transform(&self, offset: f64) {
        let imp = self.imp();
        if imp.thumb_transform_provider.borrow().is_none() {
            let provider = gtk::CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            *imp.thumb_transform_provider.borrow_mut() = Some(provider);
        }

        let css = if offset.abs() < 0.5 {
            ".viewer-thumb-strip { transform: none; }".to_string()
        } else {
            format!(".viewer-thumb-strip {{ transform: translate({offset}px, 0); }}")
        };
        if let Some(provider) = imp.thumb_transform_provider.borrow().as_ref() {
            provider.load_from_data(&css);
        }
        let old_offset = imp.thumb_last_transform.replace(offset);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_transform_apply old_offset={} new_offset={} delta={} css=\"{}\"",
            old_offset,
            offset,
            offset - old_offset,
            css
        );
        imp.thumb_strip.get().queue_draw();
    }

    /// Position the filmstrip around the current item. GTK's adjustment is
    /// used when it has a real scroll range; otherwise a CSS transform provides
    /// virtual scrolling without increasing the window's natural width.
    fn scroll_thumb_to_current(&self) -> bool {
        self.update_thumb_scroll_position()
    }

    fn schedule_scroll_thumb_to_current(&self) {
        if self.imp().thumb_scroll_scheduled.get() {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_scroll_schedule_skip reason=already_scheduled current={} window=[{}, {}) last_transform={}",
                self.imp().current_index.get(),
                self.imp().thumb_window_start.get(),
                self.imp().thumb_window_end.get(),
                self.imp().thumb_last_transform.get()
            );
            return;
        }

        let weak = self.downgrade();
        let attempts_remaining = Rc::new(Cell::new(THUMB_CENTER_RETRY_FRAMES));
        let schedule_id = self.imp().thumb_scroll_schedule_seq.get() + 1;
        self.imp().thumb_scroll_schedule_seq.set(schedule_id);
        self.imp().thumb_scroll_scheduled.set(true);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_scroll_schedule id={} current={} window=[{}, {}) attempts={} scrolled_alloc={}x{} strip_alloc={}x{} last_transform={}",
            schedule_id,
            self.imp().current_index.get(),
            self.imp().thumb_window_start.get(),
            self.imp().thumb_window_end.get(),
            THUMB_CENTER_RETRY_FRAMES,
            self.imp().thumb_scrolled.get().allocation().width(),
            self.imp().thumb_scrolled.get().allocation().height(),
            self.imp().thumb_strip.get().allocation().width(),
            self.imp().thumb_strip.get().allocation().height(),
            self.imp().thumb_last_transform.get()
        );
        self.imp()
            .thumb_scrolled
            .get()
            .add_tick_callback(move |_, _| {
                let before = attempts_remaining.get();
                let applied = weak
                    .upgrade()
                    .map(|this| {
                        tracing::debug!(
                            target: crate::core::log_targets::VIEWER,
                            "VIEWER_JITTER thumb_scroll_tick id={} attempt={} current={} window=[{}, {}) last_transform={}",
                            schedule_id,
                            THUMB_CENTER_RETRY_FRAMES.saturating_sub(before).saturating_add(1),
                            this.imp().current_index.get(),
                            this.imp().thumb_window_start.get(),
                            this.imp().thumb_window_end.get(),
                            this.imp().thumb_last_transform.get()
                        );
                        this.scroll_thumb_to_current()
                    })
                    .unwrap_or(true);

                let remaining = attempts_remaining.get().saturating_sub(1);
                attempts_remaining.set(remaining);
                let continue_retry = should_retry_thumb_centering(applied, remaining);
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_JITTER thumb_scroll_tick_result id={} applied={} remaining={} continue={}",
                    schedule_id,
                    applied,
                    remaining,
                    continue_retry
                );
                if continue_retry {
                    glib::ControlFlow::Continue
                } else {
                    if let Some(this) = weak.upgrade() {
                        this.imp().thumb_scroll_scheduled.set(false);
                    }
                    glib::ControlFlow::Break
                }
            });
    }

    /// Wire the horizontal adjustment's `value-changed` signal so that
    /// scrolling near either edge of the strip lazy-loads another half-row
    /// of thumbnails (see `try_extend_thumb_window`).
    pub(super) fn setup_thumb_strip_listener(&self) {
        let scrolled = self.imp().thumb_scrolled.get();
        let hadj = scrolled.hadjustment();
        let weak = self.downgrade();
        hadj.connect_value_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.on_thumb_adj_changed();
            }
        });
    }

    fn on_thumb_adj_changed(&self) {
        let imp = self.imp();

        if imp.thumb_programmatic_scroll.get() {
            let hadj = imp.thumb_scrolled.get().hadjustment();
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_adj_skip reason=programmatic value={} page_size={} upper={} current={} window=[{}, {})",
                hadj.value(),
                hadj.page_size(),
                hadj.upper(),
                imp.current_index.get(),
                imp.thumb_window_start.get(),
                imp.thumb_window_end.get()
            );
            return;
        }

        let scrolled = imp.thumb_scrolled.get();
        let hadj = scrolled.hadjustment();
        let value = hadj.value();
        let page_size = hadj.page_size();
        let upper = hadj.upper();
        if page_size <= 0.0 {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_adj_skip reason=no_page_size value={} page_size={} upper={} current={} window=[{}, {})",
                value,
                page_size,
                upper,
                imp.current_index.get(),
                imp.thumb_window_start.get(),
                imp.thumb_window_end.get()
            );
            return;
        }

        // Distance (in pixels) from each scroll edge.
        let left_dist = value;
        let right_dist = upper - value - page_size;
        // Trigger when within ~30% of page size from the edge — far enough
        // that the user has clearly committed to scrolling further, close
        // enough that the rebuild happens before they hit the hard stop.
        let threshold = page_size * 0.3;

        let Some(n_items) = self.list_n_items() else {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_JITTER thumb_adj_skip reason=no_list value={} page_size={} upper={}",
                value,
                page_size,
                upper
            );
            return;
        };
        let start = imp.thumb_window_start.get();
        let end = imp.thumb_window_end.get();
        let mut direction: Option<i8> = None;
        if left_dist < threshold && start > 0 {
            direction = Some(-1);
        }
        if right_dist < threshold && end < n_items {
            // If both edges qualify, pick the one the user is closer to.
            direction = Some(match direction {
                Some(-1) if right_dist < left_dist => 1,
                other => other.unwrap_or(1),
            });
        }

        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_JITTER thumb_adj value={} page_size={} upper={} left_dist={} right_dist={} threshold={} direction={:?} current={} window=[{}, {}) list_len={} pending={:?}",
            value,
            page_size,
            upper,
            left_dist,
            right_dist,
            threshold,
            direction,
            imp.current_index.get(),
            start,
            end,
            n_items,
            imp.thumb_pending_extend.get()
        );
        if let Some(dir) = direction {
            self.try_extend_thumb_window(dir);
        }
    }
}
