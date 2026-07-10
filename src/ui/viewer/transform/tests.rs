use super::super::test_support::*;
use super::super::*;
use super::*;

#[test]
fn zoom_step_clamps_to_viewer_limits() {
    assert_eq!(step_zoom(1.0, 1), 1.25);
    assert_eq!(step_zoom(1.25, -1), 1.0);
    assert_eq!(step_zoom(7.9, 1), MAX_VIEWER_ZOOM);
    assert_eq!(step_zoom(MIN_VIEWER_ZOOM, -1), MIN_VIEWER_ZOOM);
}

#[gtk::test]
fn image_overlay_has_no_touch_zoom_or_pan_gestures() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let controllers = viewer.imp().image_overlay.get().observe_controllers();
    let has_zoom = controllers
        .snapshot()
        .into_iter()
        .any(|controller| controller.downcast::<gtk::GestureZoom>().is_ok());
    let has_drag = controllers
        .snapshot()
        .into_iter()
        .any(|controller| controller.downcast::<gtk::GestureDrag>().is_ok());

    assert!(
        !has_zoom,
        "image overlay should not install touch pinch zoom while buttons own zoom actions"
    );
    assert!(
        !has_drag,
        "image overlay should not install touch drag pan while buttons own zoom/navigation actions"
    );
}

#[test]
fn zoom_pan_is_clamped_and_resets_at_identity() {
    assert_eq!(
        clamp_zoom_pan(1.0, 120.0, -80.0, 1000.0, 700.0),
        (0.0, 0.0),
        "identity zoom should never keep a drag offset"
    );
    assert_eq!(
        clamp_zoom_pan(2.0, 800.0, -500.0, 1000.0, 700.0),
        (500.0, -350.0),
        "zoomed images should pan only across the extra visible area"
    );
}

#[gtk::test]
fn zoom_controls_live_in_top_right_with_reset_out_rotate_fullscreen_increase_order() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let imp = viewer.imp();

    let zoom_parent = imp
        .zoom_in_btn
        .get()
        .parent()
        .expect("zoom buttons should live inside a control container");
    assert!(
        zoom_parent
            .css_classes()
            .iter()
            .any(|class| class == "viewer-zoom-controls"),
        "zoom buttons need a distinct overlay container so they do not disturb prev/next layout"
    );
    assert_eq!(
        zoom_parent.halign(),
        gtk::Align::End,
        "zoom controls should sit at the image area's top-right edge"
    );
    assert_eq!(
        zoom_parent.valign(),
        gtk::Align::Start,
        "zoom controls should sit at the image area's top-right edge"
    );

    assert_eq!(
        zoom_parent.first_child(),
        Some(imp.zoom_reset_btn.get().upcast::<gtk::Widget>()),
        "zoom controls should start with reset"
    );
    assert_eq!(
        imp.zoom_reset_btn.get().next_sibling(),
        Some(imp.zoom_out_btn.get().upcast::<gtk::Widget>()),
        "zoom-out should follow reset"
    );
    assert_eq!(
        imp.zoom_out_btn.get().next_sibling(),
        Some(imp.rotate_left_btn.get().upcast::<gtk::Widget>()),
        "rotate-left should follow zoom-out"
    );
    assert_eq!(
        imp.rotate_left_btn.get().next_sibling(),
        Some(imp.rotate_right_btn.get().upcast::<gtk::Widget>()),
        "rotate-right should follow rotate-left"
    );
    assert_eq!(
        imp.rotate_right_btn.get().next_sibling(),
        Some(imp.fullscreen_btn.get().upcast::<gtk::Widget>()),
        "fullscreen should follow rotate-right"
    );
    assert_eq!(
        imp.fullscreen_btn.get().next_sibling(),
        Some(imp.zoom_in_btn.get().upcast::<gtk::Widget>()),
        "zoom-in should follow fullscreen"
    );

    for (name, button) in [
        ("zoom_in_btn", imp.zoom_in_btn.get()),
        ("zoom_out_btn", imp.zoom_out_btn.get()),
        ("zoom_reset_btn", imp.zoom_reset_btn.get()),
        ("rotate_left_btn", imp.rotate_left_btn.get()),
        ("rotate_right_btn", imp.rotate_right_btn.get()),
        ("fullscreen_btn", imp.fullscreen_btn.get()),
    ] {
        assert!(
            button
                .css_classes()
                .iter()
                .any(|class| class == "glass-toolbar-button"),
            "{name} should reuse the existing viewer glass button treatment"
        );
    }

    assert_eq!(
        imp.zoom_reset_btn.get().icon_name().as_deref(),
        Some("zoom-fit-best-symbolic"),
        "reset should use the fit-to-view icon"
    );
    assert!(imp.zoom_in_btn.get().is_visible());
    assert!(!imp.zoom_out_btn.get().is_visible());
    assert!(!imp.zoom_reset_btn.get().is_visible());
    assert!(imp.rotate_left_btn.get().is_visible());
    assert!(imp.rotate_right_btn.get().is_visible());
    assert!(imp.fullscreen_btn.get().is_visible());

    viewer.set_viewer_zoom_for_tests(1.25, 0.0, 0.0);
    assert!(imp.zoom_in_btn.get().is_visible());
    assert!(imp.zoom_out_btn.get().is_visible());
    assert!(imp.zoom_reset_btn.get().is_visible());
    assert!(!imp.rotate_left_btn.get().is_visible());
    assert!(!imp.rotate_right_btn.get().is_visible());
    assert!(imp.fullscreen_btn.get().is_visible());
}

#[gtk::test]
fn reset_zoom_restores_identity_state() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    viewer.set_viewer_zoom_for_tests(2.0, 100.0, -50.0);
    viewer.imp().zoom_reset_btn.get().emit_clicked();

    assert_eq!(viewer.imp().zoom_scale.get(), 1.0);
    assert_eq!(viewer.imp().zoom_pan_x.get(), 0.0);
    assert_eq!(viewer.imp().zoom_pan_y.get(), 0.0);
}
