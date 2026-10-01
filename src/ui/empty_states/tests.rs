use super::*;

#[gtk::test]
fn the_scanning_placeholder_spins_instead_of_showing_a_frozen_circle() {
    let page = super::scanning();
    let child = page
        .child()
        .expect("the scanning placeholder carries its spinner as the page child");
    let spinner = child
        .downcast::<gtk::Spinner>()
        .expect("the scanning placeholder's child should be the spinner");
    assert!(
        spinner.is_spinning(),
        "GtkSpinner does not animate until started; a stopped circle reads as a frozen page, not as progress"
    );
}
