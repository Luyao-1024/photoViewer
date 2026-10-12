//! Hash contract edits use real Settings input while a transfer is in flight.
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

fn open(shell: &Shell) -> (adw::Dialog, Ui, gtk::Switch) {
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
    ui.pump(Duration::from_millis(300));
    let switch = descendants::<gtk::Switch>(&dialog)
        .into_iter()
        .find(|switch| switch.widget_name() == "sync-sha256-etag" && switch.is_mapped())
        .unwrap();
    (dialog, ui, switch)
}

fn press(ui: &Ui, switch: &gtk::Switch) {
    ui.scroll_to_reveal(switch, "SHA-256 server contract");
    let expected = !switch.is_active();
    ui.click_native(switch, "toggle SHA-256 server contract");
    assert!(ui.wait_until(Duration::from_secs(2), || switch.is_active() == expected));
}

#[test]
fn sha256_etag_settings_default_on_and_reconfigure_without_manual_resume() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    photo_viewer::core::prefs::set_webdav_sync_enabled(true).unwrap();
    let empty = Shell::new();
    let (dialog, ui, switch) = open(&empty);
    assert!(
        switch.is_active(),
        "new tasks explicitly default to SHA-256 server mode"
    );
    press(&ui, &switch);
    assert!(!switch.is_active());
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
            credential_ref: "ux-hash".into(),
            etag_is_sha256: false,
            local_root: shell.tmp.path().to_path_buf(),
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: vec![],
        })
        .unwrap();
    assert!(shell.ui.wait_until(Duration::from_secs(4), || provider
        .entered
        .load(Ordering::Acquire)));
    let (dialog, ui, switch) = open(&shell);
    assert!(
        !switch.is_active(),
        "existing tasks retain their saved contract"
    );
    assert!(service.store().pending_change(job.id).unwrap().is_none());
    press(&ui, &switch);
    assert!(switch.is_active());
    assert!(
        service
            .store()
            .desired_job(job.id)
            .unwrap()
            .unwrap()
            .etag_is_sha256
    );
    assert!(
        !service
            .store()
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .etag_is_sha256,
        "in-flight work keeps its original contract"
    );
    press(&ui, &switch);
    press(&ui, &switch);
    provider.release_download();
    assert!(ui.wait_until(Duration::from_secs(8), || !service
        .is_job_running(job.id)
        .unwrap()
        && service.store().pending_change(job.id).unwrap().is_none()
        && service
            .store()
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .etag_is_sha256));
    assert!(
        provider.probes.load(Ordering::Acquire) >= 2,
        "new configuration automatically reruns"
    );
    dialog.close();
    ui.pump(Duration::from_millis(200));
    let (dialog, ui, switch) = open(&shell);
    assert!(
        switch.is_active(),
        "reopening settings reflects the persisted contract"
    );
    dialog.close();
    ui.pump(Duration::from_millis(200));
}
