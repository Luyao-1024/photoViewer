//! Adw.PreferencesGroup 复用构建器
//!
//! Provides a single helper `make_scale_row` that builds a `(ActionRow, Scale)`
//! pair for the editor's Adjust group. Centralising the construction here
//! keeps the per-scale defaults (range, width, value display) consistent
//! whether the row is built from Blueprint or programmatically.
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{ActionRowExt, PreferencesRowExt};

/// Build a new `AdwActionRow` titled `title` with a `GtkScale` widget
/// in its suffix. The scale spans `[-100, +100]`, defaults to 0, displays
/// the current value, and is 200 px wide.
///
/// Returns the row (to be added to a group) and the scale (to be wired
/// to a `value-changed` handler).
pub fn make_scale_row(title: &str) -> (adw::ActionRow, gtk::Scale) {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_activatable(false);
    // `gtk::Scale::builder()` exposes only style properties; configure the
    // `Range` portion (min/max/value) via the `RangeExt` methods post-build.
    let scale = gtk::Scale::builder()
        .width_request(200)
        .draw_value(true)
        .build();
    scale.set_range(-100.0, 100.0);
    scale.set_value(0.0);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    row.add_suffix(&scale);
    (row, scale)
}