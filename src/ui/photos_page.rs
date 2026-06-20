//! PhotosPage: year/month/day view (shared MediaGrid, ViewSwitcherBar at bottom).
//!
//! Hosts three MediaGrid instances. When the user clicks a tile, a `ViewerPage`
//! is pushed onto the host `AdwNavigationView` (injected via `set_nav_target`).
use std::cell::Ref;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;

use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::media_grid::MediaGrid;
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP};

mod imp {
    use super::*;
    use adw::subclass::prelude::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photos-page.ui")]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
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
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        let on_activate: Rc<dyn Fn(u32)> = {
            let weak = obj.downgrade();
            Rc::new(move |global_index| {
                if let Some(this) = weak.upgrade() {
                    this.open_viewer(global_index);
                }
            })
        };

        // Three independent MediaGrid instances — one per grouping mode.
        let year_grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Year,
            loader.clone(),
            on_activate.clone(),
        );
        let month_grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Month,
            loader.clone(),
            on_activate.clone(),
        );
        let day_grid = MediaGrid::new(media_list, GroupBy::Day, loader, on_activate);

        let stack = obj.imp().view_stack.get();
        stack.add_titled(&year_grid, Some("year"), "年");
        stack.add_titled(&month_grid, Some("month"), "月");
        stack.add_titled(&day_grid, Some("day"), "日");

        // Wire the ViewSwitcherBar to our view_stack.
        obj.imp().switcher_bar.get().set_stack(Some(&stack));

        obj
    }

    /// Inject the `AdwNavigationView` we live inside — needed to push/pop
    /// the viewer page. Called by the host (`app::build_app`) after pushing
    /// the PhotosPage.
    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
    }

    pub fn media_list(&self) -> Ref<'_, Option<gtk::gio::ListStore>> {
        self.imp().media_list.borrow()
    }

    fn open_viewer(&self, global_index: u32) {
        let media_list = match self.imp().media_list.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let loader = match self.imp().loader.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let nav = match self.imp().nav_view.borrow().as_ref() {
            Some(n) => n.clone(),
            None => return,
        };

        let viewer = ViewerPage::new(media_list, global_index);
        viewer.show_at(global_index, loader.clone());

        // Wire the viewer's keyboard callback: pops via the host NavigationView
        // for ESC, or advances/retreats the current index for ←/→.
        let viewer_weak = viewer.downgrade();
        let nav_weak = nav.downgrade();
        let loader_for_nav = loader.clone();
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
                        v.show_at(next, loader_for_nav.clone());
                    }
                }
            }
        });

        // Push the new viewer. Subsequent tile-clicks push a *new*
        // viewer; the previous one is reclaimed by the NavigationView
        // when the user pops back, so we don't need to track it here.
        nav.push(&viewer);
    }
}
