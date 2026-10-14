use super::*;
use chrono::Utc;

use std::rc::Rc;

fn media_item(id: i64, folder_path: &str, name: &str) -> MediaItem {
    let folder_path = PathBuf::from(folder_path);
    let path = folder_path.join(name);
    MediaItem {
        id,
        uri: format!("file://{}", path.display()),
        path,
        folder_path,
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc::now(),
        file_size: 10,
        blake3_hash: format!("hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}

#[gtk::test]
fn shared_media_projection_refresh_emits_pure_addition_for_new_item() {
    let _ = gtk::init();
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let existing = media_item(1, "/tmp/shared-refresh", "one.jpg");
    list.append(&glib::BoxedAnyObject::new(existing.clone()));
    let changes = Rc::new(RefCell::new(Vec::<(u32, u32, u32)>::new()));
    let changes_for_signal = changes.clone();
    list.connect_items_changed(move |_, position, removed, added| {
        changes_for_signal
            .borrow_mut()
            .push((position, removed, added));
    });

    let added = media_item(2, "/tmp/shared-refresh", "two.jpg");
    apply_media_projection_to_list(&list, vec![added, existing]);

    assert_eq!(
        *changes.borrow(),
        vec![(0, 0, 1)],
        "shared Photos refresh should insert new media without replacing existing tiles"
    );
}

#[gtk::test]
fn main_window_installs_single_keyboard_router() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardRouter")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);

    let key_controllers: Vec<gtk::EventControllerKey> = window
        .observe_controllers()
        .snapshot()
        .into_iter()
        .filter_map(|controller| controller.downcast::<gtk::EventControllerKey>().ok())
        .filter(|controller| controller.name().as_deref() == Some("photo-viewer-keyboard-router"))
        .filter(|controller| controller.propagation_phase() == gtk::PropagationPhase::Capture)
        .collect();

    assert_eq!(
        key_controllers.len(),
        1,
        "MainWindow should own one capture-phase keyboard router"
    );
    assert_eq!(
        key_controllers[0].propagation_phase(),
        gtk::PropagationPhase::Capture
    );
}

#[gtk::test]
fn changing_day_grid_columns_while_settings_is_open_defers_window_resize() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.DeferredGridColumns")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let dialog = window.build_settings_dialog(&host);
    window.imp().settings_dialog.borrow_mut().replace(dialog);

    window.set_photos_grid_columns(runtime_config::MAX_PHOTOS_GRID_COLUMNS);

    assert_eq!(
        window.imp().deferred_day_grid_columns.get(),
        Some(runtime_config::MAX_PHOTOS_GRID_COLUMNS),
        "the resize request must wait until Settings has closed"
    );
    assert!(
        window.imp().day_grid_apply_source.borrow().is_none(),
        "the Day grid must not reflow while its centered Settings dialog is visible"
    );
}

#[gtk::test]
fn navigation_view_has_no_touch_swipe_controller() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.NoSwipeRouter")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let nav = window.nav_view();
    window.connect_sidebar(&nav);

    let has_swipe = nav
        .observe_controllers()
        .snapshot()
        .into_iter()
        .any(|controller| controller.downcast::<gtk::GestureSwipe>().is_ok());
    assert!(
        !has_swipe,
        "NavigationView should not install a touch swipe controller that competes with buttons and keyboard actions"
    );
}
