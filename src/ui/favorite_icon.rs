//! The favorite mark, shared by every surface that shows one.
//!
//! One app-owned icon name for all of them — the viewer header button, the
//! photos page select-all button, and the Day-view grid tile badge — because
//! the three used to be three different glyphs:
//!
//! * the two toolbar buttons asked the icon theme for
//!   `emblem-favorite-symbolic`, which adwaita-icon-theme does not ship at all
//!   (still missing in GNOME 50), so on a stock GNOME system the buttons drew
//!   no icon; only themes like Yaru happened to carry one, and each drew its
//!   own shape.
//! * the Day-view tile badge was a `Gtk.Label` holding the "♡" character at
//!   18pt, so its weight and outline came from whatever font the system
//!   resolved. A text glyph in a photo grid next to a vector heart in a
//!   toolbar is the same mismatch the cloud badge had with its neighbours.
//!
//! The artwork ([`data/icons/photoviewer-heart-symbolic.svg`]) is a hairline
//! outline tuned to the cloud badge's ink box, and it is bundled in the
//! GResource and resolved through the icon theme
//! ([`crate::ICON_RESOURCE_PATH`]) so GTK treats it as symbolic. That matters:
//! symbolic is what lets CSS `color` drive it — white over a photo in the
//! grid, the inherited header foreground in the viewer, translucent red under
//! `.viewer-favorite-btn.favorite-active`.
//!
//! The hairline is drawn as a filled ring rather than as `stroke`, because GTK
//! recolours a symbolic icon by rewriting the first paint it finds and a
//! stroked path comes back solid. The two contours are wound in opposite
//! directions so the hole survives that rewrite whatever it does to
//! `fill-rule`; `tests/ui_favorite_icon.rs` renders the mark through GTK and
//! fails if it ever comes back filled.

/// Icon name for the favorite mark. Referenced from the templates' widgets at
/// runtime rather than written into each `.blp`, so a surface cannot drift onto
/// a different glyph: the name exists once.
pub(crate) const NAME: &str = "photoviewer-heart-symbolic";

/// Box the Day-view tile badge draws the mark in. Matches the tile's cloud
/// badge (`SquareTile`'s `sync_badge`), so the two overlays in that corner read
/// as one set, and lands on the same apparent size the old 18pt "♡" had.
pub(crate) const TILE_PIXEL_SIZE: i32 = 18;

#[cfg(test)]
mod tests;
