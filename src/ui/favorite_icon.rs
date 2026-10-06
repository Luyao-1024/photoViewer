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
//!
//! # Following the colour, not sampling it
//!
//! `GtkStyleContext::color()` is not a CSS property lookup. It is the colour
//! the context last resolved, and this app *transitions* that property:
//! `.glass-toolbar-button` — which both favorite buttons carry — declares
//! `transition: … color 120ms ease`, and the app's longest colour transition is
//! 350ms. Two consequences, both measured against a mapped window:
//!
//! * At the instant `notify::css-classes` fires, the context still holds the
//!   colour from *before* the class landed. Reading there returns the old
//!   colour, so an icon painted on that read keeps the previous state's colour
//!   forever — which is how the heart came out inverted: white while
//!   favorited, red while not.
//! * Even a read a few milliseconds later returns a *blend* (at 16ms into the
//!   120ms fade the context answers `(255, 230, 227, 252)`), so one read can
//!   never be the answer: the value is still moving.
//!
//! So the tint follows: [`follow`] watches the things that can move the colour
//! — a CSS class, a widget state flag (hover), the widget being mapped, a
//! theme switch — and then a frame-clock tick re-reads and repaints until the
//! colour stops changing. The heart then fades in step with the chrome around
//! it instead of lagging one state behind it.

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::glib::ControlFlow;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
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
/// colour the widget's style context resolved, and the CSS rule keeps working:
/// see the module docs for why that colour has to be *followed* rather than
/// read once.
///
/// Results are cached per colour, and the cache is bounded. Following a
/// transition means one fade contributes a handful of intermediate colours, so
/// a cache that only grows would turn a session of toggles into tens of
/// megabytes of textures; [`TINT_CACHE_ENTRIES`] keeps it to about a megabyte
/// and a miss costs one composite of a handful of pixels.
pub(crate) fn tinted(colour: gdk::RGBA) -> gdk::Texture {
    tinted_key(key_of(colour))
}

/// The mark painted in an already-quantised colour, which is what the follow
/// tick has: it compares colours to decide whether they are still moving, and
/// it must not compare floats to do it.
fn tinted_key(key: [u8; 4]) -> gdk::Texture {
    static CACHE: Mutex<Option<TintCache>> = Mutex::new(None);

    let mut cache = CACHE.lock().expect("the tint cache is not poisoned");
    let cache = cache.get_or_insert_with(TintCache::default);
    if let Some(found) = cache.get(&key) {
        return found;
    }
    let texture = paint(key);
    cache.insert(key, texture.clone());
    texture
}

/// How many tinted variants to keep before dropping the oldest. A 120ms fade at
/// 60Hz is about seven steps, and a heart is on at most a few buttons at once,
/// so this is roughly two orders of magnitude of headroom over a real fade.
const TINT_CACHE_ENTRIES: usize = 64;

/// The colour of a tint, quantised to the 8 bits a texture carries — so
/// comparing two of them is a comparison of what would actually be painted.
fn key_of(colour: gdk::RGBA) -> [u8; 4] {
    [
        (colour.red() * 255.0).round() as u8,
        (colour.green() * 255.0).round() as u8,
        (colour.blue() * 255.0).round() as u8,
        (colour.alpha() * 255.0).round() as u8,
    ]
}

/// Insertion-ordered so eviction is a pop off the front rather than a scan.
#[derive(Default)]
struct TintCache {
    textures: HashMap<[u8; 4], gdk::Texture>,
    order: VecDeque<[u8; 4]>,
}

impl TintCache {
    fn get(&self, key: &[u8; 4]) -> Option<gdk::Texture> {
        self.textures.get(key).cloned()
    }

    fn insert(&mut self, key: [u8; 4], texture: gdk::Texture) {
        self.textures.insert(key, texture);
        self.order.push_back(key);
        while self.order.len() > TINT_CACHE_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.textures.remove(&oldest);
            }
        }
    }
}

fn paint(key: [u8; 4]) -> gdk::Texture {
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

/// Frames the follow-tick repaints for before it gives up waiting for the
/// colour to settle. The app's longest colour transition is 350ms
/// (`box.mode-selector`), about 21 frames at 60Hz, so this is roughly twice the
/// longest fade with room for a slower compositor.
///
/// It is a bound, not a schedule: a colour that settles sooner stops sooner.
const FOLLOW_FRAMES: u32 = 45;

/// Consecutive frames of an unchanged colour that end the follow early.
///
/// Not 1. The first frames after a class change can report the *same* colour
/// twice while the fade is still getting under way — measured: frames 1 and 2
/// both answered `(255, 247, 246)` and the fade only started moving on frame 3
/// — so "unchanged since last frame" is not "finished", and reading it that way
/// leaves the heart parked a few percent into the transition, which looks
/// exactly like the wrong colour it was meant to replace.
const SETTLED_FRAMES: u32 = 3;

/// Show the mark on `button`, painted in whatever colour the button's own
/// style context resolves, and keep it that way.
///
/// The mark is a raster, so GTK will not recolour it: a paintable set once
/// would freeze the first colour it saw, the favorited heart would stay the
/// header's foreground, and `.viewer-favorite-btn.favorite-active` would do
/// nothing with nothing failing. Reading the colour back out of the style
/// context keeps CSS the one place a colour is written down.
///
/// This is wired to what can *move* that colour rather than to each call site
/// that toggles `favorite-active`, which is the difference between "this one
/// path repaints" and "the icon follows the colour":
///
/// * a CSS class — production, a test, or a future caller;
/// * a widget state flag, because the hover red
///   (`.viewer-favorite-btn.favorite-active:hover`) is a state, not a class;
/// * being mapped, for a button that resolves its icon size then;
/// * a theme switch, through the shared [`retint_on_theme_change`] watch.
pub(crate) fn follow(button: &gtk::Button) {
    button.connect_css_classes_notify(refresh);
    // GTK4 has no `state-flags` property to watch, only the signal that says
    // it moved; hover and active live there.
    button.connect_state_flags_changed(|button, _flags| refresh(button));
    button.connect_map(refresh);
    register(button);
    refresh(button);
}

/// Re-read the resolved colour now, and keep following it while it moves.
///
/// Safe to call as often as anything changes: a theme switch, a hover, a class
/// toggle, a remap. The tick it starts stops on its own, and a second call
/// while one is running only adds a follower that converges on the same colour
/// and then stops too.
pub(crate) fn refresh(button: &gtk::Button) {
    apply(button);
    follow_colour(button);
}

/// Repaint once per frame until the colour stops changing.
///
/// A frame-clock tick is the only clock that is already in step with the CSS
/// transition: `style_context().color()` hands back whatever the fade has
/// reached *now*, so reading it per frame walks the fade, and the first frame
/// on which the quantised colour repeats for [`SETTLED_FRAMES`] frames running
/// is the frame the fade is over. That is also why the follow cannot be a fixed
/// delay: at 120ms the colour is a different value at every frame, and a read at
/// 16ms is a blend rather than an answer.
fn follow_colour(button: &gtk::Button) {
    // A separate binding, so the closure can own the button while the call
    // below still borrows the one it was handed.
    let target = button.clone();
    let state = Rc::new(RefCell::new(FollowState {
        painted: None,
        stable: 0,
        frames: 0,
    }));
    button.add_tick_callback(move |_, _| {
        let colour = key_of(target.style_context().color());
        // Repaint before deciding to stop, so the frame the follow ends on is
        // also the frame the icon lands on the final colour.
        repaint_in(&target, colour);
        let done = {
            let mut state = state.borrow_mut();
            state.frames += 1;
            state.stable = if state.painted == Some(colour) {
                state.stable + 1
            } else {
                0
            };
            state.painted = Some(colour);
            state.frames >= FOLLOW_FRAMES || state.stable >= SETTLED_FRAMES
        };
        if done {
            ControlFlow::Break
        } else {
            ControlFlow::Continue
        }
    });
}

struct FollowState {
    /// The colour on the icon as of the previous frame, quantised the same way
    /// the cache key is — comparing the texture's own key is what makes "still
    /// moving" a comparison of what would actually be painted.
    painted: Option<[u8; 4]>,
    /// How many frames in a row that colour has held.
    stable: u32,
    frames: u32,
}

/// The buttons being followed, so one theme watch can re-tint all of them.
///
/// A theme switch moves `@window_fg_color` and therefore the colour of a
/// button that is not in the favourited state, and it does it without a class
/// or a state change — nothing on the widget fires. libadwaita says so out
/// loud, and the module owns the mark, so the watch lives here rather than
/// being asked of every surface.
///
/// Thread-local rather than a `static Mutex`: a `WeakRef` is a raw pointer and
/// so not `Send`, and a GTK widget only ever lives on the thread that made it.
struct Followed {
    buttons: Vec<glib::WeakRef<gtk::Button>>,
    theme_watch_installed: bool,
}

thread_local! {
    static FOLLOWED: RefCell<Followed> = const {
        RefCell::new(Followed {
            buttons: Vec::new(),
            theme_watch_installed: false,
        })
    };
}

fn register(button: &gtk::Button) {
    FOLLOWED.with(|followed| {
        let mut followed = followed.borrow_mut();
        followed.buttons.push(button.downgrade());
        if followed.theme_watch_installed {
            return;
        }
        followed.theme_watch_installed = true;
        let manager = adw::StyleManager::default();
        manager.connect_dark_notify(|_| retint_on_theme_change());
        manager.connect_color_scheme_notify(|_| retint_on_theme_change());
    });
}

fn retint_on_theme_change() {
    FOLLOWED.with(|followed| {
        followed
            .borrow_mut()
            .buttons
            .retain(|weak| match weak.upgrade() {
                Some(button) => {
                    refresh(&button);
                    true
                }
                // The surface went away; drop it so the list does not outlive the app.
                None => false,
            });
    });
}

fn apply(button: &gtk::Button) {
    let image = match button.child().and_downcast::<gtk::Image>() {
        Some(image) => image,
        None => {
            let image = gtk::Image::new();
            button.set_child(Some(&image));
            image
        }
    };
    // The header decides how big one of its icons is; ask the image node for the
    // box it was given, which is `-gtk-icon-size` resolved.
    //
    // Deliberately the image's *minimum*, not the button's natural size. The
    // button's natural size grows with this image, and
    // `ViewerPage::sync_badge_to_toolbar_icon_size` copies that natural size off
    // this very widget onto the cloud badge beside it — so reading it back here
    // is a feedback loop, and the two badges ratchet each other upwards. The
    // minimum comes from CSS and does not move when `pixel_size` is set, so it
    // settles on the size the header actually asked for.
    let (min_w, _, _, _) = image.measure(gtk::Orientation::Horizontal, -1);
    let size = min_w.max(1);
    if image.pixel_size() != size {
        image.set_pixel_size(size);
    }
    repaint(button);
}

/// Paint the mark in the colour the button's style context resolves right now.
fn repaint(button: &gtk::Button) {
    let colour = key_of(button.style_context().color());
    repaint_in(button, colour);
}

/// Paint the mark in a colour that has already been read, so the follow tick
/// asks the style context once per frame rather than twice.
fn repaint_in(button: &gtk::Button, colour: [u8; 4]) {
    if let Some(image) = button.child().and_downcast::<gtk::Image>() {
        image.set_from_paintable(Some(&tinted_key(colour)));
    }
}

#[cfg(test)]
mod tests;
