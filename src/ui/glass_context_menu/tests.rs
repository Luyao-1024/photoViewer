use super::*;

#[gtk::test]
fn opening_menu_on_new_overlay_dismisses_previous_overlay_menu() {
    let overlay_a = gtk::Overlay::new();
    let layer_a = gtk::Fixed::new();
    overlay_a.add_overlay(&layer_a);
    remember_open_menu(&overlay_a, &layer_a);

    let overlay_b = gtk::Overlay::new();
    let layer_b = gtk::Fixed::new();
    dismiss_open_menu();
    overlay_b.add_overlay(&layer_b);
    remember_open_menu(&overlay_b, &layer_b);

    assert!(
        layer_a.parent().is_none(),
        "opening another context menu should remove the previous one even on a different overlay"
    );
    assert!(
        layer_b.parent().is_some(),
        "new context menu layer should remain open"
    );

    dismiss_open_menu();
    assert!(layer_b.parent().is_none());
}

#[gtk::test]
fn context_menu_keeps_focus_on_its_anchor() {
    let _ = gtk::init();
    let overlay = gtk::Overlay::new();
    let anchor = gtk::Button::with_label("anchor");
    overlay.set_child(Some(&anchor));
    let window = gtk::Window::builder()
        .default_width(320)
        .default_height(240)
        .child(&overlay)
        .build();
    window.present();

    let context = glib::MainContext::default();
    while context.pending() {
        context.iteration(false);
    }
    assert!(anchor.grab_focus());

    show(
        &overlay,
        anchor.upcast_ref(),
        1.0,
        1.0,
        vec![GlassMenuItem::new(
            "action",
            GlassMenuItemKind::Normal,
            || {},
        )],
    );

    assert!(is_open(), "context menu should be marked open");
    assert_eq!(
        gtk::prelude::RootExt::focus(&window).as_ref(),
        Some(anchor.upcast_ref()),
        "opening the menu must not transfer focus to its overlay layer"
    );

    dismiss_open_menu();
    assert!(!is_open(), "closed context menu must clear its modal state");
    assert_eq!(
        gtk::prelude::RootExt::focus(&window).as_ref(),
        Some(anchor.upcast_ref()),
        "closing the menu must keep focus on the triggering control"
    );
    window.close();
}
