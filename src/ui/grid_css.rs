//! Hover CSS for thumbnail FlowBoxes (MediaGrid, AlbumDetailPage).
//!
//! Replaces the previous click-driven `:selected` outline + tint with a
//! `:hover` hint. Identical visual style — only the trigger changes.
//! `TrashPage` deliberately does NOT install this; it keeps click-driven
//! multi-select for batch restore / permanent-delete.
//!
//! Install is idempotent (process-wide `Once`), so multiple pages may call
//! `install()` without coordinating.

use gtk4 as gtk;

const GRID_CSS: &str = "
flowbox.thumb-grid > flowboxchild { padding: 0; }
flowbox.thumb-grid > flowboxchild:hover {
  background-color: alpha(@accent_color, 0.3);
}
flowbox.thumb-grid > flowboxchild:hover .tile {
  outline: 2px solid @accent_color;
  outline-offset: -1px;
}
";

static CSS_INSTALLED: std::sync::Once = std::sync::Once::new();

/// Register the thumbnail-grid hover CSS with the default display.
/// Idempotent: subsequent calls are no-ops.
pub fn install() {
    CSS_INSTALLED.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(GRID_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}
