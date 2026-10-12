//! Startup recovery localizes persisted failures through the real Settings path.
mod common;
#[allow(dead_code)] // The shared provider also exposes release controls used by other journeys.
#[path = "common/sync_provider.rs"]
mod sync_provider;

use common::interaction::{descendants, Ui};
use common::shell::Shell;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::core::i18n::{locale, tr, trf};
use photo_viewer::core::prefs;
use photo_viewer::core::sync::{
    NewSyncJob, SyncDirection, SyncFailureReason, SyncOverviewStatus, SyncService, UploadScope,
};
use std::process::Command;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use sync_provider::ControlledProvider;

const LOCALE_CHILD: &str = "PHOTO_VIEWER_INTERRUPTED_ERROR_TEST_LOCALE";

#[test]
fn interrupted_sync_error_is_localized_in_settings() {
    // i18n resolves once per process. Run each language in its own instance of
    // this one-test binary, sharing only the existing display/accessibility bus.
    let Ok(language) = std::env::var(LOCALE_CHILD) else {
        for language in ["en", "zh-CN"] {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "interrupted_sync_error_is_localized_in_settings",
                    "--nocapture",
                ])
                .env(LOCALE_CHILD, language)
                .status()
                .unwrap();
            assert!(
                status.success(),
                "Settings recovery journey failed for {language}"
            );
        }
        return;
    };

    // Each child owns its process and a private configuration/data/cache root.
    let config = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", config.path());
    std::env::set_var("XDG_DATA_HOME", config.path().join("data"));
    std::env::set_var("XDG_CACHE_HOME", config.path().join("cache"));
    std::env::set_var("PHOTO_VIEWER_LOCALE", &language);
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    prefs::set_webdav_sync_enabled(true).unwrap();
    assert_eq!(locale(), language);

    let shell = Shell::new();
    let service = SyncService::with_actor(shell.pool.clone(), shell.db_actor.clone());
    let job = service
        .store()
        .create_job(&NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "recovery".into(),
            credential_ref: "ux-interrupted-no-keyring".into(),
            etag_is_sha256: false,
            local_root: shell.tmp.path().to_path_buf(),
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: Vec::new(),
        })
        .unwrap();
    let provider = Arc::new(ControlledProvider::default());
    provider.insert(
        "Photos/cloud/recovery.jpg",
        std::fs::read(&shell.items[0].path).unwrap(),
    );
    provider.hold_next_download();
    service
        .set_provider_for_tests(job.id, provider.clone())
        .unwrap();
    let runner = service.clone();
    let task = runtime.spawn(async move { runner.run_saved_job(job.id).await });
    assert!(shell.ui.wait_until(Duration::from_secs(8), || provider
        .entered
        .load(Ordering::Acquire)));
    // Abort a real pending transfer before its completion can be recorded, as
    // process exit would. Do not manufacture a failure by updating database rows.
    task.abort();
    let cancelled = runtime.block_on(task).unwrap_err();
    assert!(cancelled.is_cancelled());
    assert_eq!(
        service.store().overview().unwrap().status,
        SyncOverviewStatus::Running
    );
    drop(service);
    let restarted = SyncService::with_actor(shell.pool.clone(), shell.db_actor.clone());
    assert_eq!(restarted.reconcile_interrupted_runs().unwrap(), 1);
    let stored_reason = restarted
        .store()
        .get_job(job.id)
        .unwrap()
        .unwrap()
        .last_error;
    assert_eq!(
        stored_reason.as_deref(),
        Some(SyncFailureReason::InterruptedRun.as_str())
    );

    shell
        .ui
        .click(&shell.window.imp().settings_button.get(), "Settings");
    assert!(shell.ui.wait_until(Duration::from_secs(3), || shell
        .window
        .visible_dialog()
        .is_some()));
    let dialog = shell.window.visible_dialog().unwrap();
    let ui = Ui::for_widget(&dialog);
    let sync = descendants::<adw::ExpanderRow>(&dialog)
        .into_iter()
        .find(|row| row.title() == tr("setting.section.sync"))
        .unwrap();
    ui.scroll_to_reveal(&sync, "WebDAV settings");
    ui.click(&sync, "expand WebDAV settings");
    ui.pump(Duration::from_millis(350));

    let message = tr("setting.sync.error.interrupted_run");
    assert_ne!(message, "setting.sync.error.interrupted_run");
    if language == "en" {
        assert!(
            message.is_ascii(),
            "English copy must not contain the legacy Chinese sentence"
        );
    }
    let expected = trf("setting.sync.failed", &[("error", &message)]);
    let task_title = format!("{} ↔ {}", job.local_root.display(), job.remote_root);
    let row = descendants::<adw::ActionRow>(&dialog)
        .into_iter()
        .find(|row| row.title() == task_title)
        .unwrap();
    ui.scroll_to_reveal(&row, "recovered task status");
    assert!(
        ui.wait_until(Duration::from_secs(2), || row.subtitle().as_deref()
            == Some(expected.as_str()))
    );
    let upload = descendants::<adw::ExpanderRow>(&dialog)
        .into_iter()
        .find(|row| row.title() == tr("setting.sync.upload_albums"))
        .unwrap();
    ui.scroll_to_reveal(&upload, "upload album recovery status");
    ui.click(&upload, "expand upload album selection");
    assert!(
        ui.wait_until(Duration::from_secs(2), || upload.subtitle().as_str()
            == expected.as_str())
    );
    // Wait past both status refresh intervals: neither may reintroduce the tag
    // or an old-language sentence after the initial settings render.
    ui.pump(Duration::from_millis(700));
    assert_eq!(row.subtitle().as_deref(), Some(expected.as_str()));
    assert_eq!(upload.subtitle().as_str(), expected.as_str());
    assert_eq!(restarted.reconcile_interrupted_runs().unwrap(), 0);
    assert_eq!(
        restarted
            .store()
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .last_error,
        stored_reason
    );
    assert!(restarted.store().pending_change(job.id).unwrap().is_none());
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        1,
        "opening/expanding Settings must not rerun sync"
    );
    dialog.close();
}
