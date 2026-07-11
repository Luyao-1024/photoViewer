use super::*;
use chrono::Utc;
use std::cell::Cell;

#[gtk::test]
fn video_error_background_exists_in_viewer_overlay() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    assert!(
        widget_tree_has_class(&viewer.imp().image_overlay.get(), "viewer-video-error"),
        "viewer should provide an app-owned video error background instead of exposing GtkVideo's default broken frame"
    );
    assert!(
        !viewer.imp().video_error_box.get().is_visible(),
        "video error background should stay hidden until playback reports an error"
    );
    assert_eq!(
        viewer.imp().video_error_title.get().label(),
        tr("viewer.video_error.title")
    );
}

#[gtk::test]
fn video_error_background_hides_default_video_error_surface() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    viewer.imp().video.get().set_visible(true);
    viewer.imp().picture.get().set_visible(true);
    viewer.set_spinner_visible(true);
    viewer.show_video_error_background();

    assert!(viewer.imp().video_error_box.get().is_visible());
    assert!(
        !viewer.imp().video.get().is_visible(),
        "GtkVideo should be hidden so its default broken-frame graphic is not exposed"
    );
    assert!(!viewer.imp().picture.get().is_visible());
    assert!(
        viewer
            .imp()
            .spinner
            .get()
            .has_css_class("viewer-spinner-hidden"),
        "spinner should be opacity-hidden (not removed from layout) so it fades"
    );

    viewer.show_image_stage();
    assert!(
        !viewer.imp().video_error_box.get().is_visible(),
        "leaving the failed video should clear the error background"
    );
}

#[gtk::test]
fn editing_hides_overlay_navigation_buttons() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let nav_container = viewer
        .imp()
        .prev_btn
        .get()
        .parent()
        .expect("prev button should live inside the overlay nav container");

    assert!(
        nav_container.is_visible(),
        "overlay navigation should be visible before editing"
    );

    viewer.start_editing();
    assert!(
        !nav_container.is_visible(),
        "opening the editor should hide previous/next overlay navigation"
    );

    viewer.stop_editing();
    assert!(
        nav_container.is_visible(),
        "closing the editor should restore previous/next overlay navigation"
    );
}

fn sample_media_item() -> MediaItem {
    MediaItem {
        id: 1,
        uri: "file:///tmp/sample.jpg".into(),
        path: PathBuf::from("/tmp/sample.jpg"),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc::now(),
        file_size: 1024,
        blake3_hash: "hash".into(),
        is_favorite: false,
        trashed_at: None,
    }
}

fn init_viewer_test() {
    let _ = gtk::init();
    crate::ui::grid_css::install();
}

fn widget_tree_has_class<W: IsA<gtk::Widget>>(widget: &W, class_name: &str) -> bool {
    let widget = widget.as_ref();
    if widget.css_classes().iter().any(|class| class == class_name) {
        return true;
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        if widget_tree_has_class(&current, class_name) {
            return true;
        }
        child = current.next_sibling();
    }
    false
}

#[gtk::test]
fn escape_closes_details_panel_without_navigation_pop() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    viewer.imp().details_split_view.get().set_show_sidebar(true);

    let nav_pop_fired = Rc::new(Cell::new(false));
    let nav_pop_fired_for_cb = nav_pop_fired.clone();
    viewer.connect_navigation(move |delta| {
        if delta == NAV_POP {
            nav_pop_fired_for_cb.set(true);
        }
    });

    assert_eq!(
        viewer.handle_keyboard_action(KeyboardAction::CancelOrClose),
        KeyboardResult::Handled,
        "Escape action should be consumed when details are visible"
    );
    assert!(
        !viewer.imp().details_split_view.get().shows_sidebar(),
        "Escape should close only the details panel"
    );
    assert!(
        !nav_pop_fired.get(),
        "Escape while details are visible must not pop the viewer page"
    );
}

#[gtk::test]
fn close_details_button_keeps_viewer_page_visible() {
    init_viewer_test();
    let nav = adw::NavigationView::new();
    let root = adw::NavigationPage::builder()
        .title("Root")
        .child(&gtk::Label::new(Some("root")))
        .build();
    nav.push(&root);

    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    nav.push(&viewer);
    viewer.imp().details_split_view.get().set_show_sidebar(true);

    viewer.imp().details_close_btn.get().emit_clicked();

    assert!(
        !viewer.imp().details_split_view.get().shows_sidebar(),
        "details close button should hide only the details panel"
    );
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(viewer.title().as_str()),
        "details close button must not pop the viewer page"
    );
}

#[gtk::test]
fn navigation_pop_closes_details_before_leaving_viewer() {
    init_viewer_test();
    let nav = adw::NavigationView::new();
    let root = adw::NavigationPage::builder()
        .title("Root")
        .child(&gtk::Label::new(Some("root")))
        .build();
    nav.push(&root);

    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    nav.push(&viewer);
    viewer.imp().details_split_view.get().set_show_sidebar(true);

    let _ = viewer.activate_action("navigation.pop", None);

    assert!(
        !viewer.imp().details_split_view.get().shows_sidebar(),
        "navigation pop should first close the details panel"
    );
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(viewer.title().as_str()),
        "navigation pop while details are visible must not leave viewer"
    );
}

#[gtk::test]
fn details_panel_temporarily_disables_navigation_pop() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    assert!(
        viewer.can_pop(),
        "viewer should normally allow navigation pop"
    );

    viewer.set_details_revealed(true, "test-open");
    assert!(
        !viewer.can_pop(),
        "opening details should disable NavigationView built-in pop"
    );

    viewer.set_details_revealed(false, "test-close");
    assert!(
        !viewer.can_pop(),
        "closing details should keep pop disabled during the close animation"
    );

    let ctx = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(900);
    while std::time::Instant::now() < deadline && !viewer.can_pop() {
        ctx.iteration(true);
    }

    assert!(
        viewer.can_pop(),
        "viewer should allow navigation pop again after the guard delay"
    );
}

#[gtk::test]
fn viewer_is_immediately_poppable_with_date_visible_on_open() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let label = viewer.imp().date_label.get();

    // There is no initial-open pop guard anymore. Pressing Escape / swiping
    // back / clicking back right after opening is intentional user input and
    // must work immediately, so `can_pop` stays true from the first `show_at`.
    // The date label is likewise shown immediately (it is a passive peer of the
    // title, decoupled from the back button's visibility).
    viewer.show_at(0);
    assert!(
        viewer.can_pop(),
        "viewer must remain poppable on open — immediate back/Escape/swipe is intentional"
    );
    assert!(
        label.is_visible(),
        "date label should be visible as soon as an item is shown"
    );
}
