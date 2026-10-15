//! Native settings: drafts have no side effects; only accepted choices reach the worker.
use super::*;
use crate::core::sync::{
    model::{SyncDirection, UploadScope},
    SyncService,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn button(key: &str, name: &str) -> gtk::Button {
    let button = gtk::Button::with_label(&tr(key));
    button.set_widget_name(name);
    button.add_css_class("glass-toolbar-button");
    button.set_valign(gtk::Align::Center);
    button
}
fn label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label
}
fn surface(title: &str, content: &gtk::Box) -> adw::Dialog {
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(content)
        .build();
    adw::Dialog::builder()
        .title(tr(title))
        .content_width(500)
        .content_height(580)
        .child(&scroller)
        .build()
}
fn column() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build()
}
fn row(key: &str, name: &str, value: &str) -> adw::EntryRow {
    let row = adw::EntryRow::builder().title(tr(key)).text(value).build();
    row.set_widget_name(name);
    row
}

impl MainWindow {
    pub(super) fn append_native_sync_settings(&self, content: &gtk::Box, parent: &gtk::Widget) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let service = match self.imp().db_actor.borrow().as_ref().cloned() {
            Some(actor) => SyncService::with_actor(pool, actor),
            None => SyncService::new(pool),
        };
        let group = adw::PreferencesGroup::new();
        group.set_title(&tr("native.settings"));
        group.set_description(Some(&tr("native.scope_hint")));
        group.set_widget_name("native-sync-settings");
        let enabled = gtk::Switch::builder()
            .active(prefs::sync_enabled())
            .valign(gtk::Align::Center)
            .build();
        enabled.set_widget_name("native-sync-enabled");
        let enabled_row = adw::ActionRow::builder()
            .title(tr("native.enabled"))
            .build();
        enabled_row.add_suffix(&enabled);
        group.add(&enabled_row);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let add = button("native.add", "native-sync-add");
        let sync = button("native.sync_now", "native-sync-now");
        actions.append(&add);
        actions.append(&sync);
        group.add(&actions);
        let status = label();
        status.set_widget_name("native-sync-status");
        group.add(&status);
        let jobs = gtk::Box::new(gtk::Orientation::Vertical, 8);
        group.add(&jobs);
        content.append(&group);
        let parent_add = parent.clone();
        let service_add = service.clone();
        add.connect_clicked(move |_| show_connection(&parent_add, service_add.clone()));
        let service_sync = service.clone();
        let status_sync = status.clone();
        sync.connect_clicked(move |_| {
            let service = service_sync.clone();
            let status = status_sync.clone();
            glib::spawn_future_local(async move {
                let result =
                    tokio::spawn(
                        async move { service.request_all_sync().map_err(|e| e.to_string()) },
                    )
                    .await;
                if let Ok(Err(error)) = result {
                    status.set_label(&error);
                }
            });
        });
        let changing = Rc::new(Cell::new(false));
        let parent_enabled = parent.clone();
        let service_enabled = service.clone();
        let status_enabled = status.clone();
        enabled.connect_active_notify(move |switch| {
            if changing.get() {
                return;
            }
            let value = switch.is_active();
            if !value {
                // The switch is a draft until the destructive history reset is confirmed.
                changing.set(true);
                switch.set_active(true);
                changing.set(false);
                let dialog = adw::AlertDialog::builder()
                    .heading(tr("native.disable_title"))
                    .body(tr("native.disable_body"))
                    .build();
                dialog.add_response("cancel", &tr("button.cancel"));
                dialog.add_response("disable", &tr("native.disable"));
                dialog.set_response_appearance("disable", adw::ResponseAppearance::Destructive);
                dialog.set_close_response("cancel");
                let switch = switch.clone();
                let changing = changing.clone();
                let service = service_enabled.clone();
                let status = status_enabled.clone();
                dialog.connect_response(Some("disable"), move |_, _| {
                    changing.set(true);
                    switch.set_active(false);
                    changing.set(false);
                    set_enabled(service.clone(), false, &status);
                });
                dialog.present(&parent_enabled);
            } else {
                set_enabled(service_enabled.clone(), true, &status_enabled);
            }
        });
        let weak_group = group.downgrade();
        let parent = parent.downgrade();
        let mut signature = String::new();
        glib::timeout_add_local(Duration::from_millis(500), move || {
            if weak_group.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            let Some(parent) = parent.upgrade() else {
                return glib::ControlFlow::Break;
            };
            // Small local metadata reads only; all network and secret I/O runs on Tokio.
            match (service.store().list_jobs(), service.store().overview()) {
                (Ok(list), Ok(overview)) => {
                    status.set_label(&trf(
                        "native.status",
                        &[
                            ("done", &overview.synced_items.to_string()),
                            ("total", &overview.total_items.to_string()),
                            ("conflicts", &overview.conflict_images.to_string()),
                        ],
                    ));
                    sync.set_sensitive(prefs::sync_enabled() && !list.is_empty());
                    let next = format!("{list:?}{overview:?}");
                    if signature != next {
                        signature = next;
                        while let Some(child) = jobs.first_child() {
                            jobs.remove(&child);
                        }
                        for job in list {
                            append_job(&jobs, &parent, &service, job);
                        }
                    }
                }
                (Err(error), _) | (_, Err(error)) => status.set_label(&error.to_string()),
            }
            glib::ControlFlow::Continue
        });
    }
}

fn set_enabled(service: SyncService, enabled: bool, status: &gtk::Label) {
    if let Err(error) = prefs::set_sync_enabled(enabled) {
        status.set_label(&error);
        return;
    }
    let status = status.clone();
    glib::spawn_future_local(async move {
        match tokio::spawn(async move { service.sync_enabled_changed().map_err(|e| e.to_string()) })
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => status.set_label(&error),
            Err(_) => status.set_label(&tr("native.worker_failed")),
        }
    });
}

fn append_job(
    container: &gtk::Box,
    parent: &gtk::Widget,
    service: &SyncService,
    job: crate::core::sync::store::SyncJob,
) {
    let group = adw::PreferencesGroup::new();
    let row = adw::ActionRow::builder()
        .title(job.local_root.to_string_lossy().as_ref())
        .subtitle(format!(
            "{} · {}",
            job.endpoint,
            job.native_zone
                .as_ref()
                .map(|z| z.zone_name.as_str())
                .unwrap_or("")
        ))
        .build();
    row.set_widget_name(&format!("native-sync-job-{}", job.id));
    group.add(&row);
    let state = label();
    state.set_label(&job.last_error.clone().unwrap_or_else(|| {
        tr(if job.paused {
            "native.paused"
        } else {
            "native.automatic"
        })
    }));
    group.add(&state);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let scope = button("native.albums", &format!("native-sync-scope-{}", job.id));
    let manage = button("native.manage", &format!("native-sync-manage-{}", job.id));
    let pause = button(
        if job.paused {
            "native.resume"
        } else {
            "native.pause"
        },
        &format!("native-sync-pause-{}", job.id),
    );
    let remove = button("native.remove", &format!("native-sync-remove-{}", job.id));
    for b in [&scope, &manage, &pause, &remove] {
        actions.append(b);
    }
    group.add(&actions);
    container.append(&group);
    let id = job.id;
    let parent_scope = parent.clone();
    let service_scope = service.clone();
    let root = job.local_root.clone();
    scope.connect_clicked(move |_| {
        let selected = service_scope.store().upload_albums(id).unwrap_or_default();
        let service = service_scope.clone();
        show_albums(
            &parent_scope,
            &service_scope,
            &root,
            selected,
            move |albums| {
                let service = service.clone();
                tokio::spawn(async move {
                    if let Err(e) = service.accept_upload_albums(id, &albums) {
                        tracing::warn!("scope edit failed: {e}");
                    }
                });
            },
        );
    });
    let parent_manage = parent.clone();
    let service_manage = service.clone();
    manage
        .connect_clicked(move |_| show_managed_photos(&parent_manage, service_manage.clone(), id));
    let service_pause = service.clone();
    pause.connect_clicked(move |_| {
        let service = service_pause.clone();
        tokio::spawn(async move {
            if let Err(e) = service.store().set_job_paused(id, !job.paused) {
                tracing::warn!("pause failed: {e}");
                return;
            }
            if job.paused {
                let _ = service.request_job_sync(id);
            }
        });
    });
    let parent_remove = parent.clone();
    let service_remove = service.clone();
    remove.connect_clicked(move |_| {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("native.remove"))
            .body(tr("native.remove_body"))
            .build();
        dialog.add_response("cancel", &tr("button.cancel"));
        dialog.add_response("remove", &tr("native.remove"));
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let service = service_remove.clone();
        let state = state.clone();
        dialog.connect_response(Some("remove"), move |_, _| {
            let service = service.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                match tokio::spawn(async move {
                    service.delete_job(id).await.map_err(|e| e.to_string())
                })
                .await
                {
                    Ok(Ok(true)) => {}
                    Ok(Ok(false)) => state.set_label(&tr("native.credential_cleanup")),
                    Ok(Err(e)) => state.set_label(&e),
                    Err(_) => state.set_label(&tr("native.worker_failed")),
                }
            });
        });
        dialog.present(&parent_remove);
    });
}

fn show_connection(parent: &gtk::Widget, service: SyncService) {
    let content = column();
    let endpoint = row("native.endpoint", "native-sync-endpoint", "https://");
    let key = adw::PasswordEntryRow::builder()
        .title(tr("native.api_key"))
        .build();
    key.set_widget_name("native-sync-key");
    let root = row(
        "native.root",
        "native-sync-root",
        &config::pictures_dir().to_string_lossy(),
    );
    let choose = button("native.choose_folder", "native-sync-folder");
    let probe = button("native.probe", "native-sync-probe");
    let zone = adw::ComboRow::builder().title(tr("native.zone")).build();
    zone.set_widget_name("native-sync-zone");
    zone.set_visible(false);
    let scope = button("native.choose_albums", "native-sync-albums");
    let selected = Rc::new(RefCell::new(Vec::<String>::new()));
    let scope_text = label();
    scope_text.set_label(&tr("native.none_selected"));
    let status = label();
    status.set_widget_name("native-sync-probe-status");
    let save = button("native.save", "native-sync-save");
    save.set_sensitive(false);
    let cancel = button("button.cancel", "native-sync-cancel");
    for w in [
        endpoint.upcast_ref::<gtk::Widget>(),
        key.upcast_ref(),
        root.upcast_ref(),
        choose.upcast_ref(),
        probe.upcast_ref(),
        zone.upcast_ref(),
        scope.upcast_ref(),
        scope_text.upcast_ref(),
        status.upcast_ref(),
        save.upcast_ref(),
        cancel.upcast_ref(),
    ] {
        content.append(w);
    }
    let dialog = surface("native.add", &content);
    dialog.set_widget_name("native-sync-connection-dialog");
    let draft = Rc::new(RefCell::new(None::<(String, String, Vec<String>)>));
    for field in [
        endpoint.upcast_ref::<gtk::Editable>(),
        key.upcast_ref::<gtk::Editable>(),
    ] {
        let draft = draft.clone();
        let save = save.clone();
        let zone = zone.clone();
        field.connect_changed(move |_| {
            draft.borrow_mut().take();
            save.set_sensitive(false);
            zone.set_visible(false);
        });
    }
    let root_choose = root.clone();
    let parent_choose = parent.clone();
    choose.connect_clicked(move |_| {
        let chooser = gtk::FileChooserNative::builder()
            .title(tr("native.root"))
            .action(gtk::FileChooserAction::SelectFolder)
            .build();
        if let Some(window) = parent_choose.root().and_downcast::<gtk::Window>() {
            chooser.set_transient_for(Some(&window));
        }
        let root = root_choose.clone();
        chooser.connect_response(move |chooser, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(path) = chooser.file().and_then(|f| f.path()) {
                    root.set_text(&path.to_string_lossy());
                }
            }
            chooser.destroy();
        });
        chooser.show();
    });
    let selected_root = selected.clone();
    let text_root = scope_text.clone();
    root.connect_changed(move |_| {
        selected_root.borrow_mut().clear();
        text_root.set_label(&tr("native.none_selected"));
    });
    let root_scope = root.clone();
    let service_scope = service.clone();
    let parent_scope = content.downgrade();
    let selected_scope = selected.clone();
    let text_scope = scope_text.clone();
    scope.connect_clicked(move |_| {
        let Some(parent_scope) = parent_scope.upgrade() else {
            return;
        };
        let selected = selected_scope.clone();
        let text = text_scope.clone();
        show_albums(
            parent_scope.upcast_ref(),
            &service_scope,
            &PathBuf::from(root_scope.text().as_str()),
            selected_scope.borrow().clone(),
            move |albums| {
                text.set_label(&if albums.is_empty() {
                    tr("native.none_selected")
                } else {
                    albums.join(", ")
                });
                *selected.borrow_mut() = albums;
            },
        );
    });
    let service_probe = service.clone();
    let endpoint_probe = endpoint.clone();
    let key_probe = key.clone();
    let draft_probe = draft.clone();
    let status_probe = status.clone();
    let save_probe = save.clone();
    let zone_probe = zone.clone();
    probe.connect_clicked(move |probe| {
        let address = endpoint_probe.text().trim().to_string();
        let secret = key_probe.text().to_string();
        let service = service_probe.clone();
        let draft = draft_probe.clone();
        let status = status_probe.clone();
        let save = save_probe.clone();
        let zone = zone_probe.clone();
        let probe = probe.clone();
        let endpoint = endpoint_probe.clone();
        let key = key_probe.clone();
        probe.set_sensitive(false);
        status.set_label(&tr("native.probing"));
        glib::spawn_future_local(async move {
            let request_address = address.clone();
            let request_secret = secret.clone();
            let result = tokio::spawn(async move {
                service
                    .probe_native_connection(&request_address, &request_secret)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            probe.set_sensitive(true);
            if endpoint.text().trim() != address || key.text().as_str() != secret {
                return;
            }
            match result {
                Ok(Ok(pinned)) => {
                    let zones: Vec<String> = pinned.zones.clone();
                    let names: Vec<&str> = zones.iter().map(String::as_str).collect();
                    zone.set_model(Some(&gtk::StringList::new(&names)));
                    zone.set_selected(0);
                    zone.set_visible(true);
                    save.set_sensitive(!zones.is_empty());
                    status.set_label(&tr("native.probe_ok"));
                    *draft.borrow_mut() = Some((address, secret, zones));
                }
                Ok(Err(e)) => status.set_label(&e),
                Err(_) => status.set_label(&tr("native.worker_failed")),
            }
        });
    });
    let weak_dialog = dialog.downgrade();
    save.connect_clicked(move |save| {
        let Some((endpoint, secret, zones)) = draft.borrow().clone() else {
            return;
        };
        let Some(zone) = zones.get(zone.selected() as usize).cloned() else {
            return;
        };
        let local_root = PathBuf::from(root.text().trim());
        let albums = selected.borrow().clone();
        let service = service.clone();
        let status = status.clone();
        let save = save.clone();
        let weak_dialog = weak_dialog.clone();
        save.set_sensitive(false);
        status.set_label(&tr("native.saving"));
        glib::spawn_future_local(async move {
            let result = tokio::spawn(async move {
                let reference = format!("native-{}", uuid::Uuid::new_v4());
                let job = service
                    .create_native_job_scoped(
                        &endpoint,
                        &reference,
                        &secret,
                        &local_root,
                        &zone,
                        SyncDirection::Bidirectional,
                        UploadScope::SelectedAlbums,
                        albums,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                service
                    .request_job_sync(job.id)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    if let Some(dialog) = weak_dialog.upgrade() {
                        dialog.close();
                    }
                }
                Ok(Err(e)) => {
                    status.set_label(&e);
                    save.set_sensitive(true);
                }
                Err(_) => {
                    status.set_label(&tr("native.worker_failed"));
                    save.set_sensitive(true);
                }
            }
        });
    });
    let weak_dialog = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(dialog) = weak_dialog.upgrade() {
            dialog.close();
        }
    });
    dialog.present(parent);
}

fn show_albums(
    parent: &gtk::Widget,
    service: &SyncService,
    root: &std::path::Path,
    selected: Vec<String>,
    accept: impl Fn(Vec<String>) + 'static,
) {
    let content = column();
    let hint = label();
    hint.set_label(&tr("native.scope_hint"));
    content.append(&hint);
    let status = label();
    content.append(&status);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&list);
    let save = button("native.save", "native-sync-albums-save");
    save.set_sensitive(false);
    content.append(&save);
    let cancel = button("button.cancel", "native-sync-albums-cancel");
    content.append(&cancel);
    let dialog = surface("native.albums", &content);
    let checks = Rc::new(RefCell::new(Vec::<(String, gtk::CheckButton)>::new()));
    let pool = service.pool().clone();
    let root = root.to_path_buf();
    let checks_load = checks.clone();
    let save_load = save.clone();
    glib::spawn_future_local(async move {
        let result = tokio::task::spawn_blocking(move || {
            let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
            let conn = pool.get().map_err(|e| e.to_string())?;
            let mut statement = conn
                .prepare(
                    "SELECT DISTINCT path FROM media_items WHERE trashed_at IS NULL ORDER BY path",
                )
                .map_err(|e| e.to_string())?;
            let paths = statement
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            let mut albums = std::collections::BTreeSet::from_iter(selected.iter().cloned());
            for path in paths {
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    if let Ok(relative) = parent.strip_prefix(&root) {
                        albums.insert(relative.to_string_lossy().to_string());
                    }
                }
            }
            for entry in walkdir::WalkDir::new(&root).follow_links(false) {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().is_file()
                    && crate::core::media::is_supported_media_path(entry.path())
                {
                    if let Some(parent) = entry.path().parent() {
                        if let Ok(relative) = parent.strip_prefix(&root) {
                            albums.insert(relative.to_string_lossy().to_string());
                        }
                    }
                }
            }
            Ok::<_, String>((albums, selected))
        })
        .await;
        match result {
            Ok(Ok((albums, selected))) => {
                for album in albums {
                    let check = gtk::CheckButton::with_label(&if album.is_empty() {
                        tr("native.root_album")
                    } else {
                        album.clone()
                    });
                    check.set_active(selected.contains(&album));
                    check.set_widget_name(&format!("native-sync-album:{album}"));
                    list.append(&check);
                    checks_load.borrow_mut().push((album, check));
                }
                save_load.set_sensitive(true);
            }
            Ok(Err(e)) => status.set_label(&e),
            Err(_) => status.set_label(&tr("native.worker_failed")),
        }
    });
    let weak = dialog.downgrade();
    save.connect_clicked(move |_| {
        let selected = checks
            .borrow()
            .iter()
            .filter(|(_, c)| c.is_active())
            .map(|(a, _)| a.clone())
            .collect();
        accept(selected);
        if let Some(d) = weak.upgrade() {
            d.close();
        }
    });
    let weak = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(d) = weak.upgrade() {
            d.close();
        }
    });
    dialog.present(parent);
}

fn show_managed_photos(parent: &gtk::Widget, service: SyncService, job: i64) {
    super::native_actions::show(parent, service, job);
}
