//! ViewerPage toolbar buttons + image stage should carry the liquid-glass
//! material classes introduced in Task 1, and the favorite-active visual
//! should be owned by the global CSS provider (Task 7).
//!
//! GTK is single-threaded, so all checks live in one `#[test]` function.
//! See `tests/ui_mode_selector.rs` and `tests/ui_photos_toolbar.rs` for
//! the same pattern.

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::motion_photo::{MediaAttributes, MotionPhotoFormat, MotionPhotoInfo};
use photo_viewer::ui::ViewerPage;

fn css_classes_vec<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

#[test]
fn viewer_toolbar_uses_glass_classes() {
    gtk::init().expect("GTK init failed");

    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.ViewerToolbar")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    // Install the global CSS provider so any class-resolving sanity check
    // finds the rules; the assertions only inspect the .css_classes() list
    // (which is set by the .blp template), not the computed style.
    photo_viewer::ui::grid_css::install();

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    let page = ViewerPage::new(media_list, 0);
    let imp = page.imp();

    // HeaderBar carries glass-header. (The previous `viewer-header` class
    // was removed in Fix #4 — it had no corresponding CSS rule and was
    // effectively dead code.)
    let header_classes = css_classes_vec(&imp.header_bar.get());
    assert!(
        header_classes.iter().any(|c| c == "glass-header"),
        "header_bar should carry glass-header, got {header_classes:?}",
    );
    assert!(
        !header_classes.iter().any(|c| c == "viewer-header"),
        "header_bar should no longer carry viewer-header (dead class removed), got {header_classes:?}",
    );

    // The viewer header carries a `viewer-toolbar` scope class so the global
    // CSS can give its buttons square (1:1) geometry without touching the
    // shared .glass-toolbar-button rule used by every other page's header.
    assert!(
        header_classes.iter().any(|c| c == "viewer-toolbar"),
        "header_bar should carry viewer-toolbar (scopes square button geometry), got {header_classes:?}",
    );

    // All four header buttons carry glass-toolbar-button. Header order is
    // favorite → edit → delete → details (add-to-album was removed).
    let button_classes: [(&str, Vec<String>); 4] = [
        ("favorite_btn", css_classes_vec(&imp.favorite_btn.get())),
        ("edit_btn", css_classes_vec(&imp.edit_btn.get())),
        ("delete_btn", css_classes_vec(&imp.delete_btn.get())),
        ("details_btn", css_classes_vec(&imp.details_btn.get())),
    ];
    for (name, classes) in button_classes.iter() {
        assert!(
            classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}",
        );
    }

    let motion_play_classes = css_classes_vec(&imp.motion_play_btn.get());
    assert!(
        motion_play_classes
            .iter()
            .any(|c| c == "viewer-motion-play-button"),
        "motion_play_btn should carry viewer-motion-play-button, got {motion_play_classes:?}",
    );
    assert!(
        !imp.motion_play_btn.get().is_visible(),
        "motion_play_btn should be hidden until the current item is a motion photo"
    );

    // edit_btn is icon-only (document-edit-symbolic), never a text label.
    let edit_label: Option<String> = imp.edit_btn.get().label().map(|s| s.to_string());
    assert!(
        edit_label.is_none(),
        "edit_btn should be icon-only, got label {edit_label:?}",
    );

    // delete_btn also carries glass-toolbar-danger.
    let del_classes = css_classes_vec(&imp.delete_btn.get());
    assert!(
        del_classes.iter().any(|c| c == "glass-toolbar-danger"),
        "delete_btn should carry glass-toolbar-danger, got {del_classes:?}",
    );

    // favorite_btn also carries viewer-favorite-btn.
    let fav_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        fav_classes.iter().any(|c| c == "viewer-favorite-btn"),
        "favorite_btn should carry viewer-favorite-btn, got {fav_classes:?}",
    );

    // Task 8: details_close_btn carries glass-toolbar-button.
    let close_classes = css_classes_vec(&imp.details_close_btn.get());
    assert!(
        close_classes.iter().any(|c| c == "glass-toolbar-button"),
        "details_close_btn should carry glass-toolbar-button, got {close_classes:?}",
    );

    // Task 8: details_panel (the OverlaySplitView sidebar slot) is a floating
    // glass overlay over the image, so it carries the self-contained
    // `viewer-floating-panel` class (not the flush `viewer-details-panel` /
    // `glass-base` used by the editor), and never the opaque `background`
    // class. `details_panel` is not a template_child, so we reach the widget
    // via the split view's `sidebar()` accessor.
    let split_view = imp.details_split_view.get();
    let details_panel = split_view
        .sidebar()
        .expect("OverlaySplitView should have a sidebar widget");
    let panel_classes = css_classes_vec(&details_panel);
    assert!(
        panel_classes.iter().any(|c| c == "viewer-floating-panel"),
        "details_panel should carry viewer-floating-panel, got {panel_classes:?}",
    );
    assert!(
        !panel_classes.iter().any(|c| c == "glass-base"),
        "details_panel should not carry glass-base (floating panel is self-contained), got {panel_classes:?}",
    );
    assert!(
        !panel_classes.iter().any(|c| c == "background"),
        "details_panel should no longer carry opaque `background`, got {panel_classes:?}",
    );
    assert!(
        !details_panel.is_visible(),
        "hidden details sidebar should also hide its child widget to avoid zero-width allocation warnings",
    );
    assert!(
        !imp.editor_panel.get().is_visible(),
        "hidden editor sidebar should also hide its child widget to avoid zero-width allocation warnings",
    );
    assert!(
        !imp.video.get().is_visible(),
        "video widget should start hidden until a video media item is shown",
    );
    assert!(
        !imp.video.get().is_autoplay(),
        "Gtk.Video autoplay must stay disabled so audio prefs apply before playback starts",
    );
    let name_row_classes = css_classes_vec(&imp.name_row.get());
    assert!(
        name_row_classes
            .iter()
            .any(|c| c == "viewer-details-name-row"),
        "name_row should use the larger details name treatment, got {name_row_classes:?}",
    );
    assert!(
        imp.name_row.get().is_activatable(),
        "name_row should be activatable so clicking it starts inline rename"
    );
    assert!(
        !gtk::prelude::WidgetExt::is_visible(&imp.name_entry.get()),
        "inline rename entry should stay hidden until the name row is activated"
    );

    let prev_parent = imp
        .prev_btn
        .get()
        .parent()
        .expect("prev button should have a parent container");
    let prev_parent_classes = css_classes_vec(&prev_parent);
    assert!(
        prev_parent_classes.iter().any(|c| c == "viewer-overlay-nav"),
        "prev/next buttons should live in the image overlay nav container, got {prev_parent_classes:?}",
    );
    assert_eq!(
        prev_parent.margin_bottom(),
        34,
        "prev/next overlay nav should sit just above GtkVideo's built-in controls"
    );
    for (name, classes) in [
        ("prev_btn", css_classes_vec(&imp.prev_btn.get())),
        ("next_btn", css_classes_vec(&imp.next_btn.get())),
        (
            "rotate_left_btn",
            css_classes_vec(&imp.rotate_left_btn.get()),
        ),
        (
            "rotate_right_btn",
            css_classes_vec(&imp.rotate_right_btn.get()),
        ),
    ] {
        assert!(
            classes.iter().any(|c| c == "viewer-overlay-nav-btn"),
            "{name} should use the overlay glass nav button class, got {classes:?}",
        );
    }

    let zoom_parent = imp
        .zoom_in_btn
        .get()
        .parent()
        .expect("zoom-in button should have a parent container");
    assert_eq!(
        imp.rotate_right_btn.get().next_sibling().as_ref(),
        Some(imp.zoom_in_btn.get().upcast_ref()),
        "rotate-right should sit immediately to the left of zoom-in"
    );
    assert_eq!(
        imp.rotate_left_btn.get().parent().as_ref(),
        Some(&zoom_parent),
        "rotate buttons should live in the zoom controls container"
    );

    // Task: favorite-active class toggle on the viewer favorite button.
    // The class is owned by the global CSS provider (Task 7) and must be
    // addable / removable at runtime without disturbing the base classes.
    let initial_fav_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        !initial_fav_classes.iter().any(|c| c == "favorite-active"),
        "favorite_btn should not initially carry favorite-active, got {initial_fav_classes:?}",
    );

    imp.favorite_btn.get().add_css_class("favorite-active");
    let after_add_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        after_add_classes.iter().any(|c| c == "favorite-active"),
        "favorite_btn should carry favorite-active after add_css_class, got {after_add_classes:?}",
    );
    assert!(
        after_add_classes.iter().any(|c| c == "viewer-favorite-btn"),
        "favorite_btn should still carry viewer-favorite-btn after add_css_class, got {after_add_classes:?}",
    );
    assert!(
        after_add_classes.iter().any(|c| c == "glass-toolbar-button"),
        "favorite_btn should still carry glass-toolbar-button after add_css_class, got {after_add_classes:?}",
    );

    imp.favorite_btn.get().remove_css_class("favorite-active");
    let after_remove_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        !after_remove_classes.iter().any(|c| c == "favorite-active"),
        "favorite_btn should not carry favorite-active after remove_css_class, got {after_remove_classes:?}",
    );

    assert_viewer_motion_photo_mode_shows_play_button();
    assert_viewer_video_mode_disables_editing();
}

fn assert_viewer_video_mode_disables_editing() {
    let dir = tempfile::tempdir().unwrap();
    let video_path = dir.path().join("clip.mp4");
    std::fs::write(&video_path, b"fake mp4").unwrap();
    let now = chrono::Utc::now();
    let item = MediaItem {
        id: 1,
        uri: format!("file://{}", video_path.display()),
        path: video_path.clone(),
        folder_path: video_path.parent().unwrap_or(dir.path()).to_path_buf(),
        mime_type: "video/mp4".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: None,
        height: None,
        video_duration_secs: None,
        taken_at: None,
        file_mtime: now,
        file_size: 8,
        blake3_hash: "hash".into(),
        is_favorite: false,
        trashed_at: None,
    };

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    media_list.append(&gtk::glib::BoxedAnyObject::new(item));
    let page = ViewerPage::new(media_list, 0);
    page.show_at(0);
    let imp = page.imp();

    assert!(
        imp.video.get().is_visible(),
        "video widget should be visible for video media"
    );
    assert!(
        !imp.picture.get().is_visible(),
        "image widget should be hidden for video media"
    );
    assert!(
        !imp.edit_btn.get().is_sensitive(),
        "video media should not be editable"
    );
}

fn assert_viewer_motion_photo_mode_shows_play_button() {
    let dir = tempfile::tempdir().unwrap();
    let image_path = dir.path().join("motion.jpg");
    std::fs::write(&image_path, b"fake jpeg").unwrap();
    let now = chrono::Utc::now();
    let attrs = MediaAttributes::motion_photo_json(MotionPhotoInfo {
        format: MotionPhotoFormat::GoogleMicroVideo,
        video_offset: 10,
        video_length: 20,
        presentation_timestamp_us: Some(123),
        gain_map_offset: None,
        gain_map_length: None,
    });
    let item = MediaItem {
        id: 1,
        uri: format!("file://{}", image_path.display()),
        path: image_path.clone(),
        folder_path: image_path.parent().unwrap_or(dir.path()).to_path_buf(),
        mime_type: "image/jpeg".into(),
        media_subkind: "motion_photo".into(),
        media_attributes: attrs,
        width: None,
        height: None,
        video_duration_secs: None,
        taken_at: None,
        file_mtime: now,
        file_size: 8,
        blake3_hash: "hash".into(),
        is_favorite: false,
        trashed_at: None,
    };

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    media_list.append(&gtk::glib::BoxedAnyObject::new(item));
    let page = ViewerPage::new(media_list, 0);
    page.show_at(0);

    assert!(
        page.imp().motion_play_btn.get().is_visible(),
        "motion photos should expose a viewer play button"
    );
}
