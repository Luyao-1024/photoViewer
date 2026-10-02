use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use super::action::{KeyboardAction, KeyboardResult};
use super::binding::{resolve_binding, KeyCombo, KeyboardScope};
use crate::core::log_targets;
use crate::ui::glass_context_menu;

pub fn install<W, F, H>(widget: &W, resolve_scope: F, handle_action: H)
where
    W: IsA<gtk::Widget>,
    F: Fn() -> KeyboardScope + 'static,
    H: Fn(KeyboardAction) -> KeyboardResult + 'static,
{
    let key = gtk::EventControllerKey::new();
    key.set_name(Some("photo-viewer-keyboard-router"));
    key.set_propagation_phase(gtk::PropagationPhase::Capture);

    // One physical press dispatches at most one action.
    //
    // Holding a key down makes GDK emit `key-pressed` again and again, and
    // every repeat looks identical to a fresh press to this handler. Dispatching
    // them made every toggle thrash: holding `F` walked immersive on/off/on as
    // fast as the OS repeat rate, so the 220 ms chrome transition never finished
    // and the picture just appeared to twitch, ending in whatever state the
    // repeat parity happened to leave. It also fired non-idempotent actions
    // repeatedly — holding a heart or `Delete` or `→` did the same thing many
    // times over, and `navigate_by_delta` spent the whole hold cancelling its own
    // pending switch.
    //
    // GTK 4 gives no auto-repeat flag on `GdkKeyEvent`, so the latch is
    // released on any key release instead: the next physical press is fresh.
    let latched = Rc::new(RefCell::new(None::<KeyCombo>));

    let latched_for_press = latched.clone();
    key.connect_key_pressed(move |_, key, _keycode, state| {
        let combo = KeyCombo::new(key, state);
        if *latched_for_press.borrow() == Some(combo) {
            // A repeat of a press this router already handled.
            tracing::debug!(
                target: log_targets::KEYBOARD,
                "KEY key-pressed key={} modifiers={state:?} -> SUPPRESSED as an auto-repeat of the \
                 press still held down",
                combo.key,
            );
            return glib::Propagation::Proceed;
        }
        let scope = resolve_scope();
        let Some(action) = resolve_binding(scope, combo) else {
            tracing::debug!(
                target: log_targets::KEYBOARD,
                "KEY key-pressed key={} modifiers={state:?} -> no binding in scope {scope:?}",
                combo.key,
            );
            return glib::Propagation::Proceed;
        };

        let handled = handle_action(action);
        tracing::debug!(
            target: log_targets::KEYBOARD,
            "KEY key-pressed key={} modifiers={state:?} scope={scope:?} action={action:?} -> \
             {handled:?}",
            combo.key,
        );
        if handled.is_handled() {
            *latched_for_press.borrow_mut() = Some(combo);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });

    let latched_for_release = latched.clone();
    // A release is never itself a shortcut; it only ends a physical press.
    // gtk4-rs types this signal's handler as returning nothing even though the C
    // signal is a gboolean, so nothing is propagated from here.
    key.connect_key_released(move |_, key, _keycode, state| {
        let combo = KeyCombo::new(key, state);
        let was_latched = latched_for_release.borrow().is_some();
        tracing::debug!(
            target: log_targets::KEYBOARD,
            "KEY key-released key={} modifiers={state:?} (latched={was_latched}) -> router re-armed",
            combo.key,
        );
        *latched_for_release.borrow_mut() = None;
    });

    widget.add_controller(key);
}

pub fn scope_for_focus(root: &gtk::Widget) -> KeyboardScope {
    if focus_is_text_input(root) {
        KeyboardScope::TextInput
    } else if glass_context_menu::is_open()
        || focus_has_ancestor_class(root, "glass-context-menu-layer")
    {
        KeyboardScope::Modal
    } else {
        KeyboardScope::Global
    }
}

fn focus_is_text_input(root: &gtk::Widget) -> bool {
    let Some(focus) = root.root().and_then(|root| root.focus()) else {
        return false;
    };

    focus.is::<gtk::Editable>()
        || focus.is::<gtk::TextView>()
        || focus.is::<gtk::SearchEntry>()
        || focus.is::<gtk::Entry>()
}

fn focus_has_ancestor_class(root: &gtk::Widget, class_name: &str) -> bool {
    let Some(mut current) = root.root().and_then(|root| root.focus()) else {
        return false;
    };

    loop {
        if current.has_css_class(class_name) {
            return true;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        current = parent;
    }
}

#[cfg(test)]
mod tests;
