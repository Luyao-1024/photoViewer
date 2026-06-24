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

#[test]
fn grid_css_avoids_known_unsupported_gtk_properties() {
    let css = photo_viewer::ui::grid_css::css_for_tests();

    assert!(
        !css.contains("spacing:"),
        "GTK CSS does not support `spacing`; set GtkBox spacing in the widget builder/template"
    );
}
