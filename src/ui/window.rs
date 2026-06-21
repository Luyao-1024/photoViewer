//! Main window: sidebar + content area
use std::cell::RefCell;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::ListBoxRow;
use libadwaita as adw;
use glib::subclass::types::ObjectSubclassIsExt;


use crate::core::db::DbPool;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::{AlbumsPage, TrashPage};

mod imp {
    use super::*;
    use adw::subclass::prelude::*;

    #[derive(gtk::CompositeTemplate, gtk::glib::Properties, Default)]
    #[properties(wrapper_type = super::MainWindow)]
    #[template(file = "../../data/ui/window.ui")]
    pub struct MainWindow {
        pub pool: RefCell<Option<DbPool>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        #[template_child]
        pub sidebar_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub nav_view: TemplateChild<adw::NavigationView>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "PhotoViewerWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::glib::derived_properties]
    impl ObjectImpl for MainWindow {}
    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}
    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}
}

gtk::glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends adw::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Self {
        gtk::glib::Object::builder()
            .property("application", app)
            .build()
    }

    /// Populate the sidebar ListBox with section rows.
    /// Photos / Albums / Trash — only Photos is wired up in M1; others are placeholders.
    pub fn populate_sidebar(&self) {
        let list = self.imp().sidebar_list.get();
        for (label, _target) in &[
            ("Photos", "photos"),
            ("Albums", "albums"),
            ("Trash", "trash"),
        ] {
            let row = ListBoxRow::new();
            let lbl = gtk::Label::builder()
                .label(*label)
                .halign(gtk::Align::Start)
                .margin_start(12)
                .margin_end(12)
                .margin_top(8)
                .margin_bottom(8)
                .build();
            row.set_child(Some(&lbl));
            list.append(&row);
        }
    }

    /// Accessor for the content area's NavigationView (used by later tasks).
    pub fn nav_view(&self) -> adw::NavigationView {
        self.imp().nav_view.get()
    }

    /// Inject the DB pool and thumbnail loader so the sidebar can construct
    /// pages on demand. Called from `app::build_app` once initialization
    /// (DB + scan) has completed.
    pub fn set_resources(&self, pool: DbPool, loader: Arc<ThumbnailLoader>) {
        *self.imp().pool.borrow_mut() = Some(pool);
        *self.imp().loader.borrow_mut() = Some(loader);
    }

    /// Wire the sidebar `ListBox` `row-selected` signal to push the
    /// corresponding page onto `nav_view`. Sidebar rows are:
    ///   0 → Photos   (root page, no-op)
    ///   1 → Albums   (pushes `AlbumsPage`)
    ///   2 → Trash    (pushes `TrashPage`)
    ///
    /// Requires `set_resources` to have been called first; if the resources
    /// are missing the closure silently no-ops.
    pub fn connect_sidebar(&self, nav_view: &adw::NavigationView) {
        let list = self.imp().sidebar_list.get();

        list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else { return };
                match row.index() {
                    0 => {
                        // Photos is the root page; nothing to do.
                    }
                    1 => {
                        // Albums: query album list then push a fresh AlbumsPage.
                        let pool = window.imp().pool.borrow().clone();
                        let Some(pool) = pool else { return };
                        let loader = match window.imp().loader.borrow().clone() {
                            Some(l) => l,
                            None => return,
                        };
                        let albums = crate::core::albums::list(&pool).unwrap_or_default();
                        let page = AlbumsPage::new(albums, loader);
                        nav_view.push(&page);
                    }
                    2 => {
                        // Trash: push a fresh TrashPage.
                        let pool = window.imp().pool.borrow().clone();
                        let Some(pool) = pool else { return };
                        let loader = match window.imp().loader.borrow().clone() {
                            Some(l) => l,
                            None => return,
                        };
                        let page = TrashPage::new(pool, loader);
                        nav_view.push(&page);
                    }
                    _ => {}
                }
            }),
        );
    }
}