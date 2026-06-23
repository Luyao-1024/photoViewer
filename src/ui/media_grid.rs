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

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::core::media::MediaItem;
use crate::core::section_model::{group_items, GroupBy};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/media-grid.ui")]
    pub struct MediaGrid {
        #[template_child]
        pub content: TemplateChild<gtk::Box>,
        #[template_child]
        pub scroller: TemplateChild<gtk::ScrolledWindow>,
        pub mode: Cell<GroupBy>,
        pub loader: std::cell::OnceCell<Arc<ThumbnailLoader>>,
        pub on_activate: std::cell::OnceCell<Rc<dyn Fn(u32)>>,
        pub on_background_changed: std::cell::OnceCell<Rc<dyn Fn()>>,
        /// Global indices (into the shared `ListStore`) currently in the
        /// "selected" set. The set is global — it spans year/month/day
        /// sections, because `PhotosPage` is the only host and it shares one
        /// `ListStore` across the three sub-grids.
        pub selected: RefCell<HashSet<u32>>,
        /// Latest GDK modifier state captured from the `EventControllerKey`
        /// attached to `content`. Read by `child_activated` to decide between
        /// "open viewer" (no modifier) and "toggle selection" (Shift/Ctrl).
        pub modifier_state: Cell<gdk::ModifierType>,
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
                loader: std::cell::OnceCell::new(),
                on_activate: std::cell::OnceCell::new(),
                on_background_changed: std::cell::OnceCell::new(),
                selected: RefCell::default(),
                modifier_state: Cell::new(gdk::ModifierType::empty()),
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
        obj.setup_modifier_tracker();

        crate::ui::grid_css::install();
        obj.rebuild(media_list, mode);
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

    /// Clear the selection (both in the `selected` set AND on every visible
    /// `FlowBox`). Fires the `selection-changed` callback if the set was
    /// non-empty before.
    pub fn clear_selection(&self) {
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

    /// Attach an `EventControllerKey` to `content` that keeps
    /// `modifier_state` up to date. The mask is read by `child_activated` to
    /// decide between "open viewer" and "toggle selection".
    ///
    /// gtk4-rs 0.8 only exposes the `modifiers` signal (fired whenever the
    /// modifier mask changes, e.g. on Shift / Ctrl press/release) — there is
    /// no `key-pressed` / `key-released` callback. That signal is sufficient
    /// for our needs: we only care about the mask at the moment a click is
    /// processed.
    fn setup_modifier_tracker(&self) {
        let key_ctrl = gtk::EventControllerKey::new();
        let imp_weak = self.downgrade();
        key_ctrl.connect_modifiers(move |_, state| {
            if let Some(obj) = imp_weak.upgrade() {
                obj.imp().modifier_state.set(state);
            }
            glib::Propagation::Proceed
        });
        self.imp().content.get().add_controller(key_ctrl);
    }

    /// Decide whether `state` means "user is multi-selecting" (Shift or Ctrl).
    fn is_multi_select_modifier(state: gdk::ModifierType) -> bool {
        state.contains(gdk::ModifierType::SHIFT_MASK)
            || state.contains(gdk::ModifierType::CONTROL_MASK)
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

        let spec = spec_for_mode(mode);

        let content = self.imp().content.get();
        // Clear any previously built sections.
        while let Some(child) = content.first_child() {
            content.remove(&child);
        }

        // Extract MediaItems + a uri→global-index lookup from the store.
        let items = extract_items(&media_list);
        let uri_to_index = uri_index_map(&media_list);

        // Group by year/month/day, then emit header + FlowBox per section.
        let sections = group_items(&items, mode);
        let mut section_count = 0u32;
        let mut photo_count = 0u32;
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
                .column_spacing(2)
                .row_spacing(2)
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
            for item in &section.items {
                let gi = uri_to_index.get(&item.uri).copied().unwrap_or(u32::MAX);
                global_indices.push(gi);
                let on_bg = self
                    .imp()
                    .on_background_changed
                    .get()
                    .expect("MediaGrid::rebuild called before new()")
                    .clone();
                let picture = build_photo_picture(spec, item.clone(), loader.clone(), on_bg);
                flow.append(&picture);
                photo_count += 1;
            }

            // Activation: FlowBox child-activated → look up global index.
            // Behaviour depends on the modifier state captured at click time:
            // - Shift / Ctrl held → toggle membership in `selected` (no viewer)
            // - otherwise          → forward to the host's on_activate (open viewer)
            let on_act = on_activate.clone();
            let weak = self.downgrade();
            flow.connect_child_activated(move |flow, child| {
                let idx = child.index();
                if idx < 0 {
                    return;
                }
                let Some(&gi) = global_indices.get(idx as usize) else {
                    return;
                };
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let state = this.imp().modifier_state.get();
                if Self::is_multi_select_modifier(state) {
                    // `flow` comes from the signal arg, no extra upgrade needed.
                    this.toggle_selection(gi, child, flow);
                } else {
                    on_act(gi);
                }
            });

            content.append(&flow);
            section_count += 1;
        }

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

    // Async thumbnail load.
    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request(item.uri.clone(), spec.thumb_size, tx);
    let tile_weak = tile.downgrade();
    gtk::glib::spawn_future_local(async move {
        if let Ok(texture) = rx.await {
            if let Some(t) = tile_weak.upgrade() {
                if let Some(is_light) = texture_is_light(&texture) {
                    t.set_background_is_light(is_light);
                    on_background_changed();
                }
                t.set_paintable(Some(&texture));
            }
        }
    });
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
