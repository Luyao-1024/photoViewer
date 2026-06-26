//! EditorPanel carries the same liquid-glass material as the rest of the
//! app: buttons get `glass-toolbar-button`, save_copy gets
//! `glass-toolbar-suggested`, and overwrite save gets `glass-toolbar-danger`.
//!
//! GTK is single-threaded; all checks live in one `#[test]` function.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::edit::Rotation;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::orientation::read_orientation;
use photo_viewer::ui::{grid_css, EditorPanel};
use std::cell::Cell;
use std::rc::Rc;

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

fn make_media_for_path(path: std::path::PathBuf) -> MediaItem {
    MediaItem {
        id: 1,
        uri: format!("file://{}", path.display()),
        folder_path: path.parent().unwrap().to_path_buf(),
        path,
        mime_type: "image/jpeg".into(),
        width: Some(64),
        height: Some(48),
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
    panel.configure(make_media(), pool.clone());
    let imp = panel.imp();

    // Action buttons (cancel / save_copy / save_overwrite) carry glass-toolbar-button.
    let cancel = probe_classes(&imp.cancel_btn.get());
    let save_copy = probe_classes(&imp.save_copy_btn.get());
    let save_overwrite = probe_classes(&imp.save_overwrite_btn.get());
    for (name, classes) in [
        ("cancel_btn", &cancel),
        ("save_copy_btn", &save_copy),
        ("save_overwrite_btn", &save_overwrite),
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

    assert!(
        save_overwrite.iter().any(|c| c == "glass-toolbar-danger"),
        "save_overwrite_btn should carry glass-toolbar-danger, got {save_overwrite:?}"
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

    assert!(imp.save_overwrite_btn.get().is::<gtk::Button>());

    let src = tmp.path().join("save-close.jpg");
    image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(64, 48, |_, _| image::Rgb([128, 128, 128]))
        .save(&src)
        .unwrap();
    let save_panel: EditorPanel = glib::Object::builder().build();
    save_panel.configure(make_media_for_path(src), pool.clone());
    let closed = Rc::new(Cell::new(false));
    let got_result = Rc::new(Cell::new(false));
    save_panel.connect_close({
        let closed = Rc::clone(&closed);
        move || closed.set(true)
    });
    save_panel.connect_save_result({
        let got_result = Rc::clone(&got_result);
        move |kind, _, _| {
            assert_eq!(
                kind,
                photo_viewer::ui::editor_panel::SaveResultKind::Success
            );
            got_result.set(true);
        }
    });

    save_panel.imp().save_copy_btn.get().emit_clicked();
    let ctx = glib::MainContext::default();
    for _ in 0..200 {
        while ctx.pending() {
            ctx.iteration(false);
        }
        if got_result.get() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(got_result.get(), "Save Copy should report success");
    assert!(closed.get(), "successful Save Copy should close the editor");

    let rotate_src = tmp.path().join("rotate-in-memory.jpg");
    image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(64, 48, |_, _| image::Rgb([128, 128, 128]))
        .save(&rotate_src)
        .unwrap();
    let rotate_panel: EditorPanel = glib::Object::builder().build();
    rotate_panel.configure(make_media_for_path(rotate_src.clone()), pool.clone());

    rotate_panel.imp().rotate_90_cw.get().emit_clicked();
    assert_eq!(rotate_panel.imp().state.borrow().rotation, Rotation::R90);
    for _ in 0..20 {
        while ctx.pending() {
            ctx.iteration(false);
        }
        if rotate_panel.imp().debounce_id.borrow().is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(
        rotate_panel.imp().debounce_id.borrow().is_none(),
        "fired preview debounce source should not leave a stale SourceId"
    );
    rotate_panel.imp().rotate_90_cw.get().emit_clicked();
    assert_eq!(rotate_panel.imp().state.borrow().rotation, Rotation::R180);

    assert_eq!(
        read_orientation(&rotate_src).unwrap(),
        1,
        "editing rotate should not write source orientation before save"
    );
    assert!(
        !rotate_src.with_extension("jpg.bak").exists(),
        "editing rotate should not create a source backup before save"
    );
}
