//! Section header (M1 just a label, M2 may add collapse button)
use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/section-header.ui")]
    pub struct SectionHeader {
        #[template_child]
        pub label: TemplateChild<gtk::Label>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for SectionHeader {
        const NAME: &'static str = "SectionHeader";
        type Type = super::SectionHeader;
        type ParentType = gtk::FlowBoxChild;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SectionHeader {}
    impl WidgetImpl for SectionHeader {}
    impl FlowBoxChildImpl for SectionHeader {}
}

gtk::glib::wrapper! {
    pub struct SectionHeader(ObjectSubclass<imp::SectionHeader>)
        @extends gtk::FlowBoxChild, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl SectionHeader {
    pub fn new(text: &str) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.imp().label.get().set_label(text);
        obj
    }
}

impl Default for SectionHeader {
    fn default() -> Self {
        // Empty default — useful for `Object::new()` style construction in tests.
        gtk::glib::Object::builder().build()
    }
}