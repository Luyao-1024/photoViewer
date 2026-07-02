use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

use super::action::{KeyboardAction, KeyboardResult};
use super::binding::{resolve_binding, KeyCombo, KeyboardScope};

pub fn install<W, F, H>(widget: &W, resolve_scope: F, handle_action: H)
where
    W: IsA<gtk::Widget>,
    F: Fn() -> KeyboardScope + 'static,
    H: Fn(KeyboardAction) -> KeyboardResult + 'static,
{
    let key = gtk::EventControllerKey::new();
    key.set_name(Some("photo-viewer-keyboard-router"));
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    key.connect_key_pressed(move |_, key, _keycode, state| {
        let combo = KeyCombo::new(key, state);
        let Some(action) = resolve_binding(resolve_scope(), combo) else {
            return glib::Propagation::Proceed;
        };

        if handle_action(action).is_handled() {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    widget.add_controller(key);
}

pub fn scope_for_focus(root: &gtk::Widget) -> KeyboardScope {
    if focus_is_text_input(root) {
        KeyboardScope::TextInput
    } else if focus_has_ancestor_class(root, "glass-context-menu-layer") {
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
mod tests {
    use super::*;

    #[gtk::test]
    fn focused_entry_resolves_text_input_scope() {
        let window = gtk::Window::new();
        let entry = gtk::Entry::new();
        window.set_child(Some(&entry));
        window.present();
        entry.grab_focus();

        assert_eq!(
            scope_for_focus(window.upcast_ref()),
            KeyboardScope::TextInput
        );
    }

    #[gtk::test]
    fn focused_glass_context_menu_layer_resolves_modal_scope() {
        let window = gtk::Window::new();
        let layer = gtk::Fixed::builder()
            .can_focus(true)
            .css_classes(["glass-context-menu-layer"])
            .build();
        let button = gtk::Button::with_label("Item");
        layer.put(&button, 0.0, 0.0);
        window.set_child(Some(&layer));
        window.present();
        button.grab_focus();

        assert_eq!(scope_for_focus(window.upcast_ref()), KeyboardScope::Modal);
    }
}
