use super::*;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// The whole point of the action tier is that the button actually fires: a
/// label with no callback is a decoration that lies about being an undo.
#[gtk::test]
fn an_action_toast_offers_its_button_and_fires_it() {
    let _ = adw::init();
    let overlay = adw::ToastOverlay::new();
    let window = gtk::Window::builder()
        .default_width(420)
        .default_height(240)
        .child(&overlay)
        .build();
    window.present();

    let fired = Rc::new(Cell::new(0));
    let fired_for_callback = fired.clone();
    let toast = success_with_action(&overlay, "已收藏", "撤销", move || {
        fired_for_callback.set(fired_for_callback.get() + 1)
    });

    assert_eq!(toast.button_label().as_deref(), Some("撤销"));
    assert!(
        toast.timeout() > 3,
        "an undo has to stay reachable, got {}s",
        toast.timeout()
    );
    assert_eq!(fired.get(), 0);
    toast.emit_by_name::<()>("button-clicked", &[]);
    assert_eq!(fired.get(), 1, "the toast button must run the rollback");
}
