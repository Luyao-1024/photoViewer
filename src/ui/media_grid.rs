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
//! Each tile is a `SquareTile` (`crate::ui::square_tile`) — a `GtkWidget` subclass wrapping
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

mod loading;
mod render;
mod selection;
mod updates;
mod viewport;
mod virtual_paging;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use chrono::Datelike;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::refresh::LibraryStats;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::runtime_config;
use crate::core::section_model::{GroupBy, SectionKey};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::square_tile::SquareTile;

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

#[derive(Debug)]
struct GridMetadataSnapshot {
    mode: GroupBy,
    live_total: Option<u32>,
    library_stats: Option<LibraryStats>,
    section_counts: Option<HashMap<SectionKey, u32>>,
}

#[tracing::instrument(name = "grid:load_metadata", skip(pool), fields(mode = ?mode))]
fn load_grid_metadata(pool: crate::core::db::DbPool, mode: GroupBy) -> GridMetadataSnapshot {
    let repo = MediaRepository::new(pool);
    GridMetadataSnapshot {
        mode,
        live_total: repo.count(MediaQuery::LiveAll).ok(),
        library_stats: repo.library_stats().ok(),
        section_counts: repo.section_counts(mode).ok(),
    }
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
pub type CoverCallback = Rc<dyn Fn(MediaId)>;

#[derive(Clone)]
pub struct MediaGridCallbacks {
    pub on_activate: ActivateCallback,
    pub on_background_changed: SimpleCallback,
    pub on_add_to_album: SelectionCallback,
    pub on_move_to_trash: SelectionCallback,
    pub on_set_favorite: FavoriteCallback,
    pub on_query_favorite_state: FavoriteStateCallback,
    pub on_set_album_cover: Option<CoverCallback>,
}

#[derive(Clone)]
struct DisplayedItem {
    flow_child: gtk::FlowBoxChild,
    window_index: u32,
    media_id: MediaId,
    section_key: SectionKey,
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use chrono::{TimeZone, Utc};

    use std::path::PathBuf;
    use std::rc::Rc;

    pub(crate) fn noop_callbacks() -> MediaGridCallbacks {
        MediaGridCallbacks {
            on_activate: Rc::new(|_| {}),
            on_background_changed: Rc::new(|| {}),
            on_add_to_album: Rc::new(|_| {}),
            on_move_to_trash: Rc::new(|_| {}),
            on_set_favorite: Rc::new(|_, _| {}),
            on_query_favorite_state: Rc::new(|_| FavoriteMenuState::default()),
            on_set_album_cover: None,
        }
    }

    pub(crate) fn sample_item(id: i64, name: &str) -> MediaItem {
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

    pub(crate) fn insert_sample_item(pool: &crate::core::db::DbPool, item: &MediaItem) -> i64 {
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

    pub(super) fn tile_count(grid: &MediaGrid) -> u32 {
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

    pub(super) fn first_section_flow(grid: &MediaGrid) -> Option<gtk::FlowBox> {
        let content = grid.imp().content.get();
        let mut child = content.first_child();
        while let Some(widget) = child {
            if let Some(flow) = widget.downcast_ref::<gtk::FlowBox>() {
                return Some(flow.clone());
            }
            child = widget.next_sibling();
        }
        None
    }

    pub(super) fn flow_child_at(flow: &gtk::FlowBox, index: u32) -> Option<gtk::FlowBoxChild> {
        flow.child_at_index(index as i32)
    }

    pub(super) fn first_square_tile(grid: &MediaGrid) -> Option<SquareTile> {
        first_section_flow(grid)?
            .first_child()
            .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
            .and_then(|child| child.child())
            .and_then(|child| child.downcast::<SquareTile>().ok())
    }

    pub(super) fn square_tiles(grid: &MediaGrid) -> Vec<SquareTile> {
        let content = grid.imp().content.get();
        let mut tiles = Vec::new();
        let mut section = content.first_child();
        while let Some(widget) = section {
            if let Some(flow) = widget.downcast_ref::<gtk::FlowBox>() {
                let mut child = flow.first_child();
                while let Some(flow_child) = child {
                    let next = flow_child.next_sibling();
                    if let Some(tile) = flow_child
                        .downcast::<gtk::FlowBoxChild>()
                        .ok()
                        .and_then(|child| child.child())
                        .and_then(|child| child.downcast::<SquareTile>().ok())
                    {
                        tiles.push(tile);
                    }
                    child = next;
                }
            }
            section = widget.next_sibling();
        }
        tiles
    }

    pub(super) fn section_flow_selection_modes(grid: &MediaGrid) -> Vec<gtk::SelectionMode> {
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
        pub full_library_context: Cell<bool>,
        pub flat_sections: Cell<bool>,
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
        pub on_set_album_cover: std::cell::OnceCell<Option<CoverCallback>>,
        pub context_menu_overlay: RefCell<Option<gtk::Overlay>>,
        /// Flattened metadata for every rendered tile in current mode.
        pub(super) displayed_items: RefCell<Vec<DisplayedItem>>,
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
        pub stats_refresh_running: Cell<bool>,
        /// Full-library metadata used by virtual spacers, thumbnail stats, and
        /// authoritative section counts. Loaded after first paint so startup
        /// is not blocked by COUNT/GROUP BY projections.
        pub library_metadata_loading: Cell<bool>,
        pub library_metadata_dirty_pending: Cell<bool>,
        pub library_total_snapshot: Cell<Option<u32>>,
        pub library_stats_snapshot: Cell<Option<LibraryStats>>,
        pub section_count_snapshots: RefCell<HashMap<GroupBy, HashMap<SectionKey, u32>>>,
        /// Shared model changes can arrive in large bursts during first-start
        /// scanning. Rebuilding all sections for every `items-changed` signal
        /// makes the GTK main thread do O(n²) widget work. Coalesce those
        /// bursts and rebuild once after the model has had a short quiet
        /// window.
        pub rebuild_debounce: RefCell<Option<gtk::glib::SourceId>>,
        /// Progressive first render: armed at construction, consumed on the
        /// first `rebuild`. When armed, that first rebuild builds only a
        /// viewport-sized seed of tiles; `schedule_progressive_render_fill`
        /// then paces the rest of the first page in background ticks.
        pub progressive_render_pending: Cell<bool>,
        /// Bumped on mode/active/model changes to cancel any in-flight
        /// progressive fill whose captured generation no longer matches.
        pub progressive_render_gen: Cell<u32>,
    }

    impl Default for MediaGrid {
        fn default() -> Self {
            Self {
                content: TemplateChild::default(),
                scroller: TemplateChild::default(),
                mode: Cell::default(),
                enable_context_menu: Cell::new(false),
                full_library_context: Cell::new(true),
                flat_sections: Cell::new(false),
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
                on_set_album_cover: std::cell::OnceCell::new(),
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
                stats_refresh_running: Cell::new(false),
                library_metadata_loading: Cell::new(false),
                library_metadata_dirty_pending: Cell::new(false),
                library_total_snapshot: Cell::new(None),
                library_stats_snapshot: Cell::new(None),
                section_count_snapshots: RefCell::new(HashMap::new()),
                rebuild_debounce: RefCell::new(None),
                progressive_render_pending: Cell::new(false),
                progressive_render_gen: Cell::new(0),
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
pub(super) struct ViewSpec {
    pixel_size: i32,
    thumb_size: ThumbnailSize,
    mode: GroupBy,
}

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

fn scroll_ratio_from_adjustment_value(value: f64, upper: f64, page_size: f64) -> f64 {
    let scrollable = (upper - page_size).max(0.0);
    if scrollable > 0.0 {
        (value / scrollable).clamp(0.0, 1.0)
    } else {
        0.0
    }
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
        Self::new_with_options(
            media_list,
            mode,
            loader,
            callbacks,
            enable_context_menu,
            initial_active,
            true,
            true,
        )
    }

    pub fn new_for_album(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        callbacks: MediaGridCallbacks,
    ) -> Self {
        Self::new_with_options(
            media_list, mode, loader, callbacks, false, true, true, false,
        )
    }

    pub fn new_for_album_with_context_menu(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        callbacks: MediaGridCallbacks,
    ) -> Self {
        Self::new_with_options(media_list, mode, loader, callbacks, true, true, true, false)
    }

    fn new_with_options(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        callbacks: MediaGridCallbacks,
        enable_context_menu: bool,
        initial_active: bool,
        progressive_first_render: bool,
        full_library_context: bool,
    ) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.imp().mode.set(mode);
        obj.imp().active.set(initial_active);
        // Arm progressive first render for grids that should seed the first
        // page. Photos uses it at startup and lazy mode activation; album pages
        // use the same rule while keeping full-library metadata disabled.
        obj.imp()
            .progressive_render_pending
            .set(progressive_first_render);
        obj.imp().full_library_context.set(full_library_context);
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
        obj.imp()
            .on_set_album_cover
            .set(callbacks.on_set_album_cover)
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
        obj.connect_model_changes(&media_list);
        obj.connect_scroll_handlers();
        obj
    }

    pub fn set_mode(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        self.invalidate_progressive_render_fill();
        self.imp().mode.set(mode);
        *self.imp().media_list.borrow_mut() = Some(media_list.clone());
        self.rebuild(media_list, mode);
    }

    pub fn set_active(&self, active: bool) {
        let from = self.imp().active.get();
        let dirty_model = self.imp().dirty_model.get();
        let list_len = self
            .imp()
            .media_list
            .borrow()
            .as_ref()
            .map(|list| list.n_items())
            .unwrap_or(0);
        let span = tracing::info_span!(
            target: crate::core::log_targets::BROWSING,
            "grid:set_active",
            mode = ?self.mode(),
            from,
            to = active,
            dirty_model,
            list_len,
            outcome = tracing::field::Empty
        );
        let _trace = span.enter();
        let prev = self.imp().active.replace(active);
        // Only cancel an in-flight startup fill on an ACTUAL visibility change.
        // `sync_active_grid_rebuilds` calls set_active on every grid for each
        // stack switch — including the no-op re-activation of the already-visible
        // grid at startup — so bumping unconditionally would cancel the fill the
        // seed render just armed (leaving the grid stuck at the seed count).
        if prev != active {
            self.invalidate_progressive_render_fill();
        }
        if active && self.imp().dirty_model.replace(false) {
            if let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() {
                span.record("outcome", "activate_rebuild");
                self.rebuild_immediately(media_list);
            } else {
                span.record("outcome", "activate_no_list");
            }
        } else if prev != active {
            span.record("outcome", "active_changed_no_rebuild");
        } else {
            span.record("outcome", "no_change");
        }
    }

    pub fn set_full_library_context(&self, enabled: bool) {
        if self.imp().full_library_context.replace(enabled) == enabled {
            return;
        }
        self.invalidate_library_metadata();
        self.imp().virtual_page_loading.set(false);
        self.imp().virtual_query_in_flight.set(false);
        self.imp().pending_virtual_page_start.set(None);
        self.imp().pending_virtual_page_ratio.set(None);
        self.imp()
            .virtual_page_generation
            .set(self.imp().virtual_page_generation.get().saturating_add(1));
        if let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() {
            self.rebuild_immediately(media_list);
        }
    }

    pub fn set_content_sized_scroll(&self, max_content_height: i32) {
        self.set_vexpand(false);
        let scroller = self.imp().scroller.get();
        scroller.set_vexpand(false);
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(max_content_height.max(1));
    }

    pub fn set_flat_sections(&self, enabled: bool) {
        if self.imp().flat_sections.replace(enabled) == enabled {
            return;
        }
        if let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() {
            self.rebuild_immediately(media_list);
        }
    }

    pub fn uses_flat_sections(&self) -> bool {
        self.imp().flat_sections.get()
    }

    /// Append a custom widget as the last child of the last section FlowBox.
    /// Used by the search page to insert a "show more" tile at the end of the
    /// grid. The widget is wrapped in a `FlowBoxChild` automatically.
    pub fn append_extra_child(&self, widget: &gtk::Widget) {
        let content = self.imp().content.get();
        // Find the last FlowBox child of the content box.
        let mut last_flow: Option<gtk::FlowBox> = None;
        let mut child = content.first_child();
        while let Some(c) = child {
            if let Ok(flow) = c.clone().downcast::<gtk::FlowBox>() {
                last_flow = Some(flow);
            }
            child = c.next_sibling();
        }
        if let Some(flow) = last_flow {
            let flow_child = gtk::FlowBoxChild::builder().child(widget).build();
            flow.append(&flow_child);
        }
    }

    pub fn hscrollbar_policy(&self) -> gtk::PolicyType {
        self.imp().scroller.get().hscrollbar_policy()
    }

    pub fn vscrollbar_policy(&self) -> gtk::PolicyType {
        self.imp().scroller.get().vscrollbar_policy()
    }

    pub fn mode(&self) -> GroupBy {
        self.imp().mode.get()
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

    /// The scrollbar thumb's travel fraction: `value / (upper - page_size)`,
    /// clamped to `[0, 1]`. `0.0` when the content does not overflow.
    pub fn scroll_fraction(&self) -> f64 {
        let adj = self.imp().scroller.get().vadjustment();
        scroll_ratio_from_adjustment_value(adj.value(), adj.upper(), adj.page_size())
    }

    /// The date section at the current scroll position, resolved from the
    /// already-loaded full-library `section_counts` so it stays correct even in
    /// virtual-paged regions whose tiles are not realized. `None` when library
    /// metadata has not loaded yet, the library is empty, or there is only one
    /// section (nothing meaningful to indicate).
    pub fn current_scroll_section_key(&self) -> Option<SectionKey> {
        let imp = self.imp();
        let total = imp.library_total_snapshot.get()?;
        let counts_by_mode = imp.section_count_snapshots.borrow();
        let counts = counts_by_mode.get(&imp.mode.get())?;
        if counts.len() <= 1 {
            return None;
        }
        let ratio = self.scroll_fraction();
        let offset = ((ratio * total as f64).round() as u32).min(total.saturating_sub(1));
        crate::core::section_model::section_for_global_offset(counts, offset)
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
                if let Some(bounds) = flow.compute_bounds(self) {
                    let contains_center = selector_mid_x >= bounds.x()
                        && selector_mid_x <= bounds.x() + bounds.width()
                        && selector_mid_y >= bounds.y()
                        && selector_mid_y <= bounds.y() + bounds.height();
                    if contains_center {
                        let local_x = (selector_mid_x - bounds.x()).floor() as i32;
                        let local_y = (selector_mid_y - bounds.y()).floor() as i32;
                        return flow
                            .child_at_pos(local_x, local_y)
                            .and_then(|child| child.first_child())
                            .and_then(|tile| tile.downcast::<SquareTile>().ok())
                            .and_then(|tile| tile.background_is_light());
                    }
                }
            }
            section_child = child.next_sibling();
        }
        None
    }

    fn detach_reusable_loaded_tiles(&self) -> HashMap<MediaId, SquareTile> {
        let displayed = self.imp().displayed_items.borrow();
        let mut reusable = HashMap::new();
        for item in displayed.iter() {
            let Some(tile) = item
                .flow_child
                .child()
                .and_then(|child| child.downcast::<SquareTile>().ok())
            else {
                continue;
            };
            if tile.has_css_class("thumb-loading") {
                continue;
            }
            if let Some(flow) = item.flow_child.parent().and_downcast::<gtk::FlowBox>() {
                flow.remove(&item.flow_child);
            }
            // Explicitly detach the tile from its (now floating) FlowBoxChild.
            // Relying on the FlowBoxChild's later finalization to unparent the
            // tile is racy under GTK toggle-ref timing; detaching here guarantees
            // a clean floating widget the rebuild can re-append without tripping
            // `gtk_flow_box_child_set_child`.
            item.flow_child.set_child(None::<&gtk::Widget>);
            reusable.insert(item.media_id, tile);
        }
        reusable
    }
}

pub(crate) fn format_tile_duration(secs: f64) -> Option<String> {
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

pub(crate) fn thumbnail_request_mtime(item: &MediaItem) -> std::time::SystemTime {
    std::time::SystemTime::from(item.file_mtime)
}

fn section_key_for_item(item: &MediaItem, mode: GroupBy) -> SectionKey {
    let dt = item.sort_datetime();
    match mode {
        GroupBy::Year => SectionKey {
            year: Some(dt.year()),
            month: None,
            day: None,
        },
        GroupBy::Month => SectionKey {
            year: Some(dt.year()),
            month: Some(dt.month()),
            day: None,
        },
        GroupBy::Day => SectionKey {
            year: Some(dt.year()),
            month: Some(dt.month()),
            day: Some(dt.day()),
        },
    }
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

fn media_item_at_displayed_index(media_list: &gio::ListStore, index: u32) -> Option<MediaItem> {
    crate::ui::media_list::media_item_at(media_list, index)
}

fn find_media_item_by_uri(media_list: &gio::ListStore, uri: &str) -> Option<(u32, MediaItem)> {
    for index in 0..media_list.n_items() {
        let Some(item) = media_item_at_displayed_index(media_list, index) else {
            continue;
        };
        if item.uri == uri {
            return Some((index, item));
        }
    }
    None
}

impl Default for MediaGrid {
    fn default() -> Self {
        gtk::glib::Object::builder().build()
    }
}

#[cfg(test)]
mod tests;
