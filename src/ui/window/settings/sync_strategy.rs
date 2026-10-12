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

/// Keep the underline allocated, so switching never moves hit targets.
fn strategy_button(label_key: &str, name: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::new();
    button.set_widget_name(name);
    button.add_css_class("glass-segment");
    button.update_property(&[gtk::accessible::Property::Label(&tr(label_key))]);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let caption = gtk::Label::new(Some(&tr(label_key)));
    caption.set_halign(gtk::Align::Center);
    content.append(&caption);
    let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    indicator.add_css_class("glass-segment-indicator");
    indicator.set_halign(gtk::Align::Center);
    indicator.set_opacity(0.0);
    content.append(&indicator);
    button.set_child(Some(&content));
    button.connect_toggled(move |button| {
        let opacity = if button.is_active() { 1.0 } else { 0.0 };
        indicator.set_opacity(opacity);
    });
    button
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
    capsule.add_css_class("sync-strategy-selector");
    capsule.set_homogeneous(true);
    capsule.set_valign(gtk::Align::Center);
    let backup = strategy_button("setting.sync.strategy.backup", "sync-strategy-backup");
    let sync = strategy_button("setting.sync.strategy.sync", "sync-strategy-sync");
    for button in [&backup, &sync] {
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
