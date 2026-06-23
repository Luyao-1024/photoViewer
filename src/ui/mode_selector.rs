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

use crate::core::i18n::tr;
use crate::ui::liquid_glass;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, graphene, gsk};
use libadwaita as adw;

/// Lens displacement strength ∈ [0, 1): how hard the pill edge pulls the
/// backdrop toward its centre. ~0.35 reads as a clear magnifying lens without
/// turning the labels behind into mush.
const GLASS_STRENGTH: f32 = 0.35;

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

    /// Snapshot of the inputs that determine the refracted backdrop. Compared
    /// each tick so we only re-capture when something behind the pill actually
    /// moved (scroll position, pill geometry, or which grid is visible).
    #[derive(Clone, PartialEq)]
    pub struct BackdropSig {
        child: Option<String>,
        vadj: f64,
        pill_w: i32,
        pill_h: i32,
        pill_x: i32,
        pill_y: i32,
    }

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
        pub stack: std::cell::RefCell<Option<(adw::ViewStack, glib::SignalHandlerId)>>,
        /// Refracted backdrop (the lens-warped region of the grid behind the
        /// pill), cached between frames. `None` until the first successful
        /// capture or when offscreen rendering is unavailable.
        pub backdrop_tex: std::cell::RefCell<Option<gdk::Texture>>,
        /// GSK renderer for the surface, lazily created and cached.
        pub renderer: std::cell::RefCell<Option<gsk::Renderer>>,
        /// Set when the backdrop may have changed; consumed by the tick
        /// callback on the next allowed frame.
        pub backdrop_dirty: Cell<bool>,
        /// Frame time (µs) of the last backdrop recompute — bounds staleness
        /// during continuous scrolling (see `tick_check`).
        pub last_recompute_us: Cell<i64>,
        /// Frame time (µs) of the last detected motion under the pill. We wait
        /// for scrolling to *settle* before the (expensive, main-thread)
        /// offscreen capture so scrolling never blocks on it.
        pub last_motion_us: Cell<i64>,
        /// Last observed backdrop signature (see [`BackdropSig`]).
        pub backdrop_sig: std::cell::RefCell<Option<BackdropSig>>,
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
            self.obj().set_labels_i18n();

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

            // Per-frame backdrop change detection for the liquid-glass
            // refraction (see `tick_check`). Cheap when nothing moves; the
            // expensive offscreen capture runs only on change, throttled to
            // ~30fps, so idle scrolling stays smooth.
            self.obj().add_tick_callback(|sel, clock| {
                sel.imp().tick_check(clock);
                glib::ControlFlow::Continue
            });
        }
    }
    impl WidgetImpl for ModeSelector {
        /// Paint the *entire* liquid-glass pill in one place — soft shadow,
        /// refracted backdrop, glass tint, and specular rim — so there is a
        /// single, pixel-aligned shape. An earlier version split the rim
        /// across CSS `box-shadow` and a rounded clip here, and the two
        /// rounded rectangles read as two misaligned pills. The parent (`Box`)
        /// then only lays out the label/dot children on top.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let alloc = obj.allocation();
            let w = alloc.width() as f32;
            let h = alloc.height() as f32;
            if w <= 0.0 || h <= 0.0 {
                self.parent_snapshot(snapshot);
                return;
            }

            let radius = 22.0_f32.min(w / 2.0).min(h / 2.0);
            let on_light = obj.has_css_class("on-light-background");
            let bounds = graphene::Rect::new(0.0, 0.0, w, h);

            // (1) Soft drop shadow: a blurred dark rounded rect, nudged down.
            let sh_bounds = graphene::Rect::new(0.0, 5.0, w, h);
            let sh_rounded = gsk::RoundedRect::from_rect(sh_bounds, radius);
            snapshot.push_blur(12.0);
            snapshot.push_rounded_clip(&sh_rounded);
            snapshot.append_color(
                &gdk::RGBA::new(0.0, 0.0, 0.0, if on_light { 0.18 } else { 0.34 }),
                &sh_bounds,
            );
            snapshot.pop(); // rounded clip
            snapshot.pop(); // blur

            // (2) Refracted backdrop (or a flat fill), clipped to the pill.
            let rounded = gsk::RoundedRect::from_rect(bounds, radius);
            snapshot.push_rounded_clip(&rounded);
            if let Some(tex) = self.backdrop_tex.borrow().as_ref() {
                snapshot.append_texture(tex, &bounds);
                let tint = if on_light { 0.14 } else { 0.10 };
                snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, tint), &bounds);
            } else {
                let a = if on_light { 0.30 } else { 0.22 };
                snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, a), &bounds);
            }

            // (3) Specular rim — bright top edge + faint bottom shade, drawn
            // inside the pill clip so it follows the rounded corners.
            let top = graphene::Rect::new(0.0, 0.0, w, 1.5);
            snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, 0.45), &top);
            let bot = graphene::Rect::new(0.0, h - 2.0, w, 2.0);
            snapshot.append_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.18), &bot);
            snapshot.pop(); // rounded clip

            // (4) Children (labels + dots) on top.
            self.parent_snapshot(snapshot);
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

        // --- Liquid-glass backdrop refraction -------------------------------

        /// The currently-visible grid (the surface the pill floats over) and
        /// its ViewStack name — used both for capture and change detection.
        fn current_grid(&self) -> Option<(gtk::Widget, String)> {
            let stack = self.stack.borrow();
            let (stack, _) = stack.as_ref()?;
            let name = stack.visible_child_name()?;
            let child = stack.visible_child()?;
            Some((child, name.to_string()))
        }

        /// The MediaGrid's vertical scroll adjustment. Its first child is the
        /// `GtkScrolledWindow` (see media-grid.blp); watching the value detects
        /// scrolling under the pill.
        fn grid_vadjustment(grid: &gtk::Widget) -> Option<gtk::Adjustment> {
            let scroller = grid.first_child()?.downcast::<gtk::ScrolledWindow>().ok()?;
            Some(scroller.vadjustment())
        }

        /// Build the change-detection signature for this frame.
        fn compute_sig(&self, grid: Option<&gtk::Widget>, name: &Option<String>) -> BackdropSig {
            let obj = self.obj();
            let alloc = obj.allocation();
            let (vadj, px, py) = match grid {
                Some(g) => {
                    let v = Self::grid_vadjustment(g).map(|a| a.value()).unwrap_or(0.0);
                    let (x, y) = obj
                        .translate_coordinates(g, 0.0, 0.0)
                        .map(|(x, y)| (x.round() as i32, y.round() as i32))
                        .unwrap_or((0, 0));
                    (v, x, y)
                }
                None => (0.0, 0, 0),
            };
            BackdropSig {
                child: name.clone(),
                vadj,
                pill_w: alloc.width(),
                pill_h: alloc.height(),
                pill_x: px,
                pill_y: py,
            }
        }

        /// Per-frame: detect whether the backdrop moved and, if so (throttled
        /// to ~30fps), re-capture + re-displace it outside the snapshot pass.
        fn tick_check(&self, clock: &gdk::FrameClock) {
            let (grid, name) = match self.current_grid() {
                Some((g, n)) => (Some(g), Some(n)),
                None => (None, None),
            };
            let now = clock.frame_time();
            let sig = self.compute_sig(grid.as_ref(), &name);
            if self.backdrop_sig.borrow().as_ref() != Some(&sig) {
                *self.backdrop_sig.borrow_mut() = Some(sig);
                self.last_motion_us.set(now);
                self.backdrop_dirty.set(true);
            }
            if !self.backdrop_dirty.get() {
                return;
            }
            // The offscreen capture (`render_texture` of the whole grid) is
            // expensive and runs on the main thread, so we do NOT capture while
            // scrolling: wait for motion to settle (~150ms idle). A staleness
            // cap (~500ms) still refreshes during non-stop scrolling, and the
            // very first capture (no texture yet) fires immediately so the
            // glass shows refraction as soon as the grid is ready, not after a
            // settle delay.
            let first = self.backdrop_tex.borrow().is_none();
            let settled = now - self.last_motion_us.get() >= 150_000;
            let stale = now - self.last_recompute_us.get() >= 500_000;
            if !first && !settled && !stale {
                return;
            }
            self.backdrop_dirty.set(false);
            self.last_recompute_us.set(now);
            if !self.recompute_backdrop(grid.as_ref()) {
                // Capture failed (e.g. grid not laid out yet, or offscreen) —
                // retry next tick so the first display still gets refraction.
                self.backdrop_dirty.set(true);
            }
        }

        /// Capture the pill's rectangle of the grid behind it, lens-displace
        /// it, cache the result, and queue a redraw. Returns whether a texture
        /// was produced (`tick_check` retries on `false` so the first capture
        /// succeeds once the grid is laid out). No-op offscreen.
        fn recompute_backdrop(&self, grid: Option<&gtk::Widget>) -> bool {
            let obj = self.obj();
            let Some(grid) = grid else {
                return false;
            };
            // Lazily create + cache a renderer for this surface.
            if self.renderer.borrow().is_none() {
                if let Some(surface) = grid.native().and_then(|n| n.surface()) {
                    if let Some(r) = gsk::Renderer::for_surface(&surface) {
                        *self.renderer.borrow_mut() = Some(r);
                    }
                }
            }
            let renderer_guard = self.renderer.borrow();
            let Some(renderer) = renderer_guard.as_ref() else {
                return false;
            };
            let alloc = obj.allocation();
            let Some((px, py)) = obj.translate_coordinates(grid, 0.0, 0.0) else {
                return false;
            };
            let rect = graphene::Rect::new(
                px as f32,
                py as f32,
                alloc.width() as f32,
                alloc.height() as f32,
            );
            if let Some(tex) = liquid_glass::refract_region(grid, &rect, renderer, GLASS_STRENGTH) {
                *self.backdrop_tex.borrow_mut() = Some(tex.upcast::<gdk::Texture>());
                obj.queue_draw();
                true
            } else {
                false
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
        if idx > 2 {
            return;
        }
        let imp = self.imp();
        if imp.active_index.get() == idx {
            return;
        }
        imp.active_index.set(idx);
        imp.apply_state();
        if let Some((stack, _)) = imp.stack.borrow().as_ref() {
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
    /// Idempotent for the same `stack` (no-op on repeat calls with
    /// the same pointer). Rebinding to a different stack is
    /// supported: the previous stack's `notify::visible-child`
    /// subscription is disconnected before the new one is installed,
    /// so no stale handler can fire against the new binding.
    pub fn set_stack(&self, stack: &adw::ViewStack) {
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
    fn build_stack() -> (adw::ViewStack, [gtk::Label; 3]) {
        let stack = adw::ViewStack::new();
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
