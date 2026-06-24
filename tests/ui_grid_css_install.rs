//! Verifies grid_css::install() guard semantics.
//!
//! GTK is single-threaded; one `#[test]` per the project's pattern.

#[test]
fn install_guard_works() {
    gtk4::init().expect("GTK init failed");

    // Positive: after install(), is_installed() is true.
    photo_viewer::ui::grid_css::install();
    assert!(
        photo_viewer::ui::grid_css::is_installed(),
        "is_installed() must be true after install()",
    );

    // assert_installed() must be a no-op (no panic) when install has run.
    photo_viewer::ui::grid_css::assert_installed();
}
