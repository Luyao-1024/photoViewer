//! The grid CSS provider keeps web-style at-rules out of the runtime CSS.
//! Liquid Glass deliberately uses `backdrop-filter` for visual quality, but
//! accessibility media queries remain disabled because GTK rejects them as
//! unknown at-rules in the supported runtime.
//!
//! GTK is single-threaded; all checks live in one `#[test]` function.

use gtk4 as gtk;
use photo_viewer::ui::grid_css;

#[test]
fn accessibility_css_avoids_unsupported_media_queries() {
    gtk::init().expect("GTK init failed");
    grid_css::install();

    let css = grid_css::css_for_tests();

    assert!(
        !css.contains("@media ("),
        "GTK CssProvider rejects web-style @media feature queries"
    );
    assert!(
        css.contains("backdrop-filter: blur("),
        "Liquid Glass should keep the backdrop-filter material"
    );
    assert!(
        !css.contains("@keyframes") && !css.contains("animation:"),
        "GTK CssProvider rejects web keyframe animation syntax"
    );
}
