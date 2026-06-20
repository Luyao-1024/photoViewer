//! Single photo thumbnail tile (M1 placeholder grey, M2 loads real thumbnails)
use std::cell::RefCell;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::*;

use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use gtk4::prelude::ObjectExt;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photo-tile.ui")]
    pub struct PhotoTile {
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
        pub item: RefCell<Option<MediaItem>>,
        /// Monotonically increasing token; the receiver compares it to drop stale responses.
        pub current_token: RefCell<u64>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for PhotoTile {
        const NAME: &'static str = "PhotoTile";
        type Type = super::PhotoTile;
        type ParentType = gtk::FlowBoxChild;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotoTile {}
    impl WidgetImpl for PhotoTile {}
    impl FlowBoxChildImpl for PhotoTile {}
}

gtk::glib::wrapper! {
    pub struct PhotoTile(ObjectSubclass<imp::PhotoTile>)
        @extends gtk::FlowBoxChild, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PhotoTile {
    pub fn new() -> Self {
        gtk::glib::Object::builder().build()
    }

    /// Placeholder while the thumbnail loads: clear the paintable so the tile shows
    /// as a blank transparent block. We intentionally do NOT paint any background —
    /// the theme's default surface colour shows through (which is transparent in
    /// most themes, or matches the window background).
    pub fn set_placeholder(&self) {
        self.imp().picture.get().set_paintable(None::<&gtk::gdk::Paintable>);
    }

    /// M2: bind a `MediaItem` and asynchronously load its thumbnail via `ThumbnailLoader`.
    ///
    /// `loader` is shared via `Arc` so the spawned local future can hold a clone for
    /// the duration of the `oneshot` await. The future is also cancellable via the
    /// per-tile `current_token`: any newer `set_item` call invalidates older responses,
    /// so rapid scroll/rebind does not paint stale textures.
    pub fn set_item(&self, item: MediaItem, loader: Arc<ThumbnailLoader>, size: ThumbnailSize) {
        *self.imp().item.borrow_mut() = Some(item.clone());

        // Show the grey placeholder while the thumbnail loads, so empty tiles don't
        // briefly render nothing underneath.
        self.set_placeholder();

        // Debounce: bump the token; older responses will be dropped when they arrive.
        let token = {
            let mut t = self.imp().current_token.borrow_mut();
            *t += 1;
            *t
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(item.uri.clone(), size, tx);

        let this_weak = self.downgrade();
        gtk::glib::spawn_future_local(async move {
            let texture = match rx.await {
                Ok(t) => t,
                Err(_) => return, // sender dropped — request was cancelled
            };

            // Drop stale response: the tile was rebound to a different item in the meantime.
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
