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
    window.close();
    while glib::MainContext::default().iteration(false) {}
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
    window.close();
    while glib::MainContext::default().iteration(false) {}
}
