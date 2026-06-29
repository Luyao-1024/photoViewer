//! MediaGrid — sectioned thumbnail grid.
//!
//! Photos are grouped by year/month/day. Each section is rendered as a
//! full-width header `GtkLabel` followed by a `GtkFlowBox` of square
//! thumbnails. The sections are stacked vertically inside one
//! `GtkScrolledWindow` (via a `GtkViewport`), so they scroll together.
//!
//! Why headers live OUTSIDE the photo container: a `GtkGridView` cannot make
//! a single item span a full row, so when headers were ordinary grid cells
//! they shared rows with photos, their label height inflated those rows, and
//! the square photos got letterboxed (the "gaps between thumbnails" bug).
//! `GtkFlowBox` with full-width header labels above each section avoids that.
//!
//! ## Sizing
//!
//! Per-view tile size is configured in `spec_for_mode`:
//! - Year  → 90×90 px on screen (thumbnail bucket Small / 256)
//! - Month → 180×180 px (thumbnail bucket Medium / 512)
//! - Day   → 270×270 px (thumbnail bucket Large / 1024)
//!
//! Each tile is a `SquareTile` (see below) — a `GtkWidget` subclass wrapping
//! a `GtkPicture` (`content-fit: cover`) that overrides `measure` to report a
//! fixed square `target × target`. `GtkPicture`'s own natural size is the
//! image's intrinsic size (which would make cells non-square), and `GtkPicture`
//! isn't subclassable in gtk4-rs 0.8, so we wrap it. The `SquareTile` must NOT
//! set a layout manager — GTK4 would otherwise measure via the layout manager
//! and bypass the `measure` override.
//!
//! ## Gap & hover hint
//!
//! The FlowBox `column-spacing` / `row-spacing` (2 px) is the thin separator
//! between tiles. The highlight (a clean accent `outline` on the
//! `flowboxchild` — the same node GTK uses for its keyboard-focus ring, so
//! mouse hover and arrow-key focus look identical) lives in
//! `crate::ui::grid_css`. `selection-mode = None` because
//! `activate-on-single-click` already routes clicks to `child_activated`.
//! `attach_kbd_nav` drives arrow-key cursor movement and hides `:hover` while
//! arrow-keying so the highlight follows the keyboard cursor, not the resting
//! pointer.
//!
//! ## Multi-select
//!
//! `MediaGrid` supports batch operations (e.g. "Add N selected photos to
//! album"). `selection_mode` is `Multiple` on every per-section FlowBox; the
//! `selected` set on `imp` records the *global* indices (into the shared
//! `ListStore`) currently selected. `child_activated` decides between
//! "open viewer" (no modifier) and "toggle membership" (Shift or Ctrl) using
//! the latest modifier mask captured by an `EventControllerKey` on the
//! content box.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::core::i18n::tr;
use crate::core::media::MediaItem;
use crate::core::runtime_config;
use crate::core::section_model::{group_items, GroupBy};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};

/// Get the current max rendered grid items from runtime configuration.
fn max_rendered_grid_items() -> usize {
    runtime_config::max_rendered_grid_items()
}

fn library_stats_text(total_media: usize, generated: usize) -> String {
    format!(
        "媒体 {} 项 · 缩略图 {}/{}",
        total_media,
        generated.min(total_media),
        total_media
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FavoriteMenuState {
    pub can_favorite: bool,
    pub can_unfavorite: bool,
}

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/media-grid.ui")]
    pub struct MediaGrid {
        #[template_child]
        pub content: TemplateChild<gtk::Box>,
        #[template_child]
        pub scroller: TemplateChild<gtk::ScrolledWindow>,
        pub mode: Cell<GroupBy>,
        pub enable_context_menu: Cell<bool>,
        pub active: Cell<bool>,
        pub dirty_model: Cell<bool>,
        pub media_list: RefCell<Option<gio::ListStore>>,
        pub loader: std::cell::OnceCell<Arc<ThumbnailLoader>>,
        pub on_activate: std::cell::OnceCell<Rc<dyn Fn(u32)>>,
        pub on_background_changed: std::cell::OnceCell<Rc<dyn Fn()>>,
        pub on_add_to_album: std::cell::OnceCell<Rc<dyn Fn(Vec<u32>)>>,
        pub on_move_to_trash: std::cell::OnceCell<Rc<dyn Fn(Vec<u32>)>>,
        pub on_set_favorite: std::cell::OnceCell<Rc<dyn Fn(Vec<u32>, bool)>>,
        pub on_query_favorite_state: std::cell::OnceCell<Rc<dyn Fn(Vec<u32>) -> FavoriteMenuState>>,
        /// Flattened `(flow_child, global_index)` for every rendered tile in
        /// current mode.
        pub displayed_items: RefCell<Vec<(gtk::FlowBoxChild, u32)>>,
        /// Global indices (into the shared `ListStore`) currently in the
        /// "selected" set. The set is global — it spans year/month/day
        /// sections, because `PhotosPage` is the only host and it shares one
        /// `ListStore` across the three sub-grids.
        pub selected: RefCell<HashSet<u32>>,
        /// 当前渲染上限：初始 = `max_rendered_grid_items()`；
        /// 滚动接近底部时自动增长（最多到 `ABSOLUTE_RENDERED_LIMIT`）。
        pub rendered_limit: Cell<usize>,
        /// 当前 GTK 模型窗口对应全库排序中的起始 offset。
        pub virtual_window_start: Cell<u32>,
        /// DB 中 live media 的总数，用于虚拟 spacer 和滚动条比例。
        pub virtual_total: Cell<u32>,
        /// 防止滚动事件在上一次 DB page 尚未返回时重复发起加载。
        pub virtual_page_loading: Cell<bool>,
        /// 每次虚拟窗口 DB 请求递增；旧请求返回后若 generation 过期则丢弃。
        pub virtual_page_generation: Cell<u64>,
        /// 替换窗口后按全库比例恢复滚动条位置。
        pub pending_scroll_ratio: Cell<Option<f64>>,
        /// Whether batch mode is explicitly enabled.
        pub is_multi_select_mode: Cell<bool>,
        /// Callback fired whenever `selected` changes. Registered by the host
        /// (`PhotosPage`) so it can show/hide the toolbar "Add to Album"
        /// button and re-render selected state across all three sub-grids.
        pub on_selection_changed: std::cell::OnceCell<Rc<dyn Fn()>>,
        /// 滚动触发「可见区提权」的去抖 SourceId（`Some` = 已挂起，合并突发滚动）。
        /// `SourceId` 非 `Copy`，故用 `RefCell` 而非 `Cell`。
        pub reprio_debounce: RefCell<Option<gtk::glib::SourceId>>,
        /// Day 视图第一栏的统计标签（总数 / 缩略图进度）。rebuild 时重建 widget，
        /// 定时器轮询更新文本。
        pub stats_label: RefCell<Option<gtk::Label>>,
        pub stats_refresh_source: RefCell<Option<gtk::glib::SourceId>>,
        /// Shared model changes can arrive in large bursts during first-start
        /// scanning. Rebuilding all sections for every `items-changed` signal
        /// makes the GTK main thread do O(n²) widget work. Coalesce those
        /// bursts and rebuild once after the model has had a short quiet
        /// window.
        pub rebuild_debounce: RefCell<Option<gtk::glib::SourceId>>,
    }

    impl Default for MediaGrid {
        fn default() -> Self {
            Self {
                content: TemplateChild::default(),
                scroller: TemplateChild::default(),
                mode: Cell::default(),
                enable_context_menu: Cell::new(false),
                active: Cell::new(true),
                dirty_model: Cell::new(false),
                media_list: RefCell::new(None),
                loader: std::cell::OnceCell::new(),
                on_activate: std::cell::OnceCell::new(),
                on_background_changed: std::cell::OnceCell::new(),
                on_add_to_album: std::cell::OnceCell::new(),
                on_move_to_trash: std::cell::OnceCell::new(),
                on_set_favorite: std::cell::OnceCell::new(),
                on_query_favorite_state: std::cell::OnceCell::new(),
                displayed_items: RefCell::new(Vec::new()),
                selected: RefCell::default(),
                rendered_limit: Cell::new(
                    max_rendered_grid_items().min(runtime_config::grid_render_absolute_cap()),
                ),
                virtual_window_start: Cell::new(0),
                virtual_total: Cell::new(0),
                virtual_page_loading: Cell::new(false),
                virtual_page_generation: Cell::new(0),
                pending_scroll_ratio: Cell::new(None),
                is_multi_select_mode: Cell::new(false),
                on_selection_changed: std::cell::OnceCell::new(),
                reprio_debounce: RefCell::new(None),
                stats_label: RefCell::new(None),
                stats_refresh_source: RefCell::new(None),
                rebuild_debounce: RefCell::new(None),
            }
        }
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for MediaGrid {
        const NAME: &'static str = "MediaGrid";
        type Type = super::MediaGrid;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MediaGrid {}
    impl WidgetImpl for MediaGrid {}
    impl BoxImpl for MediaGrid {}
}

gtk::glib::wrapper! {
    pub struct MediaGrid(ObjectSubclass<imp::MediaGrid>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// On-screen pixel size and disk thumbnail bucket per view.
#[derive(Debug, Clone, Copy)]
struct ViewSpec {
    pixel_size: i32,
    thumb_size: ThumbnailSize,
    mode: GroupBy,
}

const VIRTUAL_TILE_GAP: i32 = 2;
const VIRTUAL_PREFETCH_LOW_NUM: u32 = 1;
const VIRTUAL_PREFETCH_HIGH_NUM: u32 = 4;
const VIRTUAL_PREFETCH_DEN: u32 = 5;

fn spec_for_mode(mode: GroupBy) -> ViewSpec {
    // On-screen tile size per view (CSS px). Year shows the most photos so it
    // gets the smallest tiles; Day shows the fewest so it gets the largest.
    // Thumbnail buckets are picked ~2x the display size for retina crispness.
    match mode {
        GroupBy::Year => ViewSpec {
            pixel_size: 90,
            thumb_size: ThumbnailSize::Small,
            mode,
        },
        GroupBy::Month => ViewSpec {
            pixel_size: 180,
            thumb_size: ThumbnailSize::Medium,
            mode,
        },
        GroupBy::Day => ViewSpec {
            pixel_size: 270,
            thumb_size: ThumbnailSize::Medium,
            mode,
        },
    }
}

fn virtual_offset_for_ratio(ratio: f64, total: u32, page_size: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    let max_start = total.saturating_sub(page_size);
    let ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((total as f64 * ratio).floor() as u32).min(max_start)
}

fn virtual_page_start_for_offset(
    desired_offset: u32,
    current_start: u32,
    current_len: u32,
    total: u32,
    page_size: u32,
) -> Option<u32> {
    if total == 0 || current_len == 0 || current_len >= total {
        return None;
    }
    let low =
        current_start + current_len.saturating_mul(VIRTUAL_PREFETCH_LOW_NUM) / VIRTUAL_PREFETCH_DEN;
    let high = current_start
        + current_len.saturating_mul(VIRTUAL_PREFETCH_HIGH_NUM) / VIRTUAL_PREFETCH_DEN;
    if desired_offset >= low && desired_offset <= high {
        return None;
    }

    let centered = desired_offset.saturating_sub(page_size / 2);
    Some(centered.min(total.saturating_sub(page_size)))
}

fn estimated_virtual_columns(viewport_width: f64, spec: ViewSpec) -> u32 {
    let tile = (spec.pixel_size + VIRTUAL_TILE_GAP).max(1) as f64;
    (viewport_width / tile).floor().max(1.0) as u32
}

fn virtual_spacer_height(
    unloaded_items: u32,
    columns: u32,
    _viewport_width: f64,
    spec: ViewSpec,
) -> i32 {
    if unloaded_items == 0 {
        return 0;
    }
    let rows = unloaded_items.div_ceil(columns.max(1));
    let row_height = (spec.pixel_size + VIRTUAL_TILE_GAP).max(1) as u32;
    rows.saturating_mul(row_height).min(i32::MAX as u32) as i32
}

fn virtual_spacer(height: i32) -> gtk::Box {
    let spacer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .can_focus(false)
        .build();
    spacer.set_size_request(-1, height.max(0));
    spacer
}

fn virtual_window_item_count(start: u32, total: u32, page_size: u32) -> u32 {
    if start >= total {
        0
    } else {
        total.saturating_sub(start).min(page_size)
    }
}

fn scroll_ratio_from_adjustment_value(value: f64, upper: f64, page_size: f64) -> f64 {
    let scrollable = (upper - page_size).max(0.0);
    if scrollable > 0.0 {
        (value / scrollable).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn build_virtual_placeholder_flow(spec: ViewSpec, count: u32) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .column_spacing(8)
        .row_spacing(8)
        .max_children_per_line(100)
        .selection_mode(gtk::SelectionMode::None)
        .build();
    flow.add_css_class("thumb-grid");
    flow.add_css_class("virtual-placeholder-grid");

    for _ in 0..count {
        let tile = SquareTile::new();
        tile.set_target(spec.pixel_size);
        tile.add_css_class("thumb-loading");
        tile.add_css_class("thumb-placeholder");
        flow.append(&tile);
    }

    flow
}

impl MediaGrid {
    /// Build a MediaGrid that immediately renders `(media_list, mode)`.
    /// `on_activate` fires with the photo's global index in `media_list`
    /// when the user activates a photo (click without modifier).
    pub fn new(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        on_activate: Rc<dyn Fn(u32)>,
        on_background_changed: Rc<dyn Fn()>,
        on_add_to_album: Rc<dyn Fn(Vec<u32>)>,
        on_move_to_trash: Rc<dyn Fn(Vec<u32>)>,
        on_set_favorite: Rc<dyn Fn(Vec<u32>, bool)>,
        on_query_favorite_state: Rc<dyn Fn(Vec<u32>) -> FavoriteMenuState>,
        enable_context_menu: bool,
    ) -> Self {
        Self::new_with_initial_active(
            media_list,
            mode,
            loader,
            on_activate,
            on_background_changed,
            on_add_to_album,
            on_move_to_trash,
            on_set_favorite,
            on_query_favorite_state,
            enable_context_menu,
            true,
        )
    }

    pub fn new_with_initial_active(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        on_activate: Rc<dyn Fn(u32)>,
        on_background_changed: Rc<dyn Fn()>,
        on_add_to_album: Rc<dyn Fn(Vec<u32>)>,
        on_move_to_trash: Rc<dyn Fn(Vec<u32>)>,
        on_set_favorite: Rc<dyn Fn(Vec<u32>, bool)>,
        on_query_favorite_state: Rc<dyn Fn(Vec<u32>) -> FavoriteMenuState>,
        enable_context_menu: bool,
        initial_active: bool,
    ) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.imp().mode.set(mode);
        obj.imp().active.set(initial_active);
        obj.imp()
            .loader
            .set(loader)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_activate
            .set(on_activate)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_background_changed
            .set(on_background_changed)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_add_to_album
            .set(on_add_to_album)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_move_to_trash
            .set(on_move_to_trash)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_set_favorite
            .set(on_set_favorite)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_query_favorite_state
            .set(on_query_favorite_state)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp().enable_context_menu.set(enable_context_menu);
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());

        crate::ui::grid_css::install();
        if initial_active {
            obj.rebuild(media_list.clone(), mode);
        } else {
            obj.imp().dirty_model.set(true);
        }
        let weak = obj.downgrade();
        media_list.connect_items_changed(move |list, position, removed, added| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_DEBUG grid model_changed mode={:?} position={} removed={} added={} list_len={}",
                this.mode(),
                position,
                removed,
                added,
                list.n_items()
            );
            *this.imp().media_list.borrow_mut() = Some(list.clone());
            if this.imp().active.get() {
                if removed > 0 {
                    // 有项被移除或全量替换 → 必须重建。
                    this.rebuild_immediately(list.clone());
                }
                // 单纯追加（removed == 0）：不重建。新项在 rendered_limit 之外，
                // 待用户滚到底时由 try_expand_render_limit 触发单次重建统一拾取。
                // 否则后台分页加载 500+ 页，每页触发一次全量重建，会持续闪烁 + 丢滚动位。
            } else {
                this.imp().dirty_model.set(true);
            }
        });

        // 滚动时去抖触发可见区提权（B6）：可见 tile 的请求提到队首，
        // 消除分页 rebuild / 快速滚动时的优先级倒置。
        // 同时检测「接近底部」：若距底部 3 屏内，动态扩大渲染上限。
        let weak_scroll = obj.downgrade();
        obj.imp()
            .scroller
            .get()
            .vadjustment()
            .connect_value_changed(move |adj| {
                if let Some(this) = weak_scroll.upgrade() {
                    this.schedule_reprioritize();
                    this.try_load_virtual_page(adj);
                    this.try_expand_render_limit(adj);
                }
            });
        obj
    }

    pub fn set_mode(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        self.imp().mode.set(mode);
        *self.imp().media_list.borrow_mut() = Some(media_list.clone());
        self.rebuild(media_list, mode);
    }

    pub fn set_active(&self, active: bool) {
        self.imp().active.set(active);
        if active && self.imp().dirty_model.replace(false) {
            if let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() {
                self.rebuild_immediately(media_list);
            }
        }
    }

    pub fn mode(&self) -> GroupBy {
        self.imp().mode.get()
    }

    /// Register a callback fired whenever the selected set changes. `PhotosPage`
    /// uses this to toggle the "Add to Album" toolbar button.
    pub fn connect_selection_changed<F: Fn() + 'static>(&self, f: F) {
        self.imp()
            .on_selection_changed
            .set(Rc::new(f))
            .ok()
            .expect("MediaGrid::connect_selection_changed called more than once");
    }

    /// Snapshot of currently-selected global indices. Order is unspecified.
    pub fn selected_indices(&self) -> Vec<u32> {
        let s = self.imp().selected.borrow();
        s.iter().copied().collect()
    }

    /// Snapshot of currently rendered global indices in grid order.
    pub fn displayed_indices(&self) -> Vec<u32> {
        self.imp()
            .displayed_items
            .borrow()
            .iter()
            .map(|(_, gi)| *gi)
            .collect()
    }

    /// Select all rendered tiles and sync visible highlights.
    pub fn select_all(&self) {
        self.imp().is_multi_select_mode.set(true);
        let mut next = HashSet::new();
        let items = self.imp().displayed_items.borrow().clone();
        for (flow_child, gi) in items {
            if let Some(parent) = flow_child.parent() {
                if let Ok(flow) = parent.downcast::<gtk::FlowBox>() {
                    flow.select_child(&flow_child);
                }
            }
            next.insert(gi);
        }

        let mut changed = false;
        {
            let mut selected = self.imp().selected.borrow_mut();
            if *selected != next {
                *selected = next;
                changed = true;
            }
        }
        if changed {
            self.fire_selection_changed();
        }
    }

    /// Enable/disable explicit multi-select mode.
    /// Disabling clears selection for a clean single-select state.
    pub fn set_multi_select_mode(&self, enabled: bool) {
        self.imp().is_multi_select_mode.set(enabled);
        if !enabled {
            self.clear_selection();
        }
    }

    /// Whether explicit multi-select mode is enabled.
    pub fn is_multi_select_mode(&self) -> bool {
        self.imp().is_multi_select_mode.get()
    }

    /// Whether every currently rendered tile is selected.
    pub fn is_all_displayed_selected(&self) -> bool {
        let selected = self.imp().selected.borrow();
        let displayed = self.imp().displayed_items.borrow();
        if displayed.is_empty() {
            return false;
        }
        for (_, gi) in displayed.iter() {
            if !selected.contains(gi) {
                return false;
            }
        }
        true
    }

    fn selected_indices_sorted(&self) -> Vec<u32> {
        let mut indices: Vec<u32> = self.imp().selected.borrow().iter().copied().collect();
        indices.sort_unstable();
        indices
    }

    /// Clear the selection (both in the `selected` set AND on every visible
    /// `FlowBox`). Fires the `selection-changed` callback if the set was
    /// non-empty before.
    pub fn clear_selection(&self) {
        self.imp().is_multi_select_mode.set(false);
        let mut changed = false;
        {
            let mut s = self.imp().selected.borrow_mut();
            if !s.is_empty() {
                s.clear();
                changed = true;
            }
        }
        // Unselect all visible FlowBox children so the highlight follows.
        let content = self.imp().content.get();
        let mut child = content.first_child();
        while let Some(c) = child {
            if let Some(flow) = c.downcast_ref::<gtk::FlowBox>() {
                flow.unselect_all();
            }
            child = c.next_sibling();
        }
        if changed {
            self.fire_selection_changed();
        }
    }

    fn ensure_context_selection(
        &self,
        flow: &gtk::FlowBox,
        clicked_child: &gtk::FlowBoxChild,
        global_index: u32,
    ) -> Vec<u32> {
        let was_selected = self.imp().selected.borrow().contains(&global_index);
        if !was_selected {
            {
                let mut s = self.imp().selected.borrow_mut();
                s.clear();
                s.insert(global_index);
            }
            let content = self.imp().content.get();
            let mut section_child = content.first_child();
            while let Some(child) = section_child {
                if let Some(flow_box) = child.downcast_ref::<gtk::FlowBox>() {
                    flow_box.unselect_all();
                }
                section_child = child.next_sibling();
            }
            flow.select_child(clicked_child);
            self.fire_selection_changed();
        }
        self.selected_indices_sorted()
    }

    /// Notify whenever the scrolled viewport moves. `PhotosPage` uses this to
    /// re-evaluate whether the floating mode selector is over a light or dark
    /// thumbnail.
    pub fn connect_view_changed<F: Fn() + 'static>(&self, f: F) {
        let cb: Rc<dyn Fn()> = Rc::new(f);
        let adjustment = self.imp().scroller.get().vadjustment();
        adjustment.connect_value_changed(move |_| {
            cb();
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
                            // 越过可见下沿：后续 tile 更靠下，整体结束。
                            if b.y() >= page_h {
                                return keys;
                            }
                            // 与可见区 [0, page_h] 有交集即视为可见。
                            if b.y() + b.height() > 0.0 {
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
        let keys = self.collect_visible_cache_keys();
        if !keys.is_empty() {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_DEBUG reprioritize_visible count={} scroll_y={}",
                keys.len(),
                self.imp().scroller.get().vadjustment().value()
            );
            loader.prioritize_keys(&keys);
        }
    }

    /// 去抖调度一次可见区提权：滚动突发期间合并为一次，避免每帧遍历全量 tile。
    fn schedule_reprioritize(&self) {
        let imp = self.imp();
        if imp.reprio_debounce.borrow().is_some() {
            return; // 已挂起，合并本次
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

    /// 如果滚动接近底部（距底部 3 屏内），动态扩大 `rendered_limit` 并触发 rebuild。
    fn try_expand_render_limit(&self, adj: &gtk::Adjustment) {
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
        self.imp().rendered_limit.set(new_limit);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "VIEWER_DEBUG expand_render_limit old={limit} new={new_limit} scroll_y={}",
            adj.value()
        );
        // 触发 rebuild，让新 limit 生效
        if let Some(list) = self.imp().media_list.borrow().as_ref().cloned() {
            self.schedule_rebuild(list);
        }
    }

    fn try_load_virtual_page(&self, adj: &gtk::Adjustment) {
        let total = self.imp().virtual_total.get();
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let current_len = list.n_items();
        if total <= current_len || current_len == 0 {
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
        self.imp().virtual_page_generation.set(generation);
        self.imp().virtual_window_start.set(target_start);
        self.imp().virtual_page_loading.set(true);
        self.imp().pending_scroll_ratio.set(Some(ratio));
        self.rebuild_immediately(list.clone());
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "VIRTUAL_SCROLL load_page generation={generation} ratio={ratio:.4} desired_offset={desired_offset} current_start={current_start} current_len={current_len} target_start={target_start} total={total}"
        );

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let pool = loader.pool().clone();
            let result = gtk::gio::spawn_blocking(move || {
                crate::core::db::list_media_page(&pool, target_start, virtual_page_size)
            })
            .await;
            let items = match result {
                Ok(Ok(items)) => items,
                Ok(Err(err)) => {
                    tracing::warn!("virtual media page load failed: {err}");
                    Vec::new()
                }
                Err(err) => {
                    tracing::warn!("virtual media page load join failed: {err:?}");
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
                return;
            }
            this.imp().virtual_page_loading.set(false);
            if items.is_empty() {
                let list = this.imp().media_list.borrow().as_ref().cloned();
                if let Some(list) = list {
                    this.rebuild_immediately(list);
                }
                return;
            }
            this.imp().virtual_window_start.set(target_start);
            let additions: Vec<glib::BoxedAnyObject> =
                items.into_iter().map(glib::BoxedAnyObject::new).collect();
            let list = this.imp().media_list.borrow().as_ref().cloned();
            if let Some(list) = list {
                list.splice(0, list.n_items(), &additions);
            }
        });
    }

    /// Return the brightness class of the visible tile currently underneath
    /// `selector`, if a loaded tile overlaps it. `None` means there is no tile
    /// under the floating selector yet/anymore, so callers should keep the
    /// default dark-background foreground.
    pub fn background_is_light_under(
        &self,
        selector: &crate::ui::mode_selector::ModeSelector,
    ) -> Option<bool> {
        let selector_bounds = selector.compute_bounds(self)?;
        let selector_mid_x = selector_bounds.x() + selector_bounds.width() / 2.0;
        let selector_mid_y = selector_bounds.y() + selector_bounds.height() / 2.0;

        let content = self.imp().content.get();
        let mut section_child = content.first_child();
        while let Some(child) = section_child {
            if let Some(flow) = child.downcast_ref::<gtk::FlowBox>() {
                let mut flow_child = flow.first_child();
                while let Some(c) = flow_child {
                    let next = c.next_sibling();
                    if let Some(bounds) = c.compute_bounds(self) {
                        let contains_center = selector_mid_x >= bounds.x()
                            && selector_mid_x <= bounds.x() + bounds.width()
                            && selector_mid_y >= bounds.y()
                            && selector_mid_y <= bounds.y() + bounds.height();
                        if contains_center {
                            return c
                                .first_child()
                                .and_then(|tile| tile.downcast::<SquareTile>().ok())
                                .and_then(|tile| tile.background_is_light());
                        }
                    }
                    flow_child = next;
                }
            }
            section_child = child.next_sibling();
        }
        None
    }

    /// Fire the `selection-changed` callback (if registered). Called whenever
    /// `selected` is mutated.
    fn fire_selection_changed(&self) {
        if let Some(cb) = self.imp().on_selection_changed.get() {
            cb();
        }
    }

    /// Toggle membership of `global_index` in the selected set, then toggle
    /// the visual highlight on `child` via its parent `FlowBox`. Fires
    /// `selection-changed`.
    fn toggle_selection(&self, global_index: u32, child: &gtk::FlowBoxChild, flow: &gtk::FlowBox) {
        let now_selected = {
            let mut s = self.imp().selected.borrow_mut();
            if s.contains(&global_index) {
                s.remove(&global_index);
                false
            } else {
                s.insert(global_index);
                true
            }
        };
        if now_selected {
            flow.select_child(child);
        } else {
            flow.unselect_child(child);
        }
        self.fire_selection_changed();
    }

    /// Tear down the current sections and rebuild them for `mode`.
    fn rebuild(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        let rebuild_started = std::time::Instant::now();
        let source_len = media_list.n_items();
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
        let enable_context_menu = self.imp().enable_context_menu.get();

        let spec = spec_for_mode(mode);

        // 重建前保存滚动位置，避免因清空/重建子 widget 导致 adjustment.value 归零。
        let saved_scroll = self.imp().scroller.get().vadjustment().value();

        let content = self.imp().content.get();
        if let Some(source) = self.imp().stats_refresh_source.borrow_mut().take() {
            source.remove();
        }
        self.imp().stats_label.borrow_mut().take();
        // Clear any previously built sections.
        while let Some(child) = content.first_child() {
            content.remove(&child);
        }
        self.imp().displayed_items.borrow_mut().clear();

        // Extract MediaItems + a uri→global-index lookup from the store.
        let extract_started = std::time::Instant::now();
        let mut items = extract_items(&media_list);
        let extract_elapsed = extract_started.elapsed();
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
        let uri_to_index = uri_index_map(&media_list);
        let total_media_count = crate::core::db::count_live_media(loader.pool())
            .unwrap_or(items.len())
            .max(items.len()) as u32;
        self.imp().virtual_total.set(total_media_count);
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

        // Group by year/month/day, then emit header + FlowBox per section.
        let mut section_count = 0u32;
        let mut photo_count = 0u32;
        let mut displayed_items = Vec::new();
        // Day 视图第一组头部下方插入统计栏
        let stats_emitted = std::cell::Cell::new(false);
        let total_media = total_media_count as usize;
        if loading_placeholder_count > 0 {
            if mode == GroupBy::Day && !stats_emitted.replace(true) {
                let generated = loader.generated_count();
                let stats_label = gtk::Label::builder()
                    .label(&library_stats_text(total_media, generated))
                    .halign(gtk::Align::Center)
                    .hexpand(true)
                    .margin_bottom(14)
                    .xalign(0.5)
                    .justify(gtk::Justification::Center)
                    .css_classes(["library-stats", "glass-raised"])
                    .build();
                content.append(&stats_label);
                *self.imp().stats_label.borrow_mut() = Some(stats_label.clone());
                self.start_stats_refresh(loader.clone(), total_media);
            }

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
            let sections = group_items(&items, mode);
            for section in sections {
                if section.items.is_empty() {
                    continue;
                }

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

                // Day 视图：第一组 header 下方插入库统计栏
                if mode == GroupBy::Day && !stats_emitted.replace(true) {
                    let generated = loader.generated_count();
                    let stats_label = gtk::Label::builder()
                        .label(&library_stats_text(total_media, generated))
                        .halign(gtk::Align::Center)
                        .hexpand(true)
                        .margin_bottom(14)
                        .xalign(0.5)
                        .justify(gtk::Justification::Center)
                        .css_classes(["library-stats", "glass-raised"])
                        .build();
                    content.append(&stats_label);
                    *self.imp().stats_label.borrow_mut() = Some(stats_label.clone());
                    self.start_stats_refresh(loader.clone(), total_media);
                }

                // FlowBox of square thumbnails. `homogeneous` makes every cell the
                // same size; with each picture's `set_size_request(target)` the
                // cells become target×target squares. `column/row spacing` is the
                // thin separator (≤3px); hover styling lives in grid_css.
                //
                // `selection_mode = Multiple` so the FlowBox itself tracks which
                // children are visually highlighted. We mirror that into
                // `imp.selected` for our own bookkeeping. The visual highlight
                // reuses the default `flowboxchild:selected` style; the focus
                // ring (driven by `:hover` / `:focus` in grid_css) is unchanged.
                let flow = gtk::FlowBox::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .homogeneous(true)
                    .column_spacing(8)
                    .row_spacing(8)
                    .max_children_per_line(100)
                    .selection_mode(gtk::SelectionMode::Multiple)
                    .build();
                flow.set_activate_on_single_click(true);
                flow.add_css_class("thumb-grid");
                // While arrow-keying between tiles, hide the `:hover` hint so the
                // highlight follows the keyboard focus, not the resting pointer.
                crate::ui::grid_css::attach_kbd_nav(&flow);

                // Build tiles + remember each child's global index for activation.
                let mut global_indices: Vec<u32> = Vec::with_capacity(section.items.len());
                let mut activation_items: Vec<(i64, String, String)> =
                    Vec::with_capacity(section.items.len());
                for item in &section.items {
                    let gi = uri_to_index.get(&item.uri).copied().unwrap_or(u32::MAX);
                    global_indices.push(gi);
                    activation_items.push((
                        item.id,
                        item.display_name().to_string(),
                        item.uri.clone(),
                    ));
                    let on_bg = self
                        .imp()
                        .on_background_changed
                        .get()
                        .expect("MediaGrid::rebuild called before new()")
                        .clone();
                    let picture = build_photo_picture(
                        spec,
                        item.clone(),
                        media_list.clone(),
                        gi,
                        loader.clone(),
                        on_bg,
                    );
                    flow.append(&picture);
                    if let Some(flow_child) = flow
                        .last_child()
                        .and_then(|w| w.downcast::<gtk::FlowBoxChild>().ok())
                    {
                        if gi != u32::MAX {
                            displayed_items.push((flow_child.clone(), gi));
                        }
                    }
                    photo_count += 1;
                }

                // Activation: FlowBox child-activated → look up global index.
                // Only explicit multi-select mode (entered via right-click “Multi-select”)
                // toggles selection; otherwise the item opens in viewer.
                let on_act = on_activate.clone();
                let weak = self.downgrade();
                let global_indices_for_activation = global_indices.clone();
                let global_indices_for_context = global_indices;
                let section_label_for_activation = section.label.clone();
                flow.connect_child_activated(move |flow, child| {
                let idx = child.index();
                if idx < 0 {
                    return;
                }
                let Some(&gi) = global_indices_for_activation.get(idx as usize) else {
                    return;
                };
                let (item_id, item_name, item_uri) = activation_items
                    .get(idx as usize)
                    .map(|(id, name, uri)| (*id, name.as_str(), uri.as_str()))
                    .unwrap_or((-1, "(missing)", "(missing)"));
                let Some(this) = weak.upgrade() else {
                    return;
                };
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
                    item_name,
                    item_uri,
                    is_multi
                );
                if is_multi {
                    // `flow` comes from the signal arg, no extra upgrade needed.
                    this.toggle_selection(gi, child, flow);
                } else {
                    on_act(gi);
                }
            });

                if enable_context_menu {
                    let weak_for_context = self.downgrade();
                    let section_label_for_ctx = section.label.clone();
                    let global_indices_for_context = global_indices_for_context.clone();
                    let flow_for_ctx = flow.clone();
                    let on_add_to_album_ctx = on_add_to_album.clone();
                    let on_move_to_trash_ctx = on_move_to_trash.clone();
                    let on_set_favorite_ctx = on_set_favorite.clone();
                    let on_query_favorite_state_ctx = on_query_favorite_state.clone();
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

                    let Some(flow_child_for_ctx) = flow_for_ctx.child_at_pos(x as i32, y as i32) else {
                        return;
                    };
                    let hit_idx = match flow_child_for_ctx.index() {
                        idx if idx >= 0 => idx as usize,
                        _ => return,
                    };

                    let gi = match global_indices_for_context.get(hit_idx).copied() {
                        Some(index) if index != u32::MAX => index,
                        _ => return,
                    };

                    let in_multi_mode = this.is_multi_select_mode();
                    let target_indices = if in_multi_mode {
                        this.ensure_context_selection(&flow_for_ctx, &flow_child_for_ctx, gi)
                    } else {
                        vec![gi]
                    };
                    let favorite_state = (on_query_favorite_state_ctx)(target_indices.clone());
                    let child_alloc = flow_child_for_ctx.allocation();
                    let point_in_child = gdk::Rectangle::new(
                        (x as i32 - child_alloc.x()).max(0),
                        (y as i32 - child_alloc.y()).max(0),
                        1,
                        1,
                    );
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "VIEWER_DEBUG context_menu mode={:?} section={} selected={:?} multi_select={}",
                        this.mode(),
                        section_label_for_ctx,
                        target_indices,
                        in_multi_mode,
                    );

                    let popover = gtk::Popover::new();
                    popover.set_parent(&flow_child_for_ctx);
                    popover.add_css_class("glass-menu");
                    popover.set_has_arrow(false);
                    popover.set_autohide(true);
                    popover.set_position(gtk::PositionType::Bottom);
                    // set_offset / set_pointing_to are deferred until after
                    // set_child so we can `measure()` the popover's actual
                    // width and shift it so its top-left lands on the click
                    // point (the default Bottom position centers the popover
                    // on the pointing rect, which is what puts the menu's
                    // middle under the mouse).

                    let menu = gtk::Box::builder()
                        .orientation(gtk::Orientation::Vertical)
                        .spacing(2)
                        .css_classes(["glass-menu-list"])
                        .build();

                    // Multi-select / Exit Multi-select.
                    if in_multi_mode {
                        let exit_btn = gtk::Button::builder()
                            .label(tr("photos.batch.exit_multi_select"))
                            .css_classes(["glass-menu-item", "glass-menu-item-danger"])
                            .build();

                        let popover_exit = popover.clone();
                        let weak_exit = weak_for_context.clone();
                        exit_btn.connect_clicked(move |_| {
                            if let Some(this) = weak_exit.upgrade() {
                                this.set_multi_select_mode(false);
                            }
                            popover_exit.popdown();
                        });
                        menu.append(&exit_btn);
                    } else {
                        let multi_btn = gtk::Button::builder()
                            .label(tr("photos.batch.multi_select"))
                            .css_classes(["glass-menu-item", "glass-menu-item-suggested"])
                            .build();

                        let weak_enter = weak_for_context.clone();
                        let flow_for_ctx_enter = flow_for_ctx.clone();
                        let flow_child_for_ctx_enter = flow_child_for_ctx.clone();
                        let popover_enter = popover.clone();
                        multi_btn.connect_clicked(move |_| {
                            if let Some(this) = weak_enter.upgrade() {
                                this.set_multi_select_mode(true);
                                this.ensure_context_selection(
                                    &flow_for_ctx_enter,
                                    &flow_child_for_ctx_enter,
                                    gi,
                                );
                            }
                            popover_enter.popdown();
                        });
                        menu.append(&multi_btn);
                    }

                    // Favorite / Unfavorite (single and batch context).
                    if favorite_state.can_favorite {
                        let favorite_btn = gtk::Button::builder()
                            .label(tr("photos.batch.favorite"))
                            .css_classes(["glass-menu-item"])
                            .build();
                        let indices_for_fav = target_indices.clone();
                        let on_set_favorite_fav = on_set_favorite_ctx.clone();
                        let popover_fav = popover.clone();
                        favorite_btn.connect_clicked(move |_| {
                            on_set_favorite_fav(indices_for_fav.clone(), true);
                            popover_fav.popdown();
                        });
                        menu.append(&favorite_btn);
                    }
                    if favorite_state.can_unfavorite {
                        let unfav_btn = gtk::Button::builder()
                            .label(tr("photos.batch.unfavorite"))
                            .css_classes(["glass-menu-item"])
                            .build();
                        let indices_for_unfav = target_indices.clone();
                        let on_set_favorite_unfav = on_set_favorite_ctx.clone();
                        let popover_unfav = popover.clone();
                        unfav_btn.connect_clicked(move |_| {
                            on_set_favorite_unfav(indices_for_unfav.clone(), false);
                            popover_unfav.popdown();
                        });
                        menu.append(&unfav_btn);
                    }

                    if !target_indices.is_empty() {
                        let move_album_btn = gtk::Button::builder()
                            .label(tr("photos.batch.move_to_album"))
                            .css_classes(["glass-menu-item"])
                            .build();
                        let indices_for_album = target_indices.clone();
                        let on_add_to_album_ctx = on_add_to_album_ctx.clone();
                        let popover_album = popover.clone();
                        move_album_btn.connect_clicked(move |_| {
                            on_add_to_album_ctx(indices_for_album.clone());
                            popover_album.popdown();
                        });
                        menu.append(&move_album_btn);

                        let delete_btn = gtk::Button::builder()
                            .label(tr("viewer.tooltip.move_to_trash"))
                            .css_classes(["glass-menu-item", "glass-menu-item-danger"])
                            .build();
                        let indices_for_trash = target_indices.clone();
                        let on_move_to_trash_ctx = on_move_to_trash_ctx.clone();
                        let popover_trash = popover.clone();
                        let grid_weak = this.downgrade();
                        delete_btn.connect_clicked(move |_| {
                            popover_trash.popdown();

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
                        });
                        menu.append(&delete_btn);
                    }

                    popover.set_child(Some(&menu));

                    // Anchor the popover's VISIBLE top-left at the click
                    // point.
                    //
                    // GTK4's `PositionType::Bottom` aligns the popover's
                    // top-center with the center of the pointing rect, so
                    // without an offset the popover's geometric center sits
                    // under the mouse. The naive correction is
                    // `set_offset(popover_w / 2, 0)`, but the offset has to
                    // be set BEFORE `popup()` — applying it after causes a
                    // visible "jump" because the popover is briefly mapped
                    // at the default position.
                    //
                    // The naive `popover_w / 2` still lands 25 px to the
                    // right and 5 px below the click because of how GTK4
                    // accounts for the 1×1 pointing rect and the shadow
                    // buffer around the popover. Those two constants were
                    // measured empirically (see `popover_actual` diagnostic
                    // logs in the dev history) and baked in below. If the
                    // GTK theme or popover CSS changes, retune by setting
                    // `set_offset(0, 0)` here, right-clicking, and reading
                    // off the `popover_actual` delta.
                    let (menu_min, menu_nat, _, _) =
                        menu.measure(gtk::Orientation::Horizontal, -1);
                    let menu_w = menu_min.max(menu_nat).max(1);
                    let popover_w = menu_w.max(160); // CSS `min-width: 160px`
                    let half_w = popover_w / 2;
                    const POPOVER_X_BIAS_PX: i32 = 25;
                    const POPOVER_Y_BIAS_PX: i32 = -5; // shift up by 5
                    popover.set_offset(half_w - POPOVER_X_BIAS_PX, POPOVER_Y_BIAS_PX);
                    popover.set_pointing_to(Some(&point_in_child));

                    popover.popup();
                });

                    flow.add_controller(gesture);
                }

                content.append(&flow);
                section_count += 1;
            }
        }
        let rendered_window_len = if loading_placeholder_count > 0 {
            loading_placeholder_count
        } else {
            items.len() as u32
        };
        let loaded_end = window_start.saturating_add(rendered_window_len);
        let bottom_unloaded = total_media_count.saturating_sub(loaded_end);
        let bottom_spacer_height =
            virtual_spacer_height(bottom_unloaded, virtual_columns, viewport_width, spec);
        if bottom_spacer_height > 0 {
            content.append(&virtual_spacer(bottom_spacer_height));
        }
        *self.imp().displayed_items.borrow_mut() = displayed_items;

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
                    adj.set_value(restored);
                }
            });
        }

        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "MediaGrid::rebuild mode={:?} source_len={} sections={} photos={} spec.pixel_size={} extract_ms={} total_ms={}",
            mode,
            source_len,
            section_count,
            photo_count,
            spec.pixel_size,
            extract_elapsed.as_millis(),
            rebuild_started.elapsed().as_millis()
        );

        // rebuild 后调度一次可见区提权（去抖）：让当前可见 tile 先于屏幕外的被生成。
        self.schedule_reprioritize();
    }

    fn start_stats_refresh(&self, loader: Arc<ThumbnailLoader>, total_media: usize) {
        if total_media == 0 {
            return;
        }

        let weak = self.downgrade();
        let source = glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let generated = loader.generated_count();
            if let Some(label) = this.imp().stats_label.borrow().as_ref() {
                label.set_label(&library_stats_text(total_media, generated));
            } else {
                return glib::ControlFlow::Break;
            }
            if generated >= total_media {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        *self.imp().stats_refresh_source.borrow_mut() = Some(source);
    }

    fn schedule_rebuild(&self, media_list: gtk::gio::ListStore) {
        if self.imp().rebuild_debounce.borrow().is_some() {
            return;
        }

        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(Duration::from_millis(750), move || {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.imp().rebuild_debounce.borrow_mut().take();
            this.clear_selection();
            this.rebuild(media_list, this.mode());
        });
        *self.imp().rebuild_debounce.borrow_mut() = Some(source);
    }

    fn rebuild_immediately(&self, media_list: gtk::gio::ListStore) {
        if let Some(source) = self.imp().rebuild_debounce.borrow_mut().take() {
            source.remove();
        }
        self.clear_selection();
        self.rebuild(media_list, self.mode());
    }
}

/// Square thumbnail: a `GtkWidget` subclass that reports a fixed square size
/// (overriding `measure`), holding one `GtkPicture` (`content-fit: cover`).
///
/// `GtkPicture`'s natural size is the image's intrinsic size, so a bare
/// picture makes FlowBox/GridView cells non-square; and `GtkPicture` isn't
/// subclassable in gtk4-rs 0.8. So we wrap the picture in a widget that
/// reports `target × target`. FlowBox measures its children directly, so this
/// override takes effect and every cell comes out square.
///
/// `pub` so other pages (e.g. the trash grid) can reuse it for cover thumbnails
/// that must also be 1:1 squares — see `CLAUDE.md` "Day-view grid sizing
/// gotcha" for the underlying GTK4 sizing pitfall.
pub mod square_tile {
    use super::*;
    use std::cell::{Cell, RefCell};

    mod imp {
        use super::*;

        pub struct SquareTile {
            pub picture: RefCell<Option<gtk::Picture>>,
            /// 半透明白色勾选标记，浮于缩略图右下角；始终 parented/allocated，
            /// 通过 CSS（flowboxchild:selected .thumb-checkmark）控制显隐，
            /// 仅在选中时可见。见 grid_css 的 .thumb-checkmark 规则。
            pub checkmark: RefCell<Option<gtk::Image>>,
            pub motion_badge: RefCell<Option<gtk::Image>>,
            pub duration_badge: RefCell<Option<gtk::Label>>,
            pub favorite_badge: RefCell<Option<gtk::Image>>,
            pub target: Cell<i32>,
            pub background_is_light: Cell<Option<bool>>,
            /// 该 tile 的缩略图缓存键（建 tile 时预算，用于可见区提权匹配队列项）。
            pub cache_key: RefCell<Option<String>>,
        }

        impl Default for SquareTile {
            fn default() -> Self {
                Self {
                    picture: RefCell::new(None),
                    checkmark: RefCell::new(None),
                    motion_badge: RefCell::new(None),
                    duration_badge: RefCell::new(None),
                    favorite_badge: RefCell::new(None),
                    target: Cell::new(90),
                    background_is_light: Cell::new(None),
                    cache_key: RefCell::new(None),
                }
            }
        }

        #[gtk::glib::object_subclass]
        impl ObjectSubclass for SquareTile {
            const NAME: &'static str = "PvSquareTile";
            type Type = super::SquareTile;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for SquareTile {
            fn constructed(&self) {
                self.parent_constructed();
                let obj = self.obj();
                let picture = gtk::Picture::builder()
                    .content_fit(gtk::ContentFit::Cover)
                    .can_shrink(true)
                    .build();
                obj.add_css_class("thumb-tile");
                obj.add_css_class("glass-thumb-card");
                obj.set_overflow(gtk::Overflow::Hidden);
                picture.add_css_class("thumb-image");
                picture.set_parent(&*obj);
                *self.picture.borrow_mut() = Some(picture);

                // Selection checkmark: a translucent-white tick pinned to the
                // bottom-right, drawn above the picture. It is always
                // parented/allocated but invisible (opacity 0) until the
                // wrapping FlowBoxChild becomes :selected, when CSS reveals it.
                // Parented after the picture so GTK draws it on top.
                let checkmark = gtk::Image::builder()
                    .icon_name("object-select-symbolic")
                    .pixel_size(22)
                    .build();
                checkmark.add_css_class("thumb-checkmark");
                checkmark.set_parent(&*obj);
                *self.checkmark.borrow_mut() = Some(checkmark);

                let motion_badge = gtk::Image::builder()
                    .icon_name("media-playback-start-symbolic")
                    .pixel_size(18)
                    .visible(false)
                    .build();
                motion_badge.add_css_class("thumb-motion-badge");
                motion_badge.set_parent(&*obj);
                *self.motion_badge.borrow_mut() = Some(motion_badge);

                let duration_badge = gtk::Label::builder()
                    .visible(false)
                    .halign(gtk::Align::Start)
                    .valign(gtk::Align::End)
                    .build();
                duration_badge.add_css_class("thumb-video-duration");
                duration_badge.set_parent(&*obj);
                *self.duration_badge.borrow_mut() = Some(duration_badge);

                let favorite_badge = gtk::Image::builder()
                    .icon_name("emblem-favorite-symbolic")
                    .pixel_size(20)
                    .visible(false)
                    .build();
                favorite_badge.add_css_class("thumb-favorite-badge");
                favorite_badge.set_parent(&*obj);
                *self.favorite_badge.borrow_mut() = Some(favorite_badge);
            }

            fn dispose(&self) {
                if let Some(p) = self.picture.borrow_mut().take() {
                    p.unparent();
                }
                if let Some(c) = self.checkmark.borrow_mut().take() {
                    c.unparent();
                }
                if let Some(b) = self.motion_badge.borrow_mut().take() {
                    b.unparent();
                }
                if let Some(d) = self.duration_badge.borrow_mut().take() {
                    d.unparent();
                }
                if let Some(f) = self.favorite_badge.borrow_mut().take() {
                    f.unparent();
                }
            }
        }

        impl WidgetImpl for SquareTile {
            // Fixed square size: `target` in both orientations (height-for-width
            // returns the given width, so it stays square at any column size).
            // NB: do NOT set a layout manager here — GTK4 would then measure via
            // the layout manager and bypass this override.
            fn measure(
                &self,
                orientation: gtk::Orientation,
                for_size: i32,
            ) -> (i32, i32, i32, i32) {
                let target = self.target.get().max(1);
                let size = if orientation == gtk::Orientation::Vertical && for_size > 0 {
                    for_size
                } else {
                    target
                };
                (size, size, -1, -1)
            }

            fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
                if let Some(p) = self.picture.borrow().as_ref() {
                    p.size_allocate(&gtk::Allocation::new(0, 0, width, height), baseline);
                }
                // Pin the checkmark to the bottom-right corner with a small
                // margin. Use its natural (pixel-size) extent, clamped to the
                // tile so it never overflows the clipped card.
                if let Some(c) = self.checkmark.borrow().as_ref() {
                    let (_, cw, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                    let (_, ch, _, _) = c.measure(gtk::Orientation::Vertical, -1);
                    let cw = cw.clamp(1, width);
                    let ch = ch.clamp(1, height);
                    let margin = 6;
                    let x = (width - cw - margin).max(0);
                    let y = (height - ch - margin).max(0);
                    c.size_allocate(&gtk::Allocation::new(x, y, cw, ch), -1);
                }
                if let Some(b) = self.motion_badge.borrow().as_ref() {
                    let (_, bw, _, _) = b.measure(gtk::Orientation::Horizontal, -1);
                    let (_, bh, _, _) = b.measure(gtk::Orientation::Vertical, -1);
                    let bw = bw.clamp(1, width);
                    let bh = bh.clamp(1, height);
                    let margin = 7;
                    let y = (height - bh - margin).max(0);
                    b.size_allocate(&gtk::Allocation::new(margin, y, bw, bh), -1);
                }
                if let Some(d) = self.duration_badge.borrow().as_ref() {
                    let (_, dw, _, _) = d.measure(gtk::Orientation::Horizontal, -1);
                    let (_, dh, _, _) = d.measure(gtk::Orientation::Vertical, -1);
                    let dw = dw.clamp(1, width);
                    let dh = dh.clamp(1, height);
                    let margin = 7;
                    let y = (height - dh - margin).max(0);
                    d.size_allocate(&gtk::Allocation::new(margin, y, dw, dh), -1);
                }
                if let Some(f) = self.favorite_badge.borrow().as_ref() {
                    let (_, fw, _, _) = f.measure(gtk::Orientation::Horizontal, -1);
                    let (_, fh, _, _) = f.measure(gtk::Orientation::Vertical, -1);
                    let fw = fw.clamp(1, width);
                    let fh = fh.clamp(1, height);
                    let margin = 7;
                    let x = (width - fw - margin).max(0);
                    f.size_allocate(&gtk::Allocation::new(x, margin, fw, fh), -1);
                }
            }
        }
    }

    gtk::glib::wrapper! {
        pub struct SquareTile(ObjectSubclass<imp::SquareTile>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Default for SquareTile {
        fn default() -> Self {
            Self::new()
        }
    }

    impl SquareTile {
        pub fn new() -> Self {
            gtk::glib::Object::builder().build()
        }

        pub fn set_target(&self, target: i32) {
            self.imp().target.set(target);
            self.queue_resize();
        }

        pub fn target(&self) -> i32 {
            self.imp().target.get()
        }

        pub fn set_paintable<P: IsA<gtk::gdk::Paintable>>(&self, paintable: Option<&P>) {
            if let Some(p) = self.imp().picture.borrow().as_ref() {
                p.set_paintable(paintable);
            }
            // 设入任意 paintable（真实 texture 或失败灰底）即停止骨架 shimmer。
            self.remove_css_class("thumb-loading");
        }

        pub fn set_background_is_light(&self, is_light: bool) {
            self.imp().background_is_light.set(Some(is_light));
        }

        pub fn background_is_light(&self) -> Option<bool> {
            self.imp().background_is_light.get()
        }

        /// 缩略图缓存键（建 tile 时预算；可见区提权时用它匹配队列项）。
        pub fn set_cache_key(&self, key: Option<String>) {
            *self.imp().cache_key.borrow_mut() = key;
        }

        pub fn cache_key(&self) -> Option<String> {
            self.imp().cache_key.borrow().clone()
        }

        pub fn set_motion_badge_visible(&self, visible: bool) {
            if let Some(badge) = self.imp().motion_badge.borrow().as_ref() {
                badge.set_visible(visible);
            }
        }

        pub fn set_video_duration(&self, label: Option<&str>) {
            if let Some(badge) = self.imp().duration_badge.borrow().as_ref() {
                badge.set_label(label.unwrap_or(""));
                badge.set_visible(label.is_some());
            }
        }

        pub fn set_favorite_badge_visible(&self, visible: bool) {
            if let Some(badge) = self.imp().favorite_badge.borrow().as_ref() {
                badge.set_visible(visible);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[gtk::test]
        fn square_tile_exposes_shared_thumbnail_css_class() {
            let _ = gtk::init();
            let tile = SquareTile::new();

            assert!(tile.has_css_class("thumb-tile"));
        }

        // Task 4: the new three-state CSS in `grid_css` targets
        // `.glass-thumb-card`, which the legacy `.thumb-tile` selector no
        // longer covers. The tile wrapper must carry the new class.
        #[gtk::test]
        fn square_tile_carries_glass_thumb_card_class() {
            let _ = gtk::init();
            let tile = SquareTile::new();

            assert!(tile.has_css_class("glass-thumb-card"));
        }

        #[gtk::test]
        fn square_tile_clips_thumbnail_to_glass_card_radius() {
            let _ = gtk::init();
            let tile = SquareTile::new();

            assert_eq!(tile.overflow(), gtk::Overflow::Hidden);
            let picture = tile
                .imp()
                .picture
                .borrow()
                .as_ref()
                .expect("SquareTile should construct its picture child")
                .clone();
            assert!(picture.has_css_class("thumb-image"));
        }

        // The translucent-white selection checkmark is parented atop the
        // picture in every tile and revealed by CSS on flowboxchild:selected.
        #[gtk::test]
        fn square_tile_has_selection_checkmark_atop_picture() {
            let _ = gtk::init();
            let tile = SquareTile::new();

            let checkmark = tile
                .imp()
                .checkmark
                .borrow()
                .as_ref()
                .expect("SquareTile should construct its checkmark child")
                .clone();
            assert!(checkmark.has_css_class("thumb-checkmark"));
            // Parented (always allocated; visibility driven by CSS opacity)
            // and drawn above the picture.
            assert!(checkmark.parent().is_some());
        }

        #[gtk::test]
        fn square_tile_has_motion_badge_for_dynamic_photos() {
            let _ = gtk::init();
            let tile = SquareTile::new();

            let badge = tile
                .imp()
                .motion_badge
                .borrow()
                .as_ref()
                .expect("SquareTile should construct its motion badge")
                .clone();
            assert!(badge.has_css_class("thumb-motion-badge"));
            assert!(!badge.is_visible());

            tile.set_motion_badge_visible(true);
            assert!(badge.is_visible());
        }

        #[gtk::test]
        fn square_tile_has_duration_and_favorite_badges() {
            let _ = gtk::init();
            let tile = SquareTile::new();

            let duration = tile
                .imp()
                .duration_badge
                .borrow()
                .as_ref()
                .expect("SquareTile should construct its duration badge")
                .clone();
            assert!(duration.has_css_class("thumb-video-duration"));
            assert!(!duration.is_visible());

            tile.set_video_duration(Some("01:23"));
            assert!(duration.is_visible());
            assert_eq!(duration.label(), "01:23");

            let favorite = tile
                .imp()
                .favorite_badge
                .borrow()
                .as_ref()
                .expect("SquareTile should construct its favorite badge")
                .clone();
            assert!(favorite.has_css_class("thumb-favorite-badge"));
            assert!(!favorite.is_visible());

            tile.set_favorite_badge_visible(true);
            assert!(favorite.is_visible());
        }
    }
}

use square_tile::SquareTile;

/// Build one square thumbnail `SquareTile` for a FlowBox cell.
///
/// `can_shrink = true` (the default) is essential: with `false`, GtkPicture
/// reports the thumbnail's intrinsic size as its minimum and the FlowBox cell
/// grows to the full image. The `set_size_request(target, target)` sets the
/// cell size; the FlowBox's `homogeneous` property then makes every cell this
/// size (a square). `content-fit: cover` crops the image to fill it.
fn build_photo_picture(
    spec: ViewSpec,
    item: MediaItem,
    media_list: gio::ListStore,
    global_index: u32,
    loader: Arc<ThumbnailLoader>,
    on_background_changed: Rc<dyn Fn()>,
) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(spec.pixel_size);
    let is_day = spec.mode == GroupBy::Day;
    tile.set_motion_badge_visible(is_day && item.is_motion_photo());
    if is_day && item.is_video() {
        let duration = item
            .video_duration_secs
            .and_then(format_tile_duration)
            .unwrap_or_else(|| "--:--".to_string());
        tile.set_video_duration(Some(&duration));
    }
    tile.set_favorite_badge_visible(is_day && item.is_favorite);

    let fallback_item = item.clone();
    let size = spec.thumb_size;
    let target_px = spec.pixel_size;
    // B5：mtime 已在扫描/notify 时入库（MediaItem.file_mtime），直接复用，
    // 跳过 request 端的主线程 stat。DateTime<Utc> → SystemTime。
    let item_mtime = thumbnail_request_mtime(&item);
    // B6：预算缓存键存到 tile，供可见区提权匹配队列项（带 file_mtime，无主线程 stat）。
    tile.set_cache_key(ThumbnailLoader::cache_key_for(
        &item.uri,
        size,
        Some(item_mtime),
    ));
    // 加载中骨架 shimmer；set_paintable 设入任意 paintable 时移除。
    tile.add_css_class("thumb-loading");

    // 缩略图请求**推迟到 tile 被 map（即真正可见）时**才发出。
    //
    // 三个 grid（年/月/日）共享同一个 ListStore，但 ViewStack 同一时刻只
    // map 可见的那个 grid；隐藏 grid 的 tile 不会 map，因此不会请求缩略图。
    // 这就避免了「隐藏的年/月 grid 与可见的日 grid 抢 worker、并把日视图的
    // 请求挤到队尾」导致的首屏空白。配合 ThumbnailLoader 的在途去重，可见
    // tile 的请求既不会被丢、也优先被生成。`requested` 保证每个 tile 只请求
    // 一次（重新 map 时走 mem_cache/disk 缓存，也不会重复生成）。
    let request_once: Rc<dyn Fn()> = Rc::new({
        let loader = loader.clone();
        let on_background_changed = on_background_changed.clone();
        let tile_weak = tile.downgrade();
        let requested = std::cell::Cell::new(false);
        move || {
            if requested.get() {
                return;
            }
            requested.set(true);

            let current_item = crate::ui::media_list::media_item_at(&media_list, global_index)
                .unwrap_or_else(|| fallback_item.clone());
            let item_name = current_item.display_name().to_string();
            let item_uri = current_item.uri.clone();
            let item_mtime = thumbnail_request_mtime(&current_item);
            let cache_key = ThumbnailLoader::cache_key_for(&item_uri, size, Some(item_mtime));
            if let Some(tile) = tile_weak.upgrade() {
                tile.set_cache_key(cache_key.clone());
            }

            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "THUMB_TRACE grid_request item_id={} item_name={} uri={} size={:?} target_px={} global_index={} media_item_mtime={} request_mtime={:?} cache_key={:?}",
                current_item.id,
                item_name,
                item_uri,
                size,
                target_px,
                global_index,
                current_item.file_mtime,
                item_mtime,
                cache_key
            );

            let (tx, rx) = tokio::sync::oneshot::channel();
            loader.request(
                item_uri.clone(),
                size,
                Some(item_mtime),
                tx,
                crate::core::thumbnails::TIER_NORMAL,
            );
            let tile_weak = tile_weak.clone();
            let on_background_changed = on_background_changed.clone();
            let item_name = item_name.clone();
            let item_uri = item_uri.clone();
            gtk::glib::spawn_future_local(async move {
                match rx.await {
                    Ok(loaded) => {
                        tracing::debug!(
                            target: crate::core::log_targets::BROWSING,
                            "VIEWER_DEBUG thumb loaded item_name={} uri={} texture={}x{}",
                            item_name,
                            item_uri,
                            loaded.texture.width(),
                            loaded.texture.height()
                        );
                        if let Some(t) = tile_weak.upgrade() {
                            // 亮度判定已在 worker 端从 pixbuf 算好随结果回传，主线程不再 download。
                            if let Some(is_light) = loaded.is_light {
                                t.set_background_is_light(is_light);
                                on_background_changed();
                            }
                            t.set_paintable(Some(&loaded.texture));
                        }
                    }
                    Err(_) => {
                        tracing::debug!(
                            target: crate::core::log_targets::BROWSING,
                            "VIEWER_DEBUG thumb dropped→灰底占位 item_name={} uri={}",
                            item_name,
                            item_uri
                        );
                        if let Some(t) = tile_weak.upgrade() {
                            t.set_paintable(Some(&gray_placeholder_texture()));
                        }
                    }
                }
            });
        }
    });

    // tile 被 map 时触发请求；若此刻已 map（防御性），立即触发。
    {
        let request_once = request_once.clone();
        tile.connect_map(move |_| request_once());
    }
    if tile.is_mapped() {
        request_once();
    }
    tile
}

fn format_tile_duration(secs: f64) -> Option<String> {
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    let total = secs.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    Some(if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    })
}

fn thumbnail_request_mtime(item: &MediaItem) -> std::time::SystemTime {
    std::fs::metadata(&item.path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or_else(|_| std::time::SystemTime::from(item.file_mtime))
}

/// 失败缩略图的占位：浅灰纯色 texture，明确表示"加载失败"，
/// 而不是让 `GtkPicture` 保持空 paintable 露出卡片背景（裸白块）。
/// 2×2 pixbuf 会被 `content-fit: cover` 自动拉伸到 tile 大小。
fn gray_placeholder_texture() -> gtk::gdk::Texture {
    let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 2, 2)
        .expect("分配 2x2 占位 pixbuf");
    pb.fill(0xC8C8C8FF); // RGBA 浅灰
    gtk::gdk::Texture::for_pixbuf(&pb)
}

/// Pull every `MediaItem` out of the `BoxedAnyObject`-wrapped store.
fn extract_items(media_list: &gio::ListStore) -> Vec<MediaItem> {
    let mut items = Vec::with_capacity(media_list.n_items() as usize);
    for i in 0..media_list.n_items() {
        if let Some(obj) = media_list.item(i) {
            if let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() {
                let cow = boxed.borrow::<MediaItem>();
                items.push((*cow).clone());
            }
        }
    }
    items
}

/// `MediaItem.uri` → its global index in the store (for activation callbacks).
fn uri_index_map(media_list: &gio::ListStore) -> std::collections::HashMap<String, u32> {
    let mut map = std::collections::HashMap::with_capacity(media_list.n_items() as usize);
    for i in 0..media_list.n_items() {
        if let Some(obj) = media_list.item(i) {
            if let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() {
                map.insert(boxed.borrow::<MediaItem>().uri.clone(), i);
            }
        }
    }
    map
}

impl Default for MediaGrid {
    fn default() -> Self {
        gtk::glib::Object::builder().build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn sample_item(id: i64, name: &str) -> MediaItem {
        let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
        MediaItem {
            id,
            uri: format!("file:///tmp/{name}"),
            path: PathBuf::from(format!("/tmp/{name}")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/png".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(100),
            height: Some(100),
            video_duration_secs: None,
            taken_at: Some(dt),
            file_mtime: dt,
            file_size: 100,
            blake3_hash: format!("hash-{id}"),
            is_favorite: false,
            trashed_at: None,
        }
    }

    fn insert_sample_item(pool: &crate::core::db::DbPool, item: &MediaItem) -> i64 {
        crate::core::db::insert_media_item(
            pool,
            &crate::core::media::NewMediaItem {
                uri: item.uri.clone(),
                path: item.path.clone(),
                folder_path: item.folder_path.clone(),
                mime_type: item.mime_type.clone(),
                media_subkind: item.media_subkind.clone(),
                media_attributes: item.media_attributes.clone(),
                width: item.width,
                height: item.height,
                video_duration_secs: item.video_duration_secs,
                taken_at: item.taken_at,
                file_mtime: item.file_mtime,
                file_size: item.file_size,
                blake3_hash: item.blake3_hash.clone(),
            },
        )
        .unwrap()
    }

    fn tile_count(grid: &MediaGrid) -> u32 {
        let content = grid.imp().content.get();
        let mut count = 0;
        let mut child = content.first_child();
        while let Some(widget) = child {
            if let Some(flow) = widget.downcast_ref::<gtk::FlowBox>() {
                count += flow.observe_children().n_items();
            }
            child = widget.next_sibling();
        }
        count
    }

    #[gtk::test]
    fn grid_rebuilds_when_backing_store_removes_item() {
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
        media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

        let grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Day,
            loader,
            Rc::new(|_| {}),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_, _| {}),
            Rc::new(|_| FavoriteMenuState::default()),
            false,
        );
        assert_eq!(tile_count(&grid), 2);

        media_list.remove(0);

        assert_eq!(
            tile_count(&grid),
            1,
            "MediaGrid must drop stale thumbnails when the shared ListStore changes"
        );
    }

    #[gtk::test]
    fn inactive_grid_defers_initial_tile_build_until_activated() {
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
        media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

        let grid = MediaGrid::new_with_initial_active(
            media_list,
            GroupBy::Month,
            loader,
            Rc::new(|_| {}),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_, _| {}),
            Rc::new(|_| FavoriteMenuState::default()),
            false,
            false,
        );

        assert_eq!(
            tile_count(&grid),
            0,
            "inactive grids should not build hidden FlowBox tiles at startup"
        );

        grid.set_active(true);

        assert_eq!(
            tile_count(&grid),
            2,
            "activating a dirty grid should build tiles from the current model"
        );
    }

    #[gtk::test]
    fn day_grid_stats_show_thumbnail_progress_and_are_centered() {
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(
            pool.clone(),
            dir.path().join("thumbs"),
        ));
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let one = sample_item(1, "one.png");
        let two = sample_item(2, "two.png");
        let generated_id = insert_sample_item(&pool, &one);
        insert_sample_item(&pool, &two);
        crate::core::db::mark_thumbnails_generated(&pool, &[generated_id]).unwrap();
        media_list.append(&glib::BoxedAnyObject::new(one));
        media_list.append(&glib::BoxedAnyObject::new(two));

        let grid = MediaGrid::new(
            media_list,
            GroupBy::Day,
            loader,
            Rc::new(|_| {}),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_, _| {}),
            Rc::new(|_| FavoriteMenuState::default()),
            false,
        );

        let stats_label = grid
            .imp()
            .stats_label
            .borrow()
            .as_ref()
            .expect("Day grid should render the library stats label")
            .clone();
        assert_eq!(stats_label.label(), "媒体 2 项 · 缩略图 1/2");
        assert_eq!(stats_label.halign(), gtk::Align::Center);
        assert_eq!(stats_label.xalign(), 0.5);
        assert!(stats_label.has_css_class("library-stats"));
        assert!(stats_label.has_css_class("glass-raised"));
    }

    #[test]
    fn thumbnail_request_mtime_prefers_file_modified_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rotated.png");
        std::fs::write(&path, b"metadata").unwrap();

        let mut item = sample_item(1, "rotated.png");
        item.path = path;
        item.file_mtime = chrono::DateTime::<Utc>::from(std::time::SystemTime::UNIX_EPOCH);

        let mtime = thumbnail_request_mtime(&item);

        assert!(mtime > std::time::SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn tile_duration_formats_minutes_and_hours() {
        assert_eq!(format_tile_duration(83.2).as_deref(), Some("01:23"));
        assert_eq!(format_tile_duration(3_661.0).as_deref(), Some("1:01:01"));
        assert_eq!(format_tile_duration(f64::NAN), None);
    }

    #[test]
    fn library_stats_text_clamps_generated_to_total() {
        assert_eq!(library_stats_text(20, 7), "媒体 20 项 · 缩略图 7/20");
        assert_eq!(library_stats_text(20, 99), "媒体 20 项 · 缩略图 20/20");
    }

    #[gtk::test]
    fn virtual_placeholder_flow_renders_loading_tiles_immediately() {
        let _ = gtk::init();
        let spec = ViewSpec {
            mode: GroupBy::Day,
            pixel_size: 270,
            thumb_size: ThumbnailSize::Large,
        };
        let flow = build_virtual_placeholder_flow(spec, 12);

        assert!(flow.has_css_class("virtual-placeholder-grid"));
        assert_eq!(flow.selection_mode(), gtk::SelectionMode::None);
        let mut count = 0;
        let mut child = flow.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();
            let tile = widget
                .downcast::<gtk::FlowBoxChild>()
                .ok()
                .and_then(|child| child.child())
                .and_then(|child| child.downcast::<SquareTile>().ok())
                .expect("placeholder flow children should wrap SquareTile");
            assert_eq!(tile.target(), 270);
            assert!(tile.has_css_class("thumb-loading"));
            assert!(tile.has_css_class("thumb-placeholder"));
            count += 1;
            child = next;
        }
        assert_eq!(count, 12);
    }

    #[test]
    fn virtual_scroll_ratio_maps_to_full_library_offset() {
        assert_eq!(virtual_offset_for_ratio(0.0, 100_000, 500), 0);
        assert_eq!(virtual_offset_for_ratio(0.50, 100_000, 500), 50_000);
        assert_eq!(virtual_offset_for_ratio(0.99, 100_000, 500), 99_000);
        assert_eq!(virtual_offset_for_ratio(1.0, 100_000, 500), 99_500);
    }

    #[test]
    fn virtual_scroll_prefetches_before_window_edge() {
        assert_eq!(
            virtual_page_start_for_offset(850, 0, 1_000, 100_000, 500),
            Some(600),
            "80%+ through the current window should prefetch ahead"
        );
        assert_eq!(
            virtual_page_start_for_offset(550, 0, 1_000, 100_000, 500),
            None,
            "middle of current window should not reload"
        );
        assert_eq!(
            virtual_page_start_for_offset(50_000, 0, 1_000, 100_000, 500),
            Some(49_750),
            "dragging the full-library scrollbar should jump near that global offset"
        );
        assert_eq!(
            virtual_page_start_for_offset(99_900, 99_000, 1_000, 100_000, 500),
            Some(99_500),
            "near the end should clamp to the last full page"
        );
    }

    #[test]
    fn virtual_spacer_height_scales_with_unloaded_items() {
        let spec = ViewSpec {
            mode: GroupBy::Day,
            pixel_size: 270,
            thumb_size: ThumbnailSize::Large,
        };
        assert_eq!(virtual_spacer_height(0, 4, 1_000.0, spec), 0);
        assert!(
            virtual_spacer_height(1_000, 4, 1_000.0, spec)
                > virtual_spacer_height(100, 4, 1_000.0, spec)
        );
    }

    #[test]
    fn virtual_loading_window_counts_placeholders_for_target_page() {
        assert_eq!(virtual_window_item_count(0, 100_000, 500), 500);
        assert_eq!(virtual_window_item_count(99_500, 100_000, 500), 500);
        assert_eq!(virtual_window_item_count(99_800, 100_000, 500), 200);
        assert_eq!(virtual_window_item_count(100_000, 100_000, 500), 0);
    }

    #[test]
    fn scroll_ratio_tracks_latest_drag_value_while_page_is_loading() {
        assert_eq!(scroll_ratio_from_adjustment_value(0.0, 1_000.0, 100.0), 0.0);
        assert_eq!(
            scroll_ratio_from_adjustment_value(450.0, 1_000.0, 100.0),
            0.5
        );
        assert_eq!(
            scroll_ratio_from_adjustment_value(2_000.0, 1_000.0, 100.0),
            1.0
        );
        assert_eq!(scroll_ratio_from_adjustment_value(20.0, 100.0, 100.0), 0.0);
    }
}
