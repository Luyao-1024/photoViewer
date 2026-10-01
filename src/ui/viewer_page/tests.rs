use super::*;
use chrono::{TimeZone, Utc};
use std::cell::Cell;

/// Pump the main context until `done` holds, so async position/badge replies
/// can land inside a `#[gtk::test]`.
fn pump_until(timeout: std::time::Duration, done: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let context = glib::MainContext::default();
    while std::time::Instant::now() < deadline {
        while context.iteration(false) {}
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    while context.iteration(false) {}
    done()
}

fn pump_for(timeout: std::time::Duration) {
    let _ = pump_until(timeout, || false);
}

fn seed_position_fixture(n: i64) -> (tempfile::TempDir, crate::core::db::DbPool) {
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("position.db")).unwrap();
    for id in 1..=n {
        let taken = Utc.with_ymd_and_hms(2026, 6, id as u32, 12, 0, 0).unwrap();
        crate::core::db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file:///tmp/lib/{id}.jpg"),
                path: PathBuf::from(format!("/tmp/lib/{id}.jpg")),
                folder_path: PathBuf::from("/tmp/lib"),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(100),
                height: Some(100),
                video_duration_secs: None,
                taken_at: Some(taken),
                file_mtime: taken,
                file_size: 100,
                blake3_hash: format!("hash-{id}"),
            },
        )
        .unwrap();
    }
    (tmp, pool)
}

fn query_viewer_with_pool(
    pool: crate::core::db::DbPool,
    items: &[MediaItem],
) -> (ViewerPage, gio::ListStore) {
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in items {
        list.append(&glib::BoxedAnyObject::new(item.clone()));
    }
    let viewer = ViewerPage::new_for_query(
        MediaQuery::LiveAll,
        MediaId::from(items[0].id),
        list.clone(),
    );
    viewer.imp().pool.replace(Some(pool));
    (viewer, list)
}

fn position_label_for(current: u32, total: u32) -> String {
    crate::core::i18n::trf(
        "viewer.position.count",
        &[
            ("current", &current.to_string()),
            ("total", &total.to_string()),
        ],
    )
}

#[gtk::test]
fn switching_media_hides_the_previous_rank_until_the_new_one_resolves() {
    init_viewer_test();
    let (_tmp, pool) = seed_position_fixture(3);
    let items = crate::core::repository::MediaRepository::new(pool.clone())
        .items(MediaQuery::LiveAll, 0, 10)
        .unwrap();
    assert_eq!(items.len(), 3, "fixture precondition: three ranked items");
    let (viewer, _list) = query_viewer_with_pool(pool, &items);
    let label = viewer.imp().position_label.get();

    viewer.show_at(0);
    assert!(
        pump_until(std::time::Duration::from_secs(3), || label.is_visible()),
        "the rank should resolve and reveal the counter for the first item"
    );
    assert_eq!(label.label(), position_label_for(1, 3));

    viewer.show_at(1);
    assert!(
        !label.is_visible(),
        "switching media must hide the previous item's rank the moment the new request starts"
    );
    assert!(
        pump_until(std::time::Duration::from_secs(3), || label.is_visible()),
        "the new item's rank should resolve"
    );
    assert_eq!(label.label(), position_label_for(2, 3));

    // Rapid switch: the skipped item's reply lands late and must never paint.
    viewer.show_at(2);
    viewer.show_at(0);
    assert!(!label.is_visible());
    assert!(
        pump_until(std::time::Duration::from_secs(3), || label.is_visible()),
        "the final item's rank should resolve after the rapid switch"
    );
    assert_eq!(
        label.label(),
        position_label_for(1, 3),
        "only the last switch's rank may end up on screen"
    );
}

#[gtk::test]
fn a_failed_rank_query_keeps_the_previous_rank_hidden() {
    init_viewer_test();
    let (_tmp, pool) = seed_position_fixture(2);
    let items = crate::core::repository::MediaRepository::new(pool.clone())
        .items(MediaQuery::LiveAll, 0, 10)
        .unwrap();
    let (viewer, _list) = query_viewer_with_pool(pool.clone(), &items);
    let label = viewer.imp().position_label.get();

    viewer.show_at(0);
    assert!(
        pump_until(std::time::Duration::from_secs(3), || label.is_visible()),
        "precondition: the first item's rank resolved"
    );

    // Break the schema so the next COUNT fails: a failed rank request must
    // leave the counter hidden instead of surviving as the last confirmed
    // number for a different item.
    pool.get()
        .unwrap()
        .execute_batch("DROP TABLE media_items;")
        .unwrap();

    viewer.show_at(1);
    assert!(
        !label.is_visible(),
        "the new switch starts with the counter hidden"
    );
    pump_for(std::time::Duration::from_millis(600));
    assert!(
        !label.is_visible(),
        "a failed rank query must not resurrect the previous item's rank"
    );
}

#[gtk::test]
fn synced_image_badge_is_available_in_viewer_header() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let badge = viewer.imp().sync_badge.get();

    assert!(badge.has_css_class("viewer-sync-badge"));
    assert!(!badge.is_visible());
    assert!(badge.resource().is_none());
}

#[gtk::test]
fn media_error_surface_exists_in_viewer_overlay() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    assert!(
        widget_tree_has_class(&viewer.imp().image_overlay.get(), "viewer-media-error"),
        "viewer should provide one app-owned media error background instead of exposing GtkVideo's default broken frame"
    );
    assert!(
        !viewer.imp().media_error_box.get().is_visible(),
        "the media error surface should stay hidden until media actually fails"
    );
    assert_eq!(
        viewer.imp().media_error_retry_btn.get().label().as_deref(),
        Some(tr("viewer.error.retry").as_str())
    );
    assert_eq!(
        viewer.imp().media_error_reveal_btn.get().label().as_deref(),
        Some(tr("viewer.error.reveal").as_str())
    );
}

#[gtk::test]
fn media_error_surface_wording_follows_the_failing_media_kind() {
    init_viewer_test();
    let video_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    video_list.append(&glib::BoxedAnyObject::new(sample_video_item()));
    let video_viewer = ViewerPage::new(video_list, 0);

    video_viewer.show_media_error_background();
    assert_eq!(
        video_viewer.imp().media_error_title.get().label(),
        tr("viewer.video_error.title")
    );
    assert_eq!(
        video_viewer
            .imp()
            .media_error_icon
            .get()
            .icon_name()
            .as_deref(),
        Some("video-x-generic-symbolic"),
        "a failed video should keep the media-specific icon"
    );

    let image_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    image_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let image_viewer = ViewerPage::new(image_list, 0);

    image_viewer.show_media_error_background();
    assert_eq!(
        image_viewer.imp().media_error_title.get().label(),
        tr("viewer.image_error.title"),
        "a failed image must not be reported with the video wording"
    );
    assert!(
        image_viewer
            .imp()
            .media_error_subtitle
            .get()
            .label()
            .contains("sample.jpg"),
        "the error has to name the file the user can go look for"
    );
    assert_eq!(
        image_viewer
            .imp()
            .media_error_icon
            .get()
            .icon_name()
            .as_deref(),
        Some("image-missing-symbolic")
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
    viewer.show_media_error_background();

    assert!(viewer.imp().media_error_box.get().is_visible());
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
        !viewer.imp().media_error_box.get().is_visible(),
        "leaving the failed media should clear the error surface"
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

fn sample_video_item() -> MediaItem {
    MediaItem {
        uri: "file:///tmp/sample.mp4".into(),
        path: PathBuf::from("/tmp/sample.mp4"),
        mime_type: "video/mp4".into(),
        media_subkind: "video".into(),
        video_duration_secs: Some(12.0),
        ..sample_media_item()
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

/// P2-2: a toast parked at the bottom of the viewer used to sit on the
/// filmstrip - the control you are still using while the toast reports what you
/// just did there. The fix is structural: `Adw.ToastOverlay` wraps the stage
/// only, so its bottom edge is the top of the filmstrip band, and no CSS
/// selector against libadwaita's internal toast node is load-bearing. This test
/// measures the painted bounds of a real toast, so moving the overlay back to
/// the page root fails here rather than in review.
#[gtk::test]
fn a_toast_lands_above_the_filmstrip() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let viewer = ViewerPage::new(media_list, 0);
    let overlay = viewer.imp().toast_overlay.get().clone();
    let filmstrip = viewer.imp().viewer_bottom_stack.get().clone();
    let window = gtk::Window::builder()
        .default_width(900)
        .default_height(600)
        .child(&viewer)
        .build();
    window.present();
    pump_for(std::time::Duration::from_millis(300));

    overlay.add_toast(adw::Toast::new("已移入回收站"));
    let toast = {
        let mut found = None;
        let mut child = overlay.first_child();
        while let Some(node) = child {
            if node.type_().name().contains("Toast") {
                found = Some(node);
                break;
            }
            child = node.next_sibling();
        }
        found.expect("the overlay should host the toast widget")
    };
    pump_for(std::time::Duration::from_millis(300));

    // `allocation()` is useless here: the toast widget's allocation includes the
    // theme margin around the card, and a child of `Adw.ToastOverlay` can even
    // report a different offset than it paints at. `compute_bounds` gives the
    // visible rectangle in the page's coordinate space, which is what the eye
    // uses to decide whether the strip is covered.
    fn painted_band(widget: &impl IsA<gtk::Widget>, page: &ViewerPage) -> (f64, f64) {
        let bounds = widget
            .compute_bounds(page)
            .expect("widget is inside the viewer page");
        (
            f64::from(bounds.y()),
            f64::from(bounds.y() + bounds.height()),
        )
    }
    let (toast_top, toast_bottom) = painted_band(&toast, &viewer);
    let (strip_top, _) = painted_band(&filmstrip, &viewer);
    let (_, stage_bottom) = painted_band(&viewer.imp().image_overlay.get(), &viewer);

    assert!(
        toast_bottom <= stage_bottom + 0.5,
        "the toast host has to stay inside the stage, it reached {toast_bottom:.1} while the \
         stage ends at {stage_bottom:.1}"
    );
    assert!(
        toast_bottom <= strip_top + 0.5,
        "a toast must not cover the filmstrip: its bottom edge is at {toast_bottom:.1} while \
         the strip starts at {strip_top:.1}"
    );
    assert!(
        toast_top < strip_top,
        "the toast has to be visible above the strip, not tucked under it"
    );
}
