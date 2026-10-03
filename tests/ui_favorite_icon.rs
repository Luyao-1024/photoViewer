//! The favorite mark has to look the same everywhere it appears.
//!
//! Three surfaces show it: the viewer header button, the photos page select-all
//! button, and the Day-view grid tile badge. They were three different glyphs —
//! the buttons asked the icon theme for `emblem-favorite-symbolic`, which
//! adwaita-icon-theme does not ship at all, while the tile badge was a "♡" text
//! character at 18pt shaped by whatever font the system resolved. Now all
//! three take one app-owned name from `favorite_icon::NAME`, and this test is
//! what keeps it that way: consistency has to be checked against the real
//! widgets, because a name that lives in three templates can drift.
//!
//! GTK is single-threaded and its windows interfere with each other inside one
//! process, so this file holds a single `#[test]`, following the pattern in
//! `tests/ui_viewer_toolbar.rs`.

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::thumbnails::ThumbnailLoader;
use photo_viewer::ui::{PhotosPage, SquareTile, ViewerPage};
use std::sync::Arc;

/// The icon name every favorite control is expected to carry, repeated here on
/// purpose: the crate's constant is `pub(crate)`, so the test states the
/// contract rather than importing the answer it is meant to check.
const SHARED_MARK: &str = "photoviewer-heart-symbolic";

#[test]
fn every_favorite_control_draws_the_same_mark() {
    gtk::init().expect("GTK init failed");
    photo_viewer::ensure_resources_registered();

    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.FavoriteIcon")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    photo_viewer::ui::grid_css::install();

    let tile = SquareTile::new();
    let tile_mark = tile
        .imp()
        .favorite_badge
        .borrow()
        .as_ref()
        .expect("the tile builds a favorite badge")
        .icon_name()
        .map(|n| n.to_string())
        .expect("the tile badge names its icon");

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    let viewer = ViewerPage::new(media_list.clone(), 0);
    let viewer_mark = viewer
        .imp()
        .favorite_btn
        .get()
        .icon_name()
        .map(|n| n.to_string())
        .expect("the viewer button names its icon");

    let tmp = tempfile::tempdir().expect("a temp dir");
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).expect("a pool");
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let photos = PhotosPage::new(media_list, loader);
    let photos_mark = photos
        .imp()
        .favorite_btn
        .get()
        .icon_name()
        .map(|n| n.to_string())
        .expect("the photos page button names its icon");

    for (surface, mark) in [
        ("Day-view tile badge", &tile_mark),
        ("viewer header button", &viewer_mark),
        ("photos page select-all button", &photos_mark),
    ] {
        assert_eq!(
            mark, SHARED_MARK,
            "the {surface} draws {mark:?}, not the shared mark; these three were three \\
             different glyphs once and the point of the shared name is that they cannot \\
             drift apart again",
        );
    }

    // And the name has to mean something: an unresolvable name leaves the
    // controls drawing nothing at all, which is what the themed emblem did on
    // a stock GNOME system.
    //
    // Deliberately no `is_symbolic()` here either. That call is deprecated as of
    // GTK 4.20 and reports differently across versions — false for this
    // resource-loaded SVG on CI's older GTK, on a build where the mark still
    // resolves and still takes a CSS `color`. Whether it recolours is not a
    // question for a flag: `assert_ring_survives_recolouring` below proves it by
    // rendering the widget and reading the pixels.
    let display = gtk::gdk::Display::default().expect("a display");
    let theme = gtk::IconTheme::for_display(&display);
    assert!(
        theme.has_icon(SHARED_MARK),
        "{SHARED_MARK} is not in the icon theme, so the favorite controls would draw nothing",
    );
    let paintable = theme.lookup_icon(
        SHARED_MARK,
        &[],
        16,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );
    assert_eq!(
        paintable.intrinsic_width(),
        16,
        "{SHARED_MARK} must resolve from the bundled resources as a 16 px icon, resolved to {:?}",
        paintable
            .file()
            .and_then(|f| f.path())
            .map(|p| p.to_string_lossy().into_owned()),
    );

    assert_ring_survives_recolouring(&display);
}

/// The mark has to stay a ring, and that has to be checked through GTK's own
/// recolouring pipeline rather than by rendering the file with librsvg.
///
/// This is the reason the inner contour is wound against the outer one. GTK
/// tints a symbolic SVG by handing librsvg a colour callback that replaces the
/// file's first paint, and that rewrite drops the `fill-rule` attribute: an
/// even-odd hole was filled in and the mark came back as a solid blob. The hole
/// surviving in a browser, or in a plain librsvg render, is not evidence —
/// only what GTK itself draws is. Opposite winding gives the same ring under
/// nonzero and under even-odd, and this is what keeps it that way.
///
/// The colour matters as much as the shape. A paintable taken straight from
/// `IconTheme` carries no style context, so GTK never recolours it and the
/// render comes back in the file's own `#bebebe`; drawing the widget instead
/// is what puts the CSS colour into the callback, and the assertions below are
/// worthless unless that replacement actually happened.
fn assert_ring_survives_recolouring(display: &gtk::gdk::Display) {
    const RENDER: i32 = 64;
    /// The colour the file itself declares. GTK replaces a symbolic icon's
    /// paint with the foreground, so ink still arriving in this colour would
    /// mean the rewrite never ran.
    const FILE_FILL: u8 = 0xbe;
    /// A ring covers roughly a sixth of the box its outline traces; a filled
    /// heart reaches about two thirds. Anything over a third is a blob.
    const MAX_FILL: f64 = 0.35;
    /// What the CSS below asks for, so "did the rewrite run" is a comparison
    /// and not a guess.
    const CSS_RED: u8 = 0xff;

    let provider = gtk::CssProvider::new();
    provider.load_from_data("image { color: #ff0000; }");
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let image = gtk::Image::from_icon_name(SHARED_MARK);
    image.set_pixel_size(RENDER);
    let window = gtk::Window::new();
    window.set_default_size(96, 96);
    window.set_child(Some(&image));
    window.present();
    // The window has to be mapped and laid out before the image has an
    // allocation to draw into. `iteration(false)` is the non-blocking form and
    // is what lets the display server's replies land; `iteration(true)` blocks
    // forever once the queue drains. The deadline keeps a display that never
    // lays out from hanging the suite.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while (image.width() == 0 || image.height() == 0) && std::time::Instant::now() < deadline {
        gtk::glib::MainContext::default().iteration(false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        image.width() > 0 && image.height() > 0,
        "the icon host window was never laid out, so there is nothing to measure",
    );

    // `WidgetPaintable` draws the widget through its own style context, which
    // is the only place the symbolic recolouring colour comes from.
    let widget_paintable = gtk::WidgetPaintable::new(Some(&image));
    let (width, height) = (image.width() as f64, image.height() as f64);
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None).expect("a cairo renderer");
    let snapshot = gtk::Snapshot::new();
    widget_paintable.snapshot(&snapshot, width, height);
    let node = snapshot.to_node().expect("a render node");
    let texture = renderer.render_texture(&node, None);
    let stride = texture.width() as usize * 4;
    let mut pixels = vec![0u8; stride * texture.height() as usize];
    texture.download(&mut pixels, stride);
    // A renderer left realized at finalize trips `gsk_renderer_dispose`'s
    // assertion and aborts the test binary, so release it here.
    renderer.unrealize();

    // Measure against the mark's own ink box rather than the requested size:
    // the texture that comes back is whatever the paintable chose, and an
    // assertion hard-coded to it would be a trap.
    let (mut min_x, mut min_y) = (usize::MAX, usize::MAX);
    let (mut max_x, mut max_y) = (0usize, 0usize);
    let mut ink = 0usize;
    let mut strongest = [0u8; 4];
    for y in 0..texture.height() as usize {
        for x in 0..texture.width() as usize {
            let p = &pixels[y * stride + x * 4..y * stride + x * 4 + 4];
            if p[3] > strongest[3] {
                strongest.copy_from_slice(p);
            }
            if p[3] > 24 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                ink += 1;
            }
        }
    }
    assert!(
        ink > 0,
        "the mark rendered no ink at all, so the checks below would pass for the wrong reason",
    );

    // Cairo hands back BGRA, so the red channel is the third byte.
    let red = strongest[2];
    assert_eq!(
        red, CSS_RED,
        "the mark's strongest pixel is rgba({strongest:?}) — GTK did not replace the file's \
         #bebebe fill with the CSS red, so this test is not exercising the pipeline that \
         destroyed the even-odd hole",
    );
    assert!(
        red != FILE_FILL,
        "the mark came back in the colour the file declares, so the symbolic rewrite never ran",
    );

    let box_width = max_x - min_x + 1;
    let box_height = max_y - min_y + 1;
    let fill = ink as f64 / (box_width * box_height) as f64;
    let centre = (min_y + max_y) / 2 * stride + (min_x + max_x) / 2 * 4;
    assert!(
        pixels[centre + 3] < 24,
        "the mark is solid at its centre (alpha {}), so the hole was filled in; the glyph is a \
         blob next to the hairline cloud badge",
        pixels[centre + 3],
    );
    assert!(
        fill <= MAX_FILL,
        "the mark fills {fill:.2} of its {box_width}x{box_height} ink box; above {MAX_FILL} it is \
         a filled shape, not the 1 unit wall the cloud badge is drawn with",
    );
}
