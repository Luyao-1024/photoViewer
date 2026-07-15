//! Virtual `GtkGridView` for large media queries.
//!
//! It exposes a stable logical model for the complete query and only keeps
//! data/thumbnail work near the viewport. Photos, album detail, Trash, and
//! search-detail pages use this renderer; bounded search previews retain
//! `MediaGrid`.

mod factory;
mod layout_index;
mod mode;
mod model;
mod range_cache;

use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::runtime_config;
use crate::core::section_model::{GroupBy, SectionKey};
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
use crate::ui::media_grid::{thumbnail_request_mtime, FavoriteMenuState, MediaGridCallbacks};
use crate::ui::mode_selector::ModeSelector;
use chrono::Datelike;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use factory::FactoryCell;
use layout_index::VirtualGridLayoutIndex;
use mode::{VirtualGridModeSpec, VirtualGridViewportMetrics};
use model::VirtualMediaModel;
use range_cache::{expanded_visible_range, MediaRange, RangeCoordinator, RequestDisposition};

/// A card's outer gap is split between the grid padding and each list-item
/// wrapper's margin. This keeps the visual 8px edge/gutter while leaving
/// GtkGridView's own `border-spacing` at zero.
const VIRTUAL_GRID_OUTER_PADDING_PX: i32 = 4;

fn virtual_grid_content_width(width: i32) -> i32 {
    width.saturating_sub(VIRTUAL_GRID_OUTER_PADDING_PX.saturating_mul(2))
}

fn virtual_grid_preferred_width(spec: VirtualGridModeSpec, columns: u32) -> i32 {
    spec.preferred_width_for_fixed_columns(columns)
        .saturating_add(VIRTUAL_GRID_OUTER_PADDING_PX.saturating_mul(2))
}

pub(crate) fn preferred_day_grid_width(columns: usize) -> i32 {
    virtual_grid_preferred_width(VirtualGridModeSpec::for_mode(GroupBy::Day), columns as u32)
}

pub(crate) use model::GridSlotState;

/// Identity for a factory binding.  A thumbnail result may only paint a tile
/// when all four parts still match, which prevents recycled list items from
/// receiving an old async result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TileBinding {
    layout_generation: u64,
    slot: u32,
    media_id: MediaId,
    cache_key: Option<String>,
}

impl TileBinding {
    pub(super) fn new(
        layout_generation: u64,
        slot: u32,
        media_id: MediaId,
        cache_key: Option<String>,
    ) -> Self {
        Self {
            layout_generation,
            slot,
            media_id,
            cache_key,
        }
    }

    pub(super) fn media_id(&self) -> MediaId {
        self.media_id
    }
}

/// Coalesces per-tile thumbnail requests issued during factory binds and
/// releases them in bounded batches.
///
/// A landing can realize ~100 tiles in a single frame; letting each bind call
/// `request_thumbnail` synchronously floods the worker queue with ~100 items at
/// once (the measured p99 queue_wait tail). Instead the factory builds the full
/// request closure and hands it here; the grid drains at most
/// `thumbnail_batch_per_frame()` per main-loop idle and re-arms while the queue
/// is non-empty, so the queue stays shallow and delivery spreads across frames.
/// Staleness is still guarded downstream by `defer_thumbnail_paint` — the
/// batcher only throttles *when* a request is issued, not its result.
pub(super) struct ThumbnailBatcher {
    pending: Vec<Box<dyn FnOnce()>>,
    scheduled: Cell<bool>,
}

impl ThumbnailBatcher {
    fn enqueue(&mut self, request: Box<dyn FnOnce()>) {
        self.pending.push(request);
    }

    /// Remove up to `batch` requests (FIFO) for the caller to invoke.
    fn drain(&mut self, batch: usize) -> Vec<Box<dyn FnOnce()>> {
        let take = batch.min(self.pending.len());
        self.pending.drain(..take).collect()
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.scheduled.set(false);
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn scheduled(&self) -> bool {
        self.scheduled.get()
    }

    fn set_scheduled(&self, value: bool) {
        self.scheduled.set(value);
    }
}

impl Default for ThumbnailBatcher {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            scheduled: Cell::new(false),
        }
    }
}

fn scroll_fraction_from_adjustment(adjustment: &gtk::Adjustment) -> f64 {
    let travel = (adjustment.upper() - adjustment.page_size()).max(0.0);
    if travel > 0.0 {
        (adjustment.value() / travel).clamp(0.0, 1.0)
    } else {
        0.0
    }
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

fn counts_for_items(items: &[MediaItem], mode: GroupBy) -> HashMap<SectionKey, u32> {
    let mut counts = HashMap::new();
    for item in items {
        *counts.entry(section_key_for_item(item, mode)).or_insert(0) += 1;
    }
    counts
}

fn normalise_authoritative_counts(
    mut counts: HashMap<SectionKey, u32>,
    reported_total: u32,
) -> (u32, HashMap<SectionKey, u32>) {
    let counted = counts.values().copied().fold(0u32, u32::saturating_add);
    // `count` and `section_counts` are two independent read queries. A
    // filesystem update between them must not leave the layout with more slots
    // than its range ceiling, or the opposite. Prefer the section aggregate
    // when it is newer, and retain a synthetic unknown section when the total
    // is newer, until the scheduled metadata refresh converges them again.
    let total = reported_total.max(counted);
    if counted < total {
        *counts
            .entry(SectionKey {
                year: None,
                month: None,
                day: None,
            })
            .or_insert(0) += total - counted;
    }
    (total, counts)
}

mod imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/virtual-media-grid.ui")]
    pub struct VirtualMediaGrid {
        #[template_child]
        pub grid: TemplateChild<gtk::GridView>,
        #[template_child]
        pub scroller: TemplateChild<gtk::ScrolledWindow>,
        pub mode: Cell<GroupBy>,
        pub active: Cell<bool>,
        pub(super) viewport_metrics: Cell<VirtualGridViewportMetrics>,
        pub(super) grid_columns: Cell<u32>,
        pub loader: OnceCell<Arc<ThumbnailLoader>>,
        pub callbacks: OnceCell<MediaGridCallbacks>,
        pub query: OnceCell<MediaQuery>,
        pub thumbnail_uri_resolver: RefCell<Option<Rc<dyn Fn(&MediaItem) -> Option<String>>>>,
        pub media_list: RefCell<Option<gio::ListStore>>,
        pub model: OnceCell<VirtualMediaModel>,
        pub selection_model: OnceCell<gtk::NoSelection>,
        pub context_menu_overlay: RefCell<Option<gtk::Overlay>>,
        pub selected: RefCell<HashSet<MediaId>>,
        pub is_multi_select_mode: Cell<bool>,
        pub on_selection_changed: OnceCell<Rc<dyn Fn()>>,
        pub on_view_changed: OnceCell<Rc<dyn Fn()>>,
        pub(super) factory_cells: RefCell<Vec<FactoryCell>>,
        pub metadata_counts: RefCell<Option<HashMap<SectionKey, u32>>>,
        pub metadata_ready: Cell<bool>,
        pub metadata_loading: Cell<bool>,
        pub metadata_dirty: Cell<bool>,
        pub metadata_generation: Cell<u64>,
        pub layout_generation: Cell<u64>,
        pub live_total: Cell<u32>,
        pub metadata_reload_source: RefCell<Option<glib::SourceId>>,
        pub pending_column_width: Cell<i32>,
        pub column_update_source: RefCell<Option<glib::SourceId>>,
        pub range: RefCell<RangeCoordinator>,
        pub last_top_slot: Cell<u32>,
        pub(super) thumb_batcher: RefCell<ThumbnailBatcher>,
    }

    impl Default for VirtualMediaGrid {
        fn default() -> Self {
            Self {
                grid: gtk::TemplateChild::default(),
                scroller: gtk::TemplateChild::default(),
                mode: Cell::default(),
                active: Cell::new(false),
                viewport_metrics: Cell::new(VirtualGridViewportMetrics::default()),
                grid_columns: Cell::new(1),
                loader: OnceCell::new(),
                callbacks: OnceCell::new(),
                query: OnceCell::new(),
                thumbnail_uri_resolver: RefCell::new(None),
                media_list: RefCell::new(None),
                model: OnceCell::new(),
                selection_model: OnceCell::new(),
                context_menu_overlay: RefCell::new(None),
                selected: RefCell::new(HashSet::new()),
                is_multi_select_mode: Cell::new(false),
                on_selection_changed: OnceCell::new(),
                on_view_changed: OnceCell::new(),
                factory_cells: RefCell::new(Vec::new()),
                metadata_counts: RefCell::new(None),
                metadata_ready: Cell::new(false),
                metadata_loading: Cell::new(false),
                metadata_dirty: Cell::new(false),
                metadata_generation: Cell::new(0),
                layout_generation: Cell::new(0),
                live_total: Cell::new(0),
                metadata_reload_source: RefCell::new(None),
                pending_column_width: Cell::new(0),
                column_update_source: RefCell::new(None),
                range: RefCell::new(RangeCoordinator::default()),
                last_top_slot: Cell::new(0),
                thumb_batcher: RefCell::new(ThumbnailBatcher::default()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VirtualMediaGrid {
        // Blueprint's `template $VirtualMediaGrid` resolves the GObject type
        // by this exact name. Keep the Rust subclass and template in lockstep.
        const NAME: &'static str = "VirtualMediaGrid";
        type Type = super::VirtualMediaGrid;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for VirtualMediaGrid {
        fn dispose(&self) {
            if let Some(source) = self.metadata_reload_source.borrow_mut().take() {
                if glib::MainContext::default()
                    .find_source_by_id(&source)
                    .is_some()
                {
                    source.remove();
                }
            }
            if let Some(source) = self.column_update_source.borrow_mut().take() {
                if glib::MainContext::default()
                    .find_source_by_id(&source)
                    .is_some()
                {
                    source.remove();
                }
            }
            self.factory_cells.borrow_mut().clear();
            self.thumb_batcher.borrow_mut().clear();
        }
    }

    impl WidgetImpl for VirtualMediaGrid {}

    impl BoxImpl for VirtualMediaGrid {}
}

glib::wrapper! {
    pub struct VirtualMediaGrid(ObjectSubclass<imp::VirtualMediaGrid>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl VirtualMediaGrid {
    pub fn new(
        media_list: gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        callbacks: MediaGridCallbacks,
        initial_active: bool,
    ) -> Self {
        Self::new_for_query(
            media_list,
            MediaQuery::LiveAll,
            mode,
            loader,
            callbacks,
            initial_active,
        )
    }

    /// Build a virtual grid backed by a complete repository query rather than
    /// the bounded GTK seed list. The seed is used only for instant paint.
    pub fn new_for_query(
        media_list: gio::ListStore,
        query: MediaQuery,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        callbacks: MediaGridCallbacks,
        initial_active: bool,
    ) -> Self {
        // The Photos page normally installs this globally, but the widget is
        // also constructed directly by focused tests. Keep its GridView
        // spacing and selection rules self-contained and idempotent.
        crate::ui::grid_css::install();
        let obj: Self = glib::Object::new();
        let imp = obj.imp();
        imp.mode.set(mode);
        let configured_columns = runtime_config::photos_grid_columns() as u32;
        let initial_metrics = if mode == GroupBy::Day {
            imp.grid_columns.set(configured_columns);
            let preferred_width = virtual_grid_preferred_width(
                VirtualGridModeSpec::for_mode(mode),
                configured_columns,
            );
            imp.scroller.get().set_min_content_width(preferred_width);
            imp.grid.get().set_width_request(preferred_width);
            VirtualGridModeSpec::for_mode(mode)
                .viewport_metrics_for_fixed_columns(0, configured_columns)
        } else {
            VirtualGridModeSpec::for_mode(mode).viewport_metrics_for_width(0)
        };
        imp.viewport_metrics.set(initial_metrics);
        imp.active.set(initial_active);
        assert!(
            imp.loader.set(loader).is_ok(),
            "VirtualMediaGrid loader initialized more than once"
        );
        assert!(
            imp.callbacks.set(callbacks).is_ok(),
            "VirtualMediaGrid callbacks initialized more than once"
        );
        assert!(
            imp.query.set(query).is_ok(),
            "VirtualMediaGrid query initialized more than once"
        );
        *imp.media_list.borrow_mut() = Some(media_list.clone());

        let initial_layout = if initial_active {
            let items = media_items_from_list(&media_list);
            VirtualGridLayoutIndex::new(
                &counts_for_items(&items, mode),
                imp.viewport_metrics.get().columns(),
            )
        } else {
            VirtualGridLayoutIndex::default()
        };
        let model = VirtualMediaModel::new(initial_layout);
        let selection_model = gtk::NoSelection::new(Some(model.clone()));
        assert!(
            imp.model.set(model).is_ok(),
            "VirtualMediaGrid model initialized more than once"
        );
        assert!(
            imp.selection_model.set(selection_model.clone()).is_ok(),
            "VirtualMediaGrid selection model initialized more than once"
        );
        imp.grid.get().set_model(Some(&selection_model));
        imp.grid.get().set_enable_rubberband(false);
        imp.grid.get().set_single_click_activate(true);
        imp.grid.get().set_min_columns(initial_metrics.columns());
        imp.grid.get().set_max_columns(initial_metrics.columns());

        factory::install(&obj);
        obj.connect_grid_signals(&media_list);

        if initial_active {
            obj.seed_provisional_items();
            obj.reload_metadata_now();
            obj.schedule_visible_range_after_layout();
        }
        obj
    }

    pub fn set_context_menu_overlay(&self, overlay: Option<&gtk::Overlay>) {
        *self.imp().context_menu_overlay.borrow_mut() = overlay.cloned();
    }

    /// Resolves the source used for thumbnail reads without changing the
    /// repository item identity. Trash keeps its original URI in the database
    /// but thumbnails must read the file from the configured trash root.
    pub fn set_thumbnail_uri_resolver<F>(&self, resolver: F)
    where
        F: Fn(&MediaItem) -> Option<String> + 'static,
    {
        *self.imp().thumbnail_uri_resolver.borrow_mut() = Some(Rc::new(resolver));
    }

    pub fn mode(&self) -> GroupBy {
        self.imp().mode.get()
    }

    fn query(&self) -> MediaQuery {
        self.imp()
            .query
            .get()
            .expect("VirtualMediaGrid query initialized in new")
            .clone()
    }

    pub fn set_grid_columns(&self, columns: usize) {
        if self.mode() != GroupBy::Day {
            tracing::trace!(
                target: "ui::grid_settings",
                mode = ?self.mode(),
                columns,
                "virtual_grid_skip_non_day_columns"
            );
            return;
        }
        let columns = columns.clamp(
            runtime_config::MIN_PHOTOS_GRID_COLUMNS,
            runtime_config::MAX_PHOTOS_GRID_COLUMNS,
        ) as u32;
        if self.imp().grid_columns.replace(columns) == columns {
            tracing::trace!(
                target: "ui::grid_settings",
                mode = ?self.mode(),
                columns,
                "virtual_grid_columns_unchanged"
            );
            return;
        }
        let preferred_width = virtual_grid_preferred_width(self.spec(), columns);
        tracing::trace!(
            target: "ui::grid_settings",
            mode = ?self.mode(),
            columns,
            preferred_width,
            scroller_width = self.imp().scroller.get().width(),
            grid_width = self.imp().grid.get().width(),
            "virtual_grid_apply_day_columns"
        );
        self.imp()
            .scroller
            .get()
            .set_min_content_width(preferred_width);
        self.imp().grid.get().set_width_request(preferred_width);
        self.imp().grid.get().set_min_columns(columns);
        self.imp().grid.get().set_max_columns(columns);
        let width = self.imp().scroller.get().width();
        self.update_columns_for_width(width);
    }

    pub fn set_active(&self, active: bool) {
        let was_active = self.imp().active.replace(active);
        if !active {
            if was_active {
                // Do not let an old visible mode apply a range after it has
                // left the stack. The worker may finish naturally, but its
                // generation no longer owns this grid's model.
                self.imp().range.borrow_mut().invalidate();
                self.imp()
                    .metadata_generation
                    .set(self.imp().metadata_generation.get().saturating_add(1));
                self.imp().metadata_loading.set(false);
            }
            return;
        }
        if was_active {
            return;
        }
        if !self.imp().metadata_ready.get() {
            self.seed_provisional_items();
        }
        if self.imp().metadata_dirty.replace(false) || !self.imp().metadata_ready.get() {
            self.reload_metadata_now();
        }
        self.schedule_visible_range_after_layout();
    }

    pub fn connect_view_changed<F: Fn() + 'static>(&self, f: F) {
        assert!(
            self.imp().on_view_changed.set(Rc::new(f)).is_ok(),
            "VirtualMediaGrid::connect_view_changed called more than once"
        );
    }

    pub fn scroll_fraction(&self) -> f64 {
        scroll_fraction_from_adjustment(&self.imp().scroller.get().vadjustment())
    }

    pub fn current_scroll_section_key(&self) -> Option<SectionKey> {
        let model = self.model();
        let layout = model.layout();
        if layout.section_count() <= 1 {
            return None;
        }
        let top_slot = self.top_slot_for_adjustment();
        let slot = top_slot.min(layout.slot_count().saturating_sub(1));
        layout.section_for_slot(slot).cloned()
    }

    /// Resolve brightness from the small set of currently realized factory
    /// cells. Unlike the FlowBox path this never walks a full library: GTK
    /// keeps this collection bounded to the viewport and its overscan.
    pub fn background_is_light_under(&self, selector: &ModeSelector) -> Option<bool> {
        let selector_bounds = selector.compute_bounds(self)?;
        let x = selector_bounds.x() + selector_bounds.width() / 2.0;
        let y = selector_bounds.y() + selector_bounds.height() / 2.0;
        self.imp().factory_cells.borrow().iter().find_map(|cell| {
            let bounds = cell.tile.compute_bounds(self)?;
            let contains_center = x >= bounds.x()
                && x <= bounds.x() + bounds.width()
                && y >= bounds.y()
                && y <= bounds.y() + bounds.height();
            contains_center
                .then(|| cell.tile.background_is_light())
                .flatten()
        })
    }

    pub fn connect_selection_changed<F: Fn() + 'static>(&self, f: F) {
        assert!(
            self.imp().on_selection_changed.set(Rc::new(f)).is_ok(),
            "VirtualMediaGrid::connect_selection_changed called more than once"
        );
    }

    pub fn selected_ids(&self) -> Vec<MediaId> {
        self.imp().selected.borrow().iter().copied().collect()
    }

    pub fn is_multi_select_mode(&self) -> bool {
        self.imp().is_multi_select_mode.get()
    }

    pub fn set_multi_select_mode(&self, enabled: bool) {
        self.imp().is_multi_select_mode.set(enabled);
        if !enabled {
            self.clear_selection();
        }
    }

    pub fn select_all(&self) {
        let ids = self.model().ready_media_ids();
        self.select_ids(&ids);
    }

    pub fn select_ids(&self, ids: &[MediaId]) {
        self.imp().is_multi_select_mode.set(!ids.is_empty());
        let next = ids.iter().copied().collect::<HashSet<_>>();
        let changed = *self.imp().selected.borrow() != next;
        *self.imp().selected.borrow_mut() = next;
        self.sync_visible_selection();
        if changed {
            self.fire_selection_changed();
        }
    }

    pub fn clear_selection(&self) {
        let changed = !self.imp().selected.borrow().is_empty();
        self.imp().selected.borrow_mut().clear();
        self.imp().is_multi_select_mode.set(false);
        self.sync_visible_selection();
        if changed {
            self.fire_selection_changed();
        }
    }

    pub fn is_all_displayed_selected(&self) -> bool {
        let ids = self.model().ready_media_ids();
        !ids.is_empty()
            && ids
                .iter()
                .all(|id| self.imp().selected.borrow().contains(id))
    }

    pub fn viewer_seed_for(&self, media_id: MediaId) -> Option<gio::ListStore> {
        let item = self.model().ready_item_for_media_id(media_id)?;
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        store.append(&glib::BoxedAnyObject::new(item));
        Some(store)
    }

    /// Total logical media slots in the current query, excluding structural
    /// filler slots used to keep date sections aligned.
    pub fn logical_media_count(&self) -> u32 {
        self.model().layout().media_count()
    }

    /// First interactive media slot in the current layout. Section-boundary
    /// filler slots may occupy position zero.
    pub fn first_media_slot(&self) -> Option<u32> {
        self.model().layout().slot_for_media_offset(0)
    }

    /// First currently resident interactive media slot. Callers that need to
    /// synthesize activation should wait for this rather than a placeholder.
    pub fn first_ready_media_slot(&self) -> Option<u32> {
        let model = self.model();
        (0..model.layout().slot_count())
            .find(|slot| matches!(model.slot_state(*slot), Some(GridSlotState::Ready { .. })))
    }

    /// Called by the bounded shared-list projection after filesystem/domain
    /// events.  The list is only a signal/initial-seed source: authoritative
    /// metadata and ranges always come from `MediaRepository`.
    pub fn refresh_from_shared_projection(&self) {
        self.schedule_metadata_reload();
    }

    pub(super) fn spec(&self) -> VirtualGridModeSpec {
        VirtualGridModeSpec::for_mode(self.mode())
    }

    fn viewport_metrics(&self) -> VirtualGridViewportMetrics {
        self.imp().viewport_metrics.get()
    }

    pub(super) fn loader(&self) -> Arc<ThumbnailLoader> {
        self.imp()
            .loader
            .get()
            .expect("VirtualMediaGrid loader initialized in new")
            .clone()
    }

    pub(super) fn layout_generation(&self) -> u64 {
        self.imp().layout_generation.get()
    }

    pub(super) fn is_selected(&self, media_id: MediaId) -> bool {
        self.imp().selected.borrow().contains(&media_id)
    }

    pub(super) fn thumbnail_uri_for(&self, item: &MediaItem) -> String {
        self.imp()
            .thumbnail_uri_resolver
            .borrow()
            .as_ref()
            .and_then(|resolver| resolver(item))
            .unwrap_or_else(|| item.uri.clone())
    }

    pub(super) fn notify_background_changed(&self) {
        if let Some(callbacks) = self.imp().callbacks.get() {
            (callbacks.on_background_changed)();
        }
    }

    fn set_list_factory(&self, factory: &gtk::SignalListItemFactory) {
        self.imp().grid.get().set_factory(Some(factory));
    }

    fn register_factory_cell(&self, cell: FactoryCell) {
        self.imp().factory_cells.borrow_mut().push(cell);
    }

    fn factory_cell_for(&self, tile: &crate::ui::square_tile::SquareTile) -> Option<FactoryCell> {
        self.imp()
            .factory_cells
            .borrow()
            .iter()
            .find(|cell| cell.tile == *tile)
            .cloned()
    }

    fn remove_factory_cell(&self, tile: &crate::ui::square_tile::SquareTile) {
        self.imp()
            .factory_cells
            .borrow_mut()
            .retain(|cell| cell.tile != *tile);
    }

    /// Throttle a factory bind's thumbnail request so a ~100-tile landing does
    /// not flood the worker queue in one frame. `request` is the full
    /// `request_thumbnail` capture; it is invoked on a later main-loop idle, at
    /// most `thumbnail_batch_per_frame()` per drain. Paint-time staleness is
    /// still guarded downstream by `defer_thumbnail_paint`.
    pub(super) fn enqueue_thumbnail(&self, request: impl FnOnce() + 'static) {
        self.imp()
            .thumb_batcher
            .borrow_mut()
            .enqueue(Box::new(request));
        self.schedule_thumb_flush();
    }

    fn schedule_thumb_flush(&self) {
        {
            let batcher = self.imp().thumb_batcher.borrow();
            if batcher.scheduled() {
                return;
            }
        }
        self.imp().thumb_batcher.borrow().set_scheduled(true);
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            let Some(grid) = weak.upgrade() else {
                return;
            };
            grid.tick_thumb_flush();
        });
    }

    /// Drain one bounded batch of pending thumbnail requests, then re-arm while
    /// any remain. A request closure only issues `loader.request_for_media`
    /// (never enqueues again) and this runs on the main thread, so there is no
    /// re-entrancy: no new request can arrive during the drain loop.
    fn tick_thumb_flush(&self) {
        let batch = runtime_config::thumbnail_batch_per_frame();
        let drained = self.imp().thumb_batcher.borrow_mut().drain(batch);
        for request in drained {
            request();
        }
        if self.imp().thumb_batcher.borrow().is_empty() {
            self.imp().thumb_batcher.borrow().set_scheduled(false);
        } else {
            let weak = self.downgrade();
            glib::idle_add_local_once(move || {
                let Some(grid) = weak.upgrade() else {
                    return;
                };
                grid.tick_thumb_flush();
            });
        }
    }

    pub(super) fn show_context_menu(
        &self,
        anchor: &gtk::Widget,
        binding: &TileBinding,
        x: f64,
        y: f64,
    ) {
        let Some(overlay) = self.imp().context_menu_overlay.borrow().clone() else {
            return;
        };
        let media_id = binding.media_id();
        let in_multi = self.is_multi_select_mode();
        let target_ids = if in_multi {
            self.ensure_context_selection(media_id)
        } else {
            vec![media_id]
        };
        let callbacks = self
            .imp()
            .callbacks
            .get()
            .expect("VirtualMediaGrid callbacks initialized in new")
            .clone();
        let favorite_state = (callbacks.on_query_favorite_state)(target_ids.clone());
        let mut items = Vec::new();

        if in_multi {
            let weak = self.downgrade();
            items.push(GlassMenuItem::new(
                tr("photos.batch.exit_multi_select"),
                GlassMenuItemKind::Danger,
                move || {
                    if let Some(grid) = weak.upgrade() {
                        grid.clear_selection();
                    }
                },
            ));
        } else {
            let weak = self.downgrade();
            items.push(GlassMenuItem::new(
                tr("photos.batch.multi_select"),
                GlassMenuItemKind::Suggested,
                move || {
                    if let Some(grid) = weak.upgrade() {
                        grid.imp().is_multi_select_mode.set(true);
                        grid.ensure_context_selection(media_id);
                    }
                },
            ));
        }

        append_favorite_menu_items(&mut items, &callbacks, favorite_state, &target_ids);
        if !target_ids.is_empty() {
            if !in_multi {
                if let Some(on_set_album_cover) = callbacks.on_set_album_cover.clone() {
                    items.push(GlassMenuItem::new(
                        tr("album.context.set_cover"),
                        GlassMenuItemKind::Normal,
                        move || on_set_album_cover(media_id),
                    ));
                }
            }
            let add_ids = target_ids.clone();
            let on_add = callbacks.on_add_to_album.clone();
            items.push(GlassMenuItem::new(
                tr("photos.batch.move_to_album"),
                GlassMenuItemKind::Normal,
                move || on_add(add_ids.clone()),
            ));

            let trash_ids = target_ids.clone();
            let on_trash = callbacks.on_move_to_trash.clone();
            let weak = self.downgrade();
            items.push(GlassMenuItem::new(
                tr("viewer.tooltip.move_to_trash"),
                GlassMenuItemKind::Danger,
                move || {
                    let count = trash_ids.len();
                    let body = if count == 1 {
                        tr("trash.confirm_body_one")
                    } else {
                        tr("trash.confirm_body_many").replace("{count}", &count.to_string())
                    };
                    let dialog = adw::AlertDialog::builder()
                        .heading(tr("trash.confirm_title"))
                        .body(body)
                        .build();
                    dialog.add_css_class("glass-alert-dialog");
                    dialog.add_response("cancel", &tr("dialog.cancel"));
                    dialog.add_response("trash", &tr("dialog.trash"));
                    dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
                    dialog.set_default_response(Some("cancel"));
                    dialog.set_close_response("cancel");
                    let ids = trash_ids.clone();
                    let callback = on_trash.clone();
                    dialog.connect_response(None, move |_, response| {
                        if response == "trash" {
                            callback(ids.clone());
                        }
                    });
                    if let Some(grid) = weak.upgrade() {
                        dialog.present(&grid);
                    }
                },
            ));
        }
        glass_context_menu::show(&overlay, anchor, x, y, items);
    }

    fn connect_grid_signals(&self, media_list: &gio::ListStore) {
        let weak = self.downgrade();
        self.imp().grid.get().connect_activate(move |_, position| {
            if let Some(grid) = weak.upgrade() {
                grid.activate_slot(position);
            }
        });

        let weak = self.downgrade();
        self.imp()
            .scroller
            .get()
            .vadjustment()
            .connect_value_changed(move |_| {
                if let Some(grid) = weak.upgrade() {
                    grid.schedule_visible_range();
                    if let Some(callback) = grid.imp().on_view_changed.get() {
                        callback();
                    }
                }
            });

        // GtkGridView does not expose a size-allocate signal at the GTK 4.8
        // level. The scrolled window's horizontal adjustment is updated for
        // every initial allocation and resize, giving us the actual viewport
        // width (after scrollbar allocation) that must agree with the layout
        // index's fixed column count and responsive row metrics.
        let weak = self.downgrade();
        self.imp()
            .scroller
            .get()
            .hadjustment()
            .connect_changed(move |adjustment| {
                let width = adjustment
                    .page_size()
                    .round()
                    .clamp(0.0, f64::from(i32::MAX)) as i32;
                if width > 0 {
                    if let Some(grid) = weak.upgrade() {
                        grid.schedule_column_update(width);
                    }
                }
            });

        let weak = self.downgrade();
        self.connect_map(move |_| {
            if let Some(grid) = weak.upgrade() {
                let width = grid.imp().scroller.get().width();
                if width > 0 {
                    grid.schedule_column_update(width);
                }
            }
        });

        let weak = self.downgrade();
        media_list.connect_items_changed(move |_, _, _, _| {
            if let Some(grid) = weak.upgrade() {
                grid.refresh_from_shared_projection();
            }
        });
    }

    fn model(&self) -> VirtualMediaModel {
        self.imp()
            .model
            .get()
            .expect("VirtualMediaGrid model initialized in new")
            .clone()
    }

    fn seed_provisional_items(&self) {
        let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let items = media_items_from_list(&media_list);
        let counts = counts_for_items(&items, self.mode());
        self.imp().metadata_counts.replace(Some(counts.clone()));
        let layout = VirtualGridLayoutIndex::new(&counts, self.viewport_metrics().columns());
        self.replace_layout_with_initial_items(layout, items);
    }

    fn reload_metadata_now(&self) {
        if !self.imp().active.get() {
            self.imp().metadata_dirty.set(true);
            return;
        }
        if self.imp().metadata_loading.replace(true) {
            self.imp().metadata_dirty.set(true);
            return;
        }
        self.imp().metadata_dirty.set(false);
        let generation = self.imp().metadata_generation.get().saturating_add(1);
        self.imp().metadata_generation.set(generation);
        let mode = self.mode();
        let query = self.query();
        let pool = self.loader().pool().clone();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let repo = MediaRepository::new(pool);
                let total = repo.count(query.clone())?;
                let counts = repo.section_counts_for_query(query, mode)?;
                Ok::<_, crate::core::error::AppError>((total, counts))
            })
            .await;
            let Some(grid) = weak.upgrade() else {
                return;
            };
            if grid.imp().metadata_generation.get() != generation {
                return;
            }
            grid.imp().metadata_loading.set(false);
            match result {
                Ok(Ok((total, counts))) => grid.apply_authoritative_metadata(total, counts),
                Ok(Err(error)) => tracing::warn!(
                    target: crate::core::log_targets::BROWSING,
                    mode = ?mode,
                    "gridview metadata query failed: {error}"
                ),
                Err(error) => tracing::warn!(
                    target: crate::core::log_targets::BROWSING,
                    mode = ?mode,
                    "gridview metadata worker failed: {error:?}"
                ),
            }
            if grid.imp().metadata_dirty.replace(false) {
                grid.schedule_metadata_reload();
            }
        });
    }

    fn apply_authoritative_metadata(&self, total: u32, counts: HashMap<SectionKey, u32>) {
        let (total, counts) = normalise_authoritative_counts(counts, total);
        self.imp().live_total.set(total);
        self.imp().metadata_counts.replace(Some(counts.clone()));
        self.imp().metadata_ready.set(true);
        // Preserve instant first paint when the shared startup window matches
        // the beginning of the canonical live ordering; the authoritative
        // range worker will replace it if it changed in the meantime. Seed
        // those items atomically with the layout notification: emitting an
        // immediate second replacement for freshly inserted GridView slots can
        // race GTK 4.20's ListItem accessibility bookkeeping.
        let mut seed = self
            .imp()
            .media_list
            .borrow()
            .as_ref()
            .map(media_items_from_list)
            .unwrap_or_default();
        seed.truncate(total as usize);
        self.replace_layout_with_initial_items(
            VirtualGridLayoutIndex::new(&counts, self.viewport_metrics().columns()),
            seed,
        );
        self.schedule_visible_range_after_layout();
    }

    fn schedule_metadata_reload(&self) {
        if !self.imp().active.get() {
            self.imp().metadata_dirty.set(true);
            return;
        }
        if self.imp().metadata_reload_source.borrow().is_some() {
            return;
        }
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            if let Some(grid) = weak.upgrade() {
                *grid.imp().metadata_reload_source.borrow_mut() = None;
                grid.reload_metadata_now();
            }
        });
        *self.imp().metadata_reload_source.borrow_mut() = Some(source);
    }

    fn replace_layout_with_initial_items(
        &self,
        layout: VirtualGridLayoutIndex,
        items: Vec<crate::core::media::MediaItem>,
    ) {
        self.imp().range.borrow_mut().invalidate();
        let generation = self.imp().layout_generation.get().saturating_add(1);
        self.imp().layout_generation.set(generation);
        let limit = items.len().min(u32::MAX as usize) as u32;
        self.model()
            .replace_layout_with_ready_range(layout, generation, 0..limit, items);
    }

    fn update_columns_for_width(&self, width: i32) {
        let next_metrics = if self.mode() == GroupBy::Day {
            self.spec().viewport_metrics_for_fixed_columns(
                virtual_grid_content_width(width),
                self.imp().grid_columns.get(),
            )
        } else {
            self.spec()
                .viewport_metrics_for_width(virtual_grid_content_width(width))
        };
        let previous_metrics = self.viewport_metrics();
        if next_metrics == previous_metrics {
            tracing::trace!(
                target: "ui::grid_settings",
                mode = ?self.mode(),
                width,
                columns = next_metrics.columns(),
                tile_size = next_metrics.tile_size(),
                "virtual_grid_width_update_unchanged"
            );
            return;
        }

        tracing::trace!(
            target: "ui::grid_settings",
            mode = ?self.mode(),
            width,
            previous_columns = previous_metrics.columns(),
            previous_tile_size = previous_metrics.tile_size(),
            columns = next_metrics.columns(),
            tile_size = next_metrics.tile_size(),
            "virtual_grid_width_update"
        );

        let old_layout = self.model().layout();
        let old_top_slot = self.top_slot_for_adjustment();
        let anchor = old_layout.anchor_media_offset_for_slot(old_top_slot);
        let columns_changed = next_metrics.columns() != previous_metrics.columns();
        self.imp().viewport_metrics.set(next_metrics);
        if columns_changed {
            self.imp()
                .grid
                .get()
                .set_min_columns(next_metrics.columns());
            self.imp()
                .grid
                .get()
                .set_max_columns(next_metrics.columns());
        }

        let Some(counts) = self.imp().metadata_counts.borrow().clone() else {
            return;
        };
        if columns_changed {
            let layout_started = std::time::Instant::now();
            let layout = VirtualGridLayoutIndex::new(&counts, next_metrics.columns());
            let restored_slot =
                anchor.and_then(|offset| layout.slot_for_media_offset_clamped(offset));
            tracing::trace!(
                target: "ui::grid_settings",
                mode = ?self.mode(),
                columns = next_metrics.columns(),
                slot_count = layout.slot_count(),
                media_count = layout.media_count(),
                elapsed_ms = layout_started.elapsed().as_secs_f64() * 1000.0,
                "virtual_grid_layout_index_built"
            );
            // A column change only reflows physical slots. Canonical media
            // offsets and bounded range residency remain valid, so retain
            // them to avoid a placeholder/database reload pass while the
            // user is dragging the window edge.
            let generation = self.imp().layout_generation.get().saturating_add(1);
            self.imp().layout_generation.set(generation);
            let reflow_started = std::time::Instant::now();
            self.model().reflow_layout(layout, generation);
            tracing::trace!(
                target: "ui::grid_settings",
                mode = ?self.mode(),
                generation,
                elapsed_ms = reflow_started.elapsed().as_secs_f64() * 1000.0,
                "virtual_grid_model_reflow_finished"
            );
            if let Some(slot) = restored_slot {
                tracing::trace!(
                    target: "ui::grid_settings",
                    mode = ?self.mode(),
                    slot,
                    "virtual_grid_restore_scroll_scheduled"
                );
                self.restore_top_slot(slot);
            }
        }
        tracing::trace!(
            target: "ui::grid_settings",
            mode = ?self.mode(),
            "virtual_grid_visible_range_scheduled"
        );
        self.schedule_visible_range_after_layout();
    }

    /// GTK updates adjustments while it is allocating list items. Changing a
    /// GridView's column properties from that signal is re-entrant and can
    /// briefly produce negative child allocations. Coalesce the latest width
    /// and wait for a short quiet period before applying adaptive Year/Month
    /// column changes. Day keeps its configured column count while dragging.
    fn schedule_column_update(&self, width: i32) {
        if width <= 0 {
            return;
        }
        self.imp().pending_column_width.set(width);
        if self.imp().column_update_source.borrow().is_some() {
            return;
        }
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            let Some(grid) = weak.upgrade() else {
                return;
            };
            *grid.imp().column_update_source.borrow_mut() = None;
            grid.update_columns_for_width(grid.imp().pending_column_width.get());
        });
        *self.imp().column_update_source.borrow_mut() = Some(source);
    }

    fn top_slot_for_adjustment(&self) -> u32 {
        let adjustment = self.imp().scroller.get().vadjustment();
        self.viewport_metrics()
            .top_slot_for_scroll_offset(adjustment.value())
    }

    fn restore_top_slot(&self, slot: u32) {
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            let Some(grid) = weak.upgrade() else {
                return;
            };
            let adjustment = grid.imp().scroller.get().vadjustment();
            let metrics = grid.viewport_metrics();
            let columns = metrics.columns().max(1);
            let row = slot / columns;
            let desired = f64::from(row) * f64::from(metrics.row_extent());
            let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
            adjustment.set_value(desired.min(maximum));
        });
    }

    fn schedule_visible_range_after_layout(&self) {
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(grid) = weak.upgrade() {
                grid.schedule_visible_range();
            }
        });
    }

    fn schedule_visible_range(&self) {
        if !self.imp().active.get() || !self.imp().metadata_ready.get() {
            return;
        }
        let Some(visible) = self.visible_media_range() else {
            return;
        };
        let top_slot = self.top_slot_for_adjustment();
        let moving_forward = top_slot >= self.imp().last_top_slot.replace(top_slot);
        let desired = expanded_visible_range(visible, self.imp().live_total.get(), moving_forward);
        let disposition = self.imp().range.borrow_mut().request(desired);
        if let RequestDisposition::Started(request) = disposition {
            self.start_range_request(request);
        }
    }

    fn visible_media_range(&self) -> Option<MediaRange> {
        let layout = self.model().layout();
        if layout.media_count() == 0 {
            return None;
        }
        let adjustment = self.imp().scroller.get().vadjustment();
        let metrics = self.viewport_metrics();
        let columns = metrics.columns().max(1);
        let first_slot = self
            .top_slot_for_adjustment()
            .min(layout.slot_count().saturating_sub(1));
        let visible_rows = (adjustment.page_size() / f64::from(metrics.row_extent()))
            .ceil()
            .max(1.0) as u32;
        let last_slot = first_slot
            .saturating_add(visible_rows.saturating_add(1).saturating_mul(columns))
            .saturating_sub(1)
            .min(layout.slot_count().saturating_sub(1));
        let start = layout.anchor_media_offset_for_slot(first_slot)?;
        let end = layout
            .anchor_media_offset_for_slot(last_slot)?
            .saturating_add(1);
        Some(MediaRange::new(start.min(end), start.max(end)))
    }

    fn start_range_request(&self, request: range_cache::RangeRequest) {
        let pool = self.loader().pool().clone();
        let query = self.query();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let range = request.range;
            let result = gio::spawn_blocking(move || {
                MediaRepository::new(pool).items(query, range.start, range.len())
            })
            .await;
            let Some(grid) = weak.upgrade() else {
                return;
            };
            // Main-thread landing: finish + replace_ready_range + evict_outside
            // + redirect_prewarm + chained re-request. Excludes the DB query
            // (done above on a blocking worker). Debug-only span.
            let _landing = tracing::debug_span!(
                "vgrid:landing",
                range_start = range.start,
                range_len = range.len(),
            )
            .entered();
            let completion = grid.imp().range.borrow_mut().finish(request.generation);
            if completion.apply_result {
                match result {
                    Ok(Ok(items)) => {
                        // Kick off the EXIF-placeholder preload before `items`
                        // move into the model. It runs concurrently in its own
                        // future and never delays the landing's rebind below.
                        grid.preload_exif_placeholders(&items);
                        grid.model()
                            .replace_ready_range(range.start..range.end, items);
                        let keep = grid
                            .visible_media_range()
                            .map(|visible| {
                                expanded_visible_range(visible, grid.imp().live_total.get(), true)
                            })
                            .unwrap_or(range);
                        grid.model().evict_outside(keep.start..keep.end);
                        let mut coordinator = grid.imp().range.borrow_mut();
                        coordinator.mark_resident(range);
                        coordinator.retain_resident_within(keep);
                        grid.loader().redirect_prewarm_to_offset(keep.start);
                    }
                    Ok(Err(error)) => tracing::warn!(
                        target: crate::core::log_targets::BROWSING,
                        start = range.start,
                        end = range.end,
                        "gridview range query failed: {error}"
                    ),
                    Err(error) => tracing::warn!(
                        target: crate::core::log_targets::BROWSING,
                        start = range.start,
                        end = range.end,
                        "gridview range worker failed: {error:?}"
                    ),
                }
            }
            if let Some(next) = completion.next {
                grid.start_range_request(next);
            }
        });
    }

    /// Concurrently extract embedded-EXIF thumbnails for a freshly landed range
    /// and cache them in memory, so mem-miss binds can paint an instant low-res
    /// placeholder (Android-style) while the full thumbnail generates off-thread.
    ///
    /// Runs in its own main-loop future so it never delays the landing's
    /// structural rebind. Each extraction reads only the file head, work is
    /// parallelised across blocking-worker chunks, and items whose full
    /// thumbnail (or a prior EXIF entry) is already resident are skipped. When
    /// every chunk finishes, visible ready slots are refreshed so already-bound
    /// tiles pick up their EXIF placeholder on the next bind.
    fn preload_exif_placeholders(&self, items: &[MediaItem]) {
        let size = self.spec().thumbnail_size();
        let loader = self.loader();
        let targets: Vec<(String, std::time::SystemTime, std::path::PathBuf)> = items
            .iter()
            .filter_map(|item| {
                let mtime = thumbnail_request_mtime(item);
                let uri = self.thumbnail_uri_for(item);
                // Skip tiles that already have a full thumbnail or an EXIF
                // placeholder resident — no point re-reading the file head.
                if loader
                    .try_load_mem_cached(&uri, size, Some(mtime))
                    .is_some()
                    || loader
                        .try_load_exif_thumb_cached(&uri, Some(mtime))
                        .is_some()
                {
                    return None;
                }
                let path_str = uri.strip_prefix("file://").unwrap_or(&uri).to_string();
                Some((uri, mtime, std::path::PathBuf::from(path_str)))
            })
            .collect();
        if targets.is_empty() {
            return;
        }

        // Bound parallelism: ~8 chunks across the GIO blocking pool.
        let chunk_size = targets.len().div_ceil(8).max(1);
        let _span = tracing::debug_span!(
            "vgrid:exif_preload",
            targets = targets.len(),
            chunks = targets.len().div_ceil(chunk_size),
        );
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let mut handles = Vec::new();
            for chunk in targets.chunks(chunk_size) {
                let chunk: Vec<_> = chunk.to_vec();
                let loader = loader.clone();
                handles.push(gio::spawn_blocking(move || {
                    let _span =
                        tracing::debug_span!("vgrid:exif_preload_chunk", items = chunk.len())
                            .entered();
                    for (uri, mtime, path) in chunk {
                        if let Some(pb) = crate::core::metadata::extract_exif_thumbnail(&path) {
                            loader.insert_exif_thumb(
                                &uri,
                                Some(mtime),
                                crate::core::thumbnails::loaded_thumb_from_pixbuf(&pb),
                            );
                        }
                    }
                }));
            }
            for handle in handles {
                let _ = handle.await;
            }
            if let Some(grid) = weak.upgrade() {
                grid.fill_exif_placeholders();
            }
        });
    }

    /// Paint cached EXIF placeholders into still-loading tiles without rebinding.
    ///
    /// Called once the landing preload has populated the EXIF mem cache. Unlike
    /// `refresh_ready_slots` (which rebinds and would clear already-painted
    /// thumbnails), this walks the factory cells and only fills tiles still
    /// showing the shimmer. `defer_thumbnail_paint`'s placeholder guard then
    /// ensures a tile whose full thumbnail already arrived is left untouched.
    fn fill_exif_placeholders(&self) {
        let loader = self.loader();
        let model = self.model();
        let cells: Vec<FactoryCell> = self.imp().factory_cells.borrow().clone();
        for cell in cells {
            // Only tiles still waiting for their thumbnail should get a preview.
            if !cell.tile.has_css_class("thumb-loading") {
                continue;
            }
            let Some(binding) = cell.binding.borrow().clone() else {
                continue;
            };
            let Some(item) = model.ready_item_for_media_id(binding.media_id()) else {
                continue;
            };
            let mtime = thumbnail_request_mtime(&item);
            let thumbnail_uri = self.thumbnail_uri_for(&item);
            let Some(exif) = loader.try_load_exif_thumb_cached(&thumbnail_uri, Some(mtime)) else {
                continue;
            };
            factory::defer_thumbnail_paint(
                cell.tile.downgrade(),
                self.downgrade(),
                cell.binding.clone(),
                binding,
                exif,
                std::time::Instant::now(),
                true,
                true,
            );
        }
    }

    fn activate_slot(&self, position: u32) {
        let Some(GridSlotState::Ready { item, .. }) = self.model().slot_state(position) else {
            return;
        };
        let media_id = MediaId::from(item.id);
        if self.is_multi_select_mode() {
            self.toggle_selection(media_id);
        } else if let Some(callbacks) = self.imp().callbacks.get() {
            (callbacks.on_activate)(media_id);
        }
    }

    fn ensure_context_selection(&self, media_id: MediaId) -> Vec<MediaId> {
        let changed = {
            let mut selected = self.imp().selected.borrow_mut();
            if selected.contains(&media_id) {
                false
            } else {
                selected.clear();
                selected.insert(media_id);
                true
            }
        };
        if changed {
            self.sync_visible_selection();
            self.fire_selection_changed();
        }
        self.selected_ids_sorted()
    }

    fn toggle_selection(&self, media_id: MediaId) {
        let mut selected = self.imp().selected.borrow_mut();
        if !selected.insert(media_id) {
            selected.remove(&media_id);
        }
        drop(selected);
        self.sync_visible_selection();
        self.fire_selection_changed();
    }

    fn selected_ids_sorted(&self) -> Vec<MediaId> {
        let mut ids = self.selected_ids();
        ids.sort_unstable();
        ids
    }

    fn sync_visible_selection(&self) {
        self.model().refresh_ready_slots();
    }

    fn fire_selection_changed(&self) {
        if let Some(callback) = self.imp().on_selection_changed.get() {
            callback();
        }
    }
}

fn append_favorite_menu_items(
    items: &mut Vec<GlassMenuItem>,
    callbacks: &MediaGridCallbacks,
    favorite_state: FavoriteMenuState,
    ids: &[MediaId],
) {
    if favorite_state.can_favorite {
        let ids = ids.to_vec();
        let callback = callbacks.on_set_favorite.clone();
        items.push(GlassMenuItem::new(
            tr("photos.batch.favorite"),
            GlassMenuItemKind::Normal,
            move || callback(ids.clone(), true),
        ));
    }
    if favorite_state.can_unfavorite {
        let ids = ids.to_vec();
        let callback = callbacks.on_set_favorite.clone();
        items.push(GlassMenuItem::new(
            tr("photos.batch.unfavorite"),
            GlassMenuItemKind::Normal,
            move || callback(ids.clone(), false),
        ));
    }
}

fn media_items_from_list(list: &gio::ListStore) -> Vec<MediaItem> {
    (0..list.n_items())
        .filter_map(|index| crate::ui::media_list::media_item_at(list, index))
        .collect()
}

#[cfg(test)]
mod tests;
