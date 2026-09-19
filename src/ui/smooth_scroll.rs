//! macOS-style smooth wheel scrolling for `GtkScrolledWindow` surfaces.
//!
//! GTK4 scrolls one wheel notch by teleporting the vertical adjustment: the
//! `kinetic-scrolling` property only applies to touch input, and discrete
//! wheel events (`GDK_SCROLL_UNIT_WHEEL`) get no easing at all. Discrete
//! notches are therefore consumed on a capture-phase scroll controller and
//! animated frame by frame instead: scrolling starts immediately, and
//! decelerates to rest after the last notch. Touchpad (surface-unit) deltas
//! are never consumed — GTK already gives them native smooth handling.
//!
//! Two interchangeable glide models:
//!
//! * *Chaser* (`set_glide_tau_ms`): frame-rate-independent exponential
//!   approach toward an accumulated target. N notches always land exactly N
//!   detent steps away — faithful to native distances, but rapid flicks carry
//!   no farther than their notch sum.
//! * *Momentum* (`set_momentum_glide`): notches inject velocity and the decay
//!   time constant grows with speed, so fast flicks fly far beyond their
//!   notch sum — the macOS momentum behavior. Single notches still travel
//!   exactly one detent step.
//!
//! The per-notch distance always matches GTK's own detent step (`pow(
//! page_size, 2/3)` in gtkscrolledwindow.c), so enabling this changes only
//! the motion between notches, not how far a lone notch travels.

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

/// Exponent GTK uses for its per-notch wheel step
/// (`get_wheel_detent_scroll_step` in gtkscrolledwindow.c). Matching it keeps
/// the smoothed per-notch distance identical to the native teleport it
/// replaces.
const WHEEL_PAGE_STEP_EXPONENT: f64 = 2.0 / 3.0;

/// Default exponential-approach time constant (ms). Higher values give a
/// longer, floatier glide; 110ms puts a single notch at ~95% of its distance
/// after ~330ms.
const WHEEL_GLIDE_TAU_MS: f64 = 110.0;

/// Remaining distance (px) at which the glide snaps to rest.
const WHEEL_GLIDE_SETTLE_PX: f64 = 0.5;

/// Tolerance (px) for recognizing the animator's own adjustment writes in
/// `value-changed`; any larger external delta cancels the glide.
const SELF_WRITE_EPSILON_PX: f64 = 0.5;

/// Cap for one animation frame's dt (ms) so an unmapped pause or a jank spike
/// cannot teleport the viewport straight to the target.
const MAX_GLIDE_FRAME_DT_MS: f64 = 100.0;

/// Smallest accepted glide time constant (ms) in [`SmoothScroller::set_glide_tau_ms`],
/// keeping the glide perceptible instead of degenerating into a teleport.
const MIN_GLIDE_TAU_MS: f64 = 10.0;

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

/// Per-notch wheel travel for a viewport of `page_size` pixels. Mirrors GTK's
/// own detent step so smoothing does not change the distance.
pub(crate) fn wheel_step_for_page(page_size: f64) -> f64 {
    page_size.powf(WHEEL_PAGE_STEP_EXPONENT)
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

/// One frame-rate-independent exponential-approach step toward `target`.
/// `dt = tau * ln(2)` covers half the remaining distance; a non-positive tau
/// jumps straight to the target.
pub(crate) fn approach_value(current: f64, target: f64, dt_ms: f64, tau_ms: f64) -> f64 {
    if tau_ms <= 0.0 {
        return target;
    }
    let alpha = 1.0 - (-dt_ms / tau_ms).exp();
    current + (target - current) * alpha
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
    /// Distance-conserving exponential chase toward an accumulated target.
    /// N notches always land exactly N detent steps away — but rapid flicks
    /// therefore carry no farther than their notch sum.
    Chaser,
    /// macOS-style momentum: notches inject velocity, and the decay time
    /// constant grows with speed, so fast flicks fly far beyond their notch
    /// sum while single notches still travel exactly one step. This is the
    /// shipped default; its slow decay is tuned to web-browser smooth
    /// scrolling rather than the floatier macOS feel.
    #[default]
    Momentum,
}

struct GlideState {
    /// Accumulated, bounds-clamped destination of the current wheel burst
    /// (chaser mode).
    target: f64,
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
    /// Frame time (us) of the previous animation frame; 0 = first frame.
    last_frame_time_us: i64,
}

struct Inner {
    /// Weak on purpose: the tick closure that keeps `Inner` alive is owned by
    /// the scroller, so a strong reference here would leak every surface.
    scroller: glib::WeakRef<gtk::ScrolledWindow>,
    state: RefCell<GlideState>,
    mode: Cell<GlideMode>,
    glide_tau_ms: Cell<f64>,
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
                velocity: 0.0,
                notch_impulse: 0.0,
                animating: false,
                last_written: start_value,
                last_frame_time_us: 0,
            }),
            mode: Cell::new(GlideMode::default()),
            glide_tau_ms: Cell::new(WHEEL_GLIDE_TAU_MS),
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

    /// Tune the chaser glide's exponential time constant (ms). Higher values
    /// make the deceleration tail longer and floatier. Switches to (or stays
    /// in) chaser mode; an in-flight glide is cancelled so the new parameters
    /// apply to the next notch.
    pub fn set_glide_tau_ms(&self, tau_ms: f64) {
        self.reset_glide();
        self.inner.mode.set(GlideMode::Chaser);
        self.inner.glide_tau_ms.set(tau_ms.max(MIN_GLIDE_TAU_MS));
    }

    /// Switch to the macOS-style momentum glide. `slow_decay_ms` is the decay
    /// time constant once flick speed builds up: higher values make fast
    /// flicks carry farther. Single notches always travel exactly one detent
    /// step, as in chaser mode. An in-flight glide is cancelled.
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
            GlideMode::Chaser => {
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
                    // The running tick loop picks up the new target next frame.
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
            GlideMode::Chaser => {
                state.target = state.target.clamp(lower, max_value);
                if (state.target - current).abs() <= WHEEL_GLIDE_SETTLE_PX {
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

                let next =
                    approach_value(current, state.target, dt_ms, self.inner.glide_tau_ms.get());
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
