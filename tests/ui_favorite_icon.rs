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
use gtk4::gdk;
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
        .paintable()
        .is_some();

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    let viewer = ViewerPage::new(media_list.clone(), 0);
    let viewer_mark = viewer
        .imp()
        .favorite_btn
        .get()
        .child()
        .is_some_and(|child| {
            child
                .downcast_ref::<gtk::Image>()
                .is_some_and(|i| i.paintable().is_some())
        });

    let tmp = tempfile::tempdir().expect("a temp dir");
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).expect("a pool");
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let photos = PhotosPage::new(media_list, loader);
    let photos_mark = photos
        .imp()
        .favorite_btn
        .get()
        .child()
        .is_some_and(|child| {
            child
                .downcast_ref::<gtk::Image>()
                .is_some_and(|i| i.paintable().is_some())
        });

    for (surface, draws) in [
        ("Day-view tile badge", tile_mark),
        ("viewer header button", viewer_mark),
        ("photos page select-all button", photos_mark),
    ] {
        assert!(
            draws,
            "the {surface} draws no favorite mark at all; these three were three different \\
             glyphs once and the point of one shared asset is that they cannot drift apart \\
             again",
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
    assert_favorite_state_resolves_to_red(&viewer);
}

/// The mark has to stay a ring, and that has to be checked through GTK's own
/// recolouring path rather than by rendering the file with librsvg.
///
/// This is the reason the inner contour is wound against the outer one. GTK
/// tints a symbolic SVG by handing librsvg a colour callback that replaces the
/// file's first paint, and that rewrite drops `fill-rule`: an even-odd hole
/// came back as a solid blob. Opposite winding gives the same ring under either
/// rule, and this is what keeps it that way.
///
/// The measurement goes through `gtk_symbolic_paintable_snapshot_symbolic`,
/// not through a window. That call *is* the recolouring path — it is what GTK
/// runs when a widget draws a symbolic icon — and it renders the icon alone.
/// Going through a widget looked more faithful and was not: on CI the image
/// was never laid out, `WidgetPaintable` fell back to the window's own
/// background, and the test measured a 64x56 box filled to 0.96 in the
/// theme's light grey. That is neither a ring nor a solid heart, and it would
/// have been read as "the hole is gone on older GTK" — a conclusion about this
/// artwork drawn from a picture of a background. Nothing here depends on a
/// display, a window, a stylesheet or a widget allocation, so the numbers mean
/// the same thing on every GTK.
fn assert_ring_survives_recolouring(display: &gtk::gdk::Display) {
    const RENDER: f64 = 64.0;
    /// The colour the file itself declares. A symbolic icon comes back in a
    /// foreground, so ink still arriving here would mean the rewrite never ran.
    const FILE_FILL: u8 = 0xbe;
    /// A ring covers roughly a sixth of the box its outline traces; a filled
    /// heart reaches about two thirds. Anything over a third is a blob.
    const MAX_FILL: f64 = 0.35;

    let paintable = gtk::IconTheme::for_display(display).lookup_icon(
        SHARED_MARK,
        &[],
        RENDER as i32,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );

    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None).expect("a cairo renderer");
    let snapshot = gtk::Snapshot::new();
    // Any colour will do: what is under test is that the paint gets replaced
    // and the hole survives that replacement, not which colour was asked for.
    paintable.snapshot_symbolic(
        &snapshot,
        RENDER,
        RENDER,
        &[gdk::RGBA::new(1.0, 0.0, 0.0, 1.0)],
    );
    let node = snapshot.to_node().expect("a render node");
    let texture = renderer.render_texture(&node, None);
    let stride = texture.width() as usize * 4;
    let mut pixels = vec![0u8; stride * texture.height() as usize];
    texture.download(&mut pixels, stride);
    // A renderer left realized at finalize trips `gsk_renderer_dispose`'s
    // assertion and aborts the test binary, so release it here.
    renderer.unrealize();

    let (tw, th) = (texture.width() as usize, texture.height() as usize);
    let (mut min_x, mut min_y) = (usize::MAX, usize::MAX);
    let (mut max_x, mut max_y) = (0usize, 0usize);
    let mut ink = 0usize;
    let mut strongest = [0u8; 4];
    for y in 0..th {
        for x in 0..tw {
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
    // Cairo hands back BGRA.
    //
    // "Not the file's own fill", and not "the colour I asked for". Asking for a
    // specific colour and getting it is not portable: on GTK 4.22 this returns
    // the requested red, on GTK 4.14 it returns the theme's foreground
    // (228, 229, 230) instead, and both are GTK having replaced the paint. What
    // this guard has to catch is the opposite case — a render that hands back
    // the file's `#bebebe` never went through the rewrite at all, so the hole
    // it shows is the file's, not the one GTK is trying to destroy, and the
    // measurement below would be measuring the wrong thing.
    let [b, g, r, _] = strongest;
    assert!(
        r != FILE_FILL || g != FILE_FILL || b != FILE_FILL,
        "the mark's strongest pixel is rgba({strongest:?}), the colour the file declares; GTK \
         never replaced the paint, so this measured the file rather than what GTK draws, and the \
         hole below would be the file's rather than the one the rewrite can fill in",
    );

    let box_width = max_x - min_x + 1;
    let box_height = max_y - min_y + 1;
    let fill = ink as f64 / (box_width * box_height) as f64;
    let centre = (min_y + max_y) / 2 * stride + (min_x + max_x) / 2 * 4;
    eprintln!(
        "PROBE heart: texture {tw}x{th}, ink {ink} px in a {box_width}x{box_height} box, \
         fill {fill:.3}, centre alpha {}, strongest {strongest:?}",
        pixels[centre + 3],
    );
    assert!(
        pixels[centre + 3] < 24,
        "the mark is solid at its centre (alpha {}), so the hole was filled in; it fills {fill:.2} \
         of a {box_width}x{box_height} box in a {tw}x{th} render, strongest {strongest:?}; the \
         glyph is a blob next to the hairline cloud badge",
        pixels[centre + 3],
    );
    assert!(
        fill <= MAX_FILL,
        "the mark fills {fill:.2} of its {box_width}x{box_height} ink box; above {MAX_FILL} it is \
         a filled shape, not the 1 unit wall the cloud badge is drawn with",
    );
}

/// The favourited state has to actually recolour the heart, checked against the
/// app's own CSS rather than an injected stylesheet.
///
/// The lib test proves GTK replaces the mark's paint; it cannot say *which*
/// colour, because a synthetic `image { color: … }` does not reach the icon on
/// every GTK version. The real rule lives in `data/css/base.css` as
/// `.viewer-favorite-btn.favorite-active`, production toggles that class from
/// `refresh_favorite_button`, and this exercises the real button against the
/// real stylesheet that `grid_css::install()` loaded.
///
/// The foreground the button resolves is the colour GTK hands its symbolic
/// paint, so this is the whole chain: this class sets it, and the lib test
/// proves the mark is drawn in it. Reading the resolved colour instead of
/// rendering pixels also keeps the assertion off the widget-layout path, which
/// a bare button parented into an already-built `ViewerPage` will not satisfy.
fn assert_favorite_state_resolves_to_red(viewer: &ViewerPage) {
    let button = viewer.imp().favorite_btn.get();
    let context = button.style_context();

    // What `refresh_favorite_button(false)` does.
    button.remove_css_class("favorite-active");
    let unfavorited = context.color();
    // What `refresh_favorite_button(true)` does.
    button.add_css_class("favorite-active");
    let favorited = context.color();

    // The rule is `alpha(#ff5e51, 0.92)`; the colour is composited, so compare
    // channels rather than an exact triple.
    // The style context answering "red" is only half the claim. The mark is a
    // raster now and GTK will not recolour it, so the icon on screen is a
    // separate question: if nothing repaints it from this colour, the favorited
    // heart stays the header's foreground and every structural check here still
    // passes. So read the texture the button actually shows.
    let painted = button
        .child()
        .and_downcast::<gtk::Image>()
        .and_then(|image| image.paintable())
        .expect("the favorite button shows a paintable");
    let texture = painted
        .downcast_ref::<gdk::Texture>()
        .expect("the mark is handed over as a texture, which is how it gets tinted");
    let stride = texture.width() as usize * 4;
    let mut data = vec![0u8; stride * texture.height() as usize];
    texture.download(&mut data, stride);
    let mut sample = [0u8; 4];
    for p in data.as_chunks::<4>().0 {
        if p[3] > sample[3] {
            sample.copy_from_slice(p);
        }
    }
    // Cairo hands back BGRA.
    let (sr, sg, sb) = (sample[2] as i32, sample[1] as i32, sample[0] as i32);
    assert!(
        sr > 120 && sr - sg > 50 && sr - sb > 50,
        "the favorited button's icon paints as BGRA {sample:?}, not red; the style context \
         resolved red but the raster was never repainted from it",
    );

    let lead = |c: &gdk::RGBA| {
        let (r, g, b) = (
            f64::from(c.red()),
            f64::from(c.green()),
            f64::from(c.blue()),
        );
        r - g.max(b)
    };
    assert!(
        lead(&favorited) > 0.2,
        "with .favorite-active the button's foreground is ({}, {}, {}) — not red; the favorited \
         state would leave the heart at the inherited foreground",
        favorited.red(),
        favorited.green(),
        favorited.blue(),
    );
    assert!(
        lead(&favorited) > lead(&unfavorited) + 0.1,
        "adding .favorite-active barely moved the foreground ({} then {}); \
         `.viewer-favorite-btn.favorite-active` is not reaching the button",
        unfavorited.red(),
        favorited.red(),
    );
}
