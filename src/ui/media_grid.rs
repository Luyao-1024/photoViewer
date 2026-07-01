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
//! `crate::ui::grid_css`. Each section FlowBox defaults to
//! `selection-mode = None` and is switched to `Multiple` only while
//! multi-select is active (see `apply_selection_mode`), so a stray
//! `flowboxchild:selected` — and the `.thumb-checkmark` it reveals — cannot
//! surface outside explicit multi-select.
//! `attach_kbd_nav` drives arrow-key cursor movement and hides `:hover` while
//! arrow-keying so the highlight follows the keyboard cursor, not the resting
//! pointer.
//!
//! ## Multi-select
//!
//! `MediaGrid` supports batch operations (e.g. "Add N selected photos to
//! album"). Each per-section FlowBox's `selection_mode` tracks the
//! multi-select flag: `None` by default (so the FlowBox ignores GTK's
//! built-in selection and no child can become `:selected`, keeping the
//! `.thumb-checkmark` hidden), switched to `Multiple` only while multi-select
//! is active. `apply_selection_mode` keeps every section FlowBox in sync with
//! the flag. The `selected` set on `imp` records the *global* indices (into
//! the shared `ListStore`) currently selected. `child_activated` opens the
//! viewer normally, or toggles membership when multi-select is active — there
//! is no modifier-key path; multi-select is entered only via the right-click
//! "Multi-select" item.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::runtime_config;
use crate::core::section_model::{apply_authoritative_counts, group_items, GroupBy};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
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

fn library_stats_for(
    loader: &ThumbnailLoader,
    fallback_total: usize,
) -> crate::core::refresh::LibraryStats {
    MediaRepository::new(loader.pool().clone())
        .library_stats()
        .unwrap_or(crate::core::refresh::LibraryStats {
            live_total: fallback_total,
            thumbnails_generated: 0,
        })
}

fn build_library_stats_label(stats: crate::core::refresh::LibraryStats) -> gtk::Label {
    gtk::Label::builder()
        .label(library_stats_text(
            stats.live_total,
            stats.thumbnails_generated,
        ))
        .halign(gtk::Align::Center)
        .hexpand(true)
        .margin_top(24)
        .margin_bottom(14)
        .xalign(0.5)
        .justify(gtk::Justification::Center)
        .css_classes(["library-stats"])
        .build()
}

fn should_show_library_stats(stats: &crate::core::refresh::LibraryStats) -> bool {
    stats.live_total > 0 && stats.thumbnails_generated < stats.live_total
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FavoriteMenuState {
    pub can_favorite: bool,
    pub can_unfavorite: bool,
}

pub type ActivateCallback = Rc<dyn Fn(MediaId)>;
pub type SimpleCallback = Rc<dyn Fn()>;
pub type SelectionCallback = Rc<dyn Fn(Vec<MediaId>)>;
pub type FavoriteCallback = Rc<dyn Fn(Vec<MediaId>, bool)>;
pub type FavoriteStateCallback = Rc<dyn Fn(Vec<MediaId>) -> FavoriteMenuState>;

#[derive(Clone)]
pub struct MediaGridCallbacks {
    pub on_activate: ActivateCallback,
    pub on_background_changed: SimpleCallback,
    pub on_add_to_album: SelectionCallback,
    pub on_move_to_trash: SelectionCallback,
    pub on_set_favorite: FavoriteCallback,
    pub on_query_favorite_state: FavoriteStateCallback,
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
        pub on_activate: std::cell::OnceCell<ActivateCallback>,
        pub on_background_changed: std::cell::OnceCell<SimpleCallback>,
        pub on_add_to_album: std::cell::OnceCell<SelectionCallback>,
        pub on_move_to_trash: std::cell::OnceCell<SelectionCallback>,
        pub on_set_favorite: std::cell::OnceCell<FavoriteCallback>,
        pub on_query_favorite_state: std::cell::OnceCell<FavoriteStateCallback>,
        pub context_menu_overlay: RefCell<Option<gtk::Overlay>>,
        /// Flattened `(flow_child, local_window_index, media_id)` for every
        /// rendered tile in current mode.
        pub displayed_items: RefCell<Vec<(gtk::FlowBoxChild, u32, MediaId)>>,
        /// Stable media ids currently in the "selected" set.
        pub selected: RefCell<HashSet<MediaId>>,
        /// 当前渲染上限：初始 = `max_rendered_grid_items()`；
        /// 滚动接近底部时自动增长（最多到 `ABSOLUTE_RENDERED_LIMIT`）。
        pub rendered_limit: Cell<usize>,
        /// 当前 GTK 模型窗口对应全库排序中的起始 offset。
        pub virtual_window_start: Cell<u32>,
        /// DB 中 live media 的总数，用于虚拟 spacer 和滚动条比例。
        pub virtual_total: Cell<u32>,
        /// 防止滚动事件在上一次 DB page 尚未返回时重复发起加载。
        pub virtual_page_loading: Cell<bool>,
        /// Whether a blocking DB page query is currently running.
        pub virtual_query_in_flight: Cell<bool>,
        /// Latest target requested while a DB page query is already running.
        pub pending_virtual_page_start: Cell<Option<u32>>,
        pub pending_virtual_page_ratio: Cell<Option<f64>>,
        /// 每次虚拟窗口 DB 请求递增；旧请求返回后若 generation 过期则丢弃。
        pub virtual_page_generation: Cell<u64>,
        /// 替换窗口后按全库比例恢复滚动条位置。
        pub pending_scroll_ratio: Cell<Option<f64>>,
        /// Programmatic scroll restoration after rebuild should not be treated
        /// as a user drag that requests another DB page.
        pub restoring_scroll: Cell<bool>,
        /// `ListStore::splice` used by virtual paging emits `items-changed`;
        /// suppress the generic removal rebuild and rebuild exactly once after
        /// the page is applied.
        pub applying_virtual_page: Cell<bool>,
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
                context_menu_overlay: RefCell::new(None),
                displayed_items: RefCell::new(Vec::new()),
                selected: RefCell::default(),
                rendered_limit: Cell::new(
                    max_rendered_grid_items().min(runtime_config::grid_render_absolute_cap()),
                ),
                virtual_window_start: Cell::new(0),
                virtual_total: Cell::new(0),
                virtual_page_loading: Cell::new(false),
                virtual_query_in_flight: Cell::new(false),
                pending_virtual_page_start: Cell::new(None),
                pending_virtual_page_ratio: Cell::new(None),
                virtual_page_generation: Cell::new(0),
                pending_scroll_ratio: Cell::new(None),
                restoring_scroll: Cell::new(false),
                applying_virtual_page: Cell::new(false),
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

    let max_start = total.saturating_sub(page_size);
    if desired_offset >= max_start {
        return Some(max_start);
    }

    let centered = desired_offset.saturating_sub(page_size / 2);
    Some(centered.min(max_start))
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

fn should_consider_virtual_page_load(restoring_scroll: bool, total: u32, current_len: u32) -> bool {
    !restoring_scroll && total > current_len && current_len > 0
}

fn replace_pending_virtual_page(
    pending_start: &std::cell::Cell<Option<u32>>,
    pending_ratio: &std::cell::Cell<Option<f64>>,
    target_start: u32,
    ratio: f64,
) {
    pending_start.set(Some(target_start));
    pending_ratio.set(Some(ratio));
}

fn tile_intersects_request_window(
    tile_y: f32,
    tile_h: f32,
    page_h: f32,
    overscan_pages: f32,
) -> bool {
    let overscan = page_h.max(0.0) * overscan_pages.max(0.0);
    tile_y < page_h + overscan && tile_y + tile_h > -overscan
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
    pub fn set_context_menu_overlay(&self, overlay: Option<&gtk::Overlay>) {
        *self.imp().context_menu_overlay.borrow_mut() = overlay.cloned();
    }

    /// Build a MediaGrid that immediately renders `(media_list, mode)`.
    /// `on_activate` fires with the activated photo's stable media id when
    /// the user activates a photo (click without modifier).
    pub fn new(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        callbacks: MediaGridCallbacks,
        enable_context_menu: bool,
    ) -> Self {
        Self::new_with_initial_active(
            media_list,
            mode,
            loader,
            callbacks,
            enable_context_menu,
            true,
        )
    }

    pub fn new_with_initial_active(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        callbacks: MediaGridCallbacks,
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
            .set(callbacks.on_activate)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_background_changed
            .set(callbacks.on_background_changed)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_add_to_album
            .set(callbacks.on_add_to_album)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_move_to_trash
            .set(callbacks.on_move_to_trash)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_set_favorite
            .set(callbacks.on_set_favorite)
            .ok()
            .expect("MediaGrid::new called more than once");
        obj.imp()
            .on_query_favorite_state
            .set(callbacks.on_query_favorite_state)
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
                if this.imp().applying_virtual_page.get() {
                    return;
                }
                let was_empty = list.n_items().saturating_sub(added) == 0;
                if removed > 0 {
                    // 有项被移除或全量替换 → 必须重建。
                    this.rebuild_immediately(list.clone());
                } else if added > 0 && was_empty {
                    // 首次启动扫描从空库追加第一批媒体时，Day grid 已经按空列表
                    // 构建过；必须立即重建，否则空态切回 Day 后 tile/统计仍为空。
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
                    if !this.imp().restoring_scroll.get() {
                        this.try_load_virtual_page(adj);
                        this.try_expand_render_limit(adj);
                    }
                }
            });
        let weak_map = obj.downgrade();
        obj.connect_map(move |_| {
            let weak_map = weak_map.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(this) = weak_map.upgrade() {
                    this.reprioritize_visible();
                }
            });
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
    pub fn selected_ids(&self) -> Vec<MediaId> {
        let s = self.imp().selected.borrow();
        s.iter().copied().collect()
    }

    /// Snapshot of currently rendered global indices in grid order.
    pub fn displayed_indices(&self) -> Vec<u32> {
        self.imp()
            .displayed_items
            .borrow()
            .iter()
            .map(|(_, gi, _)| *gi)
            .collect()
    }

    /// Select all rendered tiles and sync visible highlights.
    pub fn select_all(&self) {
        self.imp().is_multi_select_mode.set(true);
        // `select_child` below only marks a child `:selected` while the
        // FlowBox is in `Multiple`; flip every section first.
        self.apply_selection_mode();
        let mut next = HashSet::new();
        let items = self.imp().displayed_items.borrow().clone();
        for (flow_child, _, media_id) in items {
            if let Some(parent) = flow_child.parent() {
                if let Ok(flow) = parent.downcast::<gtk::FlowBox>() {
                    flow.select_child(&flow_child);
                }
            }
            next.insert(media_id);
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
        // Flip every section FlowBox to `Multiple` (enabled) or `None`
        // (disabled) so the checkmark can only appear while multi-select is
        // active. Must precede callers that rely on `select_child`.
        self.apply_selection_mode();
        if !enabled {
            self.clear_selection();
        }
    }

    /// Whether explicit multi-select mode is enabled.
    pub fn is_multi_select_mode(&self) -> bool {
        self.imp().is_multi_select_mode.get()
    }

    /// Sync every section FlowBox's `selection_mode` with the multi-select
    /// flag: `None` when off (so the FlowBox ignores GTK's built-in selection
    /// and no child can become `:selected` — keeping the `.thumb-checkmark`
    /// hidden), `Multiple` when on. Without this the per-section FlowBoxes
    /// stayed on `Multiple` permanently and GTK could leave a child
    /// `:selected` even when the user never entered multi-select, surfacing a
    /// stray checkmark on a thumbnail.
    fn apply_selection_mode(&self) {
        let mode = if self.imp().is_multi_select_mode.get() {
            gtk::SelectionMode::Multiple
        } else {
            gtk::SelectionMode::None
        };
        let content = self.imp().content.get();
        let mut child = content.first_child();
        while let Some(c) = child {
            if let Some(flow) = c.downcast_ref::<gtk::FlowBox>() {
                flow.set_selection_mode(mode);
            }
            child = c.next_sibling();
        }
    }

    /// Whether every currently rendered tile is selected.
    pub fn is_all_displayed_selected(&self) -> bool {
        let selected = self.imp().selected.borrow();
        let displayed = self.imp().displayed_items.borrow();
        if displayed.is_empty() {
            return false;
        }
        for (_, _, media_id) in displayed.iter() {
            if !selected.contains(media_id) {
                return false;
            }
        }
        true
    }

    fn selected_ids_sorted(&self) -> Vec<MediaId> {
        let mut ids: Vec<MediaId> = self.imp().selected.borrow().iter().copied().collect();
        ids.sort_unstable();
        ids
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
        // Drop to `None` so no child can re-enter `:selected` — and reveal the
        // checkmark — until multi-select is explicitly re-entered.
        self.apply_selection_mode();
        if changed {
            self.fire_selection_changed();
        }
    }

    fn ensure_context_selection(
        &self,
        flow: &gtk::FlowBox,
        clicked_child: &gtk::FlowBoxChild,
        media_id: MediaId,
    ) -> Vec<MediaId> {
        let was_selected = self.imp().selected.borrow().contains(&media_id);
        if !was_selected {
            {
                let mut s = self.imp().selected.borrow_mut();
                s.clear();
                s.insert(media_id);
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
        self.selected_ids_sorted()
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
                            // 越过可见下沿：后续 tile 更靠下，整体结束。
                            if b.y() >= page_h * (1.0 + THUMB_REQUEST_OVERSCAN_PAGES) {
                                return keys;
                            }
                            // 与视口 + overscan 有交集即请求/提权；FlowBox 会 map
                            // 整个 page，不能把 map 当作真实可见性。
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
        self.imp().virtual_page_generation.set(generation);
        self.imp().virtual_window_start.set(target_start);
        self.imp().virtual_page_loading.set(true);
        self.imp().pending_scroll_ratio.set(Some(ratio));
        self.rebuild_immediately(list.clone());

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
            return;
        }

        self.imp().virtual_query_in_flight.set(true);
        self.spawn_virtual_page_query(loader, target_start, virtual_page_size, generation);
    }

    fn spawn_virtual_page_query(
        &self,
        loader: Arc<ThumbnailLoader>,
        target_start: u32,
        virtual_page_size: u32,
        generation: u64,
    ) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "VIRTUAL_SCROLL load_page generation={generation} target_start={target_start} page_size={virtual_page_size}"
        );

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let pool = loader.pool().clone();
            let page_started = std::time::Instant::now();
            let result = gtk::gio::spawn_blocking(move || {
                let db_started = std::time::Instant::now();
                let result = MediaRepository::new(pool)
                    .page(MediaQuery::LiveAll, target_start, virtual_page_size)
                    .map(|page| page.items);
                (result, db_started.elapsed())
            })
            .await;
            let items = match result {
                Ok((Ok(items), db_elapsed)) => {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "VIRTUAL_TIMING db_page_loaded generation={} target_start={} limit={} rows={} db_ms={} await_ms={}",
                        generation,
                        target_start,
                        virtual_page_size,
                        items.len(),
                        db_elapsed.as_millis(),
                        page_started.elapsed().as_millis()
                    );
                    items
                }
                Ok((Err(err), db_elapsed)) => {
                    tracing::warn!(
                        target: crate::core::log_targets::BROWSING,
                        "VIRTUAL_TIMING db_page_failed generation={} target_start={} limit={} db_ms={} await_ms={} error={err}",
                        generation,
                        target_start,
                        virtual_page_size,
                        db_elapsed.as_millis(),
                        page_started.elapsed().as_millis()
                    );
                    Vec::new()
                }
                Err(err) => {
                    tracing::warn!(
                        target: crate::core::log_targets::BROWSING,
                        "VIRTUAL_TIMING db_page_join_failed generation={} target_start={} limit={} await_ms={} error={err:?}",
                        generation,
                        target_start,
                        virtual_page_size,
                        page_started.elapsed().as_millis()
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
            let apply_started = std::time::Instant::now();
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
                    "VIRTUAL_TIMING page_applied generation={} target_start={} old_len={} new_len={} apply_rebuild_ms={}",
                    generation,
                    target_start,
                    old_len,
                    new_len,
                    apply_started.elapsed().as_millis()
                );
            }
            this.imp().virtual_query_in_flight.set(false);
        });
    }

    fn start_pending_virtual_page_query(&self, loader: Arc<ThumbnailLoader>) -> bool {
        let Some(target_start) = self.imp().pending_virtual_page_start.take() else {
            return false;
        };
        if let Some(ratio) = self.imp().pending_virtual_page_ratio.take() {
            self.imp().pending_scroll_ratio.set(Some(ratio));
        }
        let generation = self.imp().virtual_page_generation.get();
        self.imp().virtual_query_in_flight.set(true);
        self.spawn_virtual_page_query(
            loader,
            target_start,
            runtime_config::virtual_media_page_size(),
            generation,
        );
        true
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

    /// Toggle membership of `media_id` in the selected set, then toggle
    /// the visual highlight on `child` via its parent `FlowBox`. Fires
    /// `selection-changed`.
    fn toggle_selection(&self, media_id: MediaId, child: &gtk::FlowBoxChild, flow: &gtk::FlowBox) {
        let now_selected = {
            let mut s = self.imp().selected.borrow_mut();
            if s.contains(&media_id) {
                s.remove(&media_id);
                false
            } else {
                s.insert(media_id);
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
        let total_media_count = MediaRepository::new(loader.pool().clone())
            .count(MediaQuery::LiveAll)
            .unwrap_or(items.len() as u32)
            .max(items.len() as u32);
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

        let total_media = total_media_count as usize;
        if mode == GroupBy::Day && (loading_placeholder_count > 0 || !items.is_empty()) {
            let stats = library_stats_for(&loader, total_media);
            if should_show_library_stats(&stats) {
                let stats_label = build_library_stats_label(stats);
                content.append(&stats_label);
                *self.imp().stats_label.borrow_mut() = Some(stats_label.clone());
                self.start_stats_refresh(loader.clone(), total_media);
            }
        }

        // Group by year/month/day, then emit header + FlowBox per section.
        let mut section_count = 0u32;
        let mut photo_count = 0u32;
        let mut displayed_items = Vec::new();
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
            let mut sections = group_items(&items, mode);
            // section 头部计数改用整个库的 DB 聚合，而非当前虚拟分页窗口切片。
            // 窗口受 virtual_media_page_size（默认 500）截断，否则一个实际几千张的
            // 年份只会显示窗口里的 500。窗口只决定渲染哪些缩略图，不影响真实计数。
            if let Ok(counts) = MediaRepository::new(loader.pool().clone()).section_counts(mode) {
                apply_authoritative_counts(&mut sections, &counts);
            }
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

                // Build tiles + remember each child's global index for activation.
                let mut global_indices: Vec<u32> = Vec::with_capacity(section.items.len());
                let mut media_ids: Vec<MediaId> = Vec::with_capacity(section.items.len());
                let mut activation_items: Vec<(i64, String, String)> =
                    Vec::with_capacity(section.items.len());
                for item in &section.items {
                    let gi = uri_to_index.get(&item.uri).copied().unwrap_or(u32::MAX);
                    let media_id = MediaId::from(item.id);
                    global_indices.push(gi);
                    media_ids.push(media_id);
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
                            displayed_items.push((flow_child.clone(), gi, media_id));
                        }
                    }
                    photo_count += 1;
                }

                // Activation: FlowBox child-activated → look up stable media id.
                // Only explicit multi-select mode (entered via right-click “Multi-select”)
                // toggles selection; otherwise the item opens in viewer.
                let on_act = on_activate.clone();
                let weak = self.downgrade();
                let media_ids_for_activation = media_ids.clone();
                let global_indices_for_activation = global_indices.clone();
                let media_ids_for_context = media_ids;
                let section_label_for_activation = section.label.clone();
                flow.connect_child_activated(move |flow, child| {
                let idx = child.index();
                if idx < 0 {
                    return;
                }
                let Some(&media_id) = media_ids_for_activation.get(idx as usize) else {
                    return;
                };
                let gi = global_indices_for_activation
                    .get(idx as usize)
                    .copied()
                    .unwrap_or(u32::MAX);
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
                    this.toggle_selection(media_id, child, flow);
                } else {
                    on_act(media_id);
                }
            });

                if enable_context_menu {
                    let weak_for_context = self.downgrade();
                    let section_label_for_ctx = section.label.clone();
                    let media_ids_for_context = media_ids_for_context.clone();
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

                    let Some(media_id) = media_ids_for_context.get(hit_idx).copied() else {
                        return;
                    };

                    let in_multi_mode = this.is_multi_select_mode();
                    let target_indices = if in_multi_mode {
                        this.ensure_context_selection(&flow_for_ctx, &flow_child_for_ctx, media_id)
                    } else {
                        vec![media_id]
                    };
                    let favorite_state = (on_query_favorite_state_ctx)(target_indices.clone());
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "VIEWER_DEBUG context_menu mode={:?} section={} selected={:?} multi_select={}",
                        this.mode(),
                        section_label_for_ctx,
                        target_indices,
                        in_multi_mode,
                    );

                    let Some(context_overlay) = this.imp().context_menu_overlay.borrow().clone()
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
                    this.imp().restoring_scroll.set(true);
                    adj.set_value(restored);
                    this.imp().restoring_scroll.set(false);
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

        // 下一帧 layout 完成后立即请求/提权视口附近缩略图；滚动期间仍走去抖路径。
        let weak = self.downgrade();
        gtk::glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.reprioritize_visible();
            }
        });
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
            let stats = library_stats_for(&loader, total_media);
            if let Some(label) = this.imp().stats_label.borrow().as_ref() {
                label.set_label(&library_stats_text(
                    stats.live_total,
                    stats.thumbnails_generated,
                ));
            } else {
                this.imp().stats_refresh_source.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            if stats.thumbnails_generated >= stats.live_total {
                if let Some(label) = this.imp().stats_label.borrow_mut().take() {
                    if let Some(parent) = label.parent().and_downcast::<gtk::Box>() {
                        parent.remove(&label);
                    }
                }
                this.imp().stats_refresh_source.borrow_mut().take();
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
            pub thumbnail_request: RefCell<Option<Rc<dyn Fn()>>>,
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
                    thumbnail_request: RefCell::new(None),
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

        pub fn set_thumbnail_request(&self, request: Rc<dyn Fn()>) {
            *self.imp().thumbnail_request.borrow_mut() = Some(request);
        }

        pub fn request_thumbnail(&self) {
            if let Some(request) = self.imp().thumbnail_request.borrow().as_ref() {
                request();
            }
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

        #[gtk::test]
        fn square_tile_runs_registered_thumbnail_request() {
            let _ = gtk::init();
            let tile = SquareTile::new();
            let called = Rc::new(std::cell::Cell::new(false));
            let called_for_request = called.clone();

            tile.set_thumbnail_request(Rc::new(move || {
                called_for_request.set(true);
            }));
            tile.request_thumbnail();

            assert!(called.get());
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

    // 缩略图请求不由 `map` 触发：GtkFlowBox 会把当前虚拟 page 的大量 child
    // 都 map 掉。请求闭包注册在 tile 上，由 MediaGrid 的视口扫描按
    // viewport+overscan 触发，避免一次 page 500 个 tile 同时排队。
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
            let request_started = std::time::Instant::now();
            if let Some(tile) = tile_weak.upgrade() {
                tile.set_cache_key(cache_key.clone());
            }

            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "THUMB_TIMING grid_request item_id={} item_name={} uri={} size={:?} target_px={} global_index={} queue_len={} in_flight={} media_item_mtime={} request_mtime={:?} cache_key={:?}",
                current_item.id,
                item_name,
                item_uri,
                size,
                target_px,
                global_index,
                loader.queue_len(),
                loader.in_flight_len(),
                current_item.file_mtime,
                item_mtime,
                cache_key
            );

            let (tx, rx) = tokio::sync::oneshot::channel();
            loader.request_for_media(
                current_item.id,
                item_uri.clone(),
                size,
                Some(item_mtime),
                tx,
                crate::core::thumbnails::TIER_BOOST,
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
                            "THUMB_TIMING grid_loaded item_name={} uri={} elapsed_ms={} texture={}x{}",
                            item_name,
                            item_uri,
                            request_started.elapsed().as_millis(),
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
                            "THUMB_TIMING grid_dropped_placeholder item_name={} uri={} elapsed_ms={}",
                            item_name,
                            item_uri,
                            request_started.elapsed().as_millis()
                        );
                        if let Some(t) = tile_weak.upgrade() {
                            t.set_paintable(Some(&gray_placeholder_texture()));
                        }
                    }
                }
            });
        }
    });

    tile.set_thumbnail_request(request_once);
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

    fn noop_callbacks() -> MediaGridCallbacks {
        MediaGridCallbacks {
            on_activate: Rc::new(|_| {}),
            on_background_changed: Rc::new(|| {}),
            on_add_to_album: Rc::new(|_| {}),
            on_move_to_trash: Rc::new(|_| {}),
            on_set_favorite: Rc::new(|_, _| {}),
            on_query_favorite_state: Rc::new(|_| FavoriteMenuState::default()),
        }
    }

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

    fn section_flow_selection_modes(grid: &MediaGrid) -> Vec<gtk::SelectionMode> {
        let content = grid.imp().content.get();
        let mut modes = Vec::new();
        let mut child = content.first_child();
        while let Some(c) = child {
            if let Some(flow) = c.downcast_ref::<gtk::FlowBox>() {
                modes.push(flow.selection_mode());
            }
            child = c.next_sibling();
        }
        modes
    }

    #[gtk::test]
    fn section_flowbox_selection_mode_tracks_multi_select() {
        // The checkmark is revealed by `flowboxchild:selected`, which can only
        // happen while a section FlowBox is in `Multiple`. Out of multi-select
        // every section must be `None` so no stray tick can appear.
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
        media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

        let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);

        let modes = section_flow_selection_modes(&grid);
        assert!(
            !modes.is_empty(),
            "rebuild should produce section FlowBoxes"
        );
        assert!(
            modes.iter().all(|m| *m == gtk::SelectionMode::None),
            "default (non-multi) must be None, got {modes:?}"
        );

        grid.set_multi_select_mode(true);
        let modes = section_flow_selection_modes(&grid);
        assert!(
            modes.iter().all(|m| *m == gtk::SelectionMode::Multiple),
            "multi-select must flip every section to Multiple, got {modes:?}"
        );

        grid.set_multi_select_mode(false);
        let modes = section_flow_selection_modes(&grid);
        assert!(
            modes.iter().all(|m| *m == gtk::SelectionMode::None),
            "exiting multi-select must restore None, got {modes:?}"
        );
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
            noop_callbacks(),
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
            noop_callbacks(),
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
    fn active_empty_day_grid_rebuilds_when_first_scan_items_arrive() {
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(
            pool.clone(),
            dir.path().join("thumbs"),
        ));
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Day,
            loader,
            noop_callbacks(),
            false,
        );

        assert_eq!(tile_count(&grid), 0);
        assert!(
            grid.imp().stats_label.borrow().is_none(),
            "empty Day grid should not render a stats label before media exists"
        );

        let item = sample_item(1, "first-scan.png");
        insert_sample_item(&pool, &item);
        media_list.append(&glib::BoxedAnyObject::new(item));

        assert_eq!(
            tile_count(&grid),
            1,
            "active Day grid must render the first media items delivered by startup scan"
        );
        let stats_label = grid
            .imp()
            .stats_label
            .borrow()
            .as_ref()
            .expect("Day grid should create the library stats label after first media arrives")
            .clone();
        assert_eq!(stats_label.label(), "媒体 1 项 · 缩略图 0/1");
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

        let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);

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
        assert_eq!(stats_label.margin_top(), 24);
        assert!(stats_label.has_css_class("library-stats"));
        assert!(
            !stats_label.has_css_class("glass-raised"),
            "stats should be plain text, not a raised glass capsule"
        );
    }

    #[gtk::test]
    fn day_grid_stats_are_above_first_section_header() {
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(
            pool.clone(),
            dir.path().join("thumbs"),
        ));
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let one = sample_item(1, "one.png");
        insert_sample_item(&pool, &one);
        media_list.append(&glib::BoxedAnyObject::new(one));

        let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);
        let content = grid.imp().content.get();
        let first_child = content
            .first_child()
            .expect("Day grid should have a first content child");

        assert!(
            first_child.has_css_class("library-stats"),
            "Day grid stats should be the first content child, above the first date header"
        );
    }

    #[gtk::test]
    fn completed_day_grid_stats_are_hidden() {
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(
            pool.clone(),
            dir.path().join("thumbs"),
        ));
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let item = sample_item(1, "complete.png");
        let media_id = insert_sample_item(&pool, &item);
        crate::core::db::mark_thumbnails_generated(&pool, &[media_id]).unwrap();
        media_list.append(&glib::BoxedAnyObject::new(item));

        let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);
        assert!(
            grid.imp().stats_label.borrow().is_none(),
            "completed thumbnail generation should hide the Day grid stats label"
        );
        assert!(
            grid.imp().stats_refresh_source.borrow().is_none(),
            "completed thumbnail generation should not start a stats refresh timeout"
        );
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
    fn virtual_scroll_absolute_end_targets_last_page() {
        assert_eq!(
            virtual_page_start_for_offset(99_500, 0, 500, 100_000, 500),
            Some(99_500),
            "dragging to the absolute end must load the final page, not a centered window above bottom spacer"
        );
        assert_eq!(
            virtual_page_start_for_offset(99_593, 0, 500, 100_093, 500),
            Some(99_593),
            "non-page-aligned library totals must still land on the final partial boundary"
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

    #[test]
    fn programmatic_scroll_restore_does_not_request_virtual_page() {
        assert!(
            should_consider_virtual_page_load(false, 100_000, 500),
            "user-driven scrolling in a virtualized library should still consider page loads"
        );
        assert!(
            !should_consider_virtual_page_load(true, 100_000, 500),
            "restoring scroll after a rebuild must not recursively request another virtual page"
        );
        assert!(
            !should_consider_virtual_page_load(false, 500, 500),
            "fully loaded small libraries do not need virtual page loads"
        );
        assert!(
            !should_consider_virtual_page_load(false, 100_000, 0),
            "an empty current window cannot be used to target a virtual page"
        );
    }

    #[test]
    fn coalesced_virtual_page_target_keeps_latest_drag_target() {
        let pending_start = std::cell::Cell::new(None);
        let pending_ratio = std::cell::Cell::new(None);

        replace_pending_virtual_page(&pending_start, &pending_ratio, 20_000, 0.20);
        replace_pending_virtual_page(&pending_start, &pending_ratio, 55_000, 0.55);

        assert_eq!(pending_start.get(), Some(55_000));
        assert_eq!(pending_ratio.get(), Some(0.55));
    }

    #[test]
    fn thumbnail_request_window_includes_viewport_and_one_page_overscan() {
        assert!(tile_intersects_request_window(0.0, 270.0, 900.0, 1.0));
        assert!(tile_intersects_request_window(1_700.0, 270.0, 900.0, 1.0));
        assert!(!tile_intersects_request_window(1_900.0, 270.0, 900.0, 1.0));
        assert!(tile_intersects_request_window(-250.0, 270.0, 900.0, 1.0));
        assert!(!tile_intersects_request_window(-1_200.0, 270.0, 900.0, 1.0));
    }
}
