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
use photo_viewer::ui::ViewerPage;

fn css_classes_vec<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

#[test]
fn viewer_toolbar_uses_glass_classes() {
    gtk::init().expect("GTK init failed");

    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.ViewerToolbar")
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

    // All five toolbar buttons carry glass-toolbar-button.
    let button_classes: [(&str, Vec<String>); 5] = [
        ("details_btn", css_classes_vec(&imp.details_btn.get())),
        ("delete_btn", css_classes_vec(&imp.delete_btn.get())),
        ("favorite_btn", css_classes_vec(&imp.favorite_btn.get())),
        (
            "add_to_album_btn",
            css_classes_vec(&imp.add_to_album_btn.get()),
        ),
        ("edit_btn", css_classes_vec(&imp.edit_btn.get())),
    ];
    for (name, classes) in button_classes.iter() {
        assert!(
            classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}",
        );
    }

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
        !imp.video_progress.get().is_visible(),
        "video progress should start hidden until a video media item is shown",
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
    for (name, classes) in [
        ("prev_btn", css_classes_vec(&imp.prev_btn.get())),
        ("next_btn", css_classes_vec(&imp.next_btn.get())),
    ] {
        assert!(
            classes.iter().any(|c| c == "viewer-overlay-nav-btn"),
            "{name} should use the overlay glass nav button class, got {classes:?}",
        );
    }

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

    assert_viewer_video_mode_shows_video_progress_and_disables_editing();
}

fn assert_viewer_video_mode_shows_video_progress_and_disables_editing() {
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
        width: None,
        height: None,
        taken_at: None,
        file_mtime: now,
        file_size: 8,
        blake3_hash: "hash".into(),
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
        imp.video_progress.get().is_visible(),
        "video progress should be visible for video media"
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
