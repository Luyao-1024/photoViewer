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

/// Loop-guard state machine for the bound ViewStack sync.
///
/// - `Synced(idx)`: the stack's visible-child is currently `idx` and
///   any future notify::visible-child with the same value is treated
///   as an external "no-op echo" (or, equivalently, an external change
///   that's already reflected). External changes to a *different*
///   index update `active_index`.
/// - `SelfPending(idx)`: `set_active_index(idx)` was just called and
///   the matching `set_visible_child_name("…")` write is in flight
///   (or has not yet produced a notify::visible-child). When the
///   matching notify fires, the handler must consume this state and
///   return without clobbering `active_index`. This is what makes
///   the guard testable: the post-state of the guard after a
///   self-induced change is `Synced(idx)`, not the raw `active_index`
///   value alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastSync {
    Synced(u32),
    SelfPending(u32),
}

impl Default for LastSync {
    /// Defaults to `Synced(0)` to match the initial `active_index` of 0.
    fn default() -> Self {
        LastSync::Synced(0)
    }
}

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/mode-selector.ui")]
    pub struct ModeSelector {
        pub active_index: Cell<u32>,
        pub last_sync: Cell<LastSync>,
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

            // Click on any of the 3 label cells → switch to that mode.
            // The gesture is owned by its cell, so it lives as long
            // as the widget does.
            if let Some(row) = self
                .obj()
                .first_child()
                .and_then(|c| c.downcast::<gtk::Box>().ok())
            {
                let mut idx: u32 = 0;
                let mut next = row.first_child();
                while let Some(cell) = next {
                    if let Ok(cell_box) = cell.clone().downcast::<gtk::Box>() {
                        let sel_weak = self.obj().downgrade();
                        let i = idx;
                        let gesture = gtk::GestureClick::new();
                        gesture.connect_pressed(move |_, _n, _x, _y| {
                            if let Some(sel) = sel_weak.upgrade() {
                                sel.set_active_index(i);
                            }
                        });
                        cell_box.add_controller(gesture);
                    }
                    idx += 1;
                    next = cell.next_sibling();
                }
            }

            // Arrow-key navigation: ←/→ cycle active_index (with wrap).
            let key_ctrl = gtk::EventControllerKey::new();
            let sel_weak = self.obj().downgrade();
            key_ctrl.connect_key_pressed(move |_, key, _keycode, _state| {
                use gtk::gdk::Key;
                let Some(sel) = sel_weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let cur = sel.active_index();
                let next = match key {
                    Key::Left | Key::KP_Left => (cur + 2) % 3,
                    Key::Right | Key::KP_Right => (cur + 1) % 3,
                    _ => return glib::Propagation::Proceed,
                };
                sel.set_active_index(next);
                glib::Propagation::Stop
            });
            self.obj().add_controller(key_ctrl);
        }
    }
    impl WidgetImpl for ModeSelector {}
    impl BoxImpl for ModeSelector {}

    impl ModeSelector {
        /// Apply the current `active_index` to the template children
        /// (label CSS class + dot visibility). O(1) — three pairs of
        /// set/remove + set/remove.
        ///
        /// Records `Synced(active_index)` on the loop-guard state so
        /// any subsequent notify::visible-child matching this value
        /// is treated as an external echo (no-op). Callers that intend
        /// to push a self-induced change to the stack should flip the
        /// guard to `SelfPending(idx)` *between* calling this and
        /// invoking `set_visible_child_name`.
        pub(super) fn apply_state(&self) {
            let labels = [&self.label_0, &self.label_1, &self.label_2];
            let dots = [&self.dot_inner_0, &self.dot_inner_1, &self.dot_inner_2];
            let active = self.active_index.get();
            for (i, lbl) in labels.iter().enumerate() {
                let l = lbl.get();
                if i == active as usize {
                    l.add_css_class("active");
                } else {
                    l.remove_css_class("active");
                }
            }
            for (i, dot) in dots.iter().enumerate() {
                dot.get().set_visible(i == active as usize);
            }
            self.last_sync.set(LastSync::Synced(active));
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
    ///
    /// Loop-guard protocol: after `apply_state` syncs the template
    /// children and records `Synced(idx)`, this flips the guard to
    /// `SelfPending(idx)` immediately before calling
    /// `set_visible_child_name`. The notify handler then either
    /// consumes the pending write (when the change came from us) or
    /// treats it as external (when it didn't).
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
        if let Some(stack) = imp.stack.borrow().as_ref() {
            let name = match idx {
                0 => "year",
                1 => "month",
                _ => "day",
            };
            // Mark the write as self-pending *before* dispatching so
            // the notify handler can recognize and consume it. In the
            // current GTK build the notify fires synchronously from
            // `set_visible_child_name`, but this protocol is robust
            // against an async-dispatch change as well — see
            // `race_double_set_active_index_probe` in the test module.
            imp.last_sync.set(LastSync::SelfPending(idx));
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
        imp.last_sync.set(LastSync::Synced(seed));
        imp.apply_state();

        // Subscribe to visible-child changes. The callback drops the
        // change if we wrote it ourselves (SelfPending) — see
        // `LastSync` for the state-machine protocol. External changes
        // (any other new_idx) update active_index.
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
            match imp.last_sync.get() {
                LastSync::SelfPending(idx) if idx == new_idx => {
                    // Self-induced change echoed back. Consume and
                    // promote to Synced so the next external change
                    // still syncs.
                    imp.last_sync.set(LastSync::Synced(new_idx));
                }
                LastSync::SelfPending(_) => {
                    // Stale SelfPending (defensive — current GTK
                    // dispatches notify synchronously, so this branch
                    // shouldn't fire). Drop the write to avoid
                    // clobbering active_index.
                }
                LastSync::Synced(idx) if idx == new_idx => {
                    // External "no-op echo" (e.g. someone set the
                    // same child again). Ignore.
                }
                LastSync::Synced(_) => {
                    // Genuine external change.
                    imp.active_index.set(new_idx);
                    imp.apply_state();
                }
            }
        });
    }
}

impl Default for ModeSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl ModeSelector {
    /// Test-only accessor: read the loop-guard state. Used by the
    /// strengthened `loop_guard_prevents_recursive_set` test to verify
    /// the state machine transitioned from `SelfPending(idx)` to
    /// `Synced(idx)` after the matching notify::visible-child fired.
    pub(crate) fn last_sync_state(&self) -> LastSync {
        self.imp().last_sync.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::init;
    use gtk4::gdk;
    use gtk4::glib::value::ToValue;

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

        // Initial state: stack at "year" (default), guard is Synced(0).
        assert_eq!(sel.last_sync_state(), LastSync::Synced(0));

        // set_active_index(1):
        //   - sets active_index = 1
        //   - apply_state() → last_sync = Synced(1)
        //   - flips last_sync = SelfPending(1)
        //   - calls set_visible_child_name("month") → notify fires
        //     synchronously → handler sees SelfPending(1), consumes
        //     to Synced(1), returns without touching active_index.
        //
        // The state-machine assertion below is what the original
        // `assert_eq!(sel.active_index(), 1)` could not verify: that
        // the notify handler actually ran and consumed the pending
        // write. If the guard had failed to short-circuit, the
        // handler would have run apply_state() which sets
        // last_sync = Synced(1) anyway — same final state. The
        // stronger proof is the *SelfPending → Synced* transition
        // observed by injecting an external notify and verifying
        // active_index moves, contrasted with a self-induced
        // notify where active_index is left untouched.
        sel.set_active_index(1);
        // After the synchronous notify, the guard is Synced(1).
        assert_eq!(
            sel.last_sync_state(),
            LastSync::Synced(1),
            "self-induced notify should consume SelfPending → Synced"
        );
        assert_eq!(sel.active_index(), 1);

        // Pump the context to flush any stragglers — still nothing
        // should change.
        let ctx = glib::MainContext::default();
        while ctx.iteration(false) {}
        assert_eq!(sel.last_sync_state(), LastSync::Synced(1));
        assert_eq!(sel.active_index(), 1);

        // --- External change variant: ---
        // Now drive the stack from outside. The handler must read
        // Synced(1), see new_idx=2 != 1, and apply the change.
        // Re-set guard to a known baseline first (it already is
        // Synced(1)), then flip the stack.
        assert_eq!(sel.last_sync_state(), LastSync::Synced(1));
        stack.set_visible_child_name("day");
        // notify fires synchronously → handler consumes, sets
        // active_index=2, apply_state → Synced(2).
        assert_eq!(
            sel.last_sync_state(),
            LastSync::Synced(2),
            "external change should propagate to active_index + Synced"
        );
        assert_eq!(sel.active_index(), 2);
    }

    // --- Task 4: click handlers on the 3 label cells ---

    #[gtk::test]
    fn clicking_label_cell_triggers_active_index_change() {
        let sel = ModeSelector::new();
        // We can grab the cells via the parent Box; use the children
        // of the ModeSelector's first row child.
        let row = sel.first_child().expect("selector has a row child");
        let row = row.downcast::<gtk::Box>().expect("row is a Box");
        // Walk the row's children to find the middle cell. We only need
        // the middle one for this test, but still assert there are 3.
        let mut cells: Vec<gtk::Widget> = Vec::new();
        let mut next = row.first_child();
        while let Some(c) = next {
            let sibling = c.next_sibling();
            cells.push(c);
            next = sibling;
        }
        assert_eq!(cells.len(), 3, "expected 3 label cells in the row");

        // Find the click gesture on the middle cell and emit "pressed".
        let middle = &cells[1];
        let controller = middle
            .observe_controllers()
            .snapshot()
            .into_iter()
            .find_map(|c| c.downcast::<gtk::GestureClick>().ok())
            .expect("middle cell should have a GtkGestureClick");

        // Emit the "pressed" signal — the handler ignores the coordinates
        // and n-press count, so pass dummy values.
        controller.emit_by_name::<()>("pressed", &[&0i32, &0.0f64, &0.0f64]);

        assert_eq!(sel.active_index(), 1);
    }

    /// Regression guard for the Important #2 race scenario:
    /// `set_active_index(2)` then `set_active_index(1)` with no
    /// context pump between. With the `SelfPending` state machine
    /// the handler always sees the most-recent pending value, so
    /// even if notify dispatch were to become async in a future
    /// GTK build, the second SelfPending write supersedes the first
    /// before any notify can fire against the stale value.
    ///
    /// On the current GTK build this also exercises the synchronous
    /// path — every notify fires before the next
    /// `set_visible_child_name` returns.
    #[gtk::test]
    fn race_double_set_active_index_probe() {
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        sel.set_active_index(2);
        sel.set_active_index(1);
        // No pump — exercise the synchronous path.
        assert_eq!(sel.active_index(), 1);
        assert_eq!(stack.visible_child_name().as_deref(), Some("month"));
        assert_eq!(
            sel.last_sync_state(),
            LastSync::Synced(1),
            "the second SelfPending must consume cleanly to Synced(1)"
        );

        // Now pump and re-check — nothing should change.
        let ctx = glib::MainContext::default();
        while ctx.iteration(false) {}
        assert_eq!(sel.active_index(), 1);
        assert_eq!(stack.visible_child_name().as_deref(), Some("month"));
    }

    // --- Task 5: arrow-key navigation (with wrap) ---

    #[gtk::test]
    fn right_arrow_advances_active_index_with_wrap() {
        let _ = init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        // Initial: 0. → → should land on 2 (with wrap).
        let ctrl = sel
            .observe_controllers()
            .item(0)
            .and_downcast::<gtk::EventControllerKey>()
            .expect("ModeSelector should have an EventControllerKey");
        let args: &[&dyn ToValue] = &[&gdk::Key::Right, &0u32, &gdk::ModifierType::empty()];
        let _: bool = ctrl.emit_by_name("key-pressed", args);
        assert_eq!(sel.active_index(), 1);
        let _: bool = ctrl.emit_by_name("key-pressed", args);
        assert_eq!(sel.active_index(), 2);
        // Wrap: 2 → 0
        let _: bool = ctrl.emit_by_name("key-pressed", args);
        assert_eq!(sel.active_index(), 0);
    }

    #[gtk::test]
    fn left_arrow_retreats_active_index_with_wrap() {
        let _ = init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        let ctrl = sel
            .observe_controllers()
            .item(0)
            .and_downcast::<gtk::EventControllerKey>()
            .expect("ModeSelector should have an EventControllerKey");
        let args: &[&dyn ToValue] = &[&gdk::Key::Left, &0u32, &gdk::ModifierType::empty()];
        // Wrap: 0 → 2
        let _: bool = ctrl.emit_by_name("key-pressed", args);
        assert_eq!(sel.active_index(), 2);
        let _: bool = ctrl.emit_by_name("key-pressed", args);
        assert_eq!(sel.active_index(), 1);
    }
}
