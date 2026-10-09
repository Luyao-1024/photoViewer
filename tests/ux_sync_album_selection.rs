//! Saved-task album selection through the real Settings input path.
mod common;

use adw::prelude::*;
use common::interaction::{descendants, find_button_with_label, find_descendant, Ui};
use common::shell::Shell;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk4 as gtk;
use libadwaita as adw;
use photo_viewer::core::i18n::{tr, trf};
use photo_viewer::core::sync::{NewSyncJob, SyncDirection, SyncStore, UploadScope};
use photo_viewer::core::{prefs, sync::SyncService};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
#[path = "common/sync_provider.rs"]
mod sync_provider;
use sync_provider::ControlledProvider;

fn expander(root: &impl IsA<gtk::Widget>, title: &str) -> adw::ExpanderRow {
    descendants::<adw::ExpanderRow>(root)
        .into_iter()
        .find(|row| row.title() == title)
        .unwrap_or_else(|| panic!("missing expander {title}"))
}

fn album_rows(root: &impl IsA<gtk::Widget>) -> Vec<adw::ActionRow> {
    let mut rows = descendants::<adw::ActionRow>(root)
        .into_iter()
        .filter(|row| find_descendant::<gtk::CheckButton>(row).is_some())
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.index());
    rows
}

fn click(ui: &Ui, widget: &impl IsA<gtk::Widget>, label: &str) {
    ui.scroll_to_reveal(widget, label);
    ui.click(widget, label);
    ui.pump(Duration::from_millis(180));
}

fn open_albums(shell: &Shell) -> (adw::Dialog, adw::ExpanderRow, Ui) {
    shell
        .ui
        .click(&shell.window.imp().settings_button.get(), "Settings");
    assert!(shell.ui.wait_until(Duration::from_secs(2), || shell
        .window
        .visible_dialog()
        .is_some()));
    let dialog = shell.window.visible_dialog().unwrap();
    let ui = Ui::for_widget(&dialog);
    ui.pump(Duration::from_millis(250));
    let sync = expander(&dialog, &tr("setting.section.sync"));
    click(&ui, &sync, "expand WebDAV settings");
    assert!(sync.is_expanded());
    let albums = expander(&dialog, &tr("setting.sync.upload_albums"));
    click(&ui, &albums, "expand upload albums");
    assert!(albums.is_expanded());
    (dialog, albums, ui)
}

#[test]
fn saved_task_album_selection_selects_all_sorts_and_persists() {
    // This binary owns its process and isolates preferences from the real user.
    let config = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", config.path());
    std::env::set_var("XDG_DATA_HOME", config.path().join("data"));
    std::env::set_var("XDG_CACHE_HOME", config.path().join("cache"));
    gtk::init().expect("GTK init");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let panicked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let panic_flag = panicked.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        panic_flag.store(true, Ordering::Release);
        previous(info);
    }));

    journey_idle_selection();
    journey_edit_while_transferring_and_restart_latest();
    journey_root_browse_and_switch_while_transferring();
    journey_disable_and_enable_while_changes_apply();
    journey_confirm_delete_while_transferring();
    journey_conflict_choice_while_transferring();
    assert!(
        !panicked.load(Ordering::Acquire),
        "a panic caught by a GTK/GLib callback must still fail UX validation"
    );
}

fn journey_idle_selection() {
    let shell = Shell::new();
    shell.seed_extra_album();
    let store = SyncStore::with_actor(shell.pool.clone(), shell.db_actor.clone());
    // Fixture: an already saved task with no upload permission. All states under
    // test below are reached by hit-tested presses, never DB/widget setters.
    let job = store
        .create_job(&NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "ux".into(),
            credential_ref: "ux-album-selection".into(),
            local_root: shell.tmp.path().to_path_buf(),
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: Vec::new(),
        })
        .unwrap();
    let empty_root = tempfile::tempdir().unwrap();
    let empty_job = store
        .create_job(&NewSyncJob {
            endpoint: "https://empty.example.test/".into(),
            username: "ux".into(),
            credential_ref: "ux-empty-album-selection".into(),
            local_root: empty_root.path().to_path_buf(),
            remote_root: "Empty".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: Vec::new(),
        })
        .unwrap();
    let (dialog, albums, ui) = open_albums(&shell);
    assert!(
        !store.get_job(job.id).unwrap().unwrap().paused,
        "opening the checklist must not pause sync"
    );
    let all_row = descendants::<adw::ActionRow>(&albums)
        .into_iter()
        .find(|row| row.title() == tr("setting.sync.upload_albums_select_all"))
        .expect("the upload selector needs a Select All switch row");
    let all = find_descendant::<gtk::Switch>(&all_row).unwrap();
    assert!(!all.is_active());
    let rows = album_rows(&albums);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title(), "photos");
    assert_eq!(rows[1].title(), "second-album");
    assert_eq!(
        all_row.index(),
        0,
        "Select All stays above the sorted albums"
    );

    let second_check = find_descendant::<gtk::CheckButton>(&rows[1]).unwrap();
    let generation = store.get_job(job.id).unwrap().unwrap().config_generation;
    click(&ui, &second_check, "enable the second album");
    assert!(second_check.is_active());
    assert_eq!(store.upload_albums(job.id).unwrap(), vec!["second-album"]);
    assert_eq!(
        album_rows(&albums)[0].title(),
        "second-album",
        "checked albums move ahead immediately"
    );
    assert!(!all.is_active(), "partial selection is not all selected");
    assert_eq!(
        all_row.subtitle().as_deref(),
        Some(
            trf(
                "setting.sync.upload_albums_select_all_partial",
                &[("selected", "1"), ("total", "2")]
            )
            .as_str()
        )
    );
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        generation + 1
    );

    // One press selects every listed album with one atomic store update.
    click(&ui, &all_row, "select all albums");
    assert!(all.is_active());
    assert_eq!(
        store.upload_albums(job.id).unwrap(),
        vec!["photos", "second-album"]
    );
    assert!(album_rows(&albums)
        .iter()
        .all(|row| find_descendant::<gtk::CheckButton>(row)
            .unwrap()
            .is_active()));
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        generation + 2
    );
    assert_eq!(album_rows(&albums)[0].title(), "photos");

    // Leaving full selection via one album immediately clears the master state
    // without causing a second (bulk) write from its notification.
    let photos_check = find_descendant::<gtk::CheckButton>(&rows[0]).unwrap();
    click(&ui, &photos_check, "disable one album after Select All");
    assert!(!all.is_active());
    assert_eq!(store.upload_albums(job.id).unwrap(), vec!["second-album"]);
    assert_eq!(album_rows(&albums)[0].title(), "second-album");
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        generation + 3
    );
    click(&ui, &all_row, "select all again from a partial selection");
    assert!(all.is_active());
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        generation + 4
    );

    click(&ui, &all_row, "clear all upload selections");
    assert!(!all.is_active());
    assert!(store.upload_albums(job.id).unwrap().is_empty());
    assert!(album_rows(&albums)
        .iter()
        .all(|row| !find_descendant::<gtk::CheckButton>(row)
            .unwrap()
            .is_active()));
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        generation + 5
    );
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().upload_scope,
        UploadScope::SelectedAlbums
    );

    // An individual row is also clickable; it updates the master state and sort.
    click(&ui, &rows[1], "select second album by its row");
    assert_eq!(store.upload_albums(job.id).unwrap(), vec!["second-album"]);
    assert!(!all.is_active());
    assert_eq!(album_rows(&albums)[0].title(), "second-album");
    dialog.close();
    ui.pump(Duration::from_millis(250));
    let (reopened, albums, reopened_ui) = open_albums(&shell);
    assert_eq!(
        album_rows(&albums)[0].title(),
        "second-album",
        "saved selections are prioritized when reopening"
    );
    assert!(find_descendant::<gtk::CheckButton>(&album_rows(&albums)[0])
        .unwrap()
        .is_active());
    assert_eq!(store.upload_albums(job.id).unwrap(), vec!["second-album"]);

    let empty_albums = descendants::<adw::ExpanderRow>(&reopened)
        .into_iter()
        .find(|row| row.title() == tr("setting.sync.upload_albums") && album_rows(row).is_empty())
        .expect("the empty task's upload selector");
    click(&reopened_ui, &empty_albums, "expand the empty album list");
    assert!(empty_albums.is_expanded());
    let empty_all_row = descendants::<adw::ActionRow>(&empty_albums)
        .into_iter()
        .find(|row| row.title() == tr("setting.sync.upload_albums_select_all"))
        .unwrap();
    let empty_all = find_descendant::<gtk::Switch>(&empty_all_row).unwrap();
    assert!(!empty_all.is_sensitive());
    assert!(!empty_all.is_active());
    assert_eq!(
        empty_all.ancestor(adw::Dialog::static_type()).as_ref(),
        Some(reopened.upcast_ref()),
        "the empty-list switch belongs to the reopened dialog"
    );
    reopened_ui.scroll_to_reveal(&empty_all, "the disabled empty-list switch");
    // Visibility and event targeting are different for an insensitive control:
    // it must really be on screen, but GTK must not dispatch a press to it.
    let (x, y) = reopened_ui.pointer_at_center_of(&empty_all).unwrap();
    let visible_target = reopened_ui
        .root()
        .pick(x, y, gtk::PickFlags::INSENSITIVE)
        .expect("the disabled switch must be visible after scrolling");
    assert!(visible_target == empty_all || visible_target.is_ancestor(&empty_all));
    assert!(!reopened_ui
        .target_of(&empty_all)
        .is_some_and(|target| target == empty_all || target.is_ancestor(&empty_all)));
    assert!(!reopened_ui
        .try_click_even_if_inert(&empty_all, "the disabled empty-list switch")
        .unwrap());
    assert!(!empty_all.is_active());
    assert!(store.upload_albums(empty_job.id).unwrap().is_empty());
    assert_eq!(
        store
            .get_job(empty_job.id)
            .unwrap()
            .unwrap()
            .config_generation,
        empty_job.config_generation
    );
    let service = SyncService::with_actor(shell.pool.clone(), shell.db_actor.clone());
    assert!(shell.ui.wait_until(Duration::from_secs(3), || !service
        .has_pending_work(job.id)
        .unwrap()));
}

fn transferring_fixture() -> (
    Shell,
    SyncStore,
    SyncService,
    photo_viewer::core::sync::SyncJob,
    Arc<ControlledProvider>,
) {
    // Enabling the feature is a fixture precondition. Every configuration edit
    // under test below still travels through the actual Settings input path.
    prefs::set_webdav_sync_enabled(true).unwrap();
    let shell = Shell::new();
    shell.seed_extra_album();
    let service = SyncService::with_actor(shell.pool.clone(), shell.db_actor.clone());
    let store = service.store().clone();
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
            credential_ref: "ux-held-transfer".into(),
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
    assert!(!store.get_job(job.id).unwrap().unwrap().paused);
    (shell, store, service, job, provider)
}

fn wait_finished(shell: &Shell, service: &SyncService, id: i64) {
    assert!(
        shell.ui.wait_until(Duration::from_secs(6), || !service
            .has_pending_work(id)
            .unwrap()
            && !service.is_job_running(id).unwrap()),
        "latest configuration must run automatically without a manual refresh"
    );
    if let Some(job) = service.store().get_job(id).unwrap() {
        assert!(
            job.last_error.is_none(),
            "unexpected synchronization error: {:?}",
            job.last_error
        );
    }
}

fn select_all_row(albums: &adw::ExpanderRow) -> adw::ActionRow {
    descendants::<adw::ActionRow>(albums)
        .into_iter()
        .find(|row| row.title() == tr("setting.sync.upload_albums_select_all"))
        .unwrap()
}

fn journey_edit_while_transferring_and_restart_latest() {
    let (shell, store, service, job, provider) = transferring_fixture();
    let (_dialog, albums, ui) = open_albums(&shell);
    assert!(
        store.pending_change(job.id).unwrap().is_none(),
        "inspection cannot enqueue a configuration change"
    );
    assert!(
        !store.get_job(job.id).unwrap().unwrap().paused,
        "expanding while transferring cannot pause first"
    );
    assert_eq!(provider.probes.load(Ordering::Acquire), 1);
    let second = album_rows(&albums)
        .into_iter()
        .find(|row| row.title() == "second-album")
        .unwrap();
    let check = find_descendant::<gtk::CheckButton>(&second).unwrap();
    assert!(check.is_sensitive());
    click(
        &ui,
        &check,
        "change upload scope while a genuine transfer is held",
    );
    assert_eq!(
        store.desired_upload_albums(job.id).unwrap(),
        vec!["second-album"]
    );
    assert!(
        store.upload_albums(job.id).unwrap().is_empty(),
        "desired scope must not replace the old run's active context"
    );
    assert!(service.is_job_running(job.id).unwrap());
    assert!(
        check.is_sensitive(),
        "applying must not freeze subsequent edits"
    );
    let all = select_all_row(&albums);
    click(&ui, &all, "select all while the first edit is applying");
    click(&ui, &all, "clear all while the old transfer is still held");
    click(
        &ui,
        &second,
        "make the last accepted edit choose only the second album",
    );
    let desired = store.pending_change(job.id).unwrap().unwrap();
    assert_eq!(desired.upload_albums, Some(vec!["second-album".into()]));
    assert!(check.is_sensitive());
    assert_eq!(
        store.overview().unwrap().status,
        photo_viewer::core::sync::SyncOverviewStatus::Applying
    );
    provider.release_download();
    wait_finished(&shell, &service, job.id);
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        2,
        "queued intermediate scopes must not each start a run"
    );
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        desired.revision
    );
    assert_eq!(store.upload_albums(job.id).unwrap(), vec!["second-album"]);
    assert!(provider
        .objects
        .lock()
        .unwrap()
        .contains_key("Photos/second-album/three.jpg"));
    assert!(!provider
        .objects
        .lock()
        .unwrap()
        .contains_key("Photos/photos/photo-0.jpg"));
    assert!(shell.photos_dir().join("cloud.gif").exists());
}

fn browse_folder(shell: &Shell, dialog: &adw::Dialog, ui: &Ui) -> adw::Dialog {
    let browse = find_button_with_label(dialog, &tr("setting.sync.browse_cloud_folder")).unwrap();
    click(ui, &browse, "browse cloud folders during a transfer");
    assert!(shell.ui.wait_until(Duration::from_secs(3), || shell
        .window
        .visible_dialog()
        .is_some_and(|dialog| dialog.title() == tr("setting.sync.choose_cloud_folder"))));
    shell.window.visible_dialog().unwrap()
}

fn journey_root_browse_and_switch_while_transferring() {
    let (shell, store, service, job, provider) = transferring_fixture();
    let gif = provider.objects.lock().unwrap()["Photos/photos/cloud.gif"].clone();
    provider.insert("New/photos/new.gif", gif);
    let (dialog, _albums, ui) = open_albums(&shell);
    let picker = browse_folder(&shell, &dialog, &ui);
    let picker_ui = Ui::for_widget(&picker);
    let cancel = find_button_with_label(&picker, &tr("button.cancel")).unwrap();
    click(&picker_ui, &cancel, "cancel a read-only cloud browser");
    assert!(service.is_job_running(job.id).unwrap());
    assert!(!store.get_job(job.id).unwrap().unwrap().paused);
    assert!(store.pending_change(job.id).unwrap().is_none());
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        job.config_generation
    );

    let picker = browse_folder(&shell, &dialog, &ui);
    let picker_ui = Ui::for_widget(&picker);
    let select = find_button_with_label(&picker, &tr("setting.sync.use_cloud_folder")).unwrap();
    click(&picker_ui, &select, "submit the same cloud folder");
    assert!(store.pending_change(job.id).unwrap().is_none());
    assert!(!store.get_job(job.id).unwrap().unwrap().paused);
    assert!(service.is_job_running(job.id).unwrap());

    let picker = browse_folder(&shell, &dialog, &ui);
    let picker_ui = Ui::for_widget(&picker);
    let folders = find_descendant::<gtk::DropDown>(&picker).unwrap();
    picker_ui.scroll_to_reveal(&folders, "cloud folder choices");
    picker_ui.click_native(&folders, "open the cloud-folder drop-down");
    picker_ui.native_key("Home");
    picker_ui.native_key("Return");
    let model = folders.model().unwrap();
    let chosen = model
        .item(folders.selected())
        .unwrap()
        .downcast::<gtk::StringObject>()
        .unwrap()
        .string();
    assert_eq!(
        chosen, "New",
        "native keyboard input should choose the first cloud folder"
    );
    let select = find_button_with_label(&picker, &tr("setting.sync.use_cloud_folder")).unwrap();
    click(&picker_ui, &select, "accept the changed folder choice");
    assert!(shell.ui.wait_until(Duration::from_secs(2), || shell
        .window
        .visible_dialog()
        .is_some_and(|d| d.is::<adw::AlertDialog>())));
    let confirmation = shell.window.visible_dialog().unwrap();
    let confirm_ui = Ui::for_widget(&confirmation);
    let apply =
        find_button_with_label(&confirmation, &tr("setting.sync.switch_cloud_folder")).unwrap();
    click(&confirm_ui, &apply, "confirm the cloud-folder modification");
    assert_eq!(
        store.desired_job(job.id).unwrap().unwrap().remote_root,
        "New"
    );
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().remote_root,
        "Photos",
        "old publication must retain its root context until quiescence"
    );
    provider.release_download();
    wait_finished(&shell, &service, job.id);
    assert_eq!(store.get_job(job.id).unwrap().unwrap().remote_root, "New");
    assert!(
        shell.photos_dir().join("cloud.gif").exists(),
        "switching never deletes the completed old download"
    );
    assert!(
        shell.photos_dir().join("new.gif").exists(),
        "new root starts automatically"
    );
    assert!(store
        .entries(job.id)
        .unwrap()
        .iter()
        .all(|entry| entry.relative_path != "photos/cloud.gif"));
}

fn journey_disable_and_enable_while_changes_apply() {
    let (shell, store, service, job, provider) = transferring_fixture();
    let (dialog, albums, ui) = open_albums(&shell);
    let second = album_rows(&albums)
        .into_iter()
        .find(|row| row.title() == "second-album")
        .unwrap();
    click(&ui, &second, "accept a scope change before disabling sync");
    let sync = expander(&dialog, &tr("setting.section.sync"));
    let switch = descendants::<gtk::Switch>(&sync)
        .into_iter()
        .find(|sw| sw.tooltip_text().as_deref() == Some(tr("setting.sync.global_enable").as_str()))
        .unwrap();
    ui.scroll_to_reveal(&switch, "global sync switch");
    ui.click_native(&switch, "disable sync while the edit is applying");
    assert!(!prefs::webdav_sync_enabled());
    provider.release_download();
    wait_finished(&shell, &service, job.id);
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        1,
        "opt-out must suppress all automatic restarts"
    );
    assert_eq!(
        store.upload_albums(job.id).unwrap(),
        vec!["second-album"],
        "the edit remains saved while disabled"
    );
    ui.scroll_to_reveal(&switch, "enable sync again");
    ui.click_native(&switch, "enable sync using the saved latest scope");
    assert!(prefs::webdav_sync_enabled());
    wait_finished(&shell, &service, job.id);
    assert_eq!(provider.probes.load(Ordering::Acquire), 2);
    assert!(provider
        .objects
        .lock()
        .unwrap()
        .contains_key("Photos/second-album/three.jpg"));
    assert!(!provider
        .objects
        .lock()
        .unwrap()
        .contains_key("Photos/photos/photo-0.jpg"));
}

fn journey_confirm_delete_while_transferring() {
    let (shell, store, service, job, provider) = transferring_fixture();
    let (dialog, _albums, ui) = open_albums(&shell);
    let delete = descendants::<gtk::Button>(&dialog)
        .into_iter()
        .find(|button| {
            button.tooltip_text().as_deref()
                == Some(tr("setting.sync.delete_relation_tooltip").as_str())
        })
        .unwrap();
    click(&ui, &delete, "open relation deletion during a transfer");
    let confirmation = shell.window.visible_dialog().unwrap();
    let confirm_ui = Ui::for_widget(&confirmation);
    let cancel = find_button_with_label(&confirmation, &tr("button.cancel")).unwrap();
    click(&confirm_ui, &cancel, "cancel relation deletion");
    assert!(store.pending_change(job.id).unwrap().is_none());
    assert!(!store.get_job(job.id).unwrap().unwrap().paused);
    click(
        &ui,
        &delete,
        "confirm relation deletion only after reviewing it",
    );
    let confirmation = shell.window.visible_dialog().unwrap();
    let confirm_ui = Ui::for_widget(&confirmation);
    let remove =
        find_button_with_label(&confirmation, &tr("setting.sync.delete_relation")).unwrap();
    click(&confirm_ui, &remove, "accept relation deletion");
    assert!(store.pending_change(job.id).unwrap().unwrap().delete);
    assert!(service.is_job_running(job.id).unwrap());
    provider.release_download();
    wait_finished(&shell, &service, job.id);
    assert!(store.get_job(job.id).unwrap().is_none());
    assert!(
        shell.ui.wait_until(Duration::from_secs(2), || shell
            .window
            .imp()
            .settings_dialog
            .borrow()
            .is_none()),
        "deletion must close stale Settings without a reentrant borrow"
    );
    assert!(shell.photos_dir().join("cloud.gif").exists());
    assert!(provider
        .objects
        .lock()
        .unwrap()
        .contains_key("Photos/photos/cloud.gif"));
    let probes = provider.probes.load(Ordering::Acquire);
    let repeat = service.clone();
    tokio::spawn(async move {
        repeat.trigger_saved_jobs_once().await.unwrap();
    });
    shell.ui.pump(Duration::from_millis(250));
    assert_eq!(
        provider.probes.load(Ordering::Acquire),
        probes,
        "a remembered trigger cannot resurrect a deleted task"
    );
}

fn journey_conflict_choice_while_transferring() {
    prefs::set_webdav_sync_enabled(true).unwrap();
    let shell = Shell::new();
    let service = SyncService::with_actor(shell.pool.clone(), shell.db_actor.clone());
    let store = service.store().clone();
    let job = store
        .create_job(&NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "ux".into(),
            credential_ref: "ux-conflict-edit".into(),
            local_root: shell.tmp.path().to_path_buf(),
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: vec!["photos".into()],
        })
        .unwrap();
    let provider = Arc::new(ControlledProvider::default());
    let local = std::fs::read(shell.photos_dir().join("photo-0.jpg")).unwrap();
    let remote = std::fs::read(shell.photos_dir().join("photo-1.jpg")).unwrap();
    assert_ne!(local, remote);
    provider.insert("Photos/photos/photo-0.jpg", remote.clone());
    service
        .set_provider_for_tests(job.id, provider.clone())
        .unwrap();
    let first = service.clone();
    let id = job.id;
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = finished.clone();
    tokio::spawn(async move {
        first.run_saved_job(id).await.unwrap();
        done.store(true, Ordering::Release);
    });
    assert!(shell
        .ui
        .wait_until(Duration::from_secs(4), || finished.load(Ordering::Acquire)));
    assert_eq!(store.open_conflicts(job.id).unwrap().len(), 1);
    let gif = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/media/animated_source.gif"),
    )
    .unwrap();
    provider.insert("Photos/photos/zz-cloud.gif", gif);
    provider.hold_next_download();
    let running = service.clone();
    tokio::spawn(async move {
        running.run_saved_job(id).await.unwrap();
    });
    assert!(shell.ui.wait_until(Duration::from_secs(4), || provider
        .entered
        .load(Ordering::Acquire)));
    let before = provider.probes.load(Ordering::Acquire);
    let (dialog, _albums, ui) = open_albums(&shell);
    let use_cloud = find_button_with_label(&dialog, &tr("setting.sync.use_remote")).unwrap();
    click(
        &ui,
        &use_cloud,
        "accept a conflict choice while another file is transferring",
    );
    assert!(shell.ui.wait_until(Duration::from_secs(2), || store
        .pending_change(job.id)
        .unwrap()
        .is_some_and(|change| change.conflict.is_some())));
    assert!(service.is_job_running(job.id).unwrap());
    assert_eq!(
        std::fs::read(shell.photos_dir().join("photo-0.jpg")).unwrap(),
        local,
        "accepted choice waits for safe quiescence instead of clobbering an active publication"
    );
    provider.release_download();
    wait_finished(&shell, &service, job.id);
    assert!(store.open_conflicts(job.id).unwrap().is_empty());
    assert_eq!(
        std::fs::read(shell.photos_dir().join("photo-0.jpg")).unwrap(),
        remote
    );
    assert!(
        provider.probes.load(Ordering::Acquire) >= before + 2,
        "a valid conflict choice is followed by automatic reconciliation"
    );
    assert!(store.pending_change(job.id).unwrap().is_none());
}
