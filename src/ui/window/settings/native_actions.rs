use super::*;
use crate::core::sync::{store::PhotoAction, SyncService};

pub(super) fn show(parent: &gtk::Widget, service: SyncService, job: i64) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .build();
    let hint = gtk::Label::new(Some(&tr("native.manage_hint")));
    hint.set_wrap(true);
    content.append(&hint);
    let status = gtk::Label::new(None);
    status.set_wrap(true);
    status.set_selectable(true);
    status.set_widget_name("native-sync-action-status");
    content.append(&status);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();
    let dialog = adw::Dialog::builder()
        .title(tr("native.manage"))
        .content_width(540)
        .content_height(600)
        .child(&scroller)
        .build();
    match service.store().managed_photos(job) {
        Err(e) => status.set_label(&e.to_string()),
        Ok(photos) => {
            for photo in photos {
                let group = adw::PreferencesGroup::new();
                let row = adw::ActionRow::builder()
                    .title(&photo.relative_path)
                    .build();
                row.set_widget_name(&format!("native-sync-photo-{}", photo.entry_id));
                group.add(&row);
                if let Some(detail) = &photo.conflict {
                    let reason = serde_json::from_str::<serde_json::Value>(detail)
                        .ok()
                        .and_then(|v| v["reason"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    let text = gtk::Label::new(Some(&tr(match reason.as_str() {
                        "remote hidden photo has no supported local representation" => {
                            "native.hidden_conflict"
                        }
                        "remote photo was deleted while local changes remain" => {
                            "native.deleted_conflict"
                        }
                        _ => "native.version_conflict",
                    })));
                    text.set_wrap(true);
                    group.add(&text);
                }
                let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let choices: Vec<(&str, PhotoAction)> = if photo.conflict.is_some() {
                    vec![
                        ("native.keep_local", PhotoAction::KeepLocal),
                        ("native.use_cloud", PhotoAction::UseCloud),
                    ]
                } else if photo.remote {
                    vec![("native.delete_cloud", PhotoAction::DeleteCloud)]
                } else if photo.retired {
                    vec![("native.recreate", PhotoAction::Recreate)]
                } else {
                    vec![]
                };
                for (key, action) in choices {
                    let button = gtk::Button::with_label(&tr(key));
                    button.add_css_class("glass-toolbar-button");
                    button.set_widget_name(&format!(
                        "native-sync-photo-action-{}-{action:?}",
                        photo.entry_id
                    ));
                    actions.append(&button);
                    let captured = photo.clone();
                    let service = service.clone();
                    let host = content.downgrade();
                    let status = status.clone();
                    button.connect_clicked(move |button| {
                        let Some(host) = host.upgrade() else { return };
                        let confirmation = adw::AlertDialog::builder()
                            .heading(tr(key))
                            .body(tr(match action {
                                PhotoAction::DeleteCloud => "native.delete_cloud_body",
                                PhotoAction::UseCloud => "native.use_cloud_body",
                                PhotoAction::KeepLocal => "native.keep_local_body",
                                PhotoAction::Recreate => "native.recreate_body",
                            }))
                            .build();
                        confirmation.add_response("cancel", &tr("button.cancel"));
                        confirmation.add_response("accept", &tr(key));
                        confirmation.set_close_response("cancel");
                        if matches!(action, PhotoAction::DeleteCloud) {
                            confirmation.set_response_appearance(
                                "accept",
                                adw::ResponseAppearance::Destructive,
                            );
                        }
                        let service = service.clone();
                        let photo = captured.clone();
                        let status = status.clone();
                        let button = button.clone();
                        confirmation.connect_response(Some("accept"), move |_, _| {
                            button.set_sensitive(false);
                            let service = service.clone();
                            let photo = photo.clone();
                            let status = status.clone();
                            let button = button.clone();
                            glib::spawn_future_local(async move {
                                match tokio::spawn(async move {
                                    service
                                        .accept_photo_action(job, photo, action)
                                        .await
                                        .map_err(|e| e.to_string())
                                })
                                .await
                                {
                                    Ok(Ok(path)) => status.set_label(
                                        &path
                                            .map(|p| {
                                                trf(
                                                    "native.recovery_saved",
                                                    &[("path", &p.to_string_lossy())],
                                                )
                                            })
                                            .unwrap_or_else(|| tr("native.action_accepted")),
                                    ),
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
                        confirmation.present(&host);
                    });
                }
                group.add(&actions);
                content.append(&group);
            }
        }
    }
    dialog.present(parent);
}
