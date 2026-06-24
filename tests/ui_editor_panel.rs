//! EditorPanel carries the same liquid-glass material as the rest of the
//! app: buttons get `glass-toolbar-button`, save_copy gets
//! `glass-toolbar-suggested`, and the save-menu popover built by
//! `setup_save_menu` gets `glass-menu`.
//!
//! GTK is single-threaded; all checks live in one `#[test]` function.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::media::MediaItem;
use photo_viewer::ui::{grid_css, EditorPanel};

fn probe_classes<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

fn make_media() -> MediaItem {
    MediaItem {
        id: 1,
        uri: "file:///tmp/one.jpg".into(),
        path: "/tmp/one.jpg".into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        width: Some(100),
        height: Some(100),
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 100,
        blake3_hash: "hash".into(),
        trashed_at: None,
    }
}

#[test]
fn editor_panel_buttons_use_glass() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.EditorPanelGlass")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    grid_css::install();

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let panel: EditorPanel = glib::Object::builder().build();
    panel.configure(make_media(), pool);
    let imp = panel.imp();

    // Action buttons (cancel / save_copy / save_menu) carry glass-toolbar-button.
    let cancel = probe_classes(&imp.cancel_btn.get());
    let save_copy = probe_classes(&imp.save_copy_btn.get());
    let save_menu = probe_classes(&imp.save_menu_btn.get());
    for (name, classes) in [
        ("cancel_btn", &cancel),
        ("save_copy_btn", &save_copy),
        ("save_menu_btn", &save_menu),
    ] {
        assert!(
            classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}"
        );
    }

    // save_copy_btn also carries glass-toolbar-suggested (the primary action).
    assert!(
        save_copy.iter().any(|c| c == "glass-toolbar-suggested"),
        "save_copy_btn should carry glass-toolbar-suggested, got {save_copy:?}"
    );

    // Rotate + crop buttons carry glass-toolbar-button.
    for (name, btn) in [
        ("rotate_90_cw", imp.rotate_90_cw.get()),
        ("rotate_180", imp.rotate_180.get()),
        ("rotate_90_ccw", imp.rotate_90_ccw.get()),
        ("start_crop_btn", imp.start_crop_btn.get()),
    ] {
        let classes = probe_classes(&btn);
        assert!(
            classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}"
        );
    }

    // The save_menu_btn's popover is a GtkPopoverMenu carrying glass-menu.
    let popover: gtk::Popover = imp
        .save_menu_btn
        .get()
        .popover()
        .expect("save_menu_btn should have a popover");
    let pop_classes = probe_classes(&popover);
    assert!(
        pop_classes.iter().any(|c| c == "glass-menu"),
        "save popover should carry glass-menu, got {pop_classes:?}"
    );
}
