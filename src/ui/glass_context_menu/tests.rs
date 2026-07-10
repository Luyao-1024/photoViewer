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
