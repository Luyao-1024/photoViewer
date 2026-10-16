use super::*;
use crate::core::{
    diagnostics,
    sync::{
        service::cleanup::{database_usage, DataKind},
        SyncService,
    },
};
use gtk::gio;
impl MainWindow {
    pub(super) fn append_log_settings(&self, group: &adw::PreferencesGroup, parent: &gtk::Widget) {
        let levels = ["off", "error", "warn", "info", "debug", "trace"];
        let labels: Vec<_> = levels
            .iter()
            .map(|level| tr(&format!("setting.logs.level.{level}")))
            .collect();
        let labels: Vec<_> = labels.iter().map(String::as_str).collect();
        let row = adw::ComboRow::builder()
            .title(tr("setting.logs.level"))
            .subtitle(tr("setting.logs.level_description"))
            .model(&gtk::StringList::new(&labels))
            .build();
        row.set_widget_name("settings-log-level");
        row.set_selected(
            levels
                .iter()
                .position(|level| *level == prefs::log_level())
                .unwrap_or(3) as u32,
        );
        let parent = parent.clone();
        row.connect_selected_notify(move |row| {
            if let Err(error) = diagnostics::set_log_level(levels[row.selected() as usize]) {
                show_settings_error_dialog(&parent, &error.to_string());
            }
        });
        group.add(&row);
    }
    pub(super) fn append_cleanup_settings(&self, container: &gtk::Box, parent: &gtk::Widget) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
        content.set_widget_name("settings-cleanup");
        container.append(&content);
        let limit_group = adw::PreferencesGroup::new();
        let limit = adw::SpinRow::with_range(1.0, 500.0, 1.0);
        limit.set_title(&tr("setting.logs.limit"));
        limit.set_subtitle(&tr("setting.logs.limit_description"));
        limit.set_widget_name("settings-log-limit");
        limit.set_value(prefs::log_limit_mb() as f64);
        let parent_limit = parent.clone();
        let pending = std::rc::Rc::new(std::cell::RefCell::new(None::<glib::SourceId>));
        limit.connect_value_notify(move |row| {
            if let Some(source) = pending.borrow_mut().take() {
                source.remove();
            }
            let pending = pending.clone();
            let mb = row.value() as u64;
            let parent = parent_limit.clone();
            let callback_pending = pending.clone();
            let source = glib::timeout_add_local_once(Duration::from_millis(250), move || {
                callback_pending.borrow_mut().take();
                glib::spawn_future_local(async move {
                    match gio::spawn_blocking(move || {
                        diagnostics::set_log_limit_mb(mb).map_err(|e| e.to_string())
                    })
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => show_settings_error_dialog(&parent, &error),
                        Err(error) => show_settings_error_dialog(&parent, &format!("{error:?}")),
                    }
                });
            });
            *pending.borrow_mut() = Some(source);
        });
        limit_group.add(&limit);
        content.append(&limit_group);
        let rows = adw::PreferencesGroup::new();
        rows.set_title(&tr("setting.cleanup.title"));
        rows.set_description(Some(&tr("setting.cleanup.description")));
        content.append(&rows);
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let Some(actor) = self.imp().db_actor.borrow().clone() else {
            return;
        };
        let service = SyncService::with_actor(pool.clone(), actor.clone());
        for (name, key, detail, kind) in [
            (
                "library",
                "setting.cleanup.library",
                "setting.cleanup.library_description",
                Some(DataKind::Library),
            ),
            (
                "logs",
                "setting.cleanup.logs",
                "setting.cleanup.logs_description",
                None,
            ),
            (
                "accounts",
                "setting.cleanup.accounts",
                "setting.cleanup.accounts_description",
                Some(DataKind::Accounts),
            ),
            (
                "all",
                "setting.cleanup.all",
                "setting.cleanup.all_description",
                Some(DataKind::All),
            ),
        ] {
            let row = adw::ActionRow::builder().title(tr(key)).build();
            row.set_widget_name(&format!("settings-cleanup-{name}"));
            let calculate = {
                let pool = pool.clone();
                move || match kind {
                    None => diagnostics::log_size(),
                    Some(DataKind::Library) => {
                        crate::core::cache::dir_size(&config::cache_dir().join("thumbnails"))
                            + database_usage(&pool, false)
                    }
                    Some(DataKind::Accounts) => {
                        database_usage(&pool, true)
                            + crate::core::cache::dir_size(&config::data_dir().join("sync-staging"))
                    }
                    Some(DataKind::All) => {
                        diagnostics::log_size()
                            + crate::core::cache::dir_size(&config::cache_dir().join("thumbnails"))
                            + database_usage(&pool, false)
                            + database_usage(&pool, true)
                            + crate::core::cache::dir_size(&config::data_dir().join("sync-staging"))
                    }
                }
            };
            update_storage_size_async(&row, calculate);
            let button = gtk::Button::with_label(&tr("setting.cleanup.clear"));
            button.set_widget_name(&format!("settings-cleanup-{name}-button"));
            button.add_css_class("glass-toolbar-button");
            button.add_css_class("glass-toolbar-danger");
            button.set_valign(gtk::Align::Center);
            row.add_suffix(&button);
            rows.add(&row);
            // Scope and login effects remain visible beside the measured size.
            let explanation = gtk::Label::new(Some(&tr(detail)));
            explanation.set_wrap(true);
            explanation.set_xalign(0.0);
            explanation.add_css_class("caption");
            explanation.set_margin_bottom(12);
            rows.add(&explanation);
            let parent = parent.clone();
            let service = service.clone();
            let actor = actor.clone();
            let row = row.downgrade();
            button.connect_clicked(move |button| {
                let button = button.clone();
                let service = service.clone();
                let actor = actor.clone();
                let row = row.clone();
                show_clear_confirm_dialog(&parent, &tr(key), &tr(detail), move || {
                    button.set_sensitive(false);
                    let button = button.clone();
                    let service = service.clone();
                    let actor = actor.clone();
                    let row = row.clone();
                    glib::spawn_future_local(async move {
                        let result = if let Some(kind) = kind {
                            tokio::spawn(async move {
                                service
                                    .clear_local_data(actor, kind)
                                    .await
                                    .map_err(|e| e.to_string())
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|v| v)
                        } else {
                            gio::spawn_blocking(|| {
                                diagnostics::clear_logs().map_err(|e| e.to_string())
                            })
                            .await
                            .map_err(|e| format!("{e:?}"))
                            .and_then(|v| v)
                        };
                        button.set_sensitive(true);
                        match result {
                            Err(error) => show_clear_error_toast(&trf(
                                "setting.clear_failed",
                                &[("error", &error)],
                            )),
                            Ok(()) if kind.is_some() => {
                                if let Err(error) = restart_application() {
                                    show_clear_error_toast(&error);
                                }
                            }
                            Ok(()) => {
                                if let Some(row) = row.upgrade() {
                                    update_storage_size_async(&row, diagnostics::log_size);
                                }
                            }
                        }
                    });
                });
            });
        }
    }
}
