//! Real Settings presses change the saved strategy while a transfer is active.
mod common;
#[path = "common/sync_provider.rs"]
mod sync_provider;

use adw::prelude::*;
use common::interaction::{descendants, Ui};
use common::shell::Shell;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk4 as gtk;
use libadwaita as adw;
use photo_viewer::core::i18n::tr;
use photo_viewer::core::sync::{NewSyncJob, SyncDirection, SyncService, UploadScope};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use sync_provider::ControlledProvider;

fn open(shell: &Shell) -> (adw::Dialog, Ui) {
    shell
        .ui
        .click(&shell.window.imp().settings_button.get(), "Settings");
    assert!(shell.ui.wait_until(Duration::from_secs(2), || shell
        .window
        .visible_dialog()
        .is_some()));
    let dialog = shell.window.visible_dialog().unwrap();
    let ui = Ui::for_widget(&dialog);
    let expander = descendants::<adw::ExpanderRow>(&dialog)
        .into_iter()
        .find(|row| row.title() == tr("setting.section.sync"))
        .unwrap();
    ui.scroll_to_reveal(&expander, "WebDAV settings");
    ui.click(&expander, "expand WebDAV settings");
    ui.pump(Duration::from_millis(180));
    (dialog, ui)
}

fn mode(root: &impl IsA<gtk::Widget>, key: &str) -> gtk::ToggleButton {
    descendants::<gtk::ToggleButton>(root)
        .into_iter()
        .find(|button| button.is_mapped() && button.label().as_deref() == Some(tr(key).as_str()))
        .unwrap()
}

fn press(ui: &Ui, button: &gtk::ToggleButton, label: &str) {
    ui.scroll_to_reveal(button, label);
    ui.click(button, label);
    ui.pump(Duration::from_millis(120));
}

#[test]
fn webdav_strategy_buttons_default_to_backup_and_reconfigure_an_active_task() {
    let config = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", config.path());
    std::env::set_var("XDG_DATA_HOME", config.path().join("data"));
    std::env::set_var("XDG_CACHE_HOME", config.path().join("cache"));
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    photo_viewer::core::prefs::set_webdav_sync_enabled(true).unwrap();

    // An unsaved connection exposes the two modes, with backup selected.
    let empty = Shell::new();
    let (dialog, ui) = open(&empty);
    let backup = mode(&dialog, "setting.sync.strategy.backup");
    let sync = mode(&dialog, "setting.sync.strategy.sync");
    assert!(backup.is_active());
    assert!(!sync.is_active());
    press(&ui, &sync, "choose sync for the new task");
    assert!(sync.is_active());
    assert!(!backup.is_active());
    press(&ui, &backup, "choose backup for the new task");
    assert!(backup.is_active());
    dialog.close();
    ui.pump(Duration::from_millis(200));
    drop(empty);

    let shell = Shell::new();
    let service = SyncService::with_actor(shell.pool.clone(), shell.db_actor.clone());
    let provider = Arc::new(ControlledProvider::default());
    let gif = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/media/animated_source.gif"),
    )
    .unwrap();
    provider.insert("Photos/photos/cloud.gif", gif);
    provider.hold_next_download();
    service.set_provider_for_tests(1, provider.clone()).unwrap();
    let job = service
        .create_job(&NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "ux".into(),
            credential_ref: "ux-strategy".into(),
            local_root: shell.tmp.path().to_path_buf(),
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: Vec::new(),
        })
        .unwrap();
    assert!(shell.ui.wait_until(Duration::from_secs(4), || provider
        .entered
        .load(Ordering::Acquire)));
    assert!(service.is_job_running(job.id).unwrap());
    service.request_automatic_sync().unwrap();
    shell.ui.pump(Duration::from_millis(100));
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        1,
        "polling does not queue a duplicate active run"
    );
    let (dialog, ui) = open(&shell);
    let backup = mode(&dialog, "setting.sync.strategy.backup");
    let sync = mode(&dialog, "setting.sync.strategy.sync");
    assert!(sync.is_active());
    assert!(
        service.store().pending_change(job.id).unwrap().is_none(),
        "opening settings is side effect free"
    );
    press(&ui, &backup, "switch an active task to backup");
    assert!(backup.is_active());
    assert_eq!(
        service
            .store()
            .desired_job(job.id)
            .unwrap()
            .unwrap()
            .direction,
        SyncDirection::UploadOnly
    );
    assert_eq!(
        service.store().get_job(job.id).unwrap().unwrap().direction,
        SyncDirection::Bidirectional,
        "in-flight work keeps its original context"
    );
    press(&ui, &sync, "supersede the pending edit with sync");
    press(&ui, &backup, "choose backup as the final mode");
    provider.release_download();
    assert!(ui.wait_until(Duration::from_secs(6), || !service
        .has_pending_work(job.id)
        .unwrap()));
    let latest = service.store().get_job(job.id).unwrap().unwrap();
    assert_eq!(latest.direction, SyncDirection::UploadOnly);
    assert!(!latest.paused);
    assert!(latest.config_generation > job.config_generation);
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        2,
        "rapid edits coalesce into one automatic restart"
    );
    dialog.close();
    ui.pump(Duration::from_millis(200));
    let (reopened, reopened_ui) = open(&shell);
    assert!(mode(&reopened, "setting.sync.strategy.backup").is_active());
    press(
        &reopened_ui,
        &mode(&reopened, "setting.sync.strategy.backup"),
        "press the already selected backup mode",
    );
    assert!(mode(&reopened, "setting.sync.strategy.backup").is_active());
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        2,
        "a no-op mode choice must not restart sync"
    );
    // A saved job remains editable while the global switch is off.
    let global_row = descendants::<adw::ExpanderRow>(&reopened)
        .into_iter()
        .find(|row| row.title() == tr("setting.section.sync"))
        .unwrap();
    let global = descendants::<gtk::Switch>(&global_row)
        .into_iter()
        .find(|switch| {
            switch.tooltip_text().as_deref() == Some(tr("setting.sync.global_enable").as_str())
        })
        .unwrap();
    reopened_ui.scroll_to_reveal(&global, "disable WebDAV");
    reopened_ui.click_native(&global, "disable WebDAV");
    assert!(reopened_ui.wait_until(Duration::from_secs(5), || {
        !photo_viewer::core::prefs::webdav_sync_enabled()
    }));
    assert!(
        !global.is_active(),
        "the real press must turn the global switch off"
    );
    assert!(!photo_viewer::core::prefs::webdav_sync_enabled());
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        2,
        "opening Settings and disabling must not add a run"
    );
    assert!(reopened_ui.wait_until(Duration::from_secs(4), || !service
        .has_pending_work(job.id)
        .unwrap()));
    assert!(
        service.store().entries(job.id).unwrap().is_empty(),
        "off clears history but preserves the chosen backup mode"
    );
    assert_eq!(
        service.store().get_job(job.id).unwrap().unwrap().direction,
        SyncDirection::UploadOnly
    );
    let sync = mode(&reopened, "setting.sync.strategy.sync");
    press(
        &reopened_ui,
        &sync,
        "save sync mode while globally disabled",
    );
    assert!(reopened_ui.wait_until(Duration::from_secs(3), || !service
        .has_pending_work(job.id)
        .unwrap()));
    assert_eq!(
        service.store().get_job(job.id).unwrap().unwrap().direction,
        SyncDirection::Bidirectional
    );
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        2,
        "disabled edits must not start network work"
    );
    reopened.close();
}
