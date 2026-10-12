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
        .find(|button| {
            button.is_mapped()
                && button.widget_name()
                    == format!("sync-strategy-{}", key.rsplit('.').next().unwrap())
        })
        .unwrap()
}

fn assert_selection(backup: &gtk::ToggleButton, sync: &gtk::ToggleButton) {
    let capsule = backup.parent().unwrap();
    assert!(
        capsule.width() <= 176,
        "strategy width: {}",
        capsule.width()
    );
    assert!(
        capsule.height() <= 42,
        "strategy height: {}",
        capsule.height()
    );
    assert_eq!(backup.width(), sync.width(), "equal, stable hit targets");
    assert_ne!(backup.is_active(), sync.is_active());
    for button in [backup, sync] {
        let indicator = descendants::<gtk::Box>(button)
            .into_iter()
            .find(|widget| widget.has_css_class("glass-segment-indicator"))
            .unwrap();
        assert_eq!(
            indicator.opacity(),
            if button.is_active() { 1.0 } else { 0.0 }
        );
    }
    let a = backup.style_context().color();
    let b = sync.style_context().color();
    assert!(
        (a.red() - b.red()).abs() + (a.green() - b.green()).abs() + (a.blue() - b.blue()).abs()
            > 0.2,
        "selected and unselected text need distinct colors: {a:?}, {b:?}"
    );
}

fn export_strategy(button: &gtk::ToggleButton, name: &str) {
    let Some(directory) = std::env::var_os("PHOTOVIEWER_STRATEGY_SCREENSHOTS") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let row = button.ancestor(adw::ActionRow::static_type()).unwrap();
    let snapshot = gtk::Snapshot::new();
    // ActionRow is transparent; preserve its theme surface in standalone PNGs.
    snapshot.append_color(
        &row.style_context().lookup_color("window_bg_color").unwrap(),
        &gtk::graphene::Rect::new(0.0, 0.0, row.width() as f32, row.height() as f32),
    );
    gtk::WidgetPaintable::new(Some(&row)).snapshot(
        &snapshot,
        f64::from(row.width()),
        f64::from(row.height()),
    );
    let root = row.root().unwrap().downcast::<gtk::Window>().unwrap();
    root.renderer()
        .unwrap()
        .render_texture(
            snapshot.to_node().unwrap(),
            Some(&gtk::graphene::Rect::new(
                0.0,
                0.0,
                row.width() as f32,
                row.height() as f32,
            )),
        )
        .save_to_png(directory.join(format!("{name}.png")))
        .unwrap();
}

fn press(ui: &Ui, button: &gtk::ToggleButton, label: &str) {
    ui.scroll_to_reveal(button, label);
    ui.click(button, label);
    ui.pump(Duration::from_millis(120));
}

#[test]
fn webdav_strategy_buttons_default_to_backup_and_reconfigure_an_active_task() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
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
    // Appearance variants are fixtures; strategy changes use real hit-tested presses.
    let style = adw::StyleManager::default();
    let original_scheme = style.color_scheme();
    let original_material = photo_viewer::core::prefs::liquid_glass_enabled();
    for (theme, scheme) in [
        ("light", adw::ColorScheme::ForceLight),
        ("dark", adw::ColorScheme::ForceDark),
    ] {
        style.set_color_scheme(scheme);
        for (material, liquid) in [("liquid", true), ("plain", false)] {
            photo_viewer::ui::grid_css::reapply(liquid);
            ui.pump(Duration::from_millis(500));
            ui.scroll_to_reveal(&backup, "compact strategy selector");
            assert_selection(&backup, &sync);
            export_strategy(&backup, &format!("{theme}-{material}-backup"));
            press(&ui, &sync, "select sync with accent text and underline");
            assert_selection(&backup, &sync);
            export_strategy(&sync, &format!("{theme}-{material}-sync"));
            press(&ui, &backup, "select backup with accent text and underline");
            assert_selection(&backup, &sync);
        }
    }
    style.set_color_scheme(original_scheme);
    photo_viewer::ui::grid_css::reapply(original_material);
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
            etag_is_sha256: false,
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
