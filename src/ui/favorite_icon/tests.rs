use super::*;
use gtk4 as gtk;
use gtk4::prelude::*;

/// The mark has to resolve from the app's own GResource, and GTK has to
/// treat it as symbolic. Both are silent failures otherwise: a missing
/// resource path leaves the buttons with no icon at all, and a non-symbolic
/// asset would ignore the CSS that makes the favorited state red.
#[gtk::test]
fn bundled_mark_resolves_from_the_icon_theme() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    let display = gtk::gdk::Display::default().expect("a display");
    let paintable = gtk::IconTheme::for_display(&display).lookup_icon(
        NAME,
        &[],
        16,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );
    assert!(
        paintable.is_symbolic(),
        "{NAME} must resolve as a symbolic icon so CSS `color` can recolor it",
    );
    assert_eq!(
        (paintable.intrinsic_width(), paintable.intrinsic_height()),
        (16, 16),
        "{NAME} should render at the requested icon size",
    );
}

/// Pins the property the favorited state depends on: rendering the mark
/// under a CSS `color` must produce that color's pixels. A switch to a
/// baked-tone bitmap would still pass every structural check and silently
/// leave the heart stuck at one color.
#[gtk::test]
fn css_color_recolors_the_mark() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    let display = gtk::gdk::Display::default().expect("a display");
    let provider = gtk::CssProvider::new();
    provider.load_from_data("image { color: #ff0000; }");
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let pixels = render_through_a_widget(NAME, 40);

    // Cairo hands back BGRA, so the red channel is the third byte.
    let mut strongest = 0u8;
    let mut red = 0usize;
    for px in pixels.chunks_exact(4) {
        if px[3] > strongest {
            strongest = px[3];
        }
        if px[3] > 200 && px[2] > 200 {
            red += 1;
        }
    }
    assert!(
        strongest > 200,
        "{NAME} rendered no opaque pixels; it did not draw at all",
    );
    assert!(
        red > 20,
        "{NAME} should be recolored by CSS `color`, but only {red} pixels came out \
         of the requested red",
    );
}

/// Render the named icon the way a widget draws it, and hand back the
/// texture's BGRA bytes.
///
/// Drawing the *widget* is the point, not a convenience. GTK recolours a
/// symbolic icon from the style context of the widget drawing it, so a
/// paintable taken straight from `IconTheme` comes back in the file's own
/// colour and proves nothing about the recolouring. And the widget has to be
/// laid out first: without a size to draw into, `WidgetPaintable` snapshots
/// nothing at all and the caller measures an empty texture.
fn render_through_a_widget(name: &str, pixel_size: i32) -> Vec<u8> {
    let image = gtk::Image::from_icon_name(name);
    image.set_pixel_size(pixel_size);
    let window = gtk::Window::new();
    window.set_default_size(pixel_size * 2, pixel_size * 2);
    window.set_child(Some(&image));
    window.present();
    // Layout waits on the display server, so a fixed sleep can expire
    // before the first allocation. `iteration(false)` is the non-blocking
    // form and is what lets the replies land — `iteration(true)` blocks
    // forever once the queue drains — and the deadline keeps a display
    // that never lays out from hanging the suite.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while (image.width() == 0 || image.height() == 0) && std::time::Instant::now() < deadline {
        gtk::glib::MainContext::default().iteration(false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        image.width() > 0 && image.height() > 0,
        "{name}: its host window was never laid out, so there is nothing to measure",
    );

    let widget_paintable = gtk::WidgetPaintable::new(Some(&image));
    let (width, height) = (image.width() as f64, image.height() as f64);
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None).expect("a cairo renderer");
    let snapshot = gtk::Snapshot::new();
    widget_paintable.snapshot(&snapshot, width, height);
    let node = snapshot.to_node().expect("the mark should render");
    let texture = renderer.render_texture(&node, None);
    let stride = texture.width() as usize * 4;
    let mut pixels = vec![0u8; stride * texture.height() as usize];
    texture.download(&mut pixels, stride);
    // A renderer left realized at finalize trips `gsk_renderer_dispose`'s
    // assertion and aborts the test binary, so release it here.
    renderer.unrealize();
    pixels
}
