//! Photo tile widget — a `GtkWidget` holding one `GtkPicture`, used by the
//! FlowBox-based pages (album detail, trash). `MediaGrid` does NOT use this;
//! its GridView factory builds its own `AspectFrame` + `GtkPicture` cells
//! directly (see `media_grid.rs`).
//!
//! KNOWN ISSUE: `PhotoTile::new` / `set_item` set `can_shrink = false` on the
//! picture — the exact pitfall that made GridView thumbnails fill the screen.
//! `set_size_request(pixel_size)` does NOT clamp the cell to a square when the
//! image is larger: with `can_shrink = false` the picture's minimum is its
//! intrinsic size, so the cell grows to the image. To get fixed square tiles
//! here too, wrap the picture in `AspectFrame(ratio 1.0)` and drop
//! `can_shrink(false)` — same fix as `media_grid.rs`.
use std::cell::RefCell;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::{ObjectExt, WidgetExt};
use gtk4::subclass::prelude::*;

use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};

const DEFAULT_TILE_SIZE: i32 = 125;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photo-tile.ui")]
    pub struct PhotoTile {
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
        pub item: RefCell<Option<MediaItem>>,
        pub current_token: RefCell<u64>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for PhotoTile {
        const NAME: &'static str = "PhotoTile";
        type Type = super::PhotoTile;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotoTile {}
    impl WidgetImpl for PhotoTile {}
}

gtk::glib::wrapper! {
    pub struct PhotoTile(ObjectSubclass<imp::PhotoTile>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PhotoTile {
    pub fn new() -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        let pic = obj.imp().picture.get();
        pic.set_size_request(DEFAULT_TILE_SIZE, DEFAULT_TILE_SIZE);
        pic.set_can_shrink(false);
        obj
    }

    pub fn set_placeholder(&self) {
        self.imp()
            .picture
            .get()
            .set_paintable(None::<&gtk::gdk::Paintable>);
    }

    pub fn set_item(
        &self,
        item: MediaItem,
        loader: Arc<ThumbnailLoader>,
        thumb_size: ThumbnailSize,
        pixel_size: i32,
    ) {
        *self.imp().item.borrow_mut() = Some(item.clone());
        let pic = self.imp().picture.get();
        pic.set_size_request(pixel_size, pixel_size);
        pic.set_can_shrink(false);
        self.set_placeholder();

        let token = {
            let mut t = self.imp().current_token.borrow_mut();
            *t += 1;
            *t
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(item.uri.clone(), thumb_size, tx);

        let this_weak = self.downgrade();
        gtk::glib::spawn_future_local(async move {
            let texture = match rx.await {
                Ok(t) => t,
                Err(_) => return,
            };
            let still_current = this_weak
                .upgrade()
                .map(|t| *t.imp().current_token.borrow() == token)
                .unwrap_or(false);
            if !still_current {
                return;
            }
            if let Some(this) = this_weak.upgrade() {
                this.imp().picture.get().set_paintable(Some(&texture));
            }
        });
    }
}

impl Default for PhotoTile {
    fn default() -> Self {
        Self::new()
    }
}
