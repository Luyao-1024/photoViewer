//! VS Code-style smooth wheel scrolling for `GtkScrolledWindow` surfaces.
//!
//! GTK4 scrolls one wheel notch by teleporting the vertical adjustment: the
//! `kinetic-scrolling` property only applies to touch input, and discrete
//! wheel events (`GDK_SCROLL_UNIT_WHEEL`) get no easing at all. Discrete
//! notches are therefore consumed on a capture-phase scroll controller and
//! animated instead. Touchpad (surface-unit) deltas are never consumed — GTK
//! already gives them native smooth handling.
//!
//! Two interchangeable glide models:
//!
//! * *Eased* (default): a faithful port of VS Code's editor smooth
//!   scrolling. Each notch extends an accumulated target, and the viewport
//!   eases from its current position to that target over a fixed
//!   [`WHEEL_GLIDE_DURATION_MS`] window with an ease-out cubic curve,
//!   re-anchored at the live position whenever a new notch arrives mid-flight
//!   (VS Code's `Scrollable.combine`). It lands exactly on the target, keeps
//!   the total distance at exactly N detent steps for N notches, and adds no
//!   momentum — every notch is fully delivered within the fixed window, which
//!   is what makes VS Code feel connected and lossless.
//! * *Momentum* (`set_momentum_glide`): macOS-style velocity stacking, where
//!   fast flicks carry far beyond their notch sum. Kept as a tuning
//!   alternative; it is not the shipped feel.
//!
//! The per-notch distance follows GTK's own detent step (`pow(page_size,
//! 2/3)` in gtkscrolledwindow.c) with a 125px floor — VS Code's effective
//! per-notch distance on Linux — so a lone notch never travels less than the
//! floor.

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Process-wide kill switch for wheel glide interception. Hydrated from the
/// persisted `smooth_scrolling` preference at app startup and flipped live by
/// the settings toggle. While off, [`SmoothScroller::handle_scroll`] falls
/// through to GTK's native wheel handling everywhere at once, with no
/// per-scroller bookkeeping.
static WHEEL_GLIDE_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable smooth wheel scrolling for every live scroller.
pub fn set_wheel_glide_enabled(enabled: bool) {
    WHEEL_GLIDE_ENABLED.store(enabled, Ordering::SeqCst);
}

fn wheel_glide_enabled() -> bool {
    WHEEL_GLIDE_ENABLED.load(Ordering::SeqCst)
}

/// Exponent GTK uses for its native per-notch wheel step
/// (`get_wheel_detent_scroll_step` in gtkscrolledwindow.c).
const WHEEL_PAGE_STEP_EXPONENT: f64 = 2.0 / 3.0;

/// Minimum per-notch wheel travel (px). GTK's viewport-scaled detent step
/// feels undersized next to VS Code (~125px effective per notch on Linux),
/// so the step never drops below this floor; viewports whose scaled step
/// already exceeds it keep the native distance.
const WHEEL_MIN_NOTCH_SCROLL_PX: f64 = 125.0;

/// Eased glide: fixed animation window (ms), matching VS Code's editor
/// smooth scrolling duration. Every notch is fully delivered within this
/// window — no floaty tail, no lost distance.
const WHEEL_GLIDE_DURATION_MS: f64 = 125.0;

/// Eased glide: the animation pretends it began this many ms earlier, so the
/// first frame already shows movement (VS Code's responsiveness trick in
/// `SmoothScrollingOperation.start`).
const WHEEL_GLIDE_HEAD_START_MS: f64 = 10.0;

/// Remaining distance (px) at which the glide snaps to rest.
const WHEEL_GLIDE_SETTLE_PX: f64 = 0.5;

/// Tolerance (px) for recognizing the animator's own adjustment writes in
/// `value-changed`; any larger external delta cancels the glide.
const SELF_WRITE_EPSILON_PX: f64 = 0.5;

/// Cap for one animation frame's dt (ms) so an unmapped pause or a jank spike
/// cannot teleport the viewport straight to the target (momentum mode).
const MAX_GLIDE_FRAME_DT_MS: f64 = 100.0;

/// Momentum glide: decay time constant (ms) at low speed, where a single
/// notch must still travel exactly one detent step and stop about as fast as
/// the approved tau=60ms chaser feel.
const MOMENTUM_FAST_TAU_MS: f64 = 60.0;

/// Momentum glide: default decay time constant (ms) once the speed rises into
/// the slow regime, controlling how far fast flicks carry. 300ms mirrors the
/// restrained smooth-scrolling of web browsers (Firefox/Edge): modest
/// acceleration on rapid flicks, unlike the floatier macOS glide.
const MOMENTUM_SLOW_TAU_MS: f64 = 300.0;

/// Momentum glide: speed (in single-notch impulse units) where the slow
/// regime starts blending in. Below this, a notch behaves like the plain
/// chaser; above it, consecutive notches stack into a longer carry.
const MOMENTUM_SLOW_GAIN_START: f64 = 1.3;

/// Momentum glide: speed (in single-notch impulse units) where the slow
/// regime is fully engaged.
const MOMENTUM_SLOW_GAIN_END: f64 = 3.0;

/// Momentum glide: stop integrating once the speed falls below this (px/s).
const MOMENTUM_MIN_VELOCITY_PX_S: f64 = 15.0;

/// Per-notch wheel travel for a viewport of `page_size` pixels. Follows
/// GTK's own detent step (`pow(page_size, 2/3)`), floored at
/// [`WHEEL_MIN_NOTCH_SCROLL_PX`] so a notch never feels undersized at
/// typical window sizes.
pub(crate) fn wheel_step_for_page(page_size: f64) -> f64 {
    page_size
        .powf(WHEEL_PAGE_STEP_EXPONENT)
        .max(WHEEL_MIN_NOTCH_SCROLL_PX)
}

/// One burst notch, accumulated onto `base_target` and clamped to the
/// adjustment bounds. Robust to degenerate bounds (`max_value < lower`) so
/// `f64::clamp` never panics.
pub(crate) fn accumulated_wheel_target(
    base_target: f64,
    notch_delta: f64,
    step_px: f64,
    lower: f64,
    max_value: f64,
) -> f64 {
    let max_value = max_value.max(lower);
    (base_target + notch_delta * step_px).clamp(lower, max_value)
}

/// One eased-glide frame: VS Code's ease-out cubic (`1 - (1 - t)^3`) between
/// `start` and `target` at normalized `progress`. The same curve as the
/// filmstrip's `compute_thumb_animated_scroll_value`; kept local because the
/// filmstrip helper is `pub(super)`-scoped and pinned by a source-structure
/// test.
pub(crate) fn eased_scroll_value(start: f64, target: f64, progress: f64) -> f64 {
    let t = progress.clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    start + (target - start) * eased
}

/// Single-notch velocity impulse (px/s) for a viewport of `page_size` pixels.
/// Sized so that one impulse decaying at [`MOMENTUM_FAST_TAU_MS`] travels
/// exactly one detent step, keeping single-notch distance identical to both
/// the chaser glide and GTK's native teleport.
pub(crate) fn momentum_impulse_for_page(page_size: f64) -> f64 {
    wheel_step_for_page(page_size) / (MOMENTUM_FAST_TAU_MS / 1000.0)
}

/// Momentum decay time constant (ms) for the current speed. Low speeds decay
/// fast (single notch = one step, no floatiness); speeds built up by rapid
/// flicks decay over `slow_decay_ms` instead, so a flick carries far beyond
/// the sum of its notches — the macOS momentum behavior.
pub(crate) fn momentum_tau_ms(speed_px_s: f64, impulse_px_s: f64, slow_decay_ms: f64) -> f64 {
    if impulse_px_s <= 0.0 {
        return MOMENTUM_FAST_TAU_MS;
    }
    let blend = ((speed_px_s / impulse_px_s - MOMENTUM_SLOW_GAIN_START)
        / (MOMENTUM_SLOW_GAIN_END - MOMENTUM_SLOW_GAIN_START))
        .clamp(0.0, 1.0);
    MOMENTUM_FAST_TAU_MS + (slow_decay_ms - MOMENTUM_FAST_TAU_MS) * blend
}

/// Which glide model drives the tick loop.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum GlideMode {
    /// VS Code-style eased retargeting: a fixed-duration ease-out cubic from
    /// the current position to an accumulated target, re-anchored on every
    /// mid-flight notch, landing exactly on the target. N notches always
    /// deliver exactly N detent steps, fully, within the fixed window. This
    /// is the shipped default.
    #[default]
    Eased,
    /// macOS-style momentum: notches inject velocity and the decay time
    /// constant grows with speed, so fast flicks fly far beyond their notch
    /// sum. Available as a tuning alternative via `set_momentum_glide`.
    Momentum,
}

struct GlideState {
    /// Accumulated, bounds-clamped destination of the current wheel burst.
    target: f64,
    /// Viewport value when the current eased animation was anchored
    /// (eased mode).
    eased_start: f64,
    /// Frame clock (us) when the eased animation was anchored; 0 = the next
    /// tick must re-anchor from the live position (fresh burst or mid-flight
    /// retarget).
    animation_start_us: i64,
    /// Current glide velocity (px/s, signed) in momentum mode.
    velocity: f64,
    /// Velocity impulse (px/s) of the most recent notch, normalizing the
    /// momentum decay regime (momentum mode).
    notch_impulse: f64,
    /// True while a tick callback is running the glide. Doubles as the cancel
    /// token: clearing it makes the in-flight tick loop break next frame.
    animating: bool,
    /// Last value the animator wrote, so writes from other sources (scrollbar
    /// drag, keyboard focus scrolling, layout restores, touchpad passthrough)
    /// are detectable in `value-changed` and cancel the glide.
    last_written: f64,
    /// Frame time (us) of the previous animation frame; 0 = first frame
    /// (momentum mode).
    last_frame_time_us: i64,
}

struct Inner {
    /// Weak on purpose: the tick closure that keeps `Inner` alive is owned by
    /// the scroller, so a strong reference here would leak every surface.
    scroller: glib::WeakRef<gtk::ScrolledWindow>,
    state: RefCell<GlideState>,
    mode: Cell<GlideMode>,
    momentum_slow_tau_ms: Cell<f64>,
}

/// macOS-like smooth scrolling for discrete mouse-wheel notches on a
/// `gtk::ScrolledWindow`. Touchpad (surface-unit) events are never consumed.
///
/// `VirtualMediaGrid` keeps a single capture scroll controller on its
/// scroller and routes it through [`SmoothScroller::handle_scroll`] so its
/// scroll-intent notification keeps firing; plain sidebar and dialog
/// scrollers can use [`SmoothScroller::install`] instead.
#[derive(Clone)]
pub struct SmoothScroller {
    inner: Rc<Inner>,
}

impl SmoothScroller {
    /// Track `scroller`'s vertical adjustment for glides, installing the
    /// external-write cancellation watcher without adding any input
    /// controller. Pair with an existing scroll controller via
    /// [`SmoothScroller::handle_scroll`].
    pub fn new(scroller: &gtk::ScrolledWindow) -> Self {
        let adjustment = scroller.vadjustment();
        let start_value = adjustment.value();
        let inner = Rc::new(Inner {
            scroller: scroller.downgrade(),
            state: RefCell::new(GlideState {
                target: start_value,
                eased_start: start_value,
                animation_start_us: 0,
                velocity: 0.0,
                notch_impulse: 0.0,
                animating: false,
                last_written: start_value,
                last_frame_time_us: 0,
            }),
            mode: Cell::new(GlideMode::default()),
            momentum_slow_tau_ms: Cell::new(MOMENTUM_SLOW_TAU_MS),
        });

        let watcher = Rc::clone(&inner);
        adjustment.connect_value_changed(move |adj| {
            let value = adj.value();
            let mut state = watcher.state.borrow_mut();
            if state.animating && (value - state.last_written).abs() > SELF_WRITE_EPSILON_PX {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SCROLL_GLIDE cancel_external value={} last_written={}",
                    value,
                    state.last_written
                );
                // Someone else moved the viewport: cancel and re-anchor so the
                // next notch starts a fresh burst from the actual position.
                state.animating = false;
                state.target = value;
                state.velocity = 0.0;
                state.last_frame_time_us = 0;
            }
        });

        SmoothScroller { inner }
    }

    /// [`SmoothScroller::new`] plus a capture-phase vertical scroll
    /// controller that intercepts discrete wheel notches (`Propagation::Stop`)
    /// and passes touchpad surface deltas through (`Propagation::Proceed`).
    pub fn install(scroller: &gtk::ScrolledWindow) {
        let handle = SmoothScroller::new(scroller);
        let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        controller.set_name(Some("photo-viewer-smooth-scroll"));
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        controller.connect_scroll(move |ctrl, _, delta_y| handle.handle_scroll(ctrl, delta_y));
        scroller.add_controller(controller);
    }

    /// Feed one scroll event from an existing capture-phase scroll
    /// controller. Returns `Propagation::Stop` when the current event is a
    /// discrete wheel notch consumed by the glide, `Propagation::Proceed` for
    /// touchpad surface deltas, scroll-stop events, or an unset current
    /// event.
    pub fn handle_scroll(
        &self,
        controller: &gtk::EventControllerScroll,
        delta_y: f64,
    ) -> glib::Propagation {
        if !wheel_glide_enabled() {
            // The user turned smooth scrolling off: native wheel handling.
            return glib::Propagation::Proceed;
        }
        let is_wheel = controller
            .current_event()
            .and_then(|event| event.downcast::<gdk::ScrollEvent>().ok())
            .is_some_and(|scroll| scroll.unit() == gdk::ScrollUnit::Wheel);
        if is_wheel {
            self.handle_wheel_delta(delta_y);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    }

    /// Switch to the macOS-style momentum glide. `slow_decay_ms` is the decay
    /// time constant once flick speed builds up: higher values make fast
    /// flicks carry farther. The shipped default is the VS Code-style eased
    /// glide instead, which always delivers exactly the notch sum; an
    /// in-flight glide is cancelled.
    pub fn set_momentum_glide(&self, slow_decay_ms: f64) {
        self.reset_glide();
        self.inner
            .momentum_slow_tau_ms
            .set(slow_decay_ms.max(MOMENTUM_FAST_TAU_MS));
        self.inner.mode.set(GlideMode::Momentum);
    }

    /// Stop any in-flight glide and clear its velocity, leaving the viewport
    /// wherever it currently is.
    fn reset_glide(&self) {
        let mut state = self.inner.state.borrow_mut();
        state.animating = false;
        state.velocity = 0.0;
    }

    /// Accumulate one discrete wheel notch and start (or extend) the glide.
    /// Exposed `pub(crate)` so tests can drive glides without synthesizing
    /// `GdkEvent`s, which gdk4-rs cannot construct.
    pub(crate) fn handle_wheel_delta(&self, delta_y: f64) {
        if !delta_y.is_finite() || delta_y == 0.0 {
            return;
        }
        let Some(scroller) = self.inner.scroller.upgrade() else {
            return;
        };
        let adjustment = scroller.vadjustment();
        let step_px = wheel_step_for_page(adjustment.page_size());
        let max_value = adjustment.upper() - adjustment.page_size();
        let mut state = self.inner.state.borrow_mut();
        match self.inner.mode.get() {
            GlideMode::Eased => {
                let base = if state.animating {
                    state.target
                } else {
                    adjustment.value()
                };
                state.target =
                    accumulated_wheel_target(base, delta_y, step_px, adjustment.lower(), max_value);
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SCROLL_GLIDE notch delta={} step_px={} target={}",
                    delta_y,
                    step_px,
                    state.target
                );
                if state.animating {
                    // Mid-flight retarget: VS Code's `Scrollable.combine`
                    // restarts the ease from the live position with a fresh
                    // duration; the running tick re-anchors on its next
                    // frame.
                    state.animation_start_us = 0;
                    return;
                }
                if (state.target - adjustment.value()).abs() <= WHEEL_GLIDE_SETTLE_PX {
                    // Clamped into a no-op at a boundary or on an unscrollable
                    // surface: nothing to animate.
                    return;
                }
            }
            GlideMode::Momentum => {
                let impulse = momentum_impulse_for_page(adjustment.page_size());
                if state.animating {
                    // Notches within a burst stack velocity, which is what
                    // makes rapid flicks carry far.
                    state.velocity += delta_y * impulse;
                } else {
                    state.velocity = delta_y * impulse;
                }
                state.notch_impulse = impulse;
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SCROLL_GLIDE notch delta={} impulse={} velocity={}",
                    delta_y,
                    impulse,
                    state.velocity
                );
            }
        }
        if !state.animating {
            state.animating = true;
            state.last_frame_time_us = 0;
            state.animation_start_us = 0;
            drop(state);
            self.spawn_glide_tick();
        }
        // else: the running tick loop picks up the new target/velocity on its
        // next frame.
    }

    fn spawn_glide_tick(&self) {
        let Some(scroller) = self.inner.scroller.upgrade() else {
            return;
        };
        let handle = self.clone();
        let weak = self.inner.scroller.clone();
        scroller.add_tick_callback(move |_, clock| {
            let Some(scroller) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            handle.glide_frame(&scroller, clock.frame_time())
        });
    }

    /// Advance the glide by one frame. Split out of the tick callback so
    /// tests can drive a deterministic synthetic frame clock instead of
    /// depending on the real one (which stalls without a mapped painting
    /// window under xvfb).
    pub(crate) fn glide_frame(
        &self,
        scroller: &gtk::ScrolledWindow,
        frame_time_us: i64,
    ) -> glib::ControlFlow {
        let adjustment = scroller.vadjustment();
        let mut state = self.inner.state.borrow_mut();
        if !state.animating {
            // Cancelled (`reset_glide()` or an external adjustment write).
            return glib::ControlFlow::Break;
        }
        let dt_ms = if state.last_frame_time_us == 0 {
            0.0
        } else {
            let elapsed_us = (frame_time_us - state.last_frame_time_us).max(0);
            (elapsed_us as f64 / 1000.0).min(MAX_GLIDE_FRAME_DT_MS)
        };
        state.last_frame_time_us = frame_time_us;

        // Bounds may have changed mid-glide (content grew or shrank):
        // re-clamp everything against the live adjustment.
        let lower = adjustment.lower();
        let max_value = (adjustment.upper() - adjustment.page_size()).max(lower);
        let current = adjustment.value();

        match self.inner.mode.get() {
            GlideMode::Eased => {
                state.target = state.target.clamp(lower, max_value);
                if state.animation_start_us == 0 {
                    // Anchor a fresh ease from the live position, pretending
                    // it began one head-start earlier so the very first frame
                    // already shows movement (VS Code's trick).
                    state.animation_start_us =
                        frame_time_us - (WHEEL_GLIDE_HEAD_START_MS * 1000.0) as i64;
                    state.eased_start = current;
                }
                let progress = ((frame_time_us - state.animation_start_us).max(0) as f64
                    / (WHEEL_GLIDE_DURATION_MS * 1000.0))
                    .clamp(0.0, 1.0);
                if progress >= 1.0 || (state.target - current).abs() <= WHEEL_GLIDE_SETTLE_PX {
                    // Land exactly on the target, as VS Code does on
                    // completion.
                    state.last_written = state.target;
                    let target = state.target;
                    state.animating = false;
                    drop(state);
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SCROLL_GLIDE settle value={}",
                        target
                    );
                    adjustment.set_value(target);
                    return glib::ControlFlow::Break;
                }

                let next = eased_scroll_value(state.eased_start, state.target, progress);
                state.last_written = next;
                drop(state);
                adjustment.set_value(next);
                glib::ControlFlow::Continue
            }
            GlideMode::Momentum => {
                let tau_ms = momentum_tau_ms(
                    state.velocity.abs(),
                    state.notch_impulse,
                    self.inner.momentum_slow_tau_ms.get(),
                );
                // Integrate the exponential decay exactly over the frame —
                // v * tau * (1 - e^(-dt/tau)) telescopes, so a lone notch
                // lands exactly one detent step away at any frame rate
                // (Euler's v * dt would overshoot ~12% at 60fps).
                let decay = (-dt_ms / tau_ms).exp();
                let travel = state.velocity * tau_ms / 1000.0 * (1.0 - decay);
                state.velocity *= decay;
                let raw_next = current + travel;
                let next = raw_next.clamp(lower, max_value);
                let hit_edge = next != raw_next;
                if hit_edge {
                    // Edges kill the momentum instead of bouncing.
                    state.velocity = 0.0;
                }
                let settled = hit_edge || state.velocity.abs() < MOMENTUM_MIN_VELOCITY_PX_S;
                state.last_written = next;
                state.animating = !settled;
                drop(state);
                adjustment.set_value(next);
                if settled {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SCROLL_GLIDE settle value={}",
                        next
                    );
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
