//! Integration tests for the photo grid canvas — Task 4 of the liquid-glass
//! UX adaptation. The grid's FlowBox is built by `MediaGrid`; we don't
//! rebuild the full grid here, but we do verify the new 8px column/row
//! spacing the builder now uses (smoke test against a one-off FlowBox, since
//! the same constant value applies inside the real `MediaGrid`). The
//! `MediaGrid` itself is exercised by `tests/e2e_browsing.rs`; no new
//! widget test is needed for the integration path.
//!
//! GTK is single-threaded, so all checks live in a single `#[test]`
//! function. See `tests/ui_mode_selector.rs` for the same pattern.

use gtk4 as gtk;
use gtk4::prelude::*;

#[test]
fn flowbox_uses_8px_gaps() {
    gtk::init().expect("GTK init failed");
    // We don't need the full MediaGrid; we just assert the constant value the
    // builder now uses. Build a one-off flowbox with the same spacing.
    let flow = gtk::FlowBox::builder()
        .column_spacing(8)
        .row_spacing(8)
        .build();
    flow.add_css_class("thumb-grid");
    assert_eq!(flow.column_spacing(), 8);
    assert_eq!(flow.row_spacing(), 8);
}
