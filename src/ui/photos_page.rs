//! PhotosPage: year/month/day view (shared MediaGrid, ViewSwitcherBar at bottom).
use std::cell::Ref;
use std::cell::RefCell;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::subclass::prelude::*;

use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::media_grid::MediaGrid;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photos-page.ui")]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub switcher_bar: TemplateChild<adw::ViewSwitcherBar>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for PhotosPage {
        const NAME: &'static str = "PhotosPage";
        type Type = super::PhotosPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotosPage {}
    impl WidgetImpl for PhotosPage {}
    impl NavigationPageImpl for PhotosPage {}
}

gtk::glib::wrapper! {
    pub struct PhotosPage(ObjectSubclass<imp::PhotosPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl PhotosPage {
    /// Build a PhotosPage backed by `media_list`, sharing `loader` across the three
    /// mode-specific MediaGrids (Year/Month/Day).
    pub fn new(media_list: gtk::gio::ListStore, loader: Arc<ThumbnailLoader>) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());

        // Three independent MediaGrid instances — one per grouping mode.
        // Switcher toggles view_stack; each grid is rendered once at construction.
        let year_grid = MediaGrid::new(media_list.clone(), GroupBy::Year, loader.clone());
        let month_grid = MediaGrid::new(media_list.clone(), GroupBy::Month, loader.clone());
        let day_grid = MediaGrid::new(media_list, GroupBy::Day, loader);

        let stack = obj.imp().view_stack.get();
        stack.add_titled(&year_grid, Some("year"), "年");
        stack.add_titled(&month_grid, Some("month"), "月");
        stack.add_titled(&day_grid, Some("day"), "日");

        // Wire the ViewSwitcherBar to our view_stack.
        obj.imp().switcher_bar.get().set_stack(Some(&stack));

        obj
    }

    pub fn media_list(&self) -> Ref<'_, Option<gtk::gio::ListStore>> {
        self.imp().media_list.borrow()
    }
}
