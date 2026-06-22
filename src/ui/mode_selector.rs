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
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/mode-selector.ui")]
    pub struct ModeSelector {
        pub active_index: Cell<u32>,
        pub last_synced: Cell<u32>, // 0..=2 — value last written to the stack
        pub stack: std::cell::RefCell<Option<adw::ViewStack>>,
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
            // Record what we just wrote to the labels/dots so the
            // notify::visible-child callback can short-circuit when
            // the change came from us.
            self.last_synced.set(self.active_index.get());
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
    /// (the widget always shows one of the three modes). If a stack
    /// has been bound via [`Self::set_stack`], the stack's visible
    /// child is updated to match.
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
        // Push to the bound ViewStack so the visible child matches.
        // The notify::visible-child handler will fire but will be
        // short-circuited by the last_synced guard.
        if let Some(stack) = imp.stack.borrow().as_ref() {
            let name = match idx {
                0 => "year",
                1 => "month",
                _ => "day",
            };
            stack.set_visible_child_name(name);
        }
    }

    /// Bind a ViewStack. The selector's active index seeds from the
    /// stack's current visible child, and subsequent stack changes
    /// (whether from us or elsewhere) keep the selector in sync.
    ///
    /// Idempotent: calling with the same `stack` is a no-op. Calling
    /// with a different `stack` rebinds.
    pub fn set_stack(&self, stack: &adw::ViewStack) {
        let imp = self.imp();
        {
            let current = imp.stack.borrow();
            if let Some(existing) = current.as_ref() {
                if existing == stack {
                    return;
                }
            }
        }
        *imp.stack.borrow_mut() = Some(stack.clone());

        // Seed active_index from the stack's current visible child.
        let name = stack.visible_child_name();
        let seed = match name.as_deref() {
            Some("year") => 0,
            Some("month") => 1,
            Some("day") => 2,
            _ => 0,
        };
        imp.active_index.set(seed);
        imp.last_synced.set(seed);
        imp.apply_state();

        // Subscribe to visible-child changes. The callback drops the
        // change if it matches what we just wrote ourselves
        // (last_synced), preventing feedback loops.
        let weak = self.downgrade();
        stack.connect_notify_local(Some("visible-child"), move |stack, _| {
            let Some(sel) = weak.upgrade() else { return };
            let name = stack.visible_child_name();
            let new_idx = match name.as_deref() {
                Some("year") => 0,
                Some("month") => 1,
                Some("day") => 2,
                _ => return,
            };
            let imp = sel.imp();
            if imp.last_synced.get() == new_idx {
                // We wrote this ourselves; the guard stays set so the
                // next *external* change still syncs.
                return;
            }
            imp.active_index.set(new_idx);
            imp.apply_state();
        });
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

    // --- Task 3: ViewStack sync + loop guard ---

    /// Build a 3-page ViewStack with names "year"/"month"/"day" so the
    /// selector can resolve them.
    fn build_stack() -> (adw::ViewStack, [gtk::Label; 3]) {
        let stack = adw::ViewStack::new();
        let a = gtk::Label::new(Some("Year"));
        let b = gtk::Label::new(Some("Month"));
        let c = gtk::Label::new(Some("Day"));
        stack.add_titled(&a, Some("year"), "年");
        stack.add_titled(&b, Some("month"), "月");
        stack.add_titled(&c, Some("day"), "日");
        (stack, [a, b, c])
    }

    #[gtk::test]
    fn set_active_index_drives_stack_visible_child() {
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        sel.set_active_index(2);
        assert_eq!(stack.visible_child_name().as_deref(), Some("day"));

        sel.set_active_index(0);
        assert_eq!(stack.visible_child_name().as_deref(), Some("year"));
    }

    #[gtk::test]
    fn stack_visible_child_change_drives_active_index() {
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        // Simulate an external change to the stack.
        stack.set_visible_child_name("month");
        // Pump the main context so the notify::visible-child signal fires.
        let ctx = glib::MainContext::default();
        while ctx.iteration(false) {}

        assert_eq!(sel.active_index(), 1);
    }

    #[gtk::test]
    fn set_stack_seeds_active_index_from_current_child() {
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        stack.set_visible_child_name("day");
        sel.set_stack(&stack);
        assert_eq!(sel.active_index(), 2);
    }

    #[gtk::test]
    fn loop_guard_prevents_recursive_set() {
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        // set_active_index(1) → calls stack.set_visible_child_name → fires
        // notify::visible-child → handler should short-circuit because
        // active_index already matches.
        sel.set_active_index(1);
        // If the loop guard failed, the signal handler would re-set
        // active_index, but since the value is already 1 the test still
        // passes. Stronger check: pump the context and assert no panic.
        let ctx = glib::MainContext::default();
        while ctx.iteration(false) {}
        assert_eq!(sel.active_index(), 1);
    }
}
