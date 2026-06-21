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
//! - Day   → 360×360 px (thumbnail bucket Large / 1024)
//!
//! Each tile is a `SquareTile` (see below) — a `GtkWidget` subclass wrapping
//! a `GtkPicture` (`content-fit: cover`) that overrides `measure` to report a
//! fixed square `target × target`. `GtkPicture`'s own natural size is the
//! image's intrinsic size (which would make cells non-square), and `GtkPicture`
//! isn't subclassable in gtk4-rs 0.8, so we wrap it. The `SquareTile` must NOT
//! set a layout manager — GTK4 would otherwise measure via the layout manager
//! and bypass the `measure` override.
//!
//! ## Gap & selection
//!
//! The FlowBox `column-spacing` / `row-spacing` (2 px) is the thin separator
//! between tiles. Selection uses the FlowBox's `selection-mode = single` +
//! `activate-on-single-click`; the selected child is tinted and its tile gets
//! an accent outline (see `GRID_CSS`) — i.e. the separator doubles as the
//! selection hint.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::core::media::MediaItem;
use crate::core::section_model::{group_items, GroupBy};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/media-grid.ui")]
    pub struct MediaGrid {
        #[template_child]
        pub content: TemplateChild<gtk::Box>,
        pub mode: Cell<GroupBy>,
        pub loader: std::cell::OnceCell<Arc<ThumbnailLoader>>,
        pub on_activate: std::cell::OnceCell<Rc<dyn Fn(u32)>>,
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
            pixel_size: 360,
            thumb_size: ThumbnailSize::Large,
        },
    }
}

/// CSS for the thumbnail FlowBoxes: remove the default FlowBoxChild padding so
/// tiles touch (the FlowBox `column/row spacing` is the thin separator), and
/// highlight the selected tile with an accent tint + outline.
const GRID_CSS: &str = "
flowbox.thumb-grid > flowboxchild { padding: 0; }
flowbox.thumb-grid > flowboxchild:selected {
  background-color: alpha(@accent_color, 0.3);
}
flowbox.thumb-grid > flowboxchild:selected .tile {
  outline: 2px solid @accent_color;
  outline-offset: -1px;
}
";

static CSS_INSTALLED: std::sync::Once = std::sync::Once::new();

fn install_grid_css() {
    CSS_INSTALLED.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(GRID_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

impl MediaGrid {
    /// Build a MediaGrid that immediately renders `(media_list, mode)`.
    /// `on_activate` fires with the photo's global index in `media_list`
    /// when the user activates a photo (click or Enter).
    pub fn new(
        media_list: gtk::gio::ListStore,
        mode: GroupBy,
        loader: Arc<ThumbnailLoader>,
        on_activate: Rc<dyn Fn(u32)>,
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

        install_grid_css();
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
            let flow = gtk::FlowBox::builder()
                .orientation(gtk::Orientation::Horizontal)
                .homogeneous(true)
                .column_spacing(2)
                .row_spacing(2)
                .max_children_per_line(100)
                .selection_mode(gtk::SelectionMode::Single)
                .build();
            flow.set_activate_on_single_click(true);
            flow.add_css_class("thumb-grid");

            // Build tiles + remember each child's global index for activation.
            let mut global_indices: Vec<u32> = Vec::with_capacity(section.items.len());
            for item in &section.items {
                let gi = uri_to_index
                    .get(&item.uri)
                    .copied()
                    .unwrap_or(u32::MAX);
                global_indices.push(gi);
                let picture = build_photo_picture(spec, item.clone(), loader.clone());
                flow.append(&picture);
                photo_count += 1;
            }

            // Activation: FlowBox child-activated → look up global index.
            let on_act = on_activate.clone();
            flow.connect_child_activated(move |_flow, child| {
                let idx = child.index();
                if idx >= 0 {
                    if let Some(&gi) = global_indices.get(idx as usize) {
                        on_act(gi);
                    }
                }
            });

            content.append(&flow);
            section_count += 1;
        }

        eprintln!(
            "[MediaGrid::rebuild] mode={:?} sections={} photos={} spec.pixel_size={}",
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
mod square_tile {
    use super::*;
    use std::cell::{Cell, RefCell};

    mod imp {
        use super::*;

        pub struct SquareTile {
            pub picture: RefCell<Option<gtk::Picture>>,
            pub target: Cell<i32>,
        }

        impl Default for SquareTile {
            fn default() -> Self {
                Self {
                    picture: RefCell::new(None),
                    target: Cell::new(90),
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

    impl SquareTile {
        pub fn new() -> Self {
            gtk::glib::Object::builder().build()
        }

        pub fn set_target(&self, target: i32) {
            self.imp().target.set(target);
            self.queue_resize();
        }

        pub fn set_paintable<P: IsA<gtk::gdk::Paintable>>(&self, paintable: Option<&P>) {
            if let Some(p) = self.imp().picture.borrow().as_ref() {
                p.set_paintable(paintable);
            }
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
) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(spec.pixel_size);
    tile.add_css_class("tile");

    // Async thumbnail load.
    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request(item.uri.clone(), spec.thumb_size, tx);
    let tile_weak = tile.downgrade();
    gtk::glib::spawn_future_local(async move {
        if let Ok(texture) = rx.await {
            if let Some(t) = tile_weak.upgrade() {
                t.set_paintable(Some(&texture));
            }
        }
    });
    tile
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
