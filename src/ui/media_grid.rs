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

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::core::i18n::tr;
use crate::core::media::MediaItem;
use crate::core::section_model::{group_items, GroupBy};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};

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
        /// Whether batch mode is explicitly enabled.
        pub is_multi_select_mode: Cell<bool>,
        /// Callback fired whenever `selected` changes. Registered by the host
        /// (`PhotosPage`) so it can show/hide the toolbar "Add to Album"
        /// button and re-render selected state across all three sub-grids.
        pub on_selection_changed: std::cell::OnceCell<Rc<dyn Fn()>>,
    }

    impl Default for MediaGrid {
        fn default() -> Self {
            Self {
                content: TemplateChild::default(),
                scroller: TemplateChild::default(),
                mode: Cell::default(),
                enable_context_menu: Cell::new(false),
                loader: std::cell::OnceCell::new(),
                on_activate: std::cell::OnceCell::new(),
                on_background_changed: std::cell::OnceCell::new(),
                on_add_to_album: std::cell::OnceCell::new(),
                on_move_to_trash: std::cell::OnceCell::new(),
                on_set_favorite: std::cell::OnceCell::new(),
                on_query_favorite_state: std::cell::OnceCell::new(),
                displayed_items: RefCell::new(Vec::new()),
                selected: RefCell::default(),
                is_multi_select_mode: Cell::new(false),
                on_selection_changed: std::cell::OnceCell::new(),
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

    impl ObjectImpl for MediaGrid {
        fn constructed(&self) {
            self.parent_constructed();
            // `content-safe-bottom` (defined in `grid_css.rs`) reserves 128px
            // at the bottom of the scrolled content so the floating mode
            // selector overlay never covers the last row of thumbnails.
            // It must be applied here, on the inner ScrolledWindow that
            // actually scrolls — padding on outer wrappers (e.g. ViewStack)
            // does not propagate into the scrolled content. The
            // `.content-safe-bottom` was previously also applied to the outer
            // ViewStack in photos-page.blp as a defensive measure; it was
            // removed in Fix #5 because that placement had no effect.
            self.scroller.add_css_class("content-safe-bottom");
        }
    }
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
}

fn spec_for_mode(mode: GroupBy) -> ViewSpec {
    // On-screen tile size per view (CSS px). Year shows the most photos so it
    // gets the smallest tiles; Day shows the fewest so it gets the largest.
    // Thumbnail buckets are picked ~2x the display size for retina crispness.
    match mode {
        GroupBy::Year => ViewSpec {
            pixel_size: 90,
            thumb_size: ThumbnailSize::Small,
        },
        GroupBy::Month => ViewSpec {
            pixel_size: 180,
            thumb_size: ThumbnailSize::Medium,
        },
        GroupBy::Day => ViewSpec {
            pixel_size: 270,
            thumb_size: ThumbnailSize::Large,
        },
    }
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
        let obj: Self = gtk::glib::Object::builder().build();
        obj.imp().mode.set(mode);
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

        crate::ui::grid_css::install();
        obj.rebuild(media_list.clone(), mode);
        let weak = obj.downgrade();
        media_list.connect_items_changed(move |list, position, removed, added| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            tracing::info!(
                "VIEWER_DEBUG grid model_changed mode={:?} position={} removed={} added={} list_len={}",
                this.mode(),
                position,
                removed,
                added,
                list.n_items()
            );
            this.clear_selection();
            this.rebuild(list.clone(), this.mode());
        });
        obj
    }

    pub fn set_mode(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        self.imp().mode.set(mode);
        self.rebuild(media_list, mode);
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

        let content = self.imp().content.get();
        // Clear any previously built sections.
        while let Some(child) = content.first_child() {
            content.remove(&child);
        }
        self.imp().displayed_items.borrow_mut().clear();

        // Extract MediaItems + a uri→global-index lookup from the store.
        let items = extract_items(&media_list);
        let uri_to_index = uri_index_map(&media_list);

        // Group by year/month/day, then emit header + FlowBox per section.
        let sections = group_items(&items, mode);
        let mut section_count = 0u32;
        let mut photo_count = 0u32;
        let mut displayed_items = Vec::new();
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
                activation_items.push((item.id, item.display_name().to_string(), item.uri.clone()));
                let on_bg = self
                    .imp()
                    .on_background_changed
                    .get()
                    .expect("MediaGrid::rebuild called before new()")
                    .clone();
                let picture = build_photo_picture(spec, item.clone(), loader.clone(), on_bg);
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
                tracing::info!(
                    "VIEWER_DEBUG grid activate mode={:?} section={} child_index={} global_index={} item_id={} item_name={} item_uri={} multi_select={}",
                    this.mode(),
                    section_label_for_activation,
                    idx,
                    gi,
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
                    tracing::info!(
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
                        delete_btn.connect_clicked(move |_| {
                            on_move_to_trash_ctx(indices_for_trash.clone());
                            popover_trash.popdown();
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
        *self.imp().displayed_items.borrow_mut() = displayed_items;

        tracing::debug!(
            "MediaGrid::rebuild mode={:?} sections={} photos={} spec.pixel_size={}",
            mode,
            section_count,
            photo_count,
            spec.pixel_size
        );
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
/// `pub` so other pages (`AlbumsPage`) can reuse it for cover thumbnails
/// that must also be 1:1 squares — see `CLAUDE.md` "Day-view grid sizing
/// gotcha" for the underlying GTK4 sizing pitfall.
pub mod square_tile {
    use super::*;
    use std::cell::{Cell, RefCell};

    mod imp {
        use super::*;

        pub struct SquareTile {
            pub picture: RefCell<Option<gtk::Picture>>,
            pub target: Cell<i32>,
            pub background_is_light: Cell<Option<bool>>,
        }

        impl Default for SquareTile {
            fn default() -> Self {
                Self {
                    picture: RefCell::new(None),
                    target: Cell::new(90),
                    background_is_light: Cell::new(None),
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
                picture.set_parent(&*obj);
                *self.picture.borrow_mut() = Some(picture);
            }

            fn dispose(&self) {
                if let Some(p) = self.picture.borrow_mut().take() {
                    p.unparent();
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
        }

        pub fn set_background_is_light(&self, is_light: bool) {
            self.imp().background_is_light.set(Some(is_light));
        }

        pub fn background_is_light(&self) -> Option<bool> {
            self.imp().background_is_light.get()
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
    loader: Arc<ThumbnailLoader>,
    on_background_changed: Rc<dyn Fn()>,
) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(spec.pixel_size);

    let item_name = item.display_name().to_string();
    let item_uri = item.uri.clone();
    let size = spec.thumb_size;
    let target_px = spec.pixel_size;

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

            tracing::info!(
                "VIEWER_DEBUG thumb request item_name={} uri={} size={:?} target_px={}",
                item_name,
                item_uri,
                size,
                target_px
            );

            let (tx, rx) = tokio::sync::oneshot::channel();
            loader.request(item_uri.clone(), size, tx);
            let tile_weak = tile_weak.clone();
            let on_background_changed = on_background_changed.clone();
            let item_name = item_name.clone();
            let item_uri = item_uri.clone();
            gtk::glib::spawn_future_local(async move {
                match rx.await {
                    Ok(texture) => {
                        tracing::info!(
                            "VIEWER_DEBUG thumb loaded item_name={} uri={} texture={}x{}",
                            item_name,
                            item_uri,
                            texture.width(),
                            texture.height()
                        );
                        if let Some(t) = tile_weak.upgrade() {
                            if let Some(is_light) = texture_is_light(&texture) {
                                t.set_background_is_light(is_light);
                                on_background_changed();
                            }
                            t.set_paintable(Some(&texture));
                        }
                    }
                    Err(_) => tracing::warn!(
                        "VIEWER_DEBUG thumb dropped item_name={} uri={}",
                        item_name,
                        item_uri
                    ),
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

fn texture_is_light(texture: &gtk::gdk::Texture) -> Option<bool> {
    let width = texture.width();
    let height = texture.height();
    if width <= 0 || height <= 0 {
        return None;
    }
    let stride = width as usize * 4;
    let mut data = vec![0u8; stride * height as usize];
    texture.download(&mut data, stride);

    let step_x = (width / 24).max(1) as usize;
    let step_y = (height / 24).max(1) as usize;
    let mut total = 0.0f64;
    let mut count = 0.0f64;
    for y in (0..height as usize).step_by(step_y) {
        for x in (0..width as usize).step_by(step_x) {
            let i = y * stride + x * 4;
            let c0 = data[i] as f64;
            let c1 = data[i + 1] as f64;
            let c2 = data[i + 2] as f64;
            total += (c0 + c1 + c2) / 3.0;
            count += 1.0;
        }
    }
    Some(total / count >= 160.0)
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
            width: Some(100),
            height: Some(100),
            taken_at: Some(dt),
            file_mtime: dt,
            file_size: 100,
            blake3_hash: format!("hash-{id}"),
            trashed_at: None,
        }
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
}
