#![allow(dead_code)]

use gtk4 as gtk;
use gtk4::prelude::*;

pub(super) fn viewer_overlay_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon_name)
        .tooltip_text(tooltip)
        .build();
    button.add_css_class("glass-toolbar-button");
    button.add_css_class("viewer-overlay-nav-btn");
    button
}
