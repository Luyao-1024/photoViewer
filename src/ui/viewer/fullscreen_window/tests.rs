use super::super::test_support::*;
use super::super::*;
use super::*;

#[gtk::test]
fn fullscreen_preview_opens_separate_window_without_changing_viewer_layout() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let texture = test_texture();
    viewer.imp().picture.get().set_paintable(Some(&texture));
    let parent = gtk::Window::builder()
        .title("Viewer parent")
        .default_width(900)
        .default_height(700)
        .build();
    parent.set_child(Some(&viewer));
    parent.present();
    while glib::MainContext::default().iteration(false) {}

    viewer.open_fullscreen_preview_window();

    let preview = viewer
        .imp()
        .fullscreen_preview_window
        .borrow()
        .as_ref()
        .cloned()
        .expect("fullscreen preview should keep a separate top-level window");
    assert!(
        preview.is_fullscreened(),
        "fullscreen preview should request fullscreen as its initial window state"
    );
    assert!(
        !preview.is_decorated(),
        "fullscreen preview should be borderless and hide the system titlebar controls"
    );
    assert!(
        preview.transient_for().is_none(),
        "fullscreen preview should be an independent top-level window so compositors honor fullscreen"
    );
    let preview_overlay = preview
        .child()
        .expect("fullscreen preview should have overlay content")
        .downcast::<gtk::Overlay>()
        .expect("fullscreen preview content should be a GtkOverlay");
    assert!(
        widget_tree_has_class(&preview_overlay, "viewer-fullscreen-preview-picture"),
        "fullscreen preview should render the media picture inside the overlay"
    );
    assert!(
        widget_tree_has_class(&preview_overlay, "viewer-overlay-nav"),
        "fullscreen preview should include the image-stage previous/next controls"
    );
    assert!(
        widget_tree_has_class(&preview_overlay, "viewer-zoom-controls"),
        "fullscreen preview should include the image-stage zoom/rotate controls"
    );
    assert!(
        widget_tree_has_button_icon(&preview_overlay, "view-restore-symbolic"),
        "fullscreen preview should include a restore button to close the enlarged window"
    );
    assert!(
        !widget_tree_has_class(&preview_overlay, "glass-header"),
        "fullscreen preview should not include the main viewer header actions"
    );
    assert!(
        viewer.imp().header_bar.get().is_visible(),
        "opening fullscreen preview must not hide the viewer header or disturb NavigationView"
    );
    assert!(viewer.imp().viewer_bottom_stack.get().is_visible());
    assert!(viewer.can_pop());
    assert_eq!(
        viewer.imp().fullscreen_btn.get().icon_name().as_deref(),
        Some(VIEWER_FULLSCREEN_ICON),
        "main viewer button should remain an open-preview command"
    );

    preview.close();
    parent.close();
    while glib::MainContext::default().iteration(false) {}
    assert!(
        viewer.imp().fullscreen_preview_window.borrow().is_none(),
        "closing the transient preview window should clear the stored handle"
    );
}

#[gtk::test]
fn fullscreen_preview_reuses_existing_window_handle() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let texture = test_texture();
    viewer.imp().picture.get().set_paintable(Some(&texture));
    let parent = gtk::Window::builder()
        .title("Viewer parent")
        .default_width(900)
        .default_height(700)
        .build();
    parent.set_child(Some(&viewer));
    parent.present();
    while glib::MainContext::default().iteration(false) {}

    viewer.open_fullscreen_preview_window();
    let first_preview = viewer
        .imp()
        .fullscreen_preview_window
        .borrow()
        .as_ref()
        .cloned()
        .expect("first fullscreen open should store a preview window");
    let first_ptr = first_preview.as_ptr();

    viewer.open_fullscreen_preview_window();
    let second_preview = viewer
        .imp()
        .fullscreen_preview_window
        .borrow()
        .as_ref()
        .cloned()
        .expect("second fullscreen open should keep a preview window stored");

    assert_eq!(
        second_preview.as_ptr(),
        first_ptr,
        "opening fullscreen preview twice should reuse the existing top-level preview window"
    );

    second_preview.close();
    parent.close();
    while glib::MainContext::default().iteration(false) {}
}

#[gtk::test]
fn fullscreen_preview_close_disconnects_paintable_sync_handler() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let first_texture = test_texture();
    viewer
        .imp()
        .picture
        .get()
        .set_paintable(Some(&first_texture));
    let parent = gtk::Window::builder()
        .title("Viewer parent")
        .default_width(900)
        .default_height(700)
        .build();
    parent.set_child(Some(&viewer));
    parent.present();
    while glib::MainContext::default().iteration(false) {}

    viewer.open_fullscreen_preview_window();
    let preview = viewer
        .imp()
        .fullscreen_preview_window
        .borrow()
        .as_ref()
        .cloned()
        .expect("fullscreen preview should be stored while open");
    let preview_overlay = preview
        .child()
        .expect("fullscreen preview should have overlay content")
        .downcast::<gtk::Overlay>()
        .expect("fullscreen preview content should be a GtkOverlay");
    let preview_picture = preview_overlay
        .child()
        .expect("fullscreen preview overlay should have a picture child")
        .downcast::<gtk::Picture>()
        .expect("fullscreen preview overlay child should be GtkPicture");
    let preview_picture_weak = preview_picture.downgrade();
    let first_paintable_ptr = preview_picture
        .paintable()
        .expect("preview picture should mirror the main viewer paintable")
        .as_ptr();

    preview.close();
    while glib::MainContext::default().iteration(false) {}

    assert!(
        viewer.imp().fullscreen_preview_window.borrow().is_none(),
        "closing the preview should clear the stored preview window handle"
    );

    let second_texture = test_texture();
    viewer
        .imp()
        .picture
        .get()
        .set_paintable(Some(&second_texture));
    while glib::MainContext::default().iteration(false) {}

    if let Some(preview_picture) = preview_picture_weak.upgrade() {
        let current_paintable_ptr = preview_picture
            .paintable()
            .map(|paintable| paintable.as_ptr());
        assert_eq!(
            current_paintable_ptr,
            Some(first_paintable_ptr),
            "closed preview picture must not receive paintable updates from the disconnected notify handler"
        );
    }

    parent.close();
    while glib::MainContext::default().iteration(false) {}
}
