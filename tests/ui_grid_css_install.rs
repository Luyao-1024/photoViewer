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
fn grid_css_keeps_web_only_rules_outside_liquid_blur() {
    let css = photo_viewer::ui::grid_css::css_for_tests();

    assert!(
        !css.contains("spacing:"),
        "GTK CSS does not support `spacing`; set GtkBox spacing in the widget builder/template"
    );
    assert!(
        css.contains("backdrop-filter: blur("),
        "Liquid Glass intentionally keeps backdrop-filter because it is the desired visual material"
    );
    assert!(
        !css.contains("@media ("),
        "GTK CssProvider in the supported runtime rejects CSS @media rules"
    );
    assert!(
        !css.contains("@keyframes") && !css.contains("animation:"),
        "GTK CssProvider in the supported runtime rejects web keyframe animation"
    );
}
