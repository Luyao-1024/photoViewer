//! AlbumDetailPage — single-album photo grid view.
//!
//! Reuses the same `PhotoTile` + `ThumbnailLoader` machinery as `MediaGrid`,
//! but without section headers: shows every photo whose `folder_path` matches
//! the album. Thumbnails are requested at the `Medium` bucket (512px).
//!
//! Items in `all_media` are `BoxedAnyObject<MediaItem>` (see
//! `app::initialize`); we unwrap via `BoxedAnyObject::borrow` before handing
//! the value to `PhotoTile::set_item`.
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::core::albums::Album;
use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::photo_tile::PhotoTile;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/album-detail-page.ui")]
    pub struct AlbumDetailPage {
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for AlbumDetailPage {
        const NAME: &'static str = "AlbumDetailPage";
        type Type = super::AlbumDetailPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumDetailPage {}
    impl WidgetImpl for AlbumDetailPage {}
    impl NavigationPageImpl for AlbumDetailPage {}
}

gtk::glib::wrapper! {
    pub struct AlbumDetailPage(ObjectSubclass<imp::AlbumDetailPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl AlbumDetailPage {
    /// Build an `AlbumDetailPage` populated with every media item in
    /// `all_media` whose `folder_path` matches `album.folder_path`.
    ///
    /// `loader` is shared via `Arc` so each `PhotoTile` can clone it for its
    /// own async thumbnail request.
    pub fn new(album: Album, all_media: gtk::gio::ListStore, loader: Arc<ThumbnailLoader>) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&album.name);
        let flow = obj.imp().flow_box.get();

        // Filter media down to this album's folder. `BoxedAnyObject::borrow`
        // returns a `Cow<MediaItem>`; clone so we hand an owned `MediaItem` to
        // the tile (which stores it for the lifetime of its binding).
        for i in 0..all_media.n_items() {
            let Some(obj) = all_media.item(i) else {
                continue;
            };
            let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let item: MediaItem = (*boxed.borrow::<MediaItem>()).clone();
            if item.folder_path == album.folder_path {
                let tile = PhotoTile::new();
                tile.set_item(item, loader.clone(), ThumbnailSize::Medium);
                flow.append(&tile);
            }
        }

        obj
    }
}

impl Default for AlbumDetailPage {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}