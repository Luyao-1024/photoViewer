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
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 100,
        blake3_hash: "hash".into(),
        is_favorite: false,
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
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 100,
        blake3_hash: "hash".into(),
        is_favorite: false,
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

    // Action buttons (reset / cancel / save_copy / save_overwrite) carry glass-toolbar-button.
    let reset = probe_classes(&imp.reset_btn.get());
    let cancel = probe_classes(&imp.cancel_btn.get());
    let save_copy = probe_classes(&imp.save_copy_btn.get());
    let save_overwrite = probe_classes(&imp.save_overwrite_btn.get());
    for (name, classes) in [
        ("reset_btn", &reset),
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
    assert!(
        reset.iter().any(|c| c == "circular"),
        "reset_btn should be a circular icon button, got {reset:?}"
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

    rotate_panel.imp().brightness_scale.get().set_value(42.0);
    rotate_panel.imp().state.borrow_mut().crop = Some((4, 5, 40, 30));
    rotate_panel.imp().reset_btn.get().emit_clicked();
    assert_eq!(rotate_panel.imp().state.borrow().rotation, Rotation::None);
    assert_eq!(rotate_panel.imp().state.borrow().brightness, 0);
    assert_eq!(rotate_panel.imp().state.borrow().crop, None);
    assert_eq!(rotate_panel.imp().brightness_scale.get().value() as i32, 0);

    rotate_panel.imp().source_dimensions.set((64, 48));
    assert!(!rotate_panel.imp().crop_ratio_box.get().is_visible());
    rotate_panel.imp().start_crop_btn.get().emit_clicked();
    assert!(
        rotate_panel.imp().crop_mode_active.get(),
        "Start Crop should enable inline crop mode"
    );
    assert!(
        rotate_panel.imp().crop_ratio_box.get().is_visible(),
        "graphical crop ratio control should stay inside the editor panel"
    );
    assert!(rotate_panel
        .imp()
        .crop_ratio_preview
        .get()
        .is::<gtk::DrawingArea>());
    assert!(
        rotate_panel
            .imp()
            .crop_ratio_prev_btn
            .get()
            .is::<gtk::Button>()
            && rotate_panel
                .imp()
                .crop_ratio_next_btn
                .get()
                .is::<gtk::Button>(),
        "crop ratio should be selected with previous/next buttons, not a dropdown"
    );
    assert_eq!(
        rotate_panel.imp().crop_ratio_prev_btn.get().width_request(),
        28,
        "crop ratio arrow buttons should stay close to the icon size"
    );
    assert!(
        probe_classes(&rotate_panel.imp().crop_ratio_prev_btn.get())
            .iter()
            .any(|c| c == "crop-ratio-arrow-button"),
        "crop ratio previous button should override generic toolbar padding"
    );
    assert_eq!(
        rotate_panel.imp().crop_ratio_next_btn.get().width_request(),
        28,
        "crop ratio arrow buttons should stay close to the icon size"
    );
    assert!(
        probe_classes(&rotate_panel.imp().crop_ratio_next_btn.get())
            .iter()
            .any(|c| c == "crop-ratio-arrow-button"),
        "crop ratio next button should override generic toolbar padding"
    );
    assert_eq!(
        rotate_panel
            .imp()
            .crop_ratio_prev_btn
            .get()
            .height_request(),
        40,
        "crop ratio arrow buttons should avoid large empty hit areas"
    );
    assert_eq!(
        rotate_panel
            .imp()
            .crop_ratio_next_btn
            .get()
            .height_request(),
        40,
        "crop ratio arrow buttons should avoid large empty hit areas"
    );
    assert_eq!(
        rotate_panel.imp().crop_ratio_preview.get().width_request(),
        150,
        "crop ratio preview should be large enough to read"
    );
    assert_eq!(
        rotate_panel.imp().crop_ratio_preview.get().height_request(),
        76,
        "crop ratio preview should be large enough to read"
    );
    assert_eq!(
        rotate_panel.imp().start_crop_btn.get().width_request(),
        132,
        "Done Crop should not occupy a full row"
    );
    assert_eq!(rotate_panel.imp().crop_ratio_index.get(), 0);
    assert_eq!(
        rotate_panel.imp().crop_ratio_label.get().label().as_str(),
        photo_viewer::core::i18n::tr("editor.crop.ratio.source")
    );
    rotate_panel.imp().crop_ratio_prev_btn.get().emit_clicked();
    assert_eq!(rotate_panel.imp().crop_ratio_index.get(), 5);
    assert_eq!(
        rotate_panel.imp().crop_ratio_label.get().label().as_str(),
        photo_viewer::core::i18n::tr("editor.crop.ratio.free")
    );
    rotate_panel.imp().crop_ratio_next_btn.get().emit_clicked();
    assert_eq!(rotate_panel.imp().crop_ratio_index.get(), 0);
    assert!(
        rotate_panel.imp().state.borrow().crop.is_some(),
        "Start Crop should create a draggable crop rectangle"
    );

    for _ in 0..100 {
        while ctx.pending() {
            ctx.iteration(false);
        }
        if rotate_panel.imp().source_image.borrow().is_some()
            && rotate_panel.imp().debounce_id.borrow().is_none()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        rotate_panel.imp().source_image.borrow().is_some(),
        "test image should be loaded before probing overlay updates"
    );
    let spinner_updates = Rc::new(Cell::new(0));
    rotate_panel.connect_spinner({
        let spinner_updates = Rc::clone(&spinner_updates);
        move |visible| {
            if visible {
                spinner_updates.set(spinner_updates.get() + 1);
            }
        }
    });
    spinner_updates.set(0);
    rotate_panel.set_crop_rect_from_overlay((8, 8, 24, 18));
    for _ in 0..20 {
        while ctx.pending() {
            ctx.iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        spinner_updates.get(),
        0,
        "dragging the crop overlay should update only the overlay, not rerender the image preview"
    );

    rotate_panel.imp().reset_btn.get().emit_clicked();
    assert!(!rotate_panel.imp().crop_mode_active.get());
    assert_eq!(rotate_panel.imp().state.borrow().crop, None);

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
