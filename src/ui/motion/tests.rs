use super::*;
use std::cell::RefCell;
use std::rc::Rc;

/// The whole strategy rests on GTK accepting a universal-selector override in
/// its CSS subset - `grid_css.rs` forbids @media, so this is the only lever.
/// A parsing error here would silently leave every transition in place.
///
/// The sheet parsed is the one [`install`] would actually register: on a GTK
/// older than 4.22 that is the authored CSS minus `backdrop-filter`, which that
/// runtime cannot parse. Asserting on the authored sheet instead made this fail
/// on CI's GTK 4.14 for a property that has nothing to do with motion — the
/// assertion is about the reduce-motion tail, so it has to be made about the
/// text this runtime is handed.
#[gtk::test]
fn reduce_motion_block_parses_in_gtk_css() {
    let errors = Rc::new(RefCell::new(Vec::new()));
    let provider = gtk::CssProvider::new();
    let recorded = errors.clone();
    provider
        .connect_parsing_error(move |_, _, error| recorded.borrow_mut().push(error.to_string()));
    provider.load_from_data(&crate::ui::grid_css::runtime_compatible_css(
        &crate::ui::grid_css::css_for_tests_with_reduced_motion(),
    ));
    assert!(
        errors.borrow().is_empty(),
        "GTK rejected the reduce-motion sheet: {:?}",
        errors.borrow()
    );
}

#[gtk::test]
fn reduced_motion_strips_template_transitions() {
    let settings = gtk::Settings::default().expect("settings backend");
    let previous = settings.is_gtk_enable_animations();
    settings.set_gtk_enable_animations(false);

    // The shape photos-page.blp uses: a stack of pages, one of which holds a
    // revealer, so the walk has to reach past the first level.
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(200)
        .build();
    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .build();
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.append(&revealer);
    stack.add_named(&inner, Some("a"));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&stack);

    apply_to(&root);

    assert_eq!(
        revealer.transition_type(),
        gtk::RevealerTransitionType::None,
        "a template slide must not survive reduce-motion"
    );
    assert_eq!(stack.transition_type(), gtk::StackTransitionType::None);
    assert_eq!(stack.transition_duration(), 0);

    settings.set_gtk_enable_animations(previous);
}

#[gtk::test]
fn animations_left_on_keep_the_authored_transitions() {
    let settings = gtk::Settings::default().expect("settings backend");
    let previous = settings.is_gtk_enable_animations();
    settings.set_gtk_enable_animations(true);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&revealer);

    apply_to(&root);

    assert_eq!(
        revealer.transition_type(),
        gtk::RevealerTransitionType::SlideDown,
        "the preference is opt-out; nothing may be stripped while it is on"
    );

    settings.set_gtk_enable_animations(previous);
}
