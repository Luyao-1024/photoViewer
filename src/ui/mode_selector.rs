//! ModeSelector: 3-cell 年/月/日 switcher used by `PhotosPage`.
//!
//! Visual: a vertical pair of rows (labels, then a dot strip). The
//! currently-active mode has its label fully opaque and its `dot_inner`
//! visible. The widget is meant to be added as an overlay child of a
//! `GtkOverlay` containing the `ViewStack` it drives.
//!
//! Active index is the single source of truth. `set_stack` wires
//! `ViewStack::visible-child` → `active_index` to keep the selector in
//! sync if the stack is changed externally.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/mode-selector.ui")]
    pub struct ModeSelector {
        #[template_child]
        pub label_0: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_1: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_2: TemplateChild<gtk::Label>,
        #[template_child]
        pub dot_inner_0: TemplateChild<gtk::Box>,
        #[template_child]
        pub dot_inner_1: TemplateChild<gtk::Box>,
        #[template_child]
        pub dot_inner_2: TemplateChild<gtk::Box>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for ModeSelector {
        const NAME: &'static str = "ModeSelector";
        type Type = super::ModeSelector;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ModeSelector {}
    impl WidgetImpl for ModeSelector {}
    impl BoxImpl for ModeSelector {}
}

gtk::glib::wrapper! {
    pub struct ModeSelector(ObjectSubclass<imp::ModeSelector>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ModeSelector {
    pub fn new() -> Self {
        gtk::glib::Object::builder().build()
    }
}

impl Default for ModeSelector {
    fn default() -> Self {
        Self::new()
    }
}
