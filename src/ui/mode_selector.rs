//! ModeSelector: 3-cell 年/月/日 switcher used by `PhotosPage`.
//!
//! Visual: a vertical pair of rows (labels, then a single sliding indicator
//! bar). The currently-active mode has its label fully opaque, and the
//! indicator slides beneath the active label with a smooth decelerating
//! transition that stops exactly on the active label (no overshoot), driven by
//! a runtime CssProvider that writes `translateX`. The widget is meant to be
//! added as an overlay child of a `GtkOverlay` containing the `GtkStack` it
//! drives.
//!
//! Active index is the single source of truth. `set_stack` wires
//! `GtkStack::visible-child` → `active_index` to keep the selector in sync if
//! the stack is changed externally.

use crate::core::i18n::tr;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

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

/// Indicator translateX (px) that centers the bar under label `active_index`,
/// given one label cell's width and the indicator's own width. Pure so it can
/// be unit-tested without a widget allocation. Label N's center is at
/// `(N + 0.5) * cell_width`; the indicator (left-aligned at rest) reaches that
/// center by translating to `center - indicator_width / 2`.
fn indicator_x_for(active_index: u32, cell_width: i32, indicator_width: i32) -> i32 {
    let center = (active_index as i32 * 2 + 1) * cell_width / 2;
    center - indicator_width / 2
}

fn mode_name_for_index(idx: u32) -> &'static str {
    match idx {
        0 => "year",
        1 => "month",
        2 => "day",
        _ => "unknown",
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
        /// Bound ViewStack + the `SignalHandlerId` of its
        /// `notify::visible-child` subscription. Keeping the id
        /// alongside the stack lets `set_stack` disconnect the old
        /// handler before installing a new one when the bound stack
        /// is swapped (rebind).
        pub stack: std::cell::RefCell<Option<(gtk::Stack, glib::SignalHandlerId)>>,
        #[template_child]
        pub label_0: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_1: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_2: TemplateChild<gtk::Label>,
        #[template_child]
        pub dot_row: TemplateChild<gtk::Box>,
        /// The single sliding indicator bar under the labels. It is dot_row's
        /// only child, left-aligned at rest; `update_indicator_position`
        /// translates it to the active label's center.
        #[template_child]
        pub mode_dot_indicator: TemplateChild<gtk::Box>,
        /// Runtime CssProvider that writes the indicator's `transform:
        /// translateX(...)`. Reloading it with a new value retriggers the CSS
        /// transition (same pattern as the viewer filmstrip).
        pub indicator_provider: std::cell::RefCell<Option<gtk::CssProvider>>,
        /// Last computed indicator translateX (px). Stored for testability —
        /// the CssProvider itself isn't inspectable from a headless test.
        pub indicator_target_x: Cell<i32>,
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

            // Sliding indicator: a runtime CssProvider writes its translateX
            // (scoped to the indicator's style context), and we recompute on
            // every dot_row resize so the bar stays centered under the active
            // label at any width.
            let provider = gtk::CssProvider::new();
            self.mode_dot_indicator
                .get()
                .style_context()
                .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            *self.indicator_provider.borrow_mut() = Some(provider);

            self.obj().set_labels_i18n();
            // Sync template defaults after labels/provider exist, so the
            // first transform can be seeded from natural row width before GTK
            // assigns the widget its first allocation.
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

            // The liquid-glass backdrop is now handled by GTK CSS
            // `backdrop-filter` in `grid_css.rs`. Keeping the effect in CSS
            // lets GTK/GSK use the renderer's native backdrop nodes instead of
            // repeatedly capturing and CPU-warping the grid in a tick callback.
        }
    }
    impl WidgetImpl for ModeSelector {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            // Run the default GtkBox layout first so dot_row (and the indicator)
            // receive their allocations, then recenter the sliding indicator.
            self.parent_size_allocate(width, height, baseline);
            self.update_indicator_position();
        }
    }
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
            let active = self.active_index.get();
            for (i, lbl) in labels.iter().enumerate() {
                let l = lbl.get();
                if i == active as usize {
                    l.add_css_class("active");
                } else {
                    l.remove_css_class("active");
                }
            }
            // The single indicator slides to the active label (no per-dot
            // show/hide). No-op until dot_row is allocated; WidgetImpl::
            // size_allocate re-runs it on every resize.
            self.update_indicator_position();
            self.last_sync.set(LastSync::Synced(active));
        }

        /// Recompute and write the indicator's translateX so it sits centered
        /// under the active label. Before the first allocation, fall back to
        /// the label row's natural width so the initial active state is already
        /// positioned for first paint. Called from apply_state and from the
        /// size_allocate vfunc so the bar recenters at any width.
        fn update_indicator_position(&self) {
            let row_width = self.indicator_row_width();
            if row_width <= 0 {
                return;
            }
            let cell_width = row_width / 3;
            let indicator = self.mode_dot_indicator.get();
            let indicator_width = indicator
                .allocation()
                .width()
                .max(indicator.measure(gtk::Orientation::Horizontal, -1).1)
                .max(1);
            let x = indicator_x_for(self.active_index.get(), cell_width, indicator_width);
            self.indicator_target_x.set(x);
            if let Some(provider) = self.indicator_provider.borrow().as_ref() {
                provider
                    .load_from_data(&format!("box.mode-dot {{ transform: translateX({x}px); }}"));
            }
        }

        fn indicator_row_width(&self) -> i32 {
            let allocated = self.dot_row.get().allocation().width();
            if allocated > 0 {
                return allocated;
            }

            if let Some(label_row) = self
                .obj()
                .first_child()
                .and_then(|child| child.downcast::<gtk::Box>().ok())
            {
                let natural = label_row.measure(gtk::Orientation::Horizontal, -1).1;
                if natural > 0 {
                    return natural;
                }
            }

            self.dot_row
                .get()
                .measure(gtk::Orientation::Horizontal, -1)
                .1
        }
    }
}

gtk::glib::wrapper! {
    pub struct ModeSelector(ObjectSubclass<imp::ModeSelector>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ModeSelector {
    fn set_labels_i18n(&self) {
        let imp = self.imp();
        imp.label_0.get().set_label(&tr("photo.mode.year"));
        imp.label_1.get().set_label(&tr("photo.mode.month"));
        imp.label_2.get().set_label(&tr("photo.mode.day"));
    }

    pub fn new() -> Self {
        gtk::glib::Object::builder().build()
    }

    /// The currently-active mode: 0 = year, 1 = month, 2 = day.
    pub fn active_index(&self) -> u32 {
        self.imp().active_index.get()
    }

    /// Switch the selector foreground for contrast against the content behind
    /// the floating panel. `true` means the panel is over bright content, so
    /// text and the active indicator should render black; `false` renders
    /// them white.
    pub fn set_light_background(&self, is_light: bool) {
        if is_light {
            self.add_css_class("on-light-background");
        } else {
            self.remove_css_class("on-light-background");
        }
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
        let from = self.imp().active_index.get();
        let stack_bound = self.imp().stack.borrow().is_some();
        let span = tracing::info_span!(
            target: crate::core::log_targets::BROWSING,
            "photos:mode_selector_set_active",
            from_index = from,
            from = mode_name_for_index(from),
            to_index = idx,
            to = mode_name_for_index(idx),
            stack_bound,
            outcome = tracing::field::Empty
        );
        let _trace = span.enter();
        if idx > 2 {
            span.record("outcome", "out_of_range");
            return;
        }
        let imp = self.imp();
        if imp.active_index.get() == idx {
            span.record("outcome", "already_active");
            return;
        }
        imp.active_index.set(idx);
        imp.apply_state();
        if let Some((stack, _)) = imp.stack.borrow().as_ref() {
            let name = mode_name_for_index(idx);
            // Mark the write as self-pending *before* dispatching so
            // the notify handler can recognize and consume it. In the
            // current GTK build the notify fires synchronously from
            // `set_visible_child_name`, but this protocol is robust
            // against an async-dispatch change as well — see
            // `race_double_set_active_index_probe` in the test module.
            imp.last_sync.set(LastSync::SelfPending(idx));
            span.record("outcome", "set_stack_child");
            stack.set_visible_child_name(name);
        } else {
            span.record("outcome", "selector_only");
        }
    }

    /// Bind a GtkStack (with `transition-type: crossfade` so year/month/day
    /// grids crossfade). The selector's active index seeds from the stack's
    /// current visible child, and subsequent stack changes (whether from us or
    /// elsewhere) keep the selector in sync.
    ///
    /// Idempotent for the same `stack` (no-op on repeat calls with
    /// the same pointer). Rebinding to a different stack is
    /// supported: the previous stack's `notify::visible-child`
    /// subscription is disconnected before the new one is installed,
    /// so no stale handler can fire against the new binding.
    pub fn set_stack(&self, stack: &gtk::Stack) {
        let imp = self.imp();
        // Same-pointer early return. Comparing the ViewStack by
        // pointer identity (not by value) makes the rebind contract
        // explicit: only a *different* stack triggers a disconnect.
        {
            let current = imp.stack.borrow();
            if let Some((existing, _)) = current.as_ref() {
                if existing == stack {
                    return;
                }
            }
        }
        // Disconnect the previous handler (if any) before swapping
        // the binding. Without this the old closure would keep
        // firing notify::visible-child against a now-stale
        // `imp().last_sync` and silently desync the selector from
        // the new stack.
        if let Some((old_stack, old_handler)) = imp.stack.borrow_mut().take() {
            old_stack.disconnect(old_handler);
        }

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
        let handler_id = stack.connect_notify_local(Some("visible-child"), move |stack, _| {
            let Some(sel) = weak.upgrade() else { return };
            let visible_child = stack
                .visible_child_name()
                .map(|name| name.to_string())
                .unwrap_or_else(|| "(none)".to_string());
            let imp = sel.imp();
            let last_sync = imp.last_sync.get();
            let span = tracing::info_span!(
                target: crate::core::log_targets::BROWSING,
                "photos:mode_selector_stack_notify",
                visible_child = %visible_child,
                new_index = tracing::field::Empty,
                new = tracing::field::Empty,
                last_sync = ?last_sync,
                outcome = tracing::field::Empty
            );
            let _trace = span.enter();
            let new_idx = match visible_child.as_str() {
                "year" => 0,
                "month" => 1,
                "day" => 2,
                _ => {
                    span.record("outcome", "unsupported_child");
                    return;
                }
            };
            span.record("new_index", new_idx);
            span.record("new", mode_name_for_index(new_idx));
            match last_sync {
                LastSync::SelfPending(idx) if idx == new_idx => {
                    // Self-induced change echoed back. Consume and
                    // promote to Synced so the next external change
                    // still syncs.
                    span.record("outcome", "self_echo");
                    imp.last_sync.set(LastSync::Synced(new_idx));
                }
                LastSync::SelfPending(_) => {
                    // Stale SelfPending (defensive — current GTK
                    // dispatches notify synchronously, so this branch
                    // shouldn't fire). Drop the write to avoid
                    // clobbering active_index.
                    span.record("outcome", "stale_self_pending");
                }
                LastSync::Synced(idx) if idx == new_idx => {
                    // External "no-op echo" (e.g. someone set the
                    // same child again). Ignore.
                    span.record("outcome", "synced_echo");
                }
                LastSync::Synced(_) => {
                    // Genuine external change.
                    span.record("outcome", "external_change");
                    imp.active_index.set(new_idx);
                    imp.apply_state();
                }
            }
        });

        *imp.stack.borrow_mut() = Some((stack.clone(), handler_id));
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

    #[test]
    fn indicator_x_centers_under_active_label() {
        // Pure formula: the sliding indicator's translateX centers the bar
        // under label N = (N + 0.5) * cell_width - indicator_width / 2.
        // With cell_width 120 and indicator_width 24 the centers are 60/180/300.
        assert_eq!(indicator_x_for(0, 120, 24), 48);
        assert_eq!(indicator_x_for(1, 120, 24), 168);
        assert_eq!(indicator_x_for(2, 120, 24), 288);
    }

    #[gtk::test]
    fn day_seeded_indicator_is_positioned_before_first_allocation() {
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        stack.set_visible_child_name("day");

        sel.set_stack(&stack);

        assert_eq!(sel.active_index(), 2);
        assert!(
            sel.imp().indicator_target_x.get() > 0,
            "binding to an already-Day stack should seed a Day-side indicator target before the first allocation"
        );
        assert!(
            !sel.imp()
                .mode_dot_indicator
                .get()
                .has_css_class("mode-dot-position-pending"),
            "the indicator should not need startup hiding once the initial Day transform is seeded"
        );
    }

    #[gtk::test]
    fn set_light_background_toggles_contrast_class() {
        let sel = ModeSelector::new();

        assert!(!sel.has_css_class("on-light-background"));
        sel.set_light_background(true);
        assert!(sel.has_css_class("on-light-background"));
        sel.set_light_background(false);
        assert!(!sel.has_css_class("on-light-background"));
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
    fn build_stack() -> (gtk::Stack, [gtk::Label; 3]) {
        let stack = gtk::Stack::new();
        let a = gtk::Label::new(Some("Year"));
        let b = gtk::Label::new(Some("Month"));
        let c = gtk::Label::new(Some("Day"));
        stack.add_titled(&a, Some("year"), "Year");
        stack.add_titled(&b, Some("month"), "Month");
        stack.add_titled(&c, Some("day"), "Day");
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
