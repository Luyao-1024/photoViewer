//! The favorite mark, shared by every surface that shows one.
//!
//! One app-owned icon name for all of them — the viewer header button, the
//! photos page select-all button, and the Day-view grid tile badge — because
//! the three used to be three different glyphs:
//!
//! * the two toolbar buttons asked the icon theme for
//!   `emblem-favorite-symbolic`, which adwaita-icon-theme does not ship at all
//!   (still missing in GNOME 50), so on a stock GNOME system the buttons drew
//!   no icon; only themes like Yaru happened to carry one, and each drew its own
//!   shape.
//! * the Day-view tile badge was a `GtkLabel` holding the "♡" character at
//!   18pt, so its weight and outline came from whatever font the system
//!   resolved. A text glyph in a photo grid next to a vector heart in a
//!   toolbar is the same mismatch the cloud badge had with its neighbours.
//!
//! # Why a raster, and why a hand-applied tint
//!
//! The artwork is [`data/icons/photoviewer-heart-symbolic.svg`], but the asset
//! the app loads is the PNG `data/icons/_generate.py` rasterises from it, the
//! same way the cloud badge is delivered. The vector form was not safe to ship:
//! on GTK 4.14 it did not render as the mark at all — its ink came out filling
//! the whole requested box, where the same file is a 0.16-fill ring on 4.22 —
//! and which of the two you get is not something a caller can ask about. The
//! raster renders the same shape everywhere, with librsvg out of the picture at
//! runtime.
//!
//! What the raster costs is tinting. GTK tints a symbolic *vector* by handing
//! librsvg a colour callback; there is no equivalent for a raster, so CSS
//! `color` alone will not move it. The mark is therefore stored as white on
//! transparent and [`tinted`] paints it, and every caller feeds that the colour
//! its own style context resolved — so CSS stays the single place a colour is
//! written down, and a theme change reaches the icon because the style context
//! changes.
//!
//! [`favorite_icon::tinted`]: tinted
//! [`data/icons/photoviewer-heart-symbolic.svg`]: ../../data/icons/photoviewer-heart-symbolic.svg

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// Icon name for the favorite mark — the asset's one name, and the word the
/// test resolves it through. Nothing loads it by name any more: a raster is
/// handed over as a paintable ([`tinted`]), so the path is the thing the app
/// actually uses. The name still exists because the test resolves the asset
/// through the icon theme under it, and because it is the single word all
/// three surfaces have in common.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const NAME: &str = "photoviewer-heart-symbolic";

/// Box the Day-view tile badge draws the mark in. Matches the tile's cloud
/// badge (`SquareTile`'s `sync_badge`), so the two overlays in that corner read
/// as one set, and lands on the same apparent size the old 18pt "♡" had.
pub(crate) const TILE_PIXEL_SIZE: i32 = 18;

/// The shipped asset: the mark as white on transparent, shape in alpha.
const RESOURCE: &str = "/io/github/luyao_1024/photoviewer/icons/photoviewer-heart-symbolic.png";

/// The mark painted in `colour`.
///
/// The tint is applied here rather than left to CSS because GTK will not do it
/// for a raster, and a caller that just sets a paintable would silently get a
/// white heart that ignores `.viewer-favorite-btn.favorite-active` — the
/// favorited state would stop being red with nothing failing. Feed this the
/// colour the widget's style context resolved and the CSS rule keeps working,
/// including on a theme switch.
///
/// Results are cached per colour: a theme and a favourited state are the only
/// two things that move it, and each is a handful of pixels to composite once.
pub(crate) fn tinted(colour: gdk::RGBA) -> gdk::Texture {
    static CACHE: Mutex<Option<HashMap<[u8; 4], gdk::Texture>>> = Mutex::new(None);

    let key = [
        (colour.red() * 255.0).round() as u8,
        (colour.green() * 255.0).round() as u8,
        (colour.blue() * 255.0).round() as u8,
        (colour.alpha() * 255.0).round() as u8,
    ];
    if let Some(found) = CACHE
        .lock()
        .expect("the tint cache is not poisoned")
        .as_ref()
        .and_then(|cache| cache.get(&key))
    {
        return found.clone();
    }

    let texture = paint(colour, key);
    CACHE
        .lock()
        .expect("the tint cache is not poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(key, texture.clone());
    texture
}

fn paint(_colour: gdk::RGBA, key: [u8; 4]) -> gdk::Texture {
    // `from_resource` reads the asset straight out of the GResource, so there is
    // no path to get wrong and no file handle to leak.
    let mask = gdk_pixbuf::Pixbuf::from_resource(RESOURCE)
        .unwrap_or_else(|err| panic!("{RESOURCE} should be in the GResource: {err}"));
    let (pw, ph) = (mask.width(), mask.height());
    let (width, height) = (pw as usize, ph as usize);
    let (channels, rowstride) = (mask.n_channels() as usize, mask.rowstride() as usize);

    // Read the shape out first and drop the borrowed buffer before touching the
    // pixbuf again, which is the contract `pixels()` asks for.
    let mut alpha = vec![0u8; width * height];
    {
        let pixels = unsafe { mask.pixels() };
        for y in 0..height {
            for x in 0..width {
                let at = y * rowstride + x * channels;
                // Stored as LA: the tone is a placeholder, the shape is alpha.
                alpha[y * width + x] = pixels[at + channels - 1];
            }
        }
    }

    let out = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, pw, ph)
        .expect("a 72x72 RGBA pixbuf allocates");
    let [r, g, b, _] = key;
    for y in 0..height {
        for x in 0..width {
            out.put_pixel(x as u32, y as u32, r, g, b, alpha[y * width + x]);
        }
    }
    gdk::Texture::for_pixbuf(&out)
}

/// Show the mark on `button`, painted in whatever colour the button's own
/// style context resolves, and keep it that way.
///
/// The mark is a raster, so GTK will not recolour it: a paintable set once
/// would freeze the first colour it saw, the favorited heart would stay the
/// header's foreground, and `.viewer-favorite-btn.favorite-active` would do
/// nothing with nothing failing. Reading the colour back out of the style
/// context keeps CSS the one place a colour is written down.
///
/// Following it is a `notify::css-classes` watch rather than a call at each
/// site that toggles `favorite-active`. That is the difference between "this
/// one path repaints" and "the icon follows the class": a class added
/// anywhere — production, a test, a future caller — repaints, and the resolved
/// colour is re-read each time so a theme switch lands too.
pub(crate) fn follow(button: &gtk::Button) {
    button.connect_css_classes_notify(apply);
    button.connect_map(apply);
    apply(button);
}

fn apply(button: &gtk::Button) {
    let colour = button.style_context().color();
    let image = match button.child().and_downcast::<gtk::Image>() {
        Some(image) => image,
        None => {
            let image = gtk::Image::new();
            button.set_child(Some(&image));
            image
        }
    };
    image.set_pixel_size(button_pixel_size(button));
    image.set_from_paintable(Some(&tinted(colour)));
}

/// A button draws its icon at the size the header resolves, which varies by
/// theme and libadwaita version. Ask the button rather than pinning a number,
/// for the same reason the cloud badge copies the header's size.
fn button_pixel_size(button: &gtk::Button) -> i32 {
    let (min_w, nat_w, _, _) = button.measure(gtk::Orientation::Horizontal, -1);
    let (min_h, nat_h, _, _) = button.measure(gtk::Orientation::Vertical, -1);
    let (min_w, min_h) = (min_w.max(0), min_h.max(0));
    let (nat_w, nat_h) = (nat_w.max(0), nat_h.max(0));
    if nat_w > 0 && nat_h > 0 && (nat_w, nat_h) != (min_w, min_h) {
        nat_w.min(nat_h)
    } else {
        min_w.min(min_h).max(1)
    }
}

#[cfg(test)]
mod tests;
