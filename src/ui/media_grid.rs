//! MediaGrid reusable component: shared by year/month/day views
//!
//! Constructed with a `gio::ListStore` of `BoxedAnyObject` wrapping `MediaItem`s
//! (see `app::initialize` in Task 10), plus a grouping `mode`. Internally builds
//! section headers + placeholder photo tiles via a `FlowBox` inside a
//! `ScrolledWindow`.
//!
//! Implementation note: `GtkScrolledWindow` is not subclassable in gtk4-rs 0.8,
//! so we subclass `GtkBox` and put the `ScrolledWindow` as our only child.
//! `FlowBox::remove_all` is also v4_12-only in 0.8, so we iterate via
//! `observe_children` to drop existing rows on rebuild.
use std::cell::Cell;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::core::media::MediaItem;
use crate::core::section_model::{group_items, GroupBy};
use crate::ui::photo_tile::PhotoTile;
use crate::ui::section_header::SectionHeader;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/media-grid.ui")]
    pub struct MediaGrid {
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
        pub mode: Cell<GroupBy>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for MediaGrid {
        const NAME: &'static str = "MediaGrid";
        type Type = super::MediaGrid;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MediaGrid {}
    impl WidgetImpl for MediaGrid {}
    impl BoxImpl for MediaGrid {}
}

gtk::glib::wrapper! {
    pub struct MediaGrid(ObjectSubclass<imp::MediaGrid>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MediaGrid {
    /// Build a MediaGrid that immediately renders `(media_list, mode)`.
    pub fn new(media_list: gtk::gio::ListStore, mode: GroupBy) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.imp().mode.set(mode);
        obj.rebuild(media_list, mode);
        obj
    }

    /// Re-render this grid with a new mode using the given list (already grouped/owned).
    pub fn set_mode(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        self.imp().mode.set(mode);
        self.rebuild(media_list, mode);
    }

    pub fn mode(&self) -> GroupBy {
        self.imp().mode.get()
    }

    fn rebuild(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        // 1. Extract MediaItem values from each BoxedAnyObject in the store.
        let mut items: Vec<MediaItem> = Vec::with_capacity(media_list.n_items() as usize);
        for i in 0..media_list.n_items() {
            if let Some(obj) = media_list.item(i) {
                if let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() {
                    let cow = boxed.borrow::<MediaItem>();
                    items.push((*cow).clone());
                }
            }
        }

        // 2. Group by year / month / day.
        let sections = group_items(&items, mode);

        // 3. Clear existing flow_box contents (gtk4-rs 0.8 lacks FlowBox::remove_all).
        let flow = self.imp().flow_box.get();
        let mut child = flow.first_child();
        while let Some(c) = child {
            let next = c.next_sibling();
            flow.remove(&c);
            child = next;
        }

        // 4. Append section headers + placeholder photo tiles.
        for section in sections {
            let header = SectionHeader::new(&section.label);
            flow.append(&header);
            for _item in &section.items {
                let tile = PhotoTile::new();
                tile.set_placeholder();
                flow.append(&tile);
            }
        }
    }
}

impl Default for MediaGrid {
    fn default() -> Self {
        // Empty store; production code should call `new` with a real list.
        gtk::glib::Object::builder().build()
    }
}