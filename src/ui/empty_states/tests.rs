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

/// P2-6: the album picker reused this factory, so the loading page is now on the
/// happy path of every dialog open. A title with nothing under it reads as a
/// stalled page.
#[gtk::test]
fn the_loading_placeholder_spins_too() {
    let page = super::loading();
    assert_eq!(
        page.title().as_str(),
        tr("empty.loading"),
        "the loading page keeps the shared wording rather than inventing one per call site"
    );
    let spinner = page
        .child()
        .expect("the loading page carries a spinner")
        .downcast::<gtk::Spinner>()
        .expect("the loading page's child should be the spinner");
    assert!(spinner.is_spinning(), "same rule as the scanning page");
}

/// A failed read must state the reason and offer the one action that can help.
/// Rendering it as an empty state is what P2-6 was about.
#[gtk::test]
fn a_failed_read_names_the_reason_and_offers_retry() {
    use std::cell::Cell;
    let clicked = Rc::new(Cell::new(false));
    let for_button = clicked.clone();
    let page = super::load_failed(
        "no such table: albums",
        Rc::new(move || for_button.set(true)),
    );

    assert_eq!(page.title().as_str(), tr("empty.load_failed.title"));
    assert_eq!(
        page.description().as_deref(),
        Some(
            trf(
                "empty.load_failed.description_with_reason",
                &[("reason", "no such table: albums")]
            )
            .as_str()
        ),
        "the database reason must be visible, not only in the log"
    );
    assert_eq!(
        super::load_failed_text(Some("   ")).as_str(),
        tr("empty.load_failed.description").as_str(),
        "a blank reason falls back to the generic text instead of an empty sentence"
    );

    let button = page
        .child()
        .expect("the error page carries its action")
        .downcast::<gtk::Button>()
        .expect("the error page's child should be the retry button");
    assert_eq!(button.label().as_deref(), Some(tr("common.retry").as_str()));
    button.emit_clicked();
    assert!(clicked.get(), "the retry button must reach the caller");
}
