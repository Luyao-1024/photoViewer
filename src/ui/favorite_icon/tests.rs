use super::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;

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

/// Pins the property everything else leans on: GTK's symbolic path has to
/// replace the mark's own paint. A baked-tone bitmap or an unresolved
/// paintable would pass every structural check here and still leave the
/// favorited state grey.
///
/// The two snapshots are the whole point — the same paintable, drawn once
/// plainly and once through `gtk_symbolic_paintable_snapshot_symbolic`. The
/// plain draw has to come back in the file's own `#bebebe` and the symbolic
/// one in the requested red. Either half alone proves less: "it drew
/// something" and "it is not grey" both pass on a picture of a window
/// background, which is exactly what an earlier version of this test was
/// measuring on CI, where the widget it went through was never laid out.
///
/// Nothing here needs a display, a window or a stylesheet, so the assertion
/// cannot be moved by what another test installed on the same display, and it
/// reads the same on every GTK version.
#[gtk::test]
fn the_symbolic_path_replaces_the_own_paint() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    let display = gtk::gdk::Display::default().expect("a display");
    let paintable = gtk::IconTheme::for_display(&display).lookup_icon(
        NAME,
        &[],
        RENDER,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );

    let plain = draw(&paintable, false);
    let recoloured = draw(&paintable, true);

    // The colour the file itself declares.
    const FILE_FILL: u8 = 0xbe;
    assert!(
        plain[3] > 200,
        "{NAME} rendered no opaque pixels at all; it did not draw",
    );
    assert!(
        plain[0] == FILE_FILL && plain[1] == FILE_FILL && plain[2] == FILE_FILL,
        "drawn plainly the mark's strongest pixel is rgba({plain:?}), not the \
         #bebebe the file declares; if the file itself has changed, this comparison no longer \
         distinguishes a plain draw from a recoloured one",
    );
    assert!(
        recoloured[3] > 200,
        "drawn symbolically the mark rendered no opaque pixels at all",
    );
    let (r, g, b) = (
        recoloured[2] as i32,
        recoloured[1] as i32,
        recoloured[0] as i32,
    );
    assert!(
        r > 200 && r - g > 60 && r - b > 60,
        "drawn symbolically the mark's strongest pixel is rgba({recoloured:?}), not the red that \
         was requested; GTK did not replace the paint, so the mark is not recolourable and the \
         favorited state would leave it stuck at one colour",
    );
}

const RENDER: i32 = 64;

/// Draw `paintable` at `RENDER` and hand back its strongest BGRA pixel.
/// `symbolic` picks which of GTK's two paths runs, and that is the difference
/// under test: the plain path keeps the file's paint, the symbolic one
/// replaces it.
fn draw(paintable: &gtk::IconPaintable, symbolic: bool) -> [u8; 4] {
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None).expect("a cairo renderer");
    let snapshot = gtk::Snapshot::new();
    if symbolic {
        paintable.snapshot_symbolic(
            &snapshot,
            f64::from(RENDER),
            f64::from(RENDER),
            &[gdk::RGBA::new(1.0, 0.0, 0.0, 1.0)],
        );
    } else {
        paintable.snapshot(&snapshot, f64::from(RENDER), f64::from(RENDER));
    }
    let node = snapshot.to_node().expect("a render node");
    let texture = renderer.render_texture(&node, None);
    let stride = texture.width() as usize * 4;
    let mut pixels = vec![0u8; stride * texture.height() as usize];
    texture.download(&mut pixels, stride);
    // A renderer left realized at finalize trips `gsk_renderer_dispose`'s
    // assertion and aborts the test binary, so release it here.
    renderer.unrealize();

    let mut strongest = [0u8; 4];
    for [b, g, r, a] in pixels.as_chunks::<4>().0 {
        if *a > strongest[3] {
            strongest = [*b, *g, *r, *a];
        }
    }
    strongest
}
