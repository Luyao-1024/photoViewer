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

use crate::core::albums::{list_with_favorites, set_album_order, Album};
use crate::core::db::DbPool;
use crate::core::i18n::{tr, trf};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::media_grid::square_tile::SquareTile;

const ALBUM_CARD_PX: i32 = 270;
const ALBUM_TILE_SIZE: ThumbnailSize = ThumbnailSize::Large;

type AlbumOpenCallback = Rc<dyn Fn(Album)>;
type AlbumOrderChangedCallback = Rc<dyn Fn()>;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/album-browser-page.ui")]
    pub struct AlbumBrowserPage {
        pub albums: RefCell<Vec<Album>>,
        pub pool: RefCell<Option<DbPool>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub on_album_open: RefCell<Option<AlbumOpenCallback>>,
        pub on_order_changed: RefCell<Option<AlbumOrderChangedCallback>>,
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
        Self::with_order_changed(pool, loader, on_album_open, None)
    }

    /// Build an album browser page with an optional callback fired after drag
    /// sorting persists a new order. The main window uses this to keep the
    /// sidebar album rows in sync while the full album page is open.
    pub fn with_order_changed(
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
        on_album_open: Rc<dyn Fn(Album)>,
        on_order_changed: Option<Rc<dyn Fn()>>,
    ) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.set_title(&tr("page.albums.title"));
        *obj.imp().pool.borrow_mut() = Some(pool);
        *obj.imp().loader.borrow_mut() = Some(loader);
        *obj.imp().on_album_open.borrow_mut() = Some(on_album_open);
        *obj.imp().on_order_changed.borrow_mut() = on_order_changed;
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
            let folder_path = album.folder_path.to_string_lossy().into_owned();
            let card = build_album_card(album, loader.clone(), on_open.clone());
            self.attach_album_dnd(&card, folder_path);
            flow_box.append(&card);
        }
    }

    /// Number of albums currently rendered in this page.
    pub fn album_count(&self) -> usize {
        self.imp().albums.borrow().len()
    }

    /// Persist a drag-to-reorder from this page, then refresh this page's cards.
    pub fn reorder_album(&self, source_path: &str, target_path: &str, drop_after: bool) {
        if source_path == target_path {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };

        let mut order: Vec<String> = self
            .imp()
            .albums
            .borrow()
            .iter()
            .map(|album| album.folder_path.to_string_lossy().into_owned())
            .filter(|path| path != source_path)
            .collect();

        let insert_at = match order.iter().position(|path| path == target_path) {
            Some(idx) => {
                if drop_after {
                    (idx + 1).min(order.len())
                } else {
                    idx
                }
            }
            None => order.len(),
        };
        order.insert(insert_at, source_path.to_string());

        if let Err(err) = set_album_order(&pool, &order) {
            tracing::warn!("failed to persist album browser order: {err}");
            return;
        }

        if let Some(callback) = self.imp().on_order_changed.borrow().as_ref() {
            callback();
        }
        self.refresh();
    }

    /// Current rendered album order.
    pub fn album_folder_paths(&self) -> Vec<String> {
        self.imp()
            .albums
            .borrow()
            .iter()
            .map(|album| album.folder_path.to_string_lossy().into_owned())
            .collect()
    }

    /// Wire drag sorting onto one album card. The string payload is the source
    /// album's `folder_path`; dropping onto another card inserts before or after
    /// that target based on the vertical half under the pointer.
    fn attach_album_dnd(&self, card: &gtk::Widget, folder_path: String) {
        let drag = gtk::DragSource::new();
        drag.set_actions(gtk::gdk::DragAction::MOVE);
        let value = glib::Value::from(folder_path.as_str());
        drag.set_content(Some(&gtk::gdk::ContentProvider::for_value(&value)));

        let drag_card = card.downgrade();
        drag.connect_drag_begin(move |_, _| {
            if let Some(card) = drag_card.upgrade() {
                card.add_css_class("album-browser-card-dragging");
            }
        });
        let drag_card = card.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(card) = drag_card.upgrade() {
                card.remove_css_class("album-browser-card-dragging");
            }
        });
        card.add_controller(drag);

        let drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let motion_card = card.downgrade();
        drop.connect_motion(move |_target, _x, y| {
            if let Some(card) = motion_card.upgrade() {
                let half = card.height().max(1) as f64 / 2.0;
                card.remove_css_class("album-browser-card-drop-before");
                card.remove_css_class("album-browser-card-drop-after");
                card.add_css_class(if y > half {
                    "album-browser-card-drop-after"
                } else {
                    "album-browser-card-drop-before"
                });
            }
            gtk::gdk::DragAction::MOVE
        });
        let leave_card = card.downgrade();
        drop.connect_leave(move |_target| {
            if let Some(card) = leave_card.upgrade() {
                card.remove_css_class("album-browser-card-drop-before");
                card.remove_css_class("album-browser-card-drop-after");
            }
        });

        let page = self.downgrade();
        let drop_card = card.downgrade();
        let target_path = folder_path;
        drop.connect_drop(move |_target, value, _x, y| {
            let Some(page) = page.upgrade() else {
                return false;
            };
            let Some(card) = drop_card.upgrade() else {
                return false;
            };
            card.remove_css_class("album-browser-card-drop-before");
            card.remove_css_class("album-browser-card-drop-after");
            let Ok(source_path) = value.get::<String>() else {
                return false;
            };
            let half = card.height().max(1) as f64 / 2.0;
            page.reorder_album(&source_path, &target_path, y > half);
            true
        });
        card.add_controller(drop);
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
            loader.request(
                cover_uri.clone(),
                ALBUM_TILE_SIZE,
                Some(mtime),
                tx,
                crate::core::thumbnails::TIER_NORMAL,
            );
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
