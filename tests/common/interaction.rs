//! Real-input harness: drive the UI the way a pointer and a keyboard do.
//!
//! ## Why this exists
//!
//! `emit_by_name("clicked")` is not a click. It asks a widget to run its handler
//! no matter what is painted on top of it, so it cannot see anything that is
//! wrong *between* the pointer and the control: a transparent overlay, a filled
//! `Gtk.Overlay` child, a stale allocation, a collapsed revealer that still
//! reserves its old box. That gap is how the viewer's Previous/Next pair could
//! stop responding while a whole suite of signal-level tests stayed green.
//!
//! It hides the mirror-image problem too: chrome a user cannot reach at all. A
//! batch button living in an un-revealed `Gtk.Revealer` still answers an emitted
//! `clicked`, so a test can "click Select All" in a state where the app never
//! showed it — and can thereby miss a mode flag that left the whole bar switched
//! off.
//!
//! Every interaction here does what the input path does:
//!
//! 1. require the widget to be really laid out (a presented window and a mapped
//!    allocation — not `compute_bounds` on an unrealized tree);
//! 2. ask GTK where a pointer at that position actually lands — `gtk_widget_pick`,
//!    the same hit test a real event runs — and assert it resolves to the control
//!    under test or to one of its descendants;
//! 3. hand press and release to the gesture controllers that control's click path
//!    runs *through*, at the pointer position in each controller's own coordinate
//!    space, because containers resolve the target item from those coordinates;
//! 4. leave the caller to assert the user-visible result.
//!
//! ## Which widgets a press passes through
//!
//! Measured against GTK 4.22 and libadwaita 1.6, not guessed:
//!
//! | control | what owns the click |
//! |---|---|
//! | `Gtk.Button`, a `ModeSelector` label cell, a sidebar album row | the widget itself |
//! | a `Gtk.GridView` / `Gtk.ListView` item | the `GtkListItemWidget` wrapper, *not* the tile |
//! | a `Gtk.ListBox` row | the `GtkListBox` |
//! | a `Gtk.FlowBox` child | the `Gtk.FlowBox` |
//! | an `AdwActionRow` | its own gesture drives only its pressed visuals; `activated` comes from the enclosing `GtkListBox` resolving the press coordinates, so the press must propagate |
//! | an `AdwAlertDialog` response | a real `Gtk.Button` inside the dialog's own surface |
//!
//! A press therefore propagates innermost → outermost until it reaches a widget
//! that actually activates something ([`Ui::ends_the_line`]), and stops there the
//! way GTK stops once a controller claims the sequence. Delivering to only the
//! innermost controller silently misses row activation; delivering all the way to
//! the window fires handlers a real press never reaches.
//!
//! ## Surfaces
//!
//! A `Ui` is bound to one *pick root*: the widget whose coordinate space the
//! pointer is expressed in. `AdwAlertDialog` and `AdwDialog` are their own
//! surfaces — `compute_bounds()` across a surface boundary returns `None` — so
//! [`Ui::for_widget`] walks to the top of the tree and binds to whatever surface
//! really holds the widget. Guessing wrong here does not look like a wrong
//! coordinate; it looks like a control with no position at all.
//!
//! ## Known fidelity limits
//!
//! - Keyboard input is emitted on the production window router rather than
//!   injected as a `GdkEvent`; GTK exposes no public synthetic-event API to Rust.
//!   The router, the binding table and its gating are still exercised. A press is
//!   always paired with a release, because the router latches a combo until
//!   release so auto-repeat is not read as fresh presses — press-only models a key
//!   held forever and swallows every later press of that key.
//! - Text goes in through [`Ui::type_search`] / [`Ui::type_entry`]: character by
//!   character through the editable's own insert path, then the completion signal
//!   the key path would have emitted. GTK 4.22 emits `search-changed` only from
//!   that path, so the keystroke itself is the one measurable departure.
//! - Because a gesture signal is emitted directly, the controller has no current
//!   `GdkEvent`, so GTK prints `gdk_event_get_modifier_state` criticals. That is
//!   noise, not a wrong answer: it does not change which widget the coordinates
//!   resolve to.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::time::{Duration, Instant};

/// A pointer and keyboard bound to one pick root.
#[derive(Clone)]
pub struct Ui {
    root: gtk::Widget,
    root_label: String,
}

impl Ui {
    /// Drive the main window's content.
    pub fn for_window(window: &impl IsA<gtk::Widget>) -> Self {
        Self::for_surface(window, "main window")
    }

    /// Drive content inside a named surface — an `AdwDialog` or `AdwAlertDialog`.
    pub fn for_surface(surface: &impl IsA<gtk::Widget>, label: &str) -> Self {
        Self {
            root: surface.as_ref().clone(),
            root_label: label.to_string(),
        }
    }

    /// Bind to whichever surface actually contains `widget`.
    ///
    /// Walks to the top of the widget tree and uses that as the pick root, which
    /// settles the dialog case without every caller having to know whether the
    /// dialog it opened shares the window's surface or has its own.
    pub fn for_widget(widget: &impl IsA<gtk::Widget>) -> Self {
        let mut root = widget.as_ref().clone();
        let mut depth = 0usize;
        while let Some(parent) = root.parent() {
            root = parent;
            depth += 1;
            // A cycle would mean a corrupted tree; fail rather than spin.
            assert!(depth < 200, "the widget ancestry is unreasonably deep");
        }
        let label = if root.is::<gtk::Window>() {
            "main window"
        } else {
            "dialog surface"
        };
        Self {
            root,
            root_label: label.to_string(),
        }
    }

    pub fn root(&self) -> &gtk::Widget {
        &self.root
    }

    pub fn root_label(&self) -> &str {
        &self.root_label
    }

    // ---------------------------------------------------------------------
    // Hit testing
    // ---------------------------------------------------------------------

    /// Where a pointer would have to be, in this surface's coordinate space, to sit
    /// at the centre of `widget`. `None` means there is no such position: a widget
    /// that is not mapped has no pointer position, and its last allocation is
    /// stale — `compute_bounds` will happily return coordinates belonging to
    /// whatever sits there now. "Nothing is there" is the honest answer for a
    /// hidden, folded, or torn-down control.
    pub fn pointer_at_center_of(&self, widget: &impl IsA<gtk::Widget>) -> Option<(f64, f64)> {
        let widget = widget.as_ref();
        if !widget.is_mapped() {
            return None;
        }
        let rect = widget.compute_bounds(&self.root)?;
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        Some((
            f64::from(rect.x()) + f64::from(rect.width()) / 2.0,
            f64::from(rect.y()) + f64::from(rect.height()) / 2.0,
        ))
    }

    /// The widget GTK hands a pointer at `(x, y)` to.
    pub fn pick(&self, x: f64, y: f64) -> Option<gtk::Widget> {
        self.root.pick(x, y, gtk::PickFlags::empty())
    }

    /// What a pointer aimed at the centre of `widget` actually lands on.
    pub fn target_of(&self, widget: &impl IsA<gtk::Widget>) -> Option<gtk::Widget> {
        let (x, y) = self.pointer_at_center_of(widget)?;
        self.pick(x, y)
    }

    /// Describe a pick result for a failure message: enough to tell the reader
    /// which widget stole the pointer.
    pub fn describe(widget: Option<&gtk::Widget>) -> String {
        let Some(w) = widget else {
            return "nothing".to_string();
        };
        let classes = w
            .css_classes()
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(".");
        let name = w.widget_name();
        // GTK reports the type name here unless the template gave the widget a
        // `name:`, which would then only repeat what is already printed.
        let name = if name.is_empty() || name == w.type_().name() {
            String::new()
        } else {
            format!(" #{name}")
        };
        format!("<{}{name} class=[{classes}]>", w.type_().name())
    }

    fn describe_widget(w: &gtk::Widget) -> String {
        Self::describe(Some(w))
    }

    /// The picked widget's ancestry, innermost first — enough to tell which
    /// container actually took the pointer.
    fn chain(widget: &gtk::Widget) -> String {
        let mut out = Vec::new();
        let mut current = Some(widget.clone());
        while let Some(w) = current {
            out.push(Self::describe_widget(&w));
            current = w.parent();
        }
        out.join(" < ")
    }

    /// `target` is where the pointer landed; `intended` is the control named. The
    /// pointer covers the control when it hit the control itself, something drawn
    /// inside it, or an ancestor wrapper that owns its click gesture (the
    /// `GtkListItemWidget` of a grid tile, the `GtkListBox` of a row).
    fn covers(&self, intended: &gtk::Widget, target: &gtk::Widget) -> bool {
        *target == *intended || intended.is_ancestor(target) || target.is_ancestor(intended)
    }

    // ---------------------------------------------------------------------
    // Reachability
    // ---------------------------------------------------------------------

    /// The reachability contract: a sensitive control that is on screen must be the
    /// widget GTK hands a pointer at its own centre to — directly, through one of
    /// its descendants, or through a container that owns its click. A control that
    /// is visible but shadowed by an invisible container fails here, and that is
    /// the class of bug this harness exists to catch.
    ///
    /// A short settle window is allowed first, because stack and revealer
    /// transitions really do hand the pointer to the outgoing page for a few
    /// frames: a `GtkStack` crossfade keeps the fading page hit-testable, so the
    /// first pointer after a page swap lands on the old page. Waiting for the
    /// transition to finish matches what a user who aims again gets, and the
    /// assertion still fails if the control never becomes reachable.
    pub fn assert_reachable(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        self.assert_reachable_within(widget, label, Duration::from_millis(1_500));
    }

    /// As [`Self::assert_reachable`] with a caller-chosen settle window. Use a
    /// longer one after an animation the scenario can observe but not cancel.
    pub fn assert_reachable_within(
        &self,
        widget: &impl IsA<gtk::Widget>,
        label: &str,
        settle: Duration,
    ) {
        let widget = widget.as_ref();
        assert!(
            widget.is_sensitive(),
            "{label} should be sensitive while it is on screen"
        );
        let deadline = Instant::now() + settle;
        loop {
            let target = self.target_of(widget);
            if let Some(found) = &target {
                if self.covers(widget, found) {
                    return;
                }
            }
            if Instant::now() >= deadline {
                match target {
                    Some(found) => panic!(
                        "a pointer at the centre of {label} must reach {label}, but GTK picked {} \
                         — chain: {}",
                        Self::describe(Some(&found)),
                        Self::chain(&found)
                    ),
                    None => {
                        let centre = self
                            .pointer_at_center_of(widget)
                            .map(|(x, y)| format!("({x:.0}, {y:.0})"))
                            .unwrap_or_else(|| "no position".to_string());
                        panic!(
                            "a pointer at the centre of {label} picked nothing. The control \
                             reports centre {} in {}'s coordinate space, and that root measures \
                             {}x{}, so the point falls outside what the surface shows: the \
                             control is scrolled out of view, folded, or covered.",
                            centre,
                            self.root_label,
                            self.root.width(),
                            self.root.height()
                        )
                    }
                }
            }
            self.pump(Duration::from_millis(60));
        }
    }

    /// Assert the opposite of reachability: this control cannot take a pointer right
    /// now, because it is hidden, folded, dimmed, or covered. Proves chrome that is
    /// meant to be inert really is.
    pub fn assert_not_reachable(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        let Some(target) = self.target_of(widget) else {
            return;
        };
        assert!(
            !self.covers(widget.as_ref(), &target) || !widget.as_ref().is_sensitive(),
            "{label} should not take a pointer right now, but GTK picked {} and it reports itself \
             sensitive",
            Self::describe(Some(&target))
        );
    }

    // ---------------------------------------------------------------------
    // Clicking
    // ---------------------------------------------------------------------

    /// A real primary-button click: resolve the pointer target first, then hand the
    /// press and release to the gesture controllers the target's own click path runs
    /// through. Panics if the pointer would not have reached `widget`.
    pub fn click(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        self.assert_reachable(widget, label);
        self.deliver(widget.as_ref(), label, 1);
    }

    /// Two real primary clicks with the main loop not running between them — what a
    /// double click actually looks like to the widget.
    ///
    /// This matters for the "rapid double activation must push exactly one page"
    /// contracts: the guard they test is a flag the main loop clears once the push
    /// lands, so clicking with a pump in between would measure two well-separated
    /// single clicks and pass for the wrong reason.
    pub fn double_click(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        self.assert_reachable(widget, label);
        self.press_release(widget.as_ref(), label, 1);
        self.press_release(widget.as_ref(), label, 1);
        self.pump(Duration::from_millis(200));
    }

    /// A real secondary-button (right) click — how the media grids open their
    /// context menu and how an album row is managed.
    pub fn right_click(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        self.assert_reachable(widget, label);
        self.deliver(widget.as_ref(), label, 3);
    }

    /// Click a control that is *not* expected to be reachable — a dimmed arrow,
    /// folded chrome, a cluster another panel has hidden. Reachability is
    /// deliberately not asserted: the contract is that the press cannot reach a
    /// live handler, so the caller asserts nothing changed. Returns whether a press
    /// was delivered at all; "no" is the expected answer for a control that is not
    /// on screen.
    pub fn try_click_even_if_inert(
        &self,
        widget: &impl IsA<gtk::Widget>,
        label: &str,
    ) -> Result<bool, String> {
        let widget = widget.as_ref();
        let Some(target) = self.target_of(widget) else {
            return Ok(false);
        };
        if !self.covers(widget, &target) || !widget.is_sensitive() {
            return Ok(false);
        }
        let (exact, catch_all) = self.gesture_chain(&target, widget, 1);
        if exact.is_empty() && catch_all.is_empty() {
            return Ok(false);
        }
        self.deliver(widget, label, 1);
        Ok(true)
    }

    /// Deliver press + release and let the resulting work settle.
    fn deliver(&self, intended: &gtk::Widget, label: &str, button: u32) {
        self.press_release(intended, label, button);
        self.pump(Duration::from_millis(120));
    }

    /// Deliver press + release for `button` along the gesture chain that owns the
    /// click path for `intended`, at the pointer position in each owner's own
    /// coordinate space.
    fn press_release(&self, intended: &gtk::Widget, label: &str, button: u32) {
        let target = self
            .target_of(intended)
            .unwrap_or_else(|| panic!("{label} has no pointer position to press at"));
        let (exact, catch_all) = self.gesture_chain(&target, intended, button);
        let chain = if exact.is_empty() { catch_all } else { exact };
        assert!(
            !chain.is_empty(),
            "no {:?}-button gesture controller owns the click path from {} up to {label} or its \
             surface",
            button,
            Self::describe_widget(&target)
        );
        let (root_x, root_y) = self
            .pointer_at_center_of(intended)
            .expect("the control has a pointer position by this point");
        for (index, (owner, gesture)) in chain.iter().enumerate() {
            let origin = owner
                .compute_bounds(&self.root)
                .map(|rect| (f64::from(rect.x()), f64::from(rect.y())))
                .unwrap_or((0.0, 0.0));
            let x = root_x - origin.0;
            let y = root_y - origin.1;
            gesture.emit_by_name::<()>("pressed", &[&1i32, &x, &y]);
            gesture.emit_by_name::<()>("released", &[&1i32, &x, &y]);
            // Stop where GTK stops: once the press has reached the widget that
            // activates, handing it further up fires a second handler that a real
            // press never reaches, because the first controller claims the sequence.
            if Self::ends_the_line(owner) || index + 1 == chain.len() {
                break;
            }
        }
    }

    /// The gesture controllers a pointer at `target` runs through on the way to
    /// `intended`, innermost first — the chain a real event propagates along.
    ///
    /// Only owners the intended control actually routes through are kept: the
    /// control itself, one of its descendants, or one of its ancestors. Anything
    /// else means the pointer landed elsewhere, which is a failure rather than
    /// something to route around.
    ///
    /// Controllers whose button filter is `0` ("any button") come back separately:
    /// they are surface-level catch-alls. Real propagation does reach them, but a
    /// press delivered there unconditionally drives input into whatever owns the
    /// surface rather than into the control under test, so they are a last resort.
    fn gesture_chain(
        &self,
        target: &gtk::Widget,
        intended: &gtk::Widget,
        button: u32,
    ) -> (
        Vec<(gtk::Widget, gtk::GestureClick)>,
        Vec<(gtk::Widget, gtk::GestureClick)>,
    ) {
        let mut exact = Vec::new();
        let mut catch_all = Vec::new();
        let mut current = Some(target.clone());
        while let Some(widget) = current {
            if self.covers(intended, &widget) {
                for gesture in click_gestures(&widget) {
                    if gesture.button() == button {
                        exact.push((widget.clone(), gesture));
                    } else if gesture.button() == 0 {
                        catch_all.push((widget.clone(), gesture));
                    }
                }
            }
            if widget == self.root {
                break;
            }
            current = widget.parent();
        }
        (exact, catch_all)
    }

    /// Whether a press that reached `widget` is the end of the line: it is itself
    /// the thing GTK activates, so nothing further needs to see the press.
    ///
    /// A row wrapper, a list box, a flow box, a grid view and a button all resolve
    /// the target from the press coordinates and act. Anything else — a `Gtk.Box`
    /// label row, an `AdwActionRow` that owns a gesture only for its pressed
    /// visuals — has to pass the press on to the container above, because that
    /// container emits the activation the control exists to cause.
    /// `GtkListBoxRow` and `GtkFlowBoxChild` are deliberately *not* on this list.
    /// An `AdwActionRow` is a `GtkListBoxRow` subclass and owns a primary
    /// `GtkGestureClick`, but measured against a real press that gesture only
    /// drives its pressed visuals — `activated` never fires from it. The row's
    /// activation comes from the enclosing `GtkListBox`'s controller resolving the
    /// press coordinates, so the press has to keep travelling.
    fn ends_the_line(widget: &gtk::Widget) -> bool {
        widget.downcast_ref::<gtk::Button>().is_some()
            || widget.downcast_ref::<gtk::ListBox>().is_some()
            || widget.downcast_ref::<gtk::ListView>().is_some()
            || widget.downcast_ref::<gtk::GridView>().is_some()
            || widget.downcast_ref::<gtk::ColumnView>().is_some()
            || widget.downcast_ref::<gtk::FlowBox>().is_some()
            || widget.downcast_ref::<gtk::Switch>().is_some()
            || widget.downcast_ref::<gtk::CheckButton>().is_some()
            || widget.downcast_ref::<gtk::ToggleButton>().is_some()
            || widget.downcast_ref::<gtk::Expander>().is_some()
            || widget.downcast_ref::<gtk::ScaleButton>().is_some()
            // The item wrapper a GridView/ListView activates through. It is not
            // exported as a Rust type, so it can only be matched by name.
            || widget.type_().name() == "GtkListItemWidget"
    }

    // ---------------------------------------------------------------------
    // Layout settling
    // ---------------------------------------------------------------------

    /// Let a control become measurable. Revealers and crossfades animate, so a hit
    /// test taken mid-transition measures a half-faded allocation; this waits for
    /// the widget to be mapped with a non-zero box, then lets the frame settle.
    pub fn wait_laid_out(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        assert!(
            self.wait_until(Duration::from_secs(10), || self
                .pointer_at_center_of(widget)
                .is_some()),
            "{label} should be laid out and on screen within 10s"
        );
        self.pump(Duration::from_millis(250));
    }

    /// As [`Self::wait_laid_out`] but without failing, for controls that may
    /// legitimately never appear.
    pub fn wait_until_laid_out(&self, widget: &impl IsA<gtk::Widget>, timeout: Duration) -> bool {
        self.wait_until(timeout, || self.pointer_at_center_of(widget).is_some())
    }

    // ---------------------------------------------------------------------
    // Typing
    // ---------------------------------------------------------------------

    /// Put `text` into a search field the way typing does, then fire the completion
    /// signal a keystroke fires.
    pub fn type_search(&self, entry: &gtk::SearchEntry, text: &str) {
        self.insert_chars(entry, text);
        entry.emit_by_name::<()>("changed", &[]);
        entry.emit_by_name::<()>("search-changed", &[]);
        self.pump(Duration::from_millis(80));
    }

    /// Put `text` into a plain entry the way typing does. Commit it with
    /// [`Self::activate_entry`], the way Enter would.
    pub fn type_entry(&self, entry: &gtk::Entry, text: &str) {
        self.insert_chars(entry, text);
        entry.emit_by_name::<()>("changed", &[]);
        self.pump(Duration::from_millis(80));
    }

    /// Insert `text` one character at a time through the editable's own insert
    /// path — the call a key handler makes.
    ///
    /// Measured against GTK 4.22: neither `set_text` nor `insert_text` nor
    /// `activate` makes a `GtkSearchEntry` emit `search-changed`, because that
    /// signal comes out of the key-handler path rather than the buffer change, and
    /// Rust has no public API to fabricate the `GdkEvent` that path needs. So the
    /// callers above emit the signal the keystroke would have emitted. The
    /// character insertion itself is real; this is the harness's one measurable
    /// departure from an event-level test.
    fn insert_chars<E>(&self, editable: &E, text: &str)
    where
        E: gtk4::prelude::EditableExt,
    {
        editable.set_text("");
        let mut position = 0i32;
        for ch in text.chars() {
            let glyph = ch.to_string();
            editable.insert_text(&glyph, &mut position);
            position += 1;
        }
    }

    /// Commit an entry the way the Enter key does.
    pub fn activate_entry<W>(&self, entry: &W)
    where
        W: IsA<gtk::Widget>,
    {
        entry.emit_by_name::<()>("activate", &[]);
        self.pump(Duration::from_millis(120));
    }

    // ---------------------------------------------------------------------
    // Keyboard
    // ---------------------------------------------------------------------

    /// One physical key press through the production capture-phase router, so the
    /// binding table and the router's own gating are exercised rather than bypassed.
    /// Returns whether the router handled it.
    pub fn press_key(
        &self,
        window: &impl IsA<gtk::Widget>,
        key: gtk::gdk::Key,
        modifiers: gtk::gdk::ModifierType,
    ) -> bool {
        self.key_gesture(window, key, modifiers, 0).dispatched > 0
    }

    /// A whole key gesture: the initial press, `repeats` auto-repeats, then the
    /// release — what a real hold looks like.
    ///
    /// Nothing is pumped inside the gesture on purpose. Dispatching a binding is
    /// synchronous, so the outcome is fully determined by the router's own gating;
    /// pumping would let unrelated queued input land mid-gesture and turn a
    /// deterministic assertion into a race.
    pub fn key_gesture(
        &self,
        window: &impl IsA<gtk::Widget>,
        key: gtk::gdk::Key,
        modifiers: gtk::gdk::ModifierType,
        repeats: usize,
    ) -> KeyGesture {
        let gesture = self.key_gesture_unsettled(window, key, modifiers, repeats);
        self.pump(Duration::from_millis(150));
        gesture
    }

    /// The same gesture without letting the main loop run afterwards.
    ///
    /// Needed wherever a real pointer event arriving in between would change the
    /// answer: anything that watches for pointer stillness anchors on the last
    /// accepted movement, and this environment delivers real motion events at
    /// unpredictable moments, so a pump between "establish the baseline" and
    /// "assert" makes the assertion a coin flip.
    pub fn key_gesture_unsettled(
        &self,
        window: &impl IsA<gtk::Widget>,
        key: gtk::gdk::Key,
        modifiers: gtk::gdk::ModifierType,
        repeats: usize,
    ) -> KeyGesture {
        let controller = keyboard_router(window.as_ref());
        let mut dispatched = 0usize;
        let mut press = || -> bool {
            let handled: bool = controller.emit_by_name("key-pressed", &[&key, &0_u32, &modifiers]);
            if handled {
                dispatched += 1;
            }
            handled
        };
        let handled_first = press();
        for _ in 0..repeats {
            press();
        }
        controller.emit_by_name::<()>("key-released", &[&key, &0_u32, &modifiers]);
        KeyGesture {
            dispatched,
            handled_first,
        }
    }

    /// Deliver one pointer motion event at surface coordinates, the way a real
    /// pointer position reaches a page-level motion watcher.
    pub fn pointer_motion(&self, watched: &impl IsA<gtk::Widget>, x: f64, y: f64) {
        let motion = watched
            .as_ref()
            .observe_controllers()
            .snapshot()
            .into_iter()
            .find_map(|c| c.downcast::<gtk::EventControllerMotion>().ok())
            .expect("the widget should watch pointer motion");
        motion.emit_by_name::<()>("motion", &[&x, &y]);
    }

    // ---------------------------------------------------------------------
    // Main loop
    // ---------------------------------------------------------------------

    /// Run the main loop for a fixed wall-clock window. GTK is single-threaded; a
    /// UX binary drives one shell at a time, serially.
    pub fn pump(&self, duration: Duration) {
        let ctx = glib::MainContext::default();
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            while ctx.iteration(false) {}
            std::thread::sleep(Duration::from_millis(5));
        }
        while ctx.iteration(false) {}
    }

    /// Run the main loop until `condition` holds or `timeout` passes. Every effect
    /// that crosses a thread — a decode, a DB write, an off-screen fetch — lands
    /// asynchronously, so results are waited for rather than assumed.
    pub fn wait_until(&self, timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let ctx = glib::MainContext::default();
        while Instant::now() < deadline {
            while ctx.iteration(false) {}
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        while ctx.iteration(false) {}
        condition()
    }
}

/// What one key gesture did.
pub struct KeyGesture {
    /// How many of the gesture's presses the router actually dispatched. One
    /// physical press must dispatch at most one action.
    pub dispatched: usize,
    /// Whether the initial press was handled.
    pub handled_first: bool,
}

/// Every `GtkGestureClick` installed directly on `widget`, in registration order.
fn click_gestures(widget: &gtk::Widget) -> Vec<gtk::GestureClick> {
    widget
        .observe_controllers()
        .snapshot()
        .into_iter()
        .filter_map(|c| c.downcast::<gtk::GestureClick>().ok())
        .collect()
}

/// The production keyboard router installed on a `MainWindow`.
pub fn keyboard_router(window: &gtk::Widget) -> gtk::EventControllerKey {
    window
        .observe_controllers()
        .snapshot()
        .into_iter()
        .find_map(|c| c.downcast::<gtk::EventControllerKey>().ok())
        .filter(|c| c.name().as_deref() == Some("photo-viewer-keyboard-router"))
        .expect("the window should install the production keyboard router")
}

// ---------------------------------------------------------------------------
// Widget-tree lookup
// ---------------------------------------------------------------------------

/// Every descendant of `root` of type `T`, in tree order, `root` included.
pub fn descendants<T>(root: &impl IsA<gtk::Widget>) -> Vec<T>
where
    T: IsA<gtk::Widget> + glib::types::StaticType + Clone,
{
    let mut matches = Vec::new();
    collect(root.as_ref(), &mut matches);
    matches
}

fn collect<T>(widget: &gtk::Widget, out: &mut Vec<T>)
where
    T: IsA<gtk::Widget> + glib::types::StaticType + Clone,
{
    if let Some(found) = widget.downcast_ref::<T>() {
        out.push(found.clone());
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        collect(&w, out);
        child = w.next_sibling();
    }
}

/// The first descendant of `root` of type `T`, `root` included.
pub fn find_descendant<T>(root: &impl IsA<gtk::Widget>) -> Option<T>
where
    T: IsA<gtk::Widget> + glib::object::ObjectType,
{
    let root = root.as_ref();
    if let Some(found) = root.downcast_ref::<T>() {
        return Some(found.clone());
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = find_descendant::<T>(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

/// Wait for a descendant of type `T` to appear — a dialog, a toast, a realized
/// tile — and return it.
pub fn wait_for_descendant<T>(root: &impl IsA<gtk::Widget>, timeout: Duration) -> Option<T>
where
    T: IsA<gtk::Widget> + glib::object::ObjectType,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while glib::MainContext::default().iteration(false) {}
        if let Some(found) = find_descendant::<T>(root) {
            return Some(found);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    find_descendant::<T>(root)
}

/// A button by its visible label. This is how a test names the control a user
/// reads: "Move to Trash", not "the second button in the response area".
pub fn find_button_with_label(root: &impl IsA<gtk::Widget>, label: &str) -> Option<gtk::Button> {
    descendants::<gtk::Button>(root)
        .into_iter()
        .find(|b| b.label().as_deref() == Some(label))
}

/// A button by a CSS class the template gives it, for icon-only controls whose
/// identity is their styling.
pub fn find_button_with_css(root: &impl IsA<gtk::Widget>, css_class: &str) -> Option<gtk::Button> {
    descendants::<gtk::Button>(root)
        .into_iter()
        .find(|b| b.has_css_class(css_class))
}

/// Wait for a labelled button to appear, then return it — a context menu item, a
/// dialog response, a toast action.
pub fn wait_for_button_with_label(
    root: &impl IsA<gtk::Widget>,
    label: &str,
    timeout: Duration,
) -> Option<gtk::Button> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while glib::MainContext::default().iteration(false) {}
        if let Some(button) = find_button_with_label(root, label) {
            return Some(button);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    find_button_with_label(root, label)
}

/// A `gtk::Label` under `root` whose text contains `needle`.
///
/// This is how a scenario names the thing a user reads — the album called
/// `second-album`, the Year cell of a segmented control — and then aims at it.
/// Clicking a label works because the harness walks up from whatever the pointer
/// hit to the gesture that owns that point, so the label's own row or cell still
/// receives the press.
pub fn find_label_containing(root: &impl IsA<gtk::Widget>, needle: &str) -> Option<gtk::Label> {
    descendants::<gtk::Label>(root)
        .into_iter()
        .find(|l| l.text().as_str().contains(needle))
}

/// Wait for a label containing `needle` to appear.
pub fn wait_for_label_containing(
    root: &impl IsA<gtk::Widget>,
    needle: &str,
    timeout: Duration,
) -> Option<gtk::Label> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while glib::MainContext::default().iteration(false) {}
        if let Some(label) = find_label_containing(root, needle) {
            return Some(label);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    find_label_containing(root, needle)
}

/// The `nth` direct child of `parent`, the way a template's child order reads.
pub fn nth_child(parent: &impl IsA<gtk::Widget>, index: usize) -> Option<gtk::Widget> {
    let mut current = parent.as_ref().first_child();
    for _ in 0..index {
        current = current?.next_sibling();
    }
    current
}

/// Whether the widget, or anything inside it, owns keyboard focus. Composite
/// widgets such as `GtkSearchEntry` put focus on an internal text widget, so
/// `has_focus()` on the public wrapper is not sufficient.
pub fn contains_focus(widget: &gtk::Widget) -> bool {
    let Some(mut focus) = widget.root().and_then(|root| root.focus()) else {
        return false;
    };
    loop {
        if focus == *widget {
            return true;
        }
        let Some(parent) = focus.parent() else {
            return false;
        };
        focus = parent;
    }
}
