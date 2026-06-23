//! AlbumsPage — grid of folder-as-album tiles with cover thumbnails.
//!
//! Each tile shows:
//! - A 240x240 cover thumbnail (loaded async via the shared `ThumbnailLoader`,
//!   sized to the `Medium` bucket — 512px).
//! - The album's display name (basename of `folder_path`, see
//!   `Album::display_name`).
//! - The photo count.
//!
//! Tiles are plain `GtkFlowBoxChild` widgets constructed by `build_album_tile`
//! and appended directly to the page's `GtkFlowBox`.
use std::cell::RefCell;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::subclass::prelude::*;

use crate::core::albums::Album;
use crate::core::db::DbPool;
use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::album_detail_page::AlbumDetailPage;
use crate::ui::empty_states;
use crate::ui::media_grid::square_tile::SquareTile;

/// Cover thumbnail size, in CSS pixels. Matches the **Day** view of
/// `MediaGrid` (`spec_for_mode(Day) = 270×270`) so a 1:1 square album cover
/// is visually the same size as a day-mode photo tile — see `CLAUDE.md`'s
/// "Day-view grid sizing gotcha" for why we have to wrap `GtkPicture`.
const ALBUM_COVER_PX: i32 = 270;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/albums-page.ui")]
    pub struct AlbumsPage {
        pub albums: RefCell<Vec<Album>>,
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub pool: RefCell<Option<DbPool>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub scrolled: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for AlbumsPage {
        const NAME: &'static str = "AlbumsPage";
        type Type = super::AlbumsPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumsPage {}
    impl WidgetImpl for AlbumsPage {}
    impl NavigationPageImpl for AlbumsPage {}
}

gtk::glib::wrapper! {
    pub struct AlbumsPage(ObjectSubclass<imp::AlbumsPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl AlbumsPage {
    /// Build an AlbumsPage and populate the flow box with one tile per album.
    /// Cover thumbnails are requested asynchronously via the shared `loader`;
    /// the page returns immediately and tiles fill in as textures arrive.
    ///
    /// If `albums` is empty, the scrolled window's child is swapped for an
    /// `AdwStatusPage` describing how to add folders.
    pub fn new(albums: Vec<Album>, loader: Arc<ThumbnailLoader>) -> Self {
        // Install the grid CSS (idempotent) so the `album-grid` class on the
        // flow box gets its outline-based `:selected` style. The MediaGrid
        // already calls this on construction, but AlbumsPage never goes
        // through MediaGrid, so we have to do it here too.
        crate::ui::grid_css::install();

        let obj: Self = glib::Object::builder().build();
        *obj.imp().albums.borrow_mut() = albums.clone();
        *obj.imp().loader.borrow_mut() = Some(loader.clone());
        let flow = obj.imp().flow_box.get();

        if albums.is_empty() {
            // Swap the scrolled window's child to a centered status page.
            let empty = empty_states::no_albums();
            empty.set_hexpand(true);
            empty.set_vexpand(true);
            obj.imp().scrolled.get().set_child(Some(&empty));
        } else {
            for album in albums {
                let tile = build_album_tile(&album, loader.clone());
                flow.append(&tile);
            }

            let weak = obj.downgrade();
            flow.connect_child_activated(move |_, child| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                this.open_album_at(child.index());
            });
        }

        obj
    }

    /// Inject the navigation target and the shared media model used to build
    /// album detail pages. The current model is filtered per album when a tile
    /// is activated.
    pub fn set_nav_target(
        &self,
        nav: &adw::NavigationView,
        media_list: gtk::gio::ListStore,
        pool: DbPool,
    ) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
        *self.imp().media_list.borrow_mut() = Some(media_list);
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    fn open_album_at(&self, index: i32) {
        if index < 0 {
            return;
        }
        let Some(album) = self.imp().albums.borrow().get(index as usize).cloned() else {
            return;
        };
        let Some(nav) = self.imp().nav_view.borrow().clone() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().clone() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };

        let Some(master_media_list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for idx in 0..master_media_list.n_items() {
            let Some(obj) = master_media_list.item(idx) else {
                continue;
            };
            let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let item = (*boxed.borrow::<MediaItem>()).clone();
            if item.folder_path == album.folder_path {
                media_list.append(&glib::BoxedAnyObject::new(item));
            }
        }

        let page = AlbumDetailPage::new(album, media_list, master_media_list, pool, loader);
        page.set_nav_target(&nav);
        nav.push(&page);
    }
}

/// Build a single album tile (a `GtkFlowBoxChild` containing a vertical box
/// with picture + name + count). The cover loads asynchronously through
/// `loader`; while the request is in flight the picture shows a grey
/// placeholder (matching `PhotoTile::set_placeholder`).
///
/// The cover is a `SquareTile` (`ALBUM_COVER_PX × ALBUM_COVER_PX`) so it is
/// always 1:1 square, regardless of the source image's aspect ratio. A bare
/// `GtkPicture` with `width_request` / `height_request` won't work because
/// its natural size is the image's intrinsic size, so the parent box ends
/// up sizing the picture to that ratio and the cover comes out rectangular
/// (especially for portrait screenshots — see the 2026-06-23 screenshot bug).
fn build_album_tile(album: &Album, loader: Arc<ThumbnailLoader>) -> gtk::FlowBoxChild {
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(6)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    // Square cover — overrides `measure` to report a fixed square size
    // regardless of the underlying picture's aspect ratio.
    let cover = SquareTile::new();
    cover.set_target(ALBUM_COVER_PX);
    // `album-cover` CSS class lets the grid_css target this widget
    // specifically (for the square selection outline) rather than the
    // whole FlowBoxChild row (which also contains the name + count labels
    // and would paint a rectangular outline around them).
    cover.add_css_class("album-cover");

    // Grey placeholder so empty tiles don't briefly render nothing underneath.
    let css = gtk::CssProvider::new();
    css.load_from_data("picture { background-color: #d0d0d0; }");
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    cover.set_paintable(None::<&gtk::gdk::Paintable>);

    if let Some(uri) = &album.cover_uri {
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(uri.clone(), ThumbnailSize::Medium, tx);
        let cover_weak = cover.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(texture) = rx.await {
                if let Some(c) = cover_weak.upgrade() {
                    c.set_paintable(Some(&texture));
                }
            }
        });
    }

    let name_label = gtk::Label::builder()
        .label(album.display_name())
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(24)
        .build();

    let count_label = gtk::Label::builder()
        .label(format!("{} photos", album.photo_count))
        .halign(gtk::Align::Start)
        .opacity(0.7)
        .build();

    box_.append(&cover);
    box_.append(&name_label);
    box_.append(&count_label);

    let row = gtk::FlowBoxChild::new();
    row.set_child(Some(&box_));
    row
}

impl Default for AlbumsPage {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}
