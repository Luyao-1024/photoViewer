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
        let enabled = gtk::Switch::builder()
            .active(prefs::sync_enabled())
            .valign(gtk::Align::Center)
            .build();
        enabled.set_widget_name("native-sync-enabled");
        enabled.update_property(&[gtk::accessible::Property::Label(&tr("native.enabled"))]);
        let entry = adw::ActionRow::builder()
            .title(tr("native.settings"))
            .activatable(true)
            .build();
        entry.set_widget_name("native-sync-entry");
        entry.add_prefix(&decoration("folder-remote-symbolic"));
        entry.add_suffix(&enabled);
        entry.add_suffix(&decoration("go-next-symbolic"));
        group.add(&entry);
        // Only failures add a message to the otherwise single-row entry.
        let status = label();
        status.add_css_class("error");
        status.set_visible(false);
        status.connect_label_notify(|label| label.set_visible(!label.label().is_empty()));
        group.add(&status);
        content.append(&group);
        let weak_window = self.downgrade();
        let parent_open = parent.clone();
        let service_open = service.clone();
        entry.connect_activated(move |entry| {
            let Some(window) = weak_window.upgrade() else {
                return;
            };
            let Some(navigation) = entry
                .ancestor(adw::NavigationView::static_type())
                .and_downcast::<adw::NavigationView>()
            else {
                return;
            };
            let page =
                window.build_native_sync_page(&parent_open, &navigation, service_open.clone());
            navigation.push(&page);
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
    }

    fn build_native_sync_page(
        &self,
        parent: &gtk::Widget,
        navigation: &adw::NavigationView,
        service: SyncService,
    ) -> adw::NavigationPage {
        let content = column();
        content.add_css_class("settings-dialog-content");
        content.set_spacing(20);
        let group = adw::PreferencesGroup::new();
        group.set_widget_name("native-sync-settings");
        group.set_title(&tr("native.overview"));
        group.set_description(Some(&tr("native.automatic")));
        let overview_row = adw::ActionRow::new();
        overview_row.set_title_lines(1);
        overview_row.set_subtitle_lines(2);
        overview_row.set_widget_name("native-sync-overview");
        overview_row.add_prefix(&decoration("emblem-synchronizing-symbolic"));
        let sync = button("native.sync_now", "native-sync-now");
        sync.set_sensitive(false);
        overview_row.add_suffix(&sync);
        group.add(&overview_row);
        group.set_visible(false);
        content.append(&group);
        let status = label();
        status.set_widget_name("native-sync-status");
        status.add_css_class("error");
        status.set_visible(false);
        status.connect_label_notify(|label| label.set_visible(!label.label().is_empty()));
        content.append(&status);
        let tasks = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = gtk::Label::new(Some(&tr("native.tasks")));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        heading.append(&title);
        let add = button("native.add", "native-sync-add");
        heading.append(&add);
        tasks.append(&heading);
        let empty_group = adw::PreferencesGroup::new();
        let empty = adw::ActionRow::builder()
            .title(tr("native.empty"))
            .subtitle(tr("native.empty_hint"))
            .build();
        empty.set_widget_name("native-sync-empty");
        empty_group.add(&empty);
        tasks.append(&empty_group);
        content.append(&tasks);
        let jobs = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.append(&jobs);
        let hint = label();
        hint.set_label(&tr("native.scope_hint"));
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        content.append(&hint);
        let parent_add = parent.clone();
        let service_add = service.clone();
        add.connect_clicked(move |_| show_connection(&parent_add, service_add.clone()));
        let service_sync = service.clone();
        let status_sync = status.clone();
        sync.connect_clicked(move |_| {
            let service = service_sync.clone();
            let status = status_sync.clone();
            status.set_label("");
            glib::spawn_future_local(async move {
                match tokio::spawn(
                    async move { service.request_all_sync().map_err(|e| e.to_string()) },
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => status.set_label(&error),
                    Err(_) => status.set_label(&tr("native.worker_failed")),
                }
            });
        });
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&content)
            .build();
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::builder().show_back_button(false).build();
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_widget_name("native-sync-back");
        back.set_tooltip_text(Some(&tr("native.back")));
        back.add_css_class("glass-toolbar-button");
        let weak_navigation = navigation.downgrade();
        back.connect_clicked(move |_| {
            if let Some(navigation) = weak_navigation.upgrade() {
                navigation.pop();
            }
        });
        header.pack_start(&back);
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroller));
        let page = adw::NavigationPage::new(&toolbar, &tr("native.settings"));
        page.set_widget_name("native-sync-page");
        let weak_page = page.downgrade();
        let parent = parent.downgrade();
        let mut cards = std::collections::BTreeMap::<i64, JobCard>::new();
        // Update existing widgets so background progress never collapses details,
        // closes an open action menu, or discards keyboard focus.
        let refresh = move || {
            let Some(page) = weak_page.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let Some(parent) = parent.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !page.is_mapped() {
                return glib::ControlFlow::Continue;
            }
            match (service.store().list_jobs(), service.store().overview()) {
                (Ok(list), Ok(overview)) => {
                    let enabled = prefs::sync_enabled();
                    overview_row.set_title(&trf(
                        "native.progress",
                        &[
                            ("done", &overview.synced_items.to_string()),
                            ("total", &overview.total_items.to_string()),
                        ],
                    ));
                    let summary = if overview.conflict_images > 0 {
                        trf(
                            "native.conflicts",
                            &[("count", &overview.conflict_images.to_string())],
                        )
                    } else {
                        tr(if enabled {
                            "native.up_to_date_hint"
                        } else {
                            "native.disabled"
                        })
                    };
                    overview_row.set_subtitle(&summary);
                    if overview.conflict_images > 0 {
                        overview_row.add_css_class("warning");
                    } else {
                        overview_row.remove_css_class("warning");
                    }
                    group.set_visible(!list.is_empty());
                    empty_group.set_visible(list.is_empty());
                    sync.set_sensitive(enabled && list.iter().any(|job| !job.paused));
                    empty.set_visible(list.is_empty());
                    cards.retain(|id, card| {
                        let keep = list.iter().any(|job| job.id == *id);
                        if !keep {
                            jobs.remove(&card.group);
                        }
                        keep
                    });
                    for job in list {
                        let card = cards
                            .entry(job.id)
                            .or_insert_with(|| append_job(&jobs, &parent, &service, &job, &status));
                        card.refresh(&service, &job);
                    }
                }
                (Err(error), _) | (_, Err(error)) => status.set_label(&error.to_string()),
            }
            glib::ControlFlow::Continue
        };
        glib::timeout_add_local(Duration::from_millis(500), refresh);
        page
    }
}

fn decoration(icon: &str) -> gtk::Image {
    let image = gtk::Image::from_icon_name(icon);
    image.set_accessible_role(gtk::AccessibleRole::Presentation);
    image
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

struct JobCard {
    group: adw::PreferencesGroup,
    row: adw::ExpanderRow,
    state: gtk::Label,
    pause: gtk::Button,
    scope: adw::ActionRow,
}
impl JobCard {
    fn refresh(&self, service: &SyncService, job: &crate::core::sync::store::SyncJob) {
        let state = tr(if !prefs::sync_enabled() {
            "native.disabled"
        } else if job.paused {
            "native.paused"
        } else if job.last_error.is_some() {
            "native.needs_attention"
        } else {
            "native.ready"
        });
        self.row.set_subtitle(&state);
        self.state
            .set_label(job.last_error.as_deref().unwrap_or(""));
        self.state.set_visible(job.last_error.is_some());
        self.pause.set_label(&tr(if job.paused {
            "native.resume"
        } else {
            "native.pause"
        }));
        let albums = service.store().upload_albums(job.id).unwrap_or_default();
        self.scope
            .set_subtitle(&if job.upload_scope == UploadScope::All {
                tr("native.all_albums")
            } else if albums.is_empty() {
                tr("native.receive_only")
            } else {
                trf(
                    "native.album_count",
                    &[("count", &albums.len().to_string())],
                )
            });
    }
}

fn detail(key: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(tr(key))
        .subtitle(value)
        .build();
    row.set_subtitle_selectable(true);
    row.set_subtitle_lines(2);
    row.set_use_markup(false);
    row
}

fn menu_action(key: &str, name: &str, menu: &gtk::Popover) -> gtk::Button {
    let action = gtk::Button::with_label(&tr(key));
    action.set_widget_name(name);
    action.set_halign(gtk::Align::Fill);
    action.add_css_class("flat");
    let menu = menu.downgrade();
    action.connect_clicked(move |_| {
        if let Some(menu) = menu.upgrade() {
            menu.popdown();
        }
    });
    action
}

fn append_job(
    container: &gtk::Box,
    parent: &gtk::Widget,
    service: &SyncService,
    job: &crate::core::sync::store::SyncJob,
    status: &gtk::Label,
) -> JobCard {
    let group = adw::PreferencesGroup::new();
    let title = job
        .local_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| job.local_root.to_string_lossy().into_owned());
    let row = adw::ExpanderRow::builder().title(&title).build();
    row.set_use_markup(false);
    row.set_title_lines(1);
    row.set_subtitle_lines(2);
    row.set_widget_name(&format!("native-sync-job-{}", job.id));
    row.add_prefix(&decoration("folder-pictures-symbolic"));
    let menu = gtk::Popover::new();
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    menu.set_child(Some(&actions));
    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(gtk::Align::Center)
        .popover(&menu)
        .build();
    more.add_css_class("glass-toolbar-button");
    more.set_widget_name(&format!("native-sync-menu-{}", job.id));
    more.set_tooltip_text(Some(&tr("native.task_actions")));
    row.add_suffix(&more);
    let state = label();
    state.add_css_class("error");
    state.set_margin_start(16);
    state.set_margin_end(16);
    state.set_margin_top(8);
    state.set_margin_bottom(8);
    row.add_row(&state);
    row.add_row(&detail("native.root", &job.local_root.to_string_lossy()));
    row.add_row(&detail("native.endpoint", &job.endpoint));
    let account = service
        .store()
        .native_scope(job.connection_id)
        .ok()
        .flatten()
        .and_then(|s| s.authentication)
        .and_then(|a| a.user)
        .map(|u| u.username);
    if let Some(account) = &account {
        row.add_row(&detail("native.username", account));
    }
    if let Some(zone) = &job.native_zone {
        row.add_row(&detail("native.zone", &zone.zone_name));
    }
    let scope_row = detail("native.albums", "");
    let scope = button("native.choose", &format!("native-sync-scope-{}", job.id));
    scope_row.add_suffix(&scope);
    scope_row.set_activatable_widget(Some(&scope));
    row.add_row(&scope_row);
    let manage_row = detail("native.manage", &tr("native.manage_summary"));
    let manage = gtk::Button::from_icon_name("go-next-symbolic");
    manage.add_css_class("glass-toolbar-button");
    manage.set_widget_name(&format!("native-sync-manage-{}", job.id));
    manage.set_tooltip_text(Some(&tr("native.manage")));
    manage.set_valign(gtk::Align::Center);
    manage_row.add_suffix(&manage);
    manage_row.set_activatable_widget(Some(&manage));
    row.add_row(&manage_row);
    group.add(&row);
    let pause = menu_action(
        if job.paused {
            "native.resume"
        } else {
            "native.pause"
        },
        &format!("native-sync-pause-{}", job.id),
        &menu,
    );
    actions.append(&pause);
    actions.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let login = menu_action(
        "native.relogin",
        &format!("native-sync-relogin-{}", job.id),
        &menu,
    );
    actions.append(&login);
    let parent_login = parent.clone();
    let service_login = service.clone();
    let job_login = job.clone();
    let account_login = account.clone();
    login.connect_clicked(move |_| {
        show_relogin(
            &parent_login,
            service_login.clone(),
            job_login.id,
            account_login.clone().unwrap_or_default(),
        )
    });
    if account.is_some() {
        let logout = menu_action(
            "native.logout",
            &format!("native-sync-logout-{}", job.id),
            &menu,
        );
        actions.append(&logout);
        let service_logout = service.clone();
        let id = job.id;
        let state_logout = status.clone();
        logout.connect_clicked(move |_| {
            let service = service_logout.clone();
            let state = state_logout.clone();
            glib::spawn_future_local(async move {
                match tokio::spawn(async move {
                    service
                        .logout_native_job(id)
                        .await
                        .map_err(|e| e.to_string())
                })
                .await
                {
                    Ok(Ok(())) => state.set_label(""),
                    Ok(Err(e)) => state.set_label(&e),
                    Err(_) => state.set_label(&tr("native.worker_failed")),
                }
            });
        });
    }
    actions.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let remove = menu_action(
        "native.remove",
        &format!("native-sync-remove-{}", job.id),
        &menu,
    );
    remove.add_css_class("destructive-action");
    actions.append(&remove);
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
            let Ok(Some(job)) = service.store().get_job(id) else {
                return;
            };
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
    let state_remove = status.clone();
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
        let state = state_remove.clone();
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
    JobCard {
        group,
        row,
        state,
        pause,
        scope: scope_row,
    }
}

fn show_connection(parent: &gtk::Widget, service: SyncService) {
    let content = column();
    let endpoint = row("native.endpoint", "native-sync-endpoint", "https://");
    let key = adw::PasswordEntryRow::builder()
        .title(tr("native.password"))
        .build();
    key.set_widget_name("native-sync-password");
    let username = row("native.username", "native-sync-username", "");
    let root = row(
        "native.root",
        "native-sync-root",
        &config::pictures_dir().to_string_lossy(),
    );
    let choose = button("native.choose_folder", "native-sync-folder");
    let probe = button("native.login", "native-sync-probe");
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
        username.upcast_ref(),
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
    let revision = Rc::new(Cell::new(0u64));
    for field in [
        endpoint.upcast_ref::<gtk::Editable>(),
        username.upcast_ref::<gtk::Editable>(),
        key.upcast_ref::<gtk::Editable>(),
    ] {
        let draft = draft.clone();
        let save = save.clone();
        let zone = zone.clone();
        let revision = revision.clone();
        field.connect_changed(move |_| {
            revision.set(revision.get().wrapping_add(1));
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
    let username_probe = username.clone();
    let key_probe = key.clone();
    let draft_probe = draft.clone();
    let status_probe = status.clone();
    let save_probe = save.clone();
    let zone_probe = zone.clone();
    probe.connect_clicked(move |probe| {
        let address = endpoint_probe.text().trim().to_string();
        let username = username_probe.text().to_string();
        let password = key_probe.text().to_string();
        let service = service_probe.clone();
        let draft = draft_probe.clone();
        let status = status_probe.clone();
        let save = save_probe.clone();
        let zone = zone_probe.clone();
        let probe = probe.clone();
        let revision = revision.clone();
        let captured = revision.get();
        probe.set_sensitive(false);
        status.set_label(&tr("native.logging_in"));
        glib::spawn_future_local(async move {
            let endpoint = address.clone();
            let result = tokio::spawn(async move {
                service
                    .login_native_connection(&endpoint, &username, &password)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            probe.set_sensitive(true);
            if revision.get() != captured {
                return;
            }
            match result {
                Ok(Ok((session, pinned))) => {
                    let zones = pinned.zones.clone();
                    let names = zones.iter().map(String::as_str).collect::<Vec<_>>();
                    zone.set_model(Some(&gtk::StringList::new(&names)));
                    zone.set_selected(0);
                    zone.set_visible(true);
                    save.set_sensitive(!zones.is_empty());
                    status.set_label(&trf(
                        "native.login_ok",
                        &[("username", &session.user.username)],
                    ));
                    *draft.borrow_mut() = Some((address, session.access_token, zones));
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

fn show_relogin(parent: &gtk::Widget, service: SyncService, job: i64, account: String) {
    let content = column();
    let hint = label();
    hint.set_label(&tr("native.relogin_hint"));
    content.append(&hint);
    let username = row("native.username", "native-sync-relogin-username", &account);
    content.append(&username);
    let password = adw::PasswordEntryRow::builder()
        .title(tr("native.password"))
        .build();
    password.set_widget_name("native-sync-relogin-password");
    content.append(&password);
    let status = label();
    status.set_widget_name("native-sync-relogin-status");
    content.append(&status);
    let login = button("native.relogin", "native-sync-relogin-submit");
    content.append(&login);
    let cancel = button("button.cancel", "native-sync-relogin-cancel");
    content.append(&cancel);
    let dialog = surface("native.relogin", &content);
    let weak = dialog.downgrade();
    login.connect_clicked(move |button| {
        let service = service.clone();
        let username = username.text().to_string();
        let password = password.text().to_string();
        let status = status.clone();
        let button = button.clone();
        let weak = weak.clone();
        button.set_sensitive(false);
        status.set_label(&tr("native.logging_in"));
        glib::spawn_future_local(async move {
            match tokio::spawn(async move {
                service
                    .reauthenticate_native_job(job, &username, &password)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            {
                Ok(Ok(())) => {
                    if let Some(dialog) = weak.upgrade() {
                        dialog.close();
                    }
                }
                Ok(Err(e)) => {
                    status.set_label(&e);
                    button.set_sensitive(true);
                }
                Err(_) => {
                    status.set_label(&tr("native.worker_failed"));
                    button.set_sensitive(true);
                }
            }
        });
    });
    let weak = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    dialog.present(parent);
}
