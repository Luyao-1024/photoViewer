//! AlbumBrowserPage — browse all albums in a full-screen grid.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use gdk_pixbuf::Pixbuf;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::NavigationPageExt;
use libadwaita::subclass::prelude::*;

use crate::core::albums::{list_with_favorites, Album};
use crate::core::db::DbPool;
use crate::core::i18n::{tr, trf};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::media_grid::square_tile::SquareTile;

const ALBUM_CARD_PX: i32 = 270;
const ALBUM_TILE_SIZE: ThumbnailSize = ThumbnailSize::Large;

type AlbumOpenCallback = Rc<dyn Fn(Album)>;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/album-browser-page.ui")]
    pub struct AlbumBrowserPage {
        pub albums: RefCell<Vec<Album>>,
        pub pool: RefCell<Option<DbPool>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub on_album_open: RefCell<Option<AlbumOpenCallback>>,
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for AlbumBrowserPage {
        const NAME: &'static str = "AlbumBrowserPage";
        type Type = super::AlbumBrowserPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumBrowserPage {}
    impl WidgetImpl for AlbumBrowserPage {}
    impl NavigationPageImpl for AlbumBrowserPage {}
}

gtk::glib::wrapper! {
    pub struct AlbumBrowserPage(ObjectSubclass<imp::AlbumBrowserPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl AlbumBrowserPage {
    /// Build an album browser page that lists all albums in one `GtkFlowBox`.
    ///
    /// The page uses `SquareTile` so each cover is visually aligned with
    /// album/detail pages.
    pub fn new(
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
        on_album_open: Rc<dyn Fn(Album)>,
    ) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.set_title(&tr("page.albums.title"));
        *obj.imp().pool.borrow_mut() = Some(pool);
        *obj.imp().loader.borrow_mut() = Some(loader);
        *obj.imp().on_album_open.borrow_mut() = Some(on_album_open);
        obj.refresh();
        obj
    }

    /// Re-read albums from DB and render all rows.
    pub fn refresh(&self) {
        let pool = match self.imp().pool.borrow().as_ref() {
            Some(pool) => pool.clone(),
            None => return,
        };
        let loader = match self.imp().loader.borrow().as_ref() {
            Some(loader) => loader.clone(),
            None => return,
        };
        let on_open = match self.imp().on_album_open.borrow().as_ref() {
            Some(cb) => cb.clone(),
            None => return,
        };

        let albums = match list_with_favorites(&pool) {
            Ok(list) => list,
            Err(err) => {
                tracing::warn!("AlbumBrowserPage::refresh failed to fetch albums: {err}");
                Vec::new()
            }
        };

        let flow_box = self.imp().flow_box.get();
        while let Some(child) = flow_box.first_child() {
            flow_box.remove(&child);
        }
        *self.imp().albums.borrow_mut() = albums.clone();
        for album in albums {
            flow_box.append(&build_album_card(album, loader.clone(), on_open.clone()));
        }
    }

    /// Number of albums currently rendered in this page.
    pub fn album_count(&self) -> usize {
        self.imp().albums.borrow().len()
    }
}

fn build_album_card(
    album: Album,
    loader: Arc<ThumbnailLoader>,
    on_album_open: Rc<dyn Fn(Album)>,
) -> gtk::Widget {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    container.add_css_class("album-browser-card");

    let cover = build_album_cover(&album, loader);
    cover.set_margin_top(4);
    cover.set_margin_end(4);
    cover.set_margin_bottom(4);
    cover.set_margin_start(4);
    container.append(&cover);

    let title = gtk::Label::builder()
        .label(album.display_name())
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(18)
        .build();
    title.set_margin_start(8);
    title.set_margin_end(8);
    title.set_margin_bottom(4);
    title.add_css_class("album-browser-title");
    container.append(&title);

    let count = gtk::Label::builder()
        .label(trf(
            "album.count",
            &[("count", &album.photo_count.to_string())],
        ))
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .margin_start(8)
        .margin_end(8)
        .build();
    count.add_css_class("album-browser-count");
    container.append(&count);

    let album_for_open = album;
    let click = gtk::GestureClick::new();
    click.connect_released(move |_, _, _, _| {
        on_album_open(album_for_open.clone());
    });
    container.add_controller(click);

    container.upcast()
}

fn build_album_cover(album: &Album, loader: Arc<ThumbnailLoader>) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(ALBUM_CARD_PX);
    tile.set_halign(gtk::Align::Fill);
    tile.set_hexpand(true);
    tile.add_css_class("album-browser-cover");
    tile.add_css_class("thumb-loading");
    tile.set_paintable(Some(&cover_placeholder_texture()));

    let Some(cover_uri) = album.cover_uri.as_deref() else {
        return tile;
    };

    let requested = Cell::new(false);
    let tile_weak = tile.downgrade();
    let mtime = SystemTime::from(album.last_modified);
    let cover_uri = cover_uri.to_string();
    let placeholder = cover_placeholder_texture();
    let request_once: Rc<dyn Fn()> = Rc::new({
        let loader = loader.clone();
        let cover_uri = cover_uri.clone();
        move || {
            if requested.get() {
                return;
            }
            requested.set(true);

            let (tx, rx) = tokio::sync::oneshot::channel();
            loader.request(cover_uri.clone(), ALBUM_TILE_SIZE, Some(mtime), tx);
            let tile_weak = tile_weak.clone();
            let placeholder = placeholder.clone();
            glib::spawn_future_local(async move {
                match rx.await {
                    Ok(loaded) => {
                        if let Some(tile) = tile_weak.upgrade() {
                            if let Some(is_light) = loaded.is_light {
                                tile.set_background_is_light(is_light);
                            }
                            tile.set_paintable(Some(&loaded.texture));
                        }
                    }
                    Err(_) => {
                        if let Some(tile) = tile_weak.upgrade() {
                            tile.set_paintable(Some(&placeholder));
                        }
                    }
                }
            });
        }
    });

    {
        let request_once = Rc::clone(&request_once);
        tile.connect_map(move |_| request_once());
    }
    if tile.is_mapped() {
        request_once();
    }
    tile
}

fn cover_placeholder_texture() -> gtk::gdk::Texture {
    let pixbuf = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 2, 2)
        .expect("allocate 2x2 placeholder pixbuf");
    pixbuf.fill(0xC8C8C8FF);
    gtk::gdk::Texture::for_pixbuf(&pixbuf)
}
