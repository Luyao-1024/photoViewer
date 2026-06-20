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
//! and appended directly to the page's `GtkFlowBox`. We do not need a separate
//! `AlbumTile` GObject subclass for the placeholder visuals — activation
//! handling is deferred to the future sidebar→nav routing task (M5-T1).
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::subclass::prelude::*;

use crate::core::albums::Album;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::empty_states;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/albums-page.ui")]
    pub struct AlbumsPage {
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
        let obj: Self = glib::Object::builder().build();
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
        }

        obj
    }
}

/// Build a single album tile (a `GtkFlowBoxChild` containing a vertical box
/// with picture + name + count). The cover loads asynchronously through
/// `loader`; while the request is in flight the picture shows a grey
/// placeholder (matching `PhotoTile::set_placeholder`).
fn build_album_tile(album: &Album, loader: Arc<ThumbnailLoader>) -> gtk::FlowBoxChild {
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(6)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .width_request(240)
        .height_request(240)
        .build();

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
    picture.set_paintable(None::<&gtk::gdk::Paintable>);

    if let Some(uri) = &album.cover_uri {
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(uri.clone(), ThumbnailSize::Medium, tx);
        let picture_weak = picture.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(texture) = rx.await {
                if let Some(p) = picture_weak.upgrade() {
                    p.set_paintable(Some(&texture));
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

    box_.append(&picture);
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
