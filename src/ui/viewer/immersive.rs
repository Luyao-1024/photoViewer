//! In-place immersive browsing.
//!
//! `F` folds the viewer's own chrome away so the picture occupies the whole
//! page, and the chrome returns while the pointer is moving. This is deliberately
//! *not* the separate system-fullscreen preview window
//! ([`super::fullscreen_window`]), which stays on `Shift+F`: that window is a
//! second top-level surface whose Escape returns to a different instance, which
//! is exactly what made the old `F` hard to predict.

use super::ViewerPage;
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
        motion_controller.connect_motion(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.note_immersive_activity();
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
        let shows_side_panel = self.imp().details_split_view.get().shows_sidebar()
            || self.imp().editor_split_view.get().shows_sidebar();
        if on && !immersive_allowed(self.imp().is_editing.get(), shows_side_panel) {
            return;
        }
        self.imp().immersive.set(on);
        self.set_chrome_revealed(!on);
        if on {
            self.arm_immersive_idle();
        } else {
            self.clear_immersive_idle();
        }
    }

    /// Pointer motion or a key press: the user is still here, so the chrome comes
    /// back and the stillness clock restarts.
    pub(super) fn note_immersive_activity(&self) {
        if !self.is_immersive() {
            return;
        }
        self.set_chrome_revealed(true);
        self.arm_immersive_idle();
    }

    /// Any path that brings a panel back on purpose (details, edit) ends the
    /// immersive state rather than fighting it.
    pub(super) fn exit_immersive_for_chrome(&self) {
        if self.is_immersive() {
            self.set_immersive(false);
        }
    }

    fn arm_immersive_idle(&self) {
        self.clear_immersive_idle();
        let weak = self.downgrade();
        let source = glib::timeout_add_local(
            std::time::Duration::from_millis(IMMERSIVE_IDLE_MS as u64),
            move || {
                let Some(this) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                *this.imp().immersive_idle_source.borrow_mut() = None;
                if this.is_immersive() {
                    this.set_chrome_revealed(false);
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
            source.remove();
        }
    }

    fn set_chrome_revealed(&self, revealed: bool) {
        let imp = self.imp();
        if imp.chrome_revealed.get() == revealed {
            return;
        }
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
