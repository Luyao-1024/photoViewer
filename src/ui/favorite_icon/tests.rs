use super::*;
use gtk4 as gtk;

/// The mark has to resolve from the app's own GResource. A missing resource
/// path is a silent failure: the buttons simply draw nothing, which is exactly
/// what `emblem-favorite-symbolic` did on a stock GNOME system.
///
/// This deliberately does not assert `gtk_icon_paintable_is_symbolic()`. That
/// call is deprecated as of GTK 4.20 and its own documentation says it judges
/// by file name only, "this behaviour may change in the future" — and it does
/// differ: it reports false for this resource-loaded SVG on CI's older GTK
/// while the icon still resolves and still takes a CSS `color`. The property
/// that actually matters is pinned by `css_color_recolors_the_mark` below,
/// which renders the mark through a widget and checks the requested colour
/// reaches the pixels. Asserting the flag here tested a deprecated flag's
/// version-dependent reporting, not the contract.
#[gtk::test]
fn bundled_mark_resolves_from_the_icon_theme() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    let display = gtk::gdk::Display::default().expect("a display");
    let theme = gtk::IconTheme::for_display(&display);
    let resource_path = crate::ICON_RESOURCE_PATH;
    assert!(
        theme.has_icon(NAME),
        "{NAME} is not in the icon theme, so the favorite controls would draw nothing at all; \
         the theme's resource path is {resource_path:?} and the asset has to sit under it",
    );
    let paintable = theme.lookup_icon(
        NAME,
        &[],
        16,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );
    assert_eq!(
        (paintable.intrinsic_width(), paintable.intrinsic_height()),
        (16, 16),
        "{NAME} should render at the requested icon size, resolved to {:?}",
        paintable
            .file()
            .and_then(|f| f.path())
            .map(|p| p.to_string_lossy().into_owned()),
    );
}
