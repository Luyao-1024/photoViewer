use super::*;
use crate::core::sync::{SyncDirection, SyncService};
use std::cell::Cell;
use std::rc::Rc;

fn description(direction: SyncDirection) -> String {
    tr(if direction == SyncDirection::UploadOnly {
        "setting.sync.strategy.backup_description"
    } else {
        "setting.sync.strategy.sync_description"
    })
}

/// One glass capsule with native grouped toggles, available while work is active.
pub(super) fn strategy_row(
    direction: SyncDirection,
    parent: &gtk::Widget,
    accept: impl Fn(SyncDirection) -> crate::core::error::Result<()> + 'static,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(tr("setting.sync.strategy"))
        .subtitle(description(direction))
        .activatable(false)
        .build();
    row.add_css_class("settings-action-row");
    row.set_widget_name("sync-strategy-row");
    let capsule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    capsule.add_css_class("glass-raised");
    capsule.add_css_class("glass-segmented");
    capsule.set_valign(gtk::Align::Center);
    let backup = gtk::ToggleButton::with_label(&tr("setting.sync.strategy.backup"));
    let sync = gtk::ToggleButton::with_label(&tr("setting.sync.strategy.sync"));
    for button in [&backup, &sync] {
        button.add_css_class("glass-segment");
        capsule.append(button);
    }
    sync.set_group(Some(&backup));
    backup.set_active(direction == SyncDirection::UploadOnly);
    sync.set_active(direction != SyncDirection::UploadOnly);
    row.add_suffix(&capsule);
    let changing = Rc::new(Cell::new(false));
    let saved = Rc::new(Cell::new(direction));
    let accept = Rc::new(accept);
    for (button, value) in [
        (&backup, SyncDirection::UploadOnly),
        (&sync, SyncDirection::Bidirectional),
    ] {
        let accept = accept.clone();
        let saved = saved.clone();
        let changing = changing.clone();
        let row = row.downgrade();
        let backup = backup.downgrade();
        let sync = sync.downgrade();
        let parent = parent.clone();
        button.connect_toggled(move |button| {
            if changing.get() || !button.is_active() || value == saved.get() {
                return;
            }
            match accept(value) {
                Ok(()) => {
                    saved.set(value);
                    if let Some(row) = row.upgrade() {
                        row.set_subtitle(&description(value));
                    }
                }
                Err(error) => {
                    changing.set(true);
                    if let (Some(backup), Some(sync)) = (backup.upgrade(), sync.upgrade()) {
                        backup.set_active(saved.get() == SyncDirection::UploadOnly);
                        sync.set_active(saved.get() != SyncDirection::UploadOnly);
                    }
                    changing.set(false);
                    show_settings_error_dialog(
                        &parent,
                        &trf(
                            "setting.sync.setting_save_failed",
                            &[("error", &error.to_string())],
                        ),
                    );
                }
            }
        });
    }
    row
}

pub(super) fn saved_strategy_row(
    job: &crate::core::sync::SyncJob,
    service: SyncService,
    parent: &gtk::Widget,
) -> adw::ActionRow {
    let id = job.id;
    strategy_row(job.direction, parent, move |direction| {
        service.accept_direction(id, direction)?;
        Ok(())
    })
}
