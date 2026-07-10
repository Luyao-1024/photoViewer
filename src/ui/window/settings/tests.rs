use super::super::test_support::*;
use super::super::*;
use super::*;

#[test]
fn restart_spec_uses_current_executable_and_preserves_args() {
    let exe = PathBuf::from("/tmp/photo-viewer");
    let args = vec!["--profile".into(), "debug".into()];

    let spec = restart_spec_from_for_tests(exe.clone(), args.clone());

    assert_eq!(spec.program, exe);
    assert_eq!(spec.args, args);
}

#[test]
fn restart_spec_requires_current_process_exit_after_spawn() {
    let spec = restart_spec_from_for_tests(PathBuf::from("/tmp/photo-viewer"), Vec::new());

    assert!(spec.exit_current_process_after_spawn);
}

#[gtk::test]
fn settings_page_exposes_video_default_mute_without_volume_control() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowSettings")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let page = window.build_settings_page(&host);

    let mut labels = Vec::new();
    collect_labels(&page.upcast::<gtk::Widget>(), &mut labels);

    assert!(
        labels
            .iter()
            .any(|label| label == &tr("setting.section.video")),
        "settings page should contain the video playback section, got {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label == &tr("setting.video_default_muted")),
        "settings page should expose the default mute setting, got {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label == &tr("setting.auto_play_motion_photo")),
        "settings page should expose the motion-photo auto-play setting, got {labels:?}"
    );
    assert!(
        !labels
            .iter()
            .any(|label| label == &tr("setting.video_volume")),
        "volume should be persisted from playback, not configured in settings"
    );
}

#[gtk::test]
fn settings_page_exposes_liquid_glass_transparency_slider() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowGlassTransparency")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let page = window.build_settings_page(&host);
    let page = page.upcast::<gtk::Widget>();

    let mut labels = Vec::new();
    collect_labels(&page, &mut labels);
    assert!(
        labels
            .iter()
            .any(|label| label == &tr("setting.liquid_glass_transparency")),
        "settings page should expose the generic transparency label, got {labels:?}"
    );
    for mark in [
        "0", "10", "20", "30", "40", "50", "60", "70", "80", "90", "100",
    ] {
        assert!(
            labels.iter().any(|label| label == mark),
            "transparency scale should expose mark {mark}, got {labels:?}"
        );
    }

    let mut scales = Vec::new();
    collect_scales(&page, &mut scales);
    assert!(
        scales.iter().any(|scale| {
            scale.adjustment().lower() == 0.0 && scale.adjustment().upper() == 100.0
        }),
        "settings page should expose a 0-100 transparency scale"
    );
}

#[gtk::test]
fn settings_page_exposes_theme_selector() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowThemeSelector")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let page = window.build_settings_page(&host);
    let page = page.upcast::<gtk::Widget>();

    let mut labels = Vec::new();
    collect_labels(&page, &mut labels);
    assert!(
        labels.iter().any(|label| label == &tr("setting.theme")),
        "settings page should expose a theme label, got {labels:?}"
    );

    let mut check_buttons = Vec::new();
    collect_check_buttons(&page, &mut check_buttons);
    let theme_labels: Vec<String> = check_buttons
        .iter()
        .filter_map(|btn| btn.label().map(|label| label.to_string()))
        .collect();
    assert!(
        theme_labels.contains(&tr("setting.theme.system")),
        "settings page should expose Follow System theme option, got {theme_labels:?}"
    );
    assert!(
        theme_labels.contains(&tr("setting.theme.light")),
        "settings page should expose Light theme option, got {theme_labels:?}"
    );
    assert!(
        theme_labels.contains(&tr("setting.theme.dark")),
        "settings page should expose Dark theme option, got {theme_labels:?}"
    );

    let mut titles = Vec::new();
    collect_preference_titles(&page, &mut titles);
    assert!(
        titles.iter().any(|title| title == &tr("setting.theme")),
        "theme selector should live in a PreferencesRow, got {titles:?}"
    );
    assert!(
        titles
            .iter()
            .any(|title| title == &tr("setting.liquid_glass")),
        "Liquid Glass toggle should live in a PreferencesRow, got {titles:?}"
    );
    assert!(
        titles
            .iter()
            .any(|title| title == &tr("setting.video_default_muted")),
        "video mute toggle should live in a PreferencesRow, got {titles:?}"
    );
}

#[gtk::test]
fn settings_page_exposes_thumbnail_generation_speed_selector() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowThumbnailSpeed")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let page = window.build_settings_page(&host);
    let page = page.upcast::<gtk::Widget>();

    let mut titles = Vec::new();
    collect_preference_titles(&page, &mut titles);
    assert!(
        titles
            .iter()
            .any(|title| title == &tr("setting.thumbnail_generation_speed")),
        "settings page should expose thumbnail generation speed, got {titles:?}"
    );

    let mut check_buttons = Vec::new();
    collect_check_buttons(&page, &mut check_buttons);
    let speed_labels: Vec<String> = check_buttons
        .iter()
        .filter_map(|btn| btn.label().map(|l| l.to_string()))
        .collect();
    assert!(
        speed_labels.contains(&tr("setting.thumbnail_generation_speed.slow")),
        "should have Slow radio button, got {speed_labels:?}"
    );
    assert!(
        speed_labels.contains(&tr("setting.thumbnail_generation_speed.normal")),
        "should have Normal radio button, got {speed_labels:?}"
    );
    assert!(
        speed_labels.contains(&tr("setting.thumbnail_generation_speed.fast")),
        "should have Fast radio button, got {speed_labels:?}"
    );
    assert!(
        speed_labels.contains(&tr("setting.thumbnail_generation_speed.fastest")),
        "should have Fastest radio button, got {speed_labels:?}"
    );
}

#[gtk::test]
fn settings_page_exposes_scan_path_management() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowScanPaths")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let page = window.build_settings_page(&host);
    let page = page.upcast::<gtk::Widget>();

    let mut titles = Vec::new();
    collect_preference_titles(&page, &mut titles);
    assert!(
        titles
            .iter()
            .any(|title| title == &tr("setting.scan_paths.custom")),
        "settings page should expose custom scan path management, got {titles:?}"
    );
    assert!(
        titles
            .iter()
            .any(|title| title == &tr("setting.scan_paths.excluded")),
        "settings page should expose excluded scan path management, got {titles:?}"
    );
}

#[gtk::test]
fn settings_page_exposes_trash_backend_controls() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowTrashBackend")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let page = window.build_settings_page(&host);
    let page = page.upcast::<gtk::Widget>();

    let mut labels = Vec::new();
    collect_labels(&page, &mut labels);
    assert!(
        labels
            .iter()
            .any(|label| label == &tr("setting.section.trash")),
        "settings page should expose the trash settings section, got {labels:?}"
    );

    let mut titles = Vec::new();
    collect_preference_titles(&page, &mut titles);
    assert!(
        titles
            .iter()
            .any(|title| title == &tr("setting.trash.backend")),
        "settings page should expose the trash backend row, got {titles:?}"
    );
    assert!(
        titles
            .iter()
            .any(|title| title == &tr("setting.trash.system_available_title")),
        "settings page should include the system-trash migration recommendation row, got {titles:?}"
    );
}

#[gtk::test]
fn settings_storage_rows_defer_size_calculation() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowStorageUsage")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let page = window.build_settings_page(&host);
    let page = page.upcast::<gtk::Widget>();
    let pending = tr("setting.storage_usage_calculating");

    assert_eq!(
        find_action_row_subtitle(&page, &tr("setting.clear_thumbnails")).as_deref(),
        Some(pending.as_str()),
        "thumbnail cache size must not be calculated while constructing Settings"
    );
    assert_eq!(
        find_action_row_subtitle(&page, &tr("setting.clear_database")).as_deref(),
        Some(pending.as_str()),
        "database size must not be calculated while constructing Settings"
    );
}

#[gtk::test]
fn settings_dialog_uses_bounded_scroll_child() {
    let _ = gtk::init();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.WindowSettingsDialogBounds")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let window = MainWindow::new(&app);
    let host = window.clone().upcast::<gtk::Widget>();
    let dialog = window.build_settings_dialog(&host);

    assert!(
        dialog.content_height() <= 700,
        "settings dialog content height should stay below an 800px window; got {}",
        dialog.content_height()
    );
    assert!(
        dialog
            .child()
            .is_some_and(|child| child.is::<gtk::ScrolledWindow>()),
        "settings dialog should use a ScrolledWindow so tall content does not over-request sheet height"
    );
}
