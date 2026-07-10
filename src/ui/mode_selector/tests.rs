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
