//! In-place immersive browsing.
//!
//! `F` folds the viewer's own chrome away so the picture occupies the whole
//! page, and the chrome returns while the pointer is moving. This is deliberately
//! *not* the separate system-fullscreen preview window
//! ([`super::fullscreen_window`]), which stays on `Shift+F`: that window is a
//! second top-level surface whose Escape returns to a different instance, which
//! is exactly what made the old `F` hard to predict.

use super::ViewerPage;
use crate::core::log_targets;
use crate::ui::motion;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;

/// How long the pointer must hold still before the chrome folds away.
pub(super) const IMMERSIVE_IDLE_MS: u32 = 2500;
/// Fold/unfold duration while animations are on, inside the 150-300 ms band the
/// reduce-motion work settled on for chrome transitions.
pub(super) const IMMERSIVE_TRANSITION_MS: u32 = 220;
/// How far the pointer has to travel before a motion event counts as the user
/// moving. One CSS pixel is below the threshold of noticing for a hand-driven
/// pointer, and above the sub-pixel jitter a resting mouse reports.
pub(super) const STILLNESS_MOVE_EPS_PX: f64 = 1.0;

/// The two rules that decide whether a press may change anything, kept pure so
/// they can be tested without a realized window.
///
/// Editing must never lose its buttons mid-stroke, and an open side panel *is*
/// chrome: folding it away behind the user's back would hide the thing they are
/// looking at.
pub(super) fn immersive_allowed(is_editing: bool, shows_side_panel: bool) -> bool {
    !is_editing && !shows_side_panel
}

impl ViewerPage {
    /// Attach the pointer watcher. It lives on the page rather than on the image
    /// stage so that moving across the header or the filmstrip also counts as
    /// activity — the question is "is the user still?", not "is the user over the
    /// photo?".
    pub(super) fn setup_immersive(&self) {
        // The template ships every region revealed, so the mirrored flag has to
        // start there; a default-false `chrome_revealed` would make the first
        // fold a no-op.
        self.imp().chrome_revealed.set(true);
        let motion_controller = gtk::EventControllerMotion::new();
        motion_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        // The position of the last *accepted* movement, not of the last event.
        //
        // A stillness clock has to measure stillness, so only a pointer that
        // actually went somewhere counts. GDK keeps delivering motion events for
        // an unchanged pointer — anything that re-targets or re-allocates what
        // sits under the pointer does it, and folding the chrome is exactly such
        // a change — and treating each of those as "the user moved" cancelled and
        // re-armed the timer on every event, so the timer could never reach its
        // deadline and the chrome stayed pinned open for as long as the pointer
        // rested anywhere on the page. Immersive browsing then read as a flicker:
        // fold, instant re-reveal, fold, re-reveal.
        //
        // Comparing against the last accepted position (not the previous event)
        // is what makes a slow drag still register: sub-pixel steps accumulate
        // until they cross the threshold, while sensor jitter around one point
        // never does.
        let anchor = std::rc::Rc::new(std::cell::RefCell::new(None::<(f64, f64)>));
        let anchor_for_motion = anchor.clone();
        motion_controller.connect_motion(move |_, x, y| {
            let moved = match *anchor_for_motion.borrow() {
                Some((ax, ay)) => {
                    (x - ax).abs() > STILLNESS_MOVE_EPS_PX || (y - ay).abs() > STILLNESS_MOVE_EPS_PX
                }
                None => true,
            };
            if !moved {
                tracing::trace!(
                    target: log_targets::VIEWER,
                    "MOTION ignored: pointer still at ({x:.1},{y:.1}), within \
                     {STILLNESS_MOVE_EPS_PX}px of the last movement",
                );
                return;
            }
            *anchor_for_motion.borrow_mut() = Some((x, y));
            if let Some(this) = weak.upgrade() {
                tracing::debug!(
                    target: log_targets::VIEWER,
                    "MOTION moved to ({x:.0},{y:.0}) on the viewer page (immersive={})",
                    this.is_immersive(),
                );
                this.note_immersive_activity("pointer motion on the page");
            }
        });
        self.add_controller(motion_controller);
    }

    pub(super) fn is_immersive(&self) -> bool {
        self.imp().immersive.get()
    }

    pub(super) fn toggle_immersive(&self) {
        let on = !self.is_immersive();
        self.set_immersive(on);
    }

    /// Enter or leave immersive browsing. Leaving always restores the chrome, so
    /// there is no way to end up immersive-but-hidden.
    pub(super) fn set_immersive(&self, on: bool) {
        let imp = self.imp();
        let editing = imp.is_editing.get();
        let details_open = imp.details_split_view.get().shows_sidebar();
        let editor_open = imp.editor_split_view.get().shows_sidebar();
        let shows_side_panel = details_open || editor_open;
        let allowed = immersive_allowed(editing, shows_side_panel);
        tracing::debug!(
            target: log_targets::VIEWER,
            "IMMERSIVE set_immersive requested on={on} allowed={allowed}              (editing={editing} details_open={details_open} editor_open={editor_open})              was={}",
            self.is_immersive(),
        );
        if on && !allowed {
            tracing::debug!(
                target: log_targets::VIEWER,
                "IMMERSIVE refused: editing or a side panel is open, so the chrome stays put",
            );
            return;
        }
        imp.immersive.set(on);
        self.set_chrome_revealed(
            !on,
            if on {
                "set_immersive(true)"
            } else {
                "set_immersive(false)"
            },
        );
        if on {
            self.arm_immersive_idle("entering immersive");
        } else {
            self.clear_immersive_idle();
        }
    }

    /// Pointer motion or a key press: the user is still here, so the chrome comes
    /// back and the stillness clock restarts.
    pub(super) fn note_immersive_activity(&self, reason: &'static str) {
        if !self.is_immersive() {
            tracing::debug!(
                target: log_targets::VIEWER,
                "IMMERSIVE activity ignored ({reason}): not immersive",
            );
            return;
        }
        tracing::debug!(
            target: log_targets::VIEWER,
            "IMMERSIVE activity while immersive ({reason}) -> re-revealing the chrome and \
             re-arming the {IMMERSIVE_IDLE_MS}ms stillness timer",
        );
        self.set_chrome_revealed(true, reason);
        self.arm_immersive_idle(reason);
    }

    /// Any path that brings a panel back on purpose (details, edit) ends the
    /// immersive state rather than fighting it.
    pub(super) fn exit_immersive_for_chrome(&self) {
        if self.is_immersive() {
            self.set_immersive(false);
        }
    }

    fn arm_immersive_idle(&self, reason: &'static str) {
        self.clear_immersive_idle();
        tracing::debug!(
            target: log_targets::VIEWER,
            "IMMERSIVE stillness timer armed for {IMMERSIVE_IDLE_MS}ms (reason={reason})",
        );
        let weak = self.downgrade();
        let source = glib::timeout_add_local(
            std::time::Duration::from_millis(IMMERSIVE_IDLE_MS as u64),
            move || {
                let Some(this) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                *this.imp().immersive_idle_source.borrow_mut() = None;
                tracing::debug!(
                    target: log_targets::VIEWER,
                    "IMMERSIVE stillness timer fired after {IMMERSIVE_IDLE_MS}ms (immersive={})",
                    this.is_immersive(),
                );
                if this.is_immersive() {
                    this.set_chrome_revealed(false, "stillness timer");
                }
                glib::ControlFlow::Break
            },
        );
        *self.imp().immersive_idle_source.borrow_mut() = Some(source);
    }

    fn clear_immersive_idle(&self) {
        // Only ever holds a *pending* source: the callback clears the cell before
        // it returns, and removing a fired SourceId aborts.
        if let Some(source) = self.imp().immersive_idle_source.borrow_mut().take() {
            tracing::debug!(
                target: log_targets::VIEWER,
                "IMMERSIVE pending stillness timer cancelled (raw id={})",
                source.as_raw(),
            );
            source.remove();
        }
    }

    /// `reason` names what asked for the change. Every chrome state change goes
    /// through here, so this log is the complete answer to "why did the chrome
    /// just appear / disappear", which is otherwise invisible from the outside.
    fn set_chrome_revealed(&self, revealed: bool, reason: &'static str) {
        let imp = self.imp();
        if imp.chrome_revealed.get() == revealed {
            tracing::debug!(
                target: log_targets::VIEWER,
                "IMMERSIVE chrome stays revealed={revealed} (no change, reason={reason})",
            );
            return;
        }
        tracing::debug!(
            target: log_targets::VIEWER,
            "IMMERSIVE chrome revealed {revealed} <- {reason} (immersive={})",
            self.is_immersive(),
        );
        imp.chrome_revealed.set(revealed);
        // The template authors a 220 ms slide, but reduce-motion can only be
        // re-read here: motion::apply_to() runs once at construction and turning
        // animations back off later does not walk already-built pages.
        let duration = if motion::enabled() {
            IMMERSIVE_TRANSITION_MS
        } else {
            0
        };
        for revealer in [
            &imp.header_revealer,
            &imp.filmstrip_revealer,
            &imp.nav_buttons_revealer,
            &imp.zoom_controls_revealer,
        ] {
            let revealer = revealer.get();
            revealer.set_transition_duration(duration);
            revealer.set_reveal_child(revealed);
        }
    }
}

#[cfg(test)]
#[path = "immersive/tests.rs"]
mod tests;
