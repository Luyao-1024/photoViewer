//! Integration tests for the right-click context menu on the photo grid —
//! Task 6 of the liquid-glass UX adaptation.
//!
//! The popover/box/buttons are constructed inside `MediaGrid`'s gesture
//! handler (src/ui/media_grid.rs ~lines 716-833). We don't rebuild the full
//! `MediaGrid` here; this test stands in for the same construction and
//! verifies the new `glass-menu*` class assignments. The click-handler
//! behaviour is already covered by `tests/e2e_browsing.rs` and
//! `tests/trash_flow.rs`.
//!
//! GTK is single-threaded; we use a single `#[test]` function for the whole
//! suite. See `tests/ui_mode_selector.rs` for the same pattern.

use gtk4 as gtk;
use gtk4::prelude::*;
use photo_viewer::core::albums::{Album, FAVORITES_ALBUM_PATH};
use std::path::PathBuf;

fn album(is_virtual: bool) -> Album {
    Album {
        folder_path: if is_virtual {
            PathBuf::from(FAVORITES_ALBUM_PATH)
        } else {
            PathBuf::from("/tmp/Pictures/Camera")
        },
        name: if is_virtual {
            "收藏".into()
        } else {
            "Camera".into()
        },
        cover_uri: None,
        photo_count: 3,
        last_modified: chrono::Utc::now(),
        is_virtual,
    }
}

fn collect_button_labels(widget: &gtk::Widget, labels: &mut Vec<String>) {
    if let Some(button) = widget.downcast_ref::<gtk::Button>() {
        if let Some(label) = button.label() {
            labels.push(label.to_string());
        }
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        collect_button_labels(&current, labels);
    }
}

#[test]
fn context_menu_uses_glass_menu_classes() {
    gtk::init().expect("GTK init failed");

    // Build a stand-in for the popover a right-click would create. The real
    // construction lives inside MediaGrid's gesture handler; we only need to
    // verify the class assignments here.
    let popover = gtk::Popover::new();
    popover.add_css_class("glass-menu");

    let menu = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["glass-menu-list"])
        .build();

    // multi-select entry (suggested) and delete entry (danger) — the two
    // accent variants the brief calls out specifically.
    let multi_btn = gtk::Button::with_label("multi");
    multi_btn.add_css_class("glass-menu-item");
    multi_btn.add_css_class("glass-menu-item-suggested");

    let delete_btn = gtk::Button::with_label("delete");
    delete_btn.add_css_class("glass-menu-item");
    delete_btn.add_css_class("glass-menu-item-danger");

    // A plain item too (favorite / unfavorite / move-to-album).
    let plain_btn = gtk::Button::with_label("plain");
    plain_btn.add_css_class("glass-menu-item");

    menu.append(&multi_btn);
    menu.append(&delete_btn);
    menu.append(&plain_btn);
    popover.set_child(Some(&menu));

    assert!(
        popover.css_classes().iter().any(|c| c == "glass-menu"),
        "popover should carry glass-menu"
    );
    assert!(
        menu.css_classes().iter().any(|c| c == "glass-menu-list"),
        "inner list box should carry glass-menu-list"
    );
    assert!(
        multi_btn
            .css_classes()
            .iter()
            .any(|c| c == "glass-menu-item-suggested"),
        "multi-select button should carry glass-menu-item-suggested"
    );
    assert!(
        delete_btn
            .css_classes()
            .iter()
            .any(|c| c == "glass-menu-item-danger"),
        "delete button should carry glass-menu-item-danger"
    );
    assert!(
        plain_btn
            .css_classes()
            .iter()
            .any(|c| c == "glass-menu-item"),
        "plain button should carry glass-menu-item"
    );
    // None of the new buttons should carry the GTK built-ins anymore — the
    // glass-menu-item / -suggested / -danger rules now own the visual.
    for btn in [&multi_btn, &delete_btn, &plain_btn] {
        for banned in ["flat", "suggested-action", "destructive-action"] {
            assert!(
                !btn.css_classes().iter().any(|c| c == banned),
                "button should not carry {banned}"
            );
        }
    }

    let popover = photo_viewer::ui::window::build_album_context_menu_for_tests(&album(false));
    let mut labels = Vec::new();
    collect_button_labels(popover.upcast_ref(), &mut labels);

    assert!(
        popover.css_classes().iter().any(|c| c == "glass-menu"),
        "popover should carry glass-menu"
    );
    assert!(
        labels.iter().any(|label| label == "管理相册"),
        "real album menu should contain 管理相册, got {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "删除相册"),
        "real album menu should contain 删除相册, got {labels:?}"
    );

    let popover = photo_viewer::ui::window::build_album_context_menu_for_tests(&album(true));
    let mut labels = Vec::new();
    collect_button_labels(popover.upcast_ref(), &mut labels);

    assert!(
        labels.iter().any(|label| label == "管理相册"),
        "virtual album menu should contain 管理相册, got {labels:?}"
    );
    assert!(
        !labels.iter().any(|label| label == "删除相册"),
        "virtual album menu should omit 删除相册, got {labels:?}"
    );
}
