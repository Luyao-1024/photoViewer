use super::*;
use gtk4 as gtk;
use gtk4::prelude::*;

/// Rendered ink of one badge, in device pixels of the render.
struct Ink {
    width: i32,
    height: i32,
    box_size: i32,
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
    pixels: usize,
}

fn ink_of(state: CloudState, dark: bool, box_size: f64) -> Ink {
    let _ = gtk::init();
    crate::ensure_resources_registered();
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None).unwrap();
    let image = gtk::Image::from_resource(resource(state, dark));
    let paintable = image.paintable().unwrap();
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, box_size, box_size);
    let node = snapshot.to_node().unwrap();
    let texture = renderer.render_texture(&node, None);
    let stride = texture.width() as usize * 4;
    let mut pixels = vec![0u8; stride * texture.height() as usize];
    texture.download(&mut pixels, stride);

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = -1;
    let mut max_y = -1;
    let mut opaque = 0usize;
    for y in 0..texture.height() as usize {
        for x in 0..texture.width() as usize {
            if pixels[y * stride + x * 4 + 3] > 24 {
                min_x = min_x.min(x as i32);
                min_y = min_y.min(y as i32);
                max_x = max_x.max(x as i32);
                max_y = max_y.max(y as i32);
                opaque += 1;
            }
        }
    }
    // A renderer that is still realized when it is finalized trips
    // `gsk_renderer_dispose`'s assertion and aborts the whole test binary
    // on some GTK versions, so release it here rather than at drop.
    renderer.unrealize();

    Ink {
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
        box_size: box_size as i32,
        min_x,
        min_y,
        max_x,
        max_y,
        pixels: opaque,
    }
}

fn variants() -> [(CloudState, bool); 4] {
    [
        (CloudState::Synced, true),
        (CloudState::Off, true),
        (CloudState::Synced, false),
        (CloudState::Off, false),
    ]
}

/// The badge sits in the same row as the toolbar's favorite glyph, which is
/// a hairline outline. Two things used to put it in a different family:
/// the artwork left most of its box empty, so it read smaller, and the
/// stroke was twice as heavy as its neighbour's. Both are artwork
/// properties, so they are asserted from the rendered pixels: rendered into
/// a 16 px box, the ink has to fill it the way a 16 px outline icon does,
/// and the silhouette has to stay close to square. The box is a nominal
/// size for this assertion; the real one is whatever the header resolves,
/// which `ViewerPage` copies onto the badge at runtime.
#[gtk::test]
fn badge_ink_fills_its_box_at_header_size() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    // A 16 px outline icon draws its ink over roughly 7/8 of the box, and a
    // cloud cannot be both square and a cloud, so 2/3 is the floor.
    const MIN_HEIGHT_FRACTION: f64 = 0.66;
    const MIN_WIDTH_FRACTION: f64 = 0.75;
    // Neighbouring toolbar glyphs are square, and a silhouette much wider
    // than it is tall spends its box on width the eye reads as emptiness:
    // that is the mechanism behind "the cloud looks smaller than the
    // heart". The old artwork measured 18x13 (1.38) in its box; this one
    // measures 14x12 (1.17).
    const MAX_ASPECT: f64 = 1.25;

    for (state, dark) in variants() {
        let ink = ink_of(state, dark, 16.0);
        let height_fraction = ink.height as f64 / ink.box_size as f64;
        let width_fraction = ink.width as f64 / ink.box_size as f64;
        assert!(
            height_fraction >= MIN_HEIGHT_FRACTION,
            "{state:?}/dark={dark}: ink height {}/{} of the box ({height_fraction:.2}) \
             is below {MIN_HEIGHT_FRACTION}; the badge would read smaller than the \
             favorite button beside it",
            ink.height,
            ink.box_size,
        );
        assert!(
            width_fraction >= MIN_WIDTH_FRACTION,
            "{state:?}/dark={dark}: ink width {}/{} of the box ({width_fraction:.2}) \
             is below {MIN_WIDTH_FRACTION}",
            ink.width,
            ink.box_size,
        );
        let aspect = ink.width as f64 / ink.height as f64;
        assert!(
            aspect <= MAX_ASPECT,
            "{state:?}/dark={dark}: ink is {}x{} ({aspect:.2}) — wider than {MAX_ASPECT} \
             reads as a squat glyph next to a square one",
            ink.width,
            ink.height,
        );
    }
}

/// A hairline is roughly a quarter of the ink pixels its bounding box could
/// hold: the stroke traces a perimeter, it does not fill the shape. The old
/// stroke-width 2 artwork came out near 0.6 and read as a heavy double
/// edge next to a vector outline icon.
#[gtk::test]
fn badge_stroke_is_a_hairline() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    // Rendered large so the ratio is not dominated by antialiasing.
    const MAX_INK_FRACTION: f64 = 0.42;
    for (state, dark) in variants() {
        let ink = ink_of(state, dark, 64.0);
        let area = (ink.width * ink.height) as f64;
        let fraction = ink.pixels as f64 / area;
        assert!(
            fraction <= MAX_INK_FRACTION,
            "{state:?}/dark={dark}: {} ink px over a {}x{} box ({fraction:.2}) is too \
             heavy for a hairline; the badge would read bolder than the favorite button",
            ink.pixels,
            ink.width,
            ink.height,
        );
    }
}

/// Switching sync state or light/dark must not move or resize the glyph:
/// all four assets are the same silhouette, so the header row never
/// shifts, and the viewer can keep a single notion of the badge's size.
#[gtk::test]
fn every_badge_variant_occupies_the_same_box() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    for box_size in [16.0, 18.0] {
        let reference = ink_of(CloudState::Synced, true, box_size);
        for (state, dark) in variants() {
            let ink = ink_of(state, dark, box_size);
            assert_eq!(
                (ink.min_x, ink.min_y, ink.max_x, ink.max_y),
                (
                    reference.min_x,
                    reference.min_y,
                    reference.max_x,
                    reference.max_y
                ),
                "{state:?}/dark={dark} at {box_size}px does not share the synced badge's \
                 ink box; switching state or theme would move the icon",
            );
        }
    }
}

/// The bitmaps are drawn at 18 px (grid tile) or at the header's icon size
/// (viewer), and the display may well be 2x. Shipping exactly the drawn
/// size is what made the badge soft while the vector toolbar icons stayed
/// sharp, so require real headroom.
#[gtk::test]
fn badges_carry_headroom_for_hidpi() {
    let _ = gtk::init();
    crate::ensure_resources_registered();

    const DRAWN_AT: i32 = 18;
    const MIN_SCALE_FACTOR: i32 = 4;
    for (state, dark) in variants() {
        let image = gtk::Image::from_resource(resource(state, dark));
        let paintable = image
            .paintable()
            .unwrap_or_else(|| panic!("{} should load", resource(state, dark)));
        let (w, h) = (paintable.intrinsic_width(), paintable.intrinsic_height());
        assert!(
            w >= DRAWN_AT * MIN_SCALE_FACTOR && h >= DRAWN_AT * MIN_SCALE_FACTOR,
            "{} is {w}x{h}; a badge drawn at {DRAWN_AT}px needs at least {} source pixels \
             to stay crisp at 2x",
            resource(state, dark),
            DRAWN_AT * MIN_SCALE_FACTOR,
        );
    }
}
