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
    use gtk::prelude::*;
    use std::cell::Cell;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/mode-selector.ui")]
    pub struct ModeSelector {
        pub active_index: Cell<u32>,
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

    impl ObjectImpl for ModeSelector {
        fn constructed(&self) {
            self.parent_constructed();
            // Sync template defaults to the current active_index.
            self.apply_state();
        }
    }
    impl WidgetImpl for ModeSelector {}
    impl BoxImpl for ModeSelector {}

    impl ModeSelector {
        /// Apply the current `active_index` to the template children
        /// (label CSS class + dot visibility). O(1) — three pairs of
        /// set/remove + set/remove.
        pub(super) fn apply_state(&self) {
            let labels = [&self.label_0, &self.label_1, &self.label_2];
            let dots = [&self.dot_inner_0, &self.dot_inner_1, &self.dot_inner_2];
            let active = self.active_index.get() as usize;
            for (i, lbl) in labels.iter().enumerate() {
                let l = lbl.get();
                if i == active {
                    l.add_css_class("active");
                } else {
                    l.remove_css_class("active");
                }
            }
            for (i, dot) in dots.iter().enumerate() {
                dot.get().set_visible(i == active);
            }
        }
    }
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

    /// The currently-active mode: 0 = year, 1 = month, 2 = day.
    pub fn active_index(&self) -> u32 {
        self.imp().active_index.get()
    }

    /// Set the active mode. Out-of-range values are silently ignored
    /// (the widget always shows one of the three modes).
    pub fn set_active_index(&self, idx: u32) {
        if idx > 2 {
            return;
        }
        let imp = self.imp();
        if imp.active_index.get() == idx {
            return;
        }
        imp.active_index.set(idx);
        imp.apply_state();
    }
}

impl Default for ModeSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;

    // GTK is a single-threaded library; `#[test]` runs each test in a fresh
    // thread, which would panic with "Attempted to initialize GTK from two
    // different threads" on the second test. The `#[gtk::test]` attribute
    // creates a main thread for GTK and runs all tests on it serially.
    // It also handles `gtk::init()` for us.

    fn labels(sel: &ModeSelector) -> [gtk::Label; 3] {
        let imp = sel.imp();
        [imp.label_0.get(), imp.label_1.get(), imp.label_2.get()]
    }

    fn dots(sel: &ModeSelector) -> [gtk::Box; 3] {
        let imp = sel.imp();
        [
            imp.dot_inner_0.get(),
            imp.dot_inner_1.get(),
            imp.dot_inner_2.get(),
        ]
    }

    #[gtk::test]
    fn default_active_index_is_zero() {
        let sel = ModeSelector::new();
        assert_eq!(sel.active_index(), 0);
    }

    #[gtk::test]
    fn set_active_index_updates_active_index() {
        let sel = ModeSelector::new();
        sel.set_active_index(1);
        assert_eq!(sel.active_index(), 1);
        sel.set_active_index(2);
        assert_eq!(sel.active_index(), 2);
        sel.set_active_index(0);
        assert_eq!(sel.active_index(), 0);
    }

    #[gtk::test]
    fn set_active_index_toggles_label_active_class() {
        let sel = ModeSelector::new();
        let ls = labels(&sel);

        // Initial: index 0 active.
        assert!(ls[0].has_css_class("active"));
        assert!(!ls[1].has_css_class("active"));
        assert!(!ls[2].has_css_class("active"));

        sel.set_active_index(2);
        assert!(!ls[0].has_css_class("active"));
        assert!(!ls[1].has_css_class("active"));
        assert!(ls[2].has_css_class("active"));
    }

    #[gtk::test]
    fn set_active_index_toggles_dot_visibility() {
        let sel = ModeSelector::new();
        let ds = dots(&sel);

        // Initial: only dot 0 visible — `active_index` defaults to 0 and
        // `constructed()` calls `apply_state()` to sync the template children
        // to the canonical source of truth.
        assert!(ds[0].is_visible());
        assert!(!ds[1].is_visible());
        assert!(!ds[2].is_visible());

        sel.set_active_index(1);
        assert!(!ds[0].is_visible());
        assert!(ds[1].is_visible());
        assert!(!ds[2].is_visible());
    }

    #[gtk::test]
    fn set_active_index_clamps_out_of_range() {
        let sel = ModeSelector::new();
        sel.set_active_index(99);
        assert_eq!(sel.active_index(), 0, "out-of-range should be a no-op");
        sel.set_active_index(2);
        sel.set_active_index(3);
        assert_eq!(
            sel.active_index(),
            2,
            "out-of-range should not change current"
        );
    }
}
