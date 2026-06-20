//! PhotosPage：年/月/日视图（M1 占位，M1-Task 12 加入真实网格）
use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;

mod imp {
    use super::*;
    use adw::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(gtk::CompositeTemplate, Default)]
    #[template(file = "../../data/ui/photos-page.ui")]
    pub struct PhotosPage {
        #[template_child]
        pub root_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub placeholder_label: TemplateChild<gtk::Label>,
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
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
    pub fn new(media_list: gtk::gio::ListStore) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        *obj.imp().media_list.borrow_mut() = Some(media_list);
        obj
    }

    pub fn media_list(&self) -> std::cell::Ref<'_, Option<gtk::gio::ListStore>> {
        self.imp().media_list.borrow()
    }
}
