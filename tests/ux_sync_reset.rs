//! Global WebDAV off clears history through real input and keeps configuration.
mod common;
#[path = "common/sync_provider.rs"]
mod sync_provider;
use adw::prelude::*;
use common::interaction::{descendants, find_button_with_label, Ui};
use common::shell::Shell;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk4 as gtk;
use libadwaita as adw;
use photo_viewer::core::sync::{NewSyncJob, SyncDirection, SyncService, UploadScope};
use photo_viewer::core::{db, i18n::tr, identity::MediaId, prefs};
use photo_viewer::ui::ViewerPage;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use sync_provider::ControlledProvider;

fn settings(shell: &Shell) -> (adw::Dialog, Ui, gtk::Switch) {
    shell
        .ui
        .click(&shell.window.imp().settings_button.get(), "Settings");
    assert!(shell.ui.wait_until(Duration::from_secs(3), || shell
        .window
        .visible_dialog()
        .is_some()));
    let dialog = shell.window.visible_dialog().unwrap();
    let ui = Ui::for_widget(&dialog);
    let row = descendants::<adw::ExpanderRow>(&dialog)
        .into_iter()
        .find(|row| row.title() == tr("setting.section.sync"))
        .unwrap();
    ui.scroll_to_reveal(&row, "WebDAV settings");
    ui.click(&row, "expand WebDAV settings");
    ui.pump(Duration::from_millis(200));
    let switch = descendants::<gtk::Switch>(&row)
        .into_iter()
        .find(|switch| {
            switch.tooltip_text().as_deref() == Some(tr("setting.sync.global_enable").as_str())
        })
        .unwrap();
    (dialog, ui, switch)
}

fn toggle(ui: &Ui, switch: &gtk::Switch, enabled: bool) {
    assert_ne!(switch.is_active(), enabled);
    ui.scroll_to_reveal(switch, "global WebDAV switch");
    ui.click_native(
        switch,
        if enabled {
            "enable WebDAV"
        } else {
            "disable WebDAV and clear history"
        },
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || prefs::webdav_sync_enabled()
            == enabled
            && switch.is_active() == enabled)
    );
}

fn wait_finished(ui: &Ui, service: &SyncService, id: i64) {
    assert!(ui.wait_until(Duration::from_secs(8), || !service
        .has_pending_work(id)
        .unwrap()
        && !service.is_job_running(id).unwrap()));
}

fn assert_empty(shell: &Shell, service: &SyncService, id: i64) {
    let conn = shell.pool.get().unwrap();
    for table in [
        "sync_entries",
        "sync_tasks",
        "sync_conflicts",
        "sync_job_changes",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table} must be cleared"
        );
    }
    let state: (Option<i64>, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT last_started_at,last_completed_at,last_error FROM sync_jobs WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, (None, None, None));
    assert!(!service.store().reset_pending().unwrap());
    assert!(!photo_viewer::config::data_dir()
        .join("sync-staging")
        .exists());
}

#[test]
fn global_off_clears_suppression_conflicts_and_staging_and_serializes_rapid_toggles() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    prefs::set_webdav_sync_enabled(true).unwrap();
    prefs::set_trash_backend(prefs::TrashBackend::App).unwrap();
    let shell = Shell::new();
    let service = SyncService::with_actor(shell.pool.clone(), shell.db_actor.clone());
    let provider = Arc::new(ControlledProvider::default());
    service.set_provider_for_tests(1, provider.clone()).unwrap();
    let job = service
        .create_job(&NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "ux".into(),
            credential_ref: "ux-reset-kept".into(),
            etag_is_sha256: false,
            local_root: shell.tmp.path().to_path_buf(),
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: vec!["photos".into()],
        })
        .unwrap();
    wait_finished(&shell.ui, &service, job.id);
    let target = shell.items[0].clone();
    let tile = shell.tile_for(
        &shell.visible_photos_grid(),
        MediaId::from(target.id),
        "photo to delete",
    );
    shell.ui.click(&tile, "open the synced photo");
    assert!(shell.ui.wait_until(Duration::from_secs(5), || shell
        .window
        .nav_view()
        .visible_page()
        .is_some_and(|page| page.is::<ViewerPage>())));
    let viewer = shell
        .window
        .nav_view()
        .visible_page()
        .unwrap()
        .downcast::<ViewerPage>()
        .unwrap();
    assert!(shell.ui.wait_until(Duration::from_secs(8), || viewer
        .imp()
        .edit_btn
        .get()
        .is_sensitive()));
    shell
        .ui
        .click(&viewer.imp().favorite_btn.get(), "keep the favorite flag");
    assert!(shell
        .ui
        .wait_until(Duration::from_secs(3), || db::is_media_favorite(
            &shell.pool,
            target.id
        )
        .unwrap()));
    shell
        .ui
        .click(&viewer.imp().delete_btn.get(), "delete the local photo");
    assert!(shell
        .ui
        .wait_until(Duration::from_secs(4), || descendants::<adw::AlertDialog>(
            &shell.window
        )
        .iter()
        .any(
            |dialog| find_button_with_label(dialog, &tr("dialog.trash")).is_some()
        )));
    let alert = descendants::<adw::AlertDialog>(&shell.window)
        .into_iter()
        .find(|dialog| find_button_with_label(dialog, &tr("dialog.trash")).is_some())
        .unwrap();
    let alert_ui = Ui::for_widget(&alert);
    alert_ui.click(
        &find_button_with_label(&alert, &tr("dialog.trash")).unwrap(),
        "confirm local deletion",
    );
    assert!(shell
        .ui
        .wait_until(Duration::from_secs(8), || !target.path.exists()));
    shell.ui.pump(Duration::from_millis(250));

    // External edits are real file contents, then the normal engine detects a conflict.
    let other = &shell.items[1];
    let mut local_bytes = std::fs::read(&other.path).unwrap();
    let mut cloud_bytes = local_bytes.clone();
    local_bytes.extend_from_slice(b"local edit");
    cloud_bytes.extend_from_slice(b"cloud edit");
    std::fs::write(&other.path, &local_bytes).unwrap();
    provider.insert(
        &format!(
            "Photos/photos/{}",
            other.path.file_name().unwrap().to_string_lossy()
        ),
        cloud_bytes,
    );
    service.request_job_sync(job.id).unwrap();
    wait_finished(&shell.ui, &service, job.id);
    assert!(service
        .store()
        .entries(job.id)
        .unwrap()
        .iter()
        .any(|entry| entry.suppressed.is_some()));
    assert!(!service.store().open_conflicts(job.id).unwrap().is_empty());
    let cloud_before = provider.objects.lock().unwrap().clone();
    let media_count = shell
        .pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM media_items", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let staging = photo_viewer::config::data_dir().join("sync-staging");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("orphan.download"), b"partial").unwrap();
    let sibling = shell.photos_dir().join(".photoviewer-sync-interrupted");
    std::fs::write(&sibling, b"partial publication").unwrap();
    let (dialog, ui, switch) = settings(&shell);
    assert!(
        descendants::<gtk::Button>(&dialog)
            .iter()
            .any(|button| button.label().as_deref() == Some(tr("setting.sync.use_local").as_str())),
        "the actual conflict must have a Settings row before cleanup"
    );
    toggle(&ui, &switch, false);
    wait_finished(&ui, &service, job.id);
    assert_empty(&shell, &service, job.id);
    assert!(!sibling.exists());
    let kept = service.store().get_job(job.id).unwrap().unwrap();
    assert_eq!(
        (
            kept.endpoint,
            kept.username,
            kept.credential_ref,
            kept.local_root,
            kept.remote_root,
            kept.direction,
            kept.upload_scope
        ),
        (
            job.endpoint.clone(),
            job.username.clone(),
            job.credential_ref.clone(),
            job.local_root.clone(),
            job.remote_root.clone(),
            job.direction,
            job.upload_scope
        )
    );
    assert_eq!(
        service.store().upload_albums(job.id).unwrap(),
        vec!["photos"]
    );
    assert_eq!(
        *provider.objects.lock().unwrap(),
        cloud_before,
        "off must not change cloud contents"
    );
    assert_eq!(std::fs::read(&other.path).unwrap(), local_bytes);
    assert!(db::is_media_favorite(&shell.pool, target.id).unwrap());
    assert!(db::list_trashed_media(&shell.pool)
        .unwrap()
        .iter()
        .any(|item| item.id == target.id));
    assert_eq!(
        shell
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM media_items", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        media_count
    );
    assert!(
        !descendants::<gtk::Button>(&dialog)
            .iter()
            .any(|button| button.label().as_deref() == Some(tr("setting.sync.use_local").as_str())),
        "cleared conflict widgets and callbacks must be removed from Settings"
    );
    toggle(&ui, &switch, true);
    wait_finished(&ui, &service, job.id);
    assert!(
        target.path.exists(),
        "a cleared suppression record permits first-time download again"
    );
    assert!(service
        .store()
        .entries(job.id)
        .unwrap()
        .iter()
        .all(|entry| entry.suppressed.is_none()));

    // A new opt-in must schedule the fresh run even if its old worker is retiring.
    provider.insert(
        "Photos/photos/reopen.gif",
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/media/animated_source.gif"),
        )
        .unwrap(),
    );
    provider.hold_next_download();
    service.request_job_sync(job.id).unwrap();
    assert!(ui.wait_until(Duration::from_secs(5), || provider
        .entered
        .load(Ordering::Acquire)));
    let before_reopen = provider.probes.load(Ordering::Acquire);
    toggle(&ui, &switch, false);
    toggle(&ui, &switch, true);
    assert!(service.store().reset_pending().unwrap());
    assert_eq!(provider.probes.load(Ordering::Acquire), before_reopen);
    provider.release_download();
    wait_finished(&ui, &service, job.id);
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        before_reopen + 1,
        "a newer opt-in must restart without waiting for the next poll"
    );
    assert!(service
        .store()
        .entries(job.id)
        .unwrap()
        .iter()
        .any(|entry| entry.relative_path == "photos/reopen.gif" && entry.baseline.is_some()));

    // Off -> on -> off during a held operation must never start the intermediate run.
    provider.insert(
        "Photos/photos/held.gif",
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/media/animated_source.gif"),
        )
        .unwrap(),
    );
    provider.hold_next_download();
    service.request_job_sync(job.id).unwrap();
    assert!(ui.wait_until(Duration::from_secs(5), || provider
        .entered
        .load(Ordering::Acquire)));
    let probes = provider.probes.load(Ordering::Acquire);
    toggle(&ui, &switch, false);
    assert!(service.store().reset_pending().unwrap());
    toggle(&ui, &switch, true);
    assert!(service.store().reset_pending().unwrap());
    assert_eq!(provider.probes.load(Ordering::Acquire), probes);
    toggle(&ui, &switch, false);
    provider.release_download();
    wait_finished(&ui, &service, job.id);
    assert_empty(&shell, &service, job.id);
    assert_eq!(provider.probes.load(Ordering::Acquire), probes);
    assert!(!prefs::webdav_sync_enabled());
    dialog.close();
}
