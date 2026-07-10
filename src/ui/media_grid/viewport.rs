use super::{tile_intersects_request_window, MediaGrid};
use crate::core::runtime_config;
use crate::ui::square_tile::SquareTile;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;

impl MediaGrid {
    pub(super) fn connect_scroll_handlers(&self) {
        let weak_scroll = self.downgrade();
        self.imp()
            .scroller
            .get()
            .vadjustment()
            .connect_value_changed(move |adj| {
                if let Some(this) = weak_scroll.upgrade() {
                    this.schedule_reprioritize();
                    if !this.imp().restoring_scroll.get() {
                        this.try_load_virtual_page(adj);
                        this.try_expand_render_limit(adj);
                    }
                }
            });

        let weak_map = self.downgrade();
        self.connect_map(move |_| {
            let weak_map = weak_map.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(this) = weak_map.upgrade() {
                    this.reprioritize_visible();
                }
            });
        });
    }

    /// 收集当前落在滚动可视区内的 tile 缓存键。
    ///
    /// 按 section→FlowBox→子节点的文档顺序遍历，对每个 tile 用
    /// `compute_bounds(scroller)` 取其在滚动窗口坐标系下的位置（已含滚动偏移），
    /// 与可见区 `[0, page_size]` 取交集；**越过可见下沿即 return**（后续 tile 更靠下），
    /// 故复杂度是 O(可见) 而非 O(全部)。
    fn collect_visible_cache_keys(&self) -> Vec<String> {
        let scroller = self.imp().scroller.get();
        let page_h = scroller.vadjustment().page_size() as f32;
        const THUMB_REQUEST_OVERSCAN_PAGES: f32 = 1.0;
        let mut keys = Vec::new();
        let content = self.imp().content.get();
        let mut section = content.first_child();
        while let Some(s) = section {
            if let Some(flow) = s.downcast_ref::<gtk::FlowBox>() {
                let mut fc = flow.first_child();
                while let Some(c) = fc {
                    let next = c.next_sibling();
                    if let Some(tile) = c
                        .first_child()
                        .and_then(|t| t.downcast::<SquareTile>().ok())
                    {
                        if let Some(b) = tile.compute_bounds(&scroller) {
                            if b.y() >= page_h * (1.0 + THUMB_REQUEST_OVERSCAN_PAGES) {
                                return keys;
                            }
                            if tile_intersects_request_window(
                                b.y(),
                                b.height(),
                                page_h,
                                THUMB_REQUEST_OVERSCAN_PAGES,
                            ) {
                                tile.request_thumbnail();
                                if let Some(k) = tile.cache_key() {
                                    keys.push(k);
                                }
                            }
                        }
                    }
                    fc = next;
                }
            }
            section = s.next_sibling();
        }
        keys
    }

    /// 把当前可见 tile 的请求提到 worker 队列队首（BOOST），消除分页 rebuild /
    /// 滚动时的优先级倒置。仅对仍在排队、未生成、未在途的 key 生效。
    pub fn reprioritize_visible(&self) {
        let Some(loader) = self.imp().loader.get() else {
            return;
        };
        let span = tracing::debug_span!(
            "grid:reprioritize",
            visible_keys = tracing::field::Empty,
            scroll_y = tracing::field::Empty,
            queue_len = tracing::field::Empty,
            in_flight = tracing::field::Empty,
        );
        let _trace = span.enter();
        let scroll_y = self.imp().scroller.get().vadjustment().value();
        span.record("scroll_y", scroll_y);
        let keys = self.collect_visible_cache_keys();
        span.record("visible_keys", keys.len());
        span.record("queue_len", loader.queue_len());
        span.record("in_flight", loader.in_flight_len());
        if !keys.is_empty() {
            loader.prioritize_keys(&keys);
        }
    }

    /// 去抖调度一次可见区提权：滚动突发期间合并为一次，避免每帧遍历全量 tile。
    fn schedule_reprioritize(&self) {
        let imp = self.imp();
        if imp.reprio_debounce.borrow().is_some() {
            return;
        }
        let weak = self.downgrade();
        let id = gtk::glib::timeout_add_local(
            std::time::Duration::from_millis(runtime_config::grid_reprioritize_debounce_ms()),
            move || {
                if let Some(this) = weak.upgrade() {
                    *this.imp().reprio_debounce.borrow_mut() = None;
                    this.reprioritize_visible();
                }
                gtk::glib::ControlFlow::Break
            },
        );
        *imp.reprio_debounce.borrow_mut() = Some(id);
    }
}

#[cfg(test)]
mod tests;
