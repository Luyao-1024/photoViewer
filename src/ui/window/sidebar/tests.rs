use super::super::test_support::*;
use super::super::*;
use super::*;

#[gtk::test]
fn sidebar_album_snapshot_updates_virtual_model_without_realizing_all_rows() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.SidebarStableAlbumRows")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);

    let albums = vec![
        sidebar_album("/tmp/camera", "Camera", 2),
        sidebar_album("/tmp/screenshots", "Screenshots", 1),
    ];
    window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
        albums: albums.clone(),
        media_type_albums: Vec::new(),
        live_count: Some(3),
    });
    let model = window
        .imp()
        .album_model
        .borrow()
        .as_ref()
        .cloned()
        .expect("virtual album model should exist");

    let mut refreshed = albums;
    refreshed[0].photo_count = 1;
    refreshed[1].photo_count = 1;
    window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
        albums: refreshed,
        media_type_albums: Vec::new(),
        live_count: Some(2),
    });

    assert_eq!(
        model.n_items(),
        2,
        "the virtual model should retain both albums"
    );
    assert_eq!(
        window.imp().album_targets.borrow()[0].photo_count,
        1,
        "target snapshot should still update to the latest count"
    );
}

#[gtk::test]
fn sidebar_album_snapshot_updates_virtual_model_after_removal() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.SidebarStableAlbumRemoval")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);

    let albums = vec![
        sidebar_album("/tmp/camera", "Camera", 2),
        sidebar_album("/tmp/downloads", "Downloads", 1),
        sidebar_album("/tmp/screenshots", "Screenshots", 3),
    ];
    window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
        albums: albums.clone(),
        media_type_albums: Vec::new(),
        live_count: Some(6),
    });
    let model = window
        .imp()
        .album_model
        .borrow()
        .as_ref()
        .cloned()
        .expect("virtual album model should exist");

    window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
        albums: vec![
            sidebar_album("/tmp/camera", "Camera", 1),
            sidebar_album("/tmp/screenshots", "Screenshots", 3),
        ],
        media_type_albums: Vec::new(),
        live_count: Some(4),
    });

    assert_eq!(
        model.n_items(),
        2,
        "the virtual model should remove the deleted album"
    );
    assert_eq!(
        window.imp().album_targets.borrow().len(),
        2,
        "target snapshot should remove only the missing album"
    );
    assert_eq!(
        window.imp().album_targets.borrow()[0].photo_count,
        1,
        "surviving row targets should still update to latest counts"
    );
}

#[gtk::test]
fn sidebar_media_type_snapshot_removes_missing_row_without_replacing_survivors() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.SidebarStableMediaTypeRemoval")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);

    let mut dynamic = sidebar_album("/virtual/dynamic", "Dynamic Photos", 2);
    dynamic.is_virtual = true;
    let mut raw = sidebar_album("/virtual/raw", "Raw", 1);
    raw.is_virtual = true;
    let mut panoramas = sidebar_album("/virtual/panoramas", "Panoramas", 3);
    panoramas.is_virtual = true;

    window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
        albums: Vec::new(),
        media_type_albums: vec![dynamic.clone(), raw, panoramas.clone()],
        live_count: Some(6),
    });
    let media_type_list = window.imp().media_type_list.get();
    let first_before = media_type_list
        .row_at_index(0)
        .expect("first media type row should exist");
    let third_before = media_type_list
        .row_at_index(2)
        .expect("third media type row should exist");

    dynamic.photo_count = 1;
    window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
        albums: Vec::new(),
        media_type_albums: vec![dynamic, panoramas],
        live_count: Some(4),
    });

    assert!(
        media_type_list.row_at_index(0).as_ref() == Some(&first_before),
        "removing one media type should keep the first surviving row mounted"
    );
    assert!(
        media_type_list.row_at_index(1).as_ref() == Some(&third_before),
        "removing one media type should keep the later surviving row mounted"
    );
    assert_eq!(
        window.imp().media_type_targets.borrow().len(),
        2,
        "target snapshot should remove only the missing media type"
    );
    assert_eq!(
        window.imp().media_type_targets.borrow()[0].photo_count,
        1,
        "surviving media type targets should still update to latest counts"
    );
}

#[test]
fn sidebar_trace_logs_stay_debug() {
    let production_source = production_source("src/ui/window/sidebar.rs");

    let mut search_from = 0;
    while let Some(relative_index) = production_source[search_from..].find("SIDEBAR_TRACE") {
        let message_index = search_from + relative_index;
        let before = &production_source[..message_index];
        let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
            .iter()
            .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, candidate)| candidate)
            .expect("SIDEBAR_TRACE message should be inside a tracing macro");
        assert_eq!(
            actual_macro, "tracing::debug!(",
            "SIDEBAR_TRACE messages are diagnostic noise and should stay out of default INFO logs"
        );
        search_from = message_index + "SIDEBAR_TRACE".len();
    }
}

#[test]
fn sidebar_album_row_summary_logs_stay_debug() {
    let production_source = production_source("src/ui/window/sidebar.rs");

    for message in [
        "SIDEBAR_ALBUM_UPDATE_IN_PLACE",
        "SIDEBAR_ALBUM_REMOVE_IN_PLACE",
        "SIDEBAR_ALBUM_REBUILD",
    ] {
        let message_index = production_source
            .find(message)
            .unwrap_or_else(|| panic!("missing log message {message}"));
        let before = &production_source[..message_index];
        let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
            .iter()
            .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, candidate)| candidate)
            .expect("log message should be inside a tracing macro");
        assert_eq!(
            actual_macro, "tracing::debug!(",
            "{message} is high-volume sidebar row diagnostics and should stay out of default INFO logs"
        );
    }
}
