use super::*;
use crate::core::sync::{SyncJob, SyncService};
use std::cell::Cell;

pub(super) fn hash_row(
    enabled: bool,
    parent: &gtk::Widget,
    accept: impl Fn(bool) -> crate::core::error::Result<()> + 'static,
) -> (adw::ActionRow, gtk::Switch) {
    let row = adw::ActionRow::builder()
        .title(tr("setting.sync.sha256_etag"))
        .subtitle(tr("setting.sync.sha256_etag_description"))
        .build();
    row.add_css_class("settings-action-row");
    let switch = gtk::Switch::builder()
        .active(enabled)
        .valign(gtk::Align::Center)
        .build();
    switch.set_widget_name("sync-sha256-etag");
    switch.update_property(&[gtk::accessible::Property::Label(&tr(
        "setting.sync.sha256_etag",
    ))]);
    row.add_suffix(&switch);
    row.set_activatable_widget(Some(&switch));
    let saved = Cell::new(enabled);
    let reverting = Cell::new(false);
    let parent = parent.clone();
    switch.connect_active_notify(move |switch| {
        if reverting.get() {
            return;
        }
        let enabled = switch.is_active();
        if let Err(error) = accept(enabled) {
            reverting.set(true);
            switch.set_active(saved.get());
            reverting.set(false);
            show_settings_error_dialog(
                &parent,
                &trf(
                    "setting.sync.changes_apply_failed",
                    &[("error", &error.to_string())],
                ),
            );
        } else {
            saved.set(enabled);
        }
    });
    (row, switch)
}

pub(super) fn saved_hash_row(
    job: &SyncJob,
    service: SyncService,
    parent: &gtk::Widget,
) -> adw::ActionRow {
    let id = job.id;
    hash_row(job.etag_is_sha256, parent, move |enabled| {
        service.accept_etag_is_sha256(id, enabled).map(|_| ())
    })
    .0
}
