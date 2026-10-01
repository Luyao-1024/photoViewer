use super::super::test_support::*;
use super::super::{ViewerPage, MIN_VIEWER_ZOOM};
use super::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::{gio, glib};

#[test]
fn wheel_zoom_consumes_a_full_notch_and_keeps_the_remainder() {
    assert_eq!(zoom_for_wheel_delta(1.0, -1.0, 0.0, 1.0), (1.25, 0.0));
    assert_eq!(zoom_for_wheel_delta(1.25, 1.0, 0.0, 1.0), (1.0, 0.0));
    assert_eq!(zoom_for_wheel_delta(1.0, 0.5, 0.0, 1.0), (1.0, 0.5));
    assert_eq!(zoom_for_wheel_delta(1.0, 0.25, 0.8, 1.0), (1.25, 0.55));
}

#[test]
fn drag_pan_uses_the_offset_from_gesture_start() {
    assert_eq!(panned_from(10.0, -4.0, 25.0, 7.0), (35.0, 3.0));
}

#[test]
fn only_an_enlarged_image_tracks_drag() {
    assert!(!should_pan_from_drag(MIN_VIEWER_ZOOM));
    assert!(should_pan_from_drag(1.25));
}

#[gtk::test]
fn image_overlay_has_desktop_wheel_and_no_touch_pinch_controller() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let controllers = viewer.imp().image_overlay.get().observe_controllers();
    let snapshot = controllers.snapshot();

    assert!(
        snapshot.iter().any(|controller| controller
            .downcast_ref::<gtk::EventControllerScroll>()
            .is_some()),
        "desktop Ctrl+wheel zoom needs an explicit scroll controller"
    );
    assert!(
        snapshot
            .iter()
            .filter(|controller| controller.downcast_ref::<gtk::GestureDrag>().is_some())
            .all(|controller| {
                controller
                    .downcast_ref::<gtk::GestureDrag>()
                    .map(|drag| drag.property::<u32>("button") == gdk::BUTTON_PRIMARY)
                    .unwrap_or(false)
            }),
        "image panning must remain limited to the primary pointer button"
    );
    assert!(
        !snapshot
            .iter()
            .any(|controller| controller.downcast_ref::<gtk::GestureZoom>().is_some()),
        "the image overlay should not install touch pinch zoom while buttons own zoom actions"
    );
}

#[gtk::test]
fn an_unmeasured_stage_ignores_pointer_zoom_and_pan() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    viewer.imp().picture.get().set_visible(true);
    assert_eq!(viewer.imp().picture.get().width(), 0);

    viewer.set_viewer_zoom_for_input(4.0, 1_000.0, 1_000.0);

    assert_eq!(viewer.imp().zoom_scale.get(), MIN_VIEWER_ZOOM);
    assert_eq!(viewer.imp().zoom_pan_x.get(), 0.0);
    assert_eq!(viewer.imp().zoom_pan_y.get(), 0.0);
}

#[gtk::test]
fn resetting_the_transform_clears_the_wheel_accumulator_and_drag_baseline() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let imp = viewer.imp();
    imp.zoom_wheel_remainder.set(0.7);
    imp.pan_gesture_start.set(Some((12.0, 30.0, 40.0, -18.0)));

    viewer.reset_viewer_transform();

    assert_eq!(imp.zoom_wheel_remainder.get(), 0.0);
    assert!(imp.pan_gesture_start.get().is_none());
    assert_eq!(imp.zoom_scale.get(), MIN_VIEWER_ZOOM);
    assert_eq!(imp.viewer_rotation_degrees.get(), 0);
}
