//! AlbumDetailPage — single-album day-grouped photo grid view.
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::core::albums::Album;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::empty_states;
use crate::ui::media_grid::MediaGrid;
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP};
use std::cell::RefCell;
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/album-detail-page.ui")]
    pub struct AlbumDetailPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub content_box: TemplateChild<gtk::Box>,
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
    /// Build an `AlbumDetailPage` populated with a pre-filtered media list.
    /// The grid uses the same `MediaGrid` Day grouping as `PhotosPage`.
    pub fn new(
        album: Album,
        media_list: gtk::gio::ListStore,
        loader: Arc<ThumbnailLoader>,
    ) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&album.display_name());
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());

        if media_list.n_items() == 0 {
            let empty = empty_states::no_album_photos();
            empty.set_hexpand(true);
            empty.set_vexpand(true);
            obj.imp().content_box.get().append(&empty);
        } else {
            let on_activate: Rc<dyn Fn(u32)> = {
                let weak = obj.downgrade();
                Rc::new(move |global_index| {
                    if let Some(this) = weak.upgrade() {
                        this.open_viewer(global_index);
                    }
                })
            };
            let on_background_changed: Rc<dyn Fn()> = Rc::new(|| {});
            let grid = MediaGrid::new(
                media_list,
                GroupBy::Day,
                loader,
                on_activate,
                on_background_changed,
            );
            obj.imp().content_box.get().append(&grid);
        }

        obj
    }

    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
    }

    fn open_viewer(&self, global_index: u32) {
        let media_list = match self.imp().media_list.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let nav = match self.imp().nav_view.borrow().as_ref() {
            Some(n) => n.clone(),
            None => return,
        };

        let viewer = ViewerPage::new(media_list, global_index);
        viewer.show_at(global_index);

        let viewer_weak = viewer.downgrade();
        let nav_weak = nav.downgrade();
        viewer.connect_navigation(move |delta: NavDelta| {
            if delta == NAV_POP {
                if let Some(n) = nav_weak.upgrade() {
                    n.pop();
                }
                return;
            }
            if let Some(v) = viewer_weak.upgrade() {
                let cur = v.current_index();
                let next = (cur as i32 + delta).max(0) as u32;
                if let Some(list) = v.imp().media_list.borrow().as_ref() {
                    if next < list.n_items() {
                        v.show_at(next);
                    }
                }
            }
        });

        nav.push(&viewer);
    }
}

impl Default for AlbumDetailPage {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}
