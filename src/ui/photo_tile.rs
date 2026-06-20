//! Single photo thumbnail tile (M1 placeholder grey, M2 will load thumbnails)
use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photo-tile.ui")]
    pub struct PhotoTile {
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
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

    /// M1 placeholder: clear paintable so the tile shows as a blank light-grey block.
    /// The CSS provider is installed globally so picture widgets get a #d0d0d0 background.
    pub fn set_placeholder(&self) {
        let css = gtk::CssProvider::new();
        css.load_from_data("picture { background-color: #d0d0d0; }");
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        self.imp().picture.get().set_paintable(None::<&gtk::gdk::Paintable>);
    }
}

impl Default for PhotoTile {
    fn default() -> Self {
        Self::new()
    }
}