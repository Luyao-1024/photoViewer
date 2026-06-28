//! Photo tile widget — a `GtkBox` holding one `GtkPicture`, used by the
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
        #[template_child]
        pub motion_badge: TemplateChild<gtk::Image>,
        pub item: RefCell<Option<MediaItem>>,
        pub current_token: RefCell<u64>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for PhotoTile {
        const NAME: &'static str = "PhotoTile";
        type Type = super::PhotoTile;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotoTile {}
    impl WidgetImpl for PhotoTile {}
    impl BoxImpl for PhotoTile {}
}

gtk::glib::wrapper! {
    pub struct PhotoTile(ObjectSubclass<imp::PhotoTile>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PhotoTile {
    pub fn new() -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.add_css_class("thumb-tile");
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
        self.imp()
            .motion_badge
            .get()
            .set_visible(item.is_motion_photo());
        self.set_placeholder();

        let token = {
            let mut t = self.imp().current_token.borrow_mut();
            *t += 1;
            *t
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(
            item.uri.clone(),
            thumb_size,
            Some(std::time::SystemTime::from(item.file_mtime)),
            tx,
        );

        let this_weak = self.downgrade();
        gtk::glib::spawn_future_local(async move {
            let texture = match rx.await {
                Ok(loaded) => loaded.texture,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk::test]
    fn photo_tile_template_loads_picture_child() {
        let _ = gtk::init();
        let tile = PhotoTile::new();
        assert!(tile.imp().picture.get().is::<gtk::Picture>());
    }

    #[gtk::test]
    fn photo_tile_picture_can_receive_selection_outline() {
        let _ = gtk::init();
        let tile = PhotoTile::new();

        assert!(tile.has_css_class("thumb-tile"));
        assert!(tile.imp().picture.get().has_css_class("photo-tile-picture"));
    }

    #[gtk::test]
    fn photo_tile_template_loads_motion_badge_child() {
        let _ = gtk::init();
        let tile = PhotoTile::new();

        assert!(tile
            .imp()
            .motion_badge
            .get()
            .has_css_class("thumb-motion-badge"));
        assert!(!tile.imp().motion_badge.get().is_visible());
    }
}
