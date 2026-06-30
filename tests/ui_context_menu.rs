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
use photo_viewer::core::i18n::tr;
use photo_viewer::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
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

fn collect_buttons(widget: &gtk::Widget, buttons: &mut Vec<gtk::Button>) {
    if let Some(button) = widget.downcast_ref::<gtk::Button>() {
        buttons.push(button.clone());
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        collect_buttons(&current, buttons);
    }
}

fn button_with_label(buttons: &[gtk::Button], label: &str) -> gtk::Button {
    buttons
        .iter()
        .find(|button| button.label().as_deref() == Some(label))
        .unwrap_or_else(|| {
            panic!(
                "missing button {label}; got labels {:?}",
                button_labels(buttons)
            )
        })
        .clone()
}

fn button_labels(buttons: &[gtk::Button]) -> Vec<String> {
    buttons
        .iter()
        .filter_map(|button| button.label().map(|label| label.to_string()))
        .collect()
}

fn assert_button_has_class(button: &gtk::Button, class_name: &str) {
    assert!(
        button.css_classes().iter().any(|class| class == class_name),
        "button {:?} should carry {class_name}, got {:?}",
        button.label(),
        button.css_classes()
    );
}

fn assert_button_lacks_class(button: &gtk::Button, class_name: &str) {
    assert!(
        !button.css_classes().iter().any(|class| class == class_name),
        "button {:?} should not carry {class_name}, got {:?}",
        button.label(),
        button.css_classes()
    );
}

#[test]
fn context_menu_uses_glass_menu_classes() {
    gtk::init().expect("GTK init failed");

    let glass_panel = glass_context_menu::build_menu_panel_for_tests(vec![
        GlassMenuItem::new("manage", GlassMenuItemKind::Normal, || {}),
        GlassMenuItem::new("multi", GlassMenuItemKind::Suggested, || {}),
        GlassMenuItem::new("delete", GlassMenuItemKind::Danger, || {}),
    ]);
    assert!(
        glass_panel.has_css_class("glass-raised"),
        "custom context menu should reuse the ModeSelector raised glass material"
    );
    assert!(
        glass_panel.has_css_class("glass-context-menu"),
        "custom context menu should use the overlay menu panel class"
    );
    assert!(
        !glass_panel.has_css_class("glass-menu"),
        "custom context menu should not use GtkPopover glass-menu styling"
    );
    let mut glass_buttons = Vec::new();
    collect_buttons(glass_panel.upcast_ref(), &mut glass_buttons);
    assert_eq!(glass_buttons.len(), 3);
    assert_button_has_class(&glass_buttons[0], "glass-context-menu-item");
    assert_button_lacks_class(&glass_buttons[0], "glass-menu-item");
    assert_button_has_class(&glass_buttons[1], "glass-context-menu-item-suggested");
    assert_button_has_class(&glass_buttons[2], "glass-context-menu-item-danger");

    let album_panel = photo_viewer::ui::window::build_album_context_menu_for_tests(&album(false));
    let mut labels = Vec::new();
    collect_button_labels(album_panel.upcast_ref(), &mut labels);
    let mut buttons = Vec::new();
    collect_buttons(album_panel.upcast_ref(), &mut buttons);
    let manage_button = button_with_label(&buttons, &tr("album.context.manage"));
    let delete_button = button_with_label(&buttons, &tr("album.context.delete"));

    assert!(
        album_panel.has_css_class("glass-raised"),
        "album context menu should carry glass-raised"
    );
    assert!(
        album_panel.has_css_class("glass-context-menu"),
        "album context menu should carry glass-context-menu"
    );
    assert!(
        !album_panel.has_css_class("glass-menu"),
        "album context menu should not use GtkPopover glass-menu styling"
    );
    assert!(
        labels
            .iter()
            .any(|label| label == &tr("album.context.manage")),
        "real album menu should contain {}, got {labels:?}",
        tr("album.context.manage")
    );
    assert!(
        labels
            .iter()
            .any(|label| label == &tr("album.context.delete")),
        "real album menu should contain {}, got {labels:?}",
        tr("album.context.delete")
    );
    assert_button_has_class(&manage_button, "glass-context-menu-item");
    assert_button_has_class(&delete_button, "glass-context-menu-item");
    assert_button_has_class(&delete_button, "glass-context-menu-item-danger");

    let album_panel = photo_viewer::ui::window::build_album_context_menu_for_tests(&album(true));
    let mut labels = Vec::new();
    collect_button_labels(album_panel.upcast_ref(), &mut labels);
    let mut buttons = Vec::new();
    collect_buttons(album_panel.upcast_ref(), &mut buttons);
    let manage_button = button_with_label(&buttons, &tr("album.context.manage"));

    assert!(
        labels
            .iter()
            .any(|label| label == &tr("album.context.manage")),
        "virtual album menu should contain {}, got {labels:?}",
        tr("album.context.manage")
    );
    assert!(
        !labels
            .iter()
            .any(|label| label == &tr("album.context.delete")),
        "virtual album menu should omit {}, got {labels:?}",
        tr("album.context.delete")
    );
    assert!(
        buttons
            .iter()
            .all(|button| button.label().as_deref() != Some(&tr("album.context.delete"))),
        "virtual album menu should not create a delete button, got {:?}",
        button_labels(&buttons)
    );
    assert_button_has_class(&manage_button, "glass-context-menu-item");
}
