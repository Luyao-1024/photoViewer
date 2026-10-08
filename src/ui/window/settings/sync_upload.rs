//! Upload selection is explicit local permission, independent of cloud discovery.
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::core::i18n::{tr, trf};
use crate::core::sync::{SyncJob, SyncService, SyncStore, UploadScope};

struct AlbumControl {
    relative_album: String,
    sort_title: String,
    row: adw::ActionRow,
    check: gtk::CheckButton,
}

pub(super) struct SyncUploadAlbumSelection {
    expander: adw::ExpanderRow,
    all_row: adw::ActionRow,
    all: gtk::Switch,
    albums: Vec<AlbumControl>,
    store: SyncStore,
    service: SyncService,
    job_id: i64,
    saved: RefCell<BTreeSet<String>>,
    updating: Cell<bool>,
}

impl SyncUploadAlbumSelection {
    pub(super) fn new(
        expander: &adw::ExpanderRow,
        options: Vec<(String, String, String)>,
        job: &SyncJob,
        service: SyncService,
    ) -> Rc<Self> {
        let store = service.store().clone();
        let selected = store
            .desired_upload_albums(job.id)
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let all_row = adw::ActionRow::builder()
            .title(tr("setting.sync.upload_albums_select_all"))
            .build();
        all_row.add_css_class("settings-action-row");
        let all = gtk::Switch::builder().valign(gtk::Align::Center).build();
        all.set_tooltip_text(Some(&tr("setting.sync.upload_albums_select_all")));
        all_row.add_suffix(&all);
        all_row.set_activatable_widget(Some(&all));
        expander.add_row(&all_row);

        let mut albums = Vec::new();
        for (relative_album, title, subtitle) in options {
            let row = adw::ActionRow::builder()
                .title(&title)
                .subtitle(&subtitle)
                .build();
            row.add_css_class("settings-action-row");
            let check = gtk::CheckButton::builder()
                .valign(gtk::Align::Center)
                .active(job.upload_scope == UploadScope::All || selected.contains(&relative_album))
                .build();
            check.update_property(&[gtk::accessible::Property::Label(&title)]);
            row.add_suffix(&check);
            row.set_activatable_widget(Some(&check));
            expander.add_row(&row);
            albums.push(AlbumControl {
                relative_album,
                sort_title: title.to_lowercase(),
                row,
                check,
            });
        }
        if albums.is_empty() {
            let empty = adw::ActionRow::builder()
                .title(tr("setting.sync.upload_albums_empty"))
                .activatable(false)
                .build();
            empty.add_css_class("settings-action-row");
            expander.add_row(&empty);
        }
        let saved = if job.upload_scope == UploadScope::All {
            albums
                .iter()
                .map(|album| album.relative_album.clone())
                .collect()
        } else {
            selected
        };
        let controls = Rc::new(Self {
            expander: expander.clone(),
            all_row,
            all,
            albums,
            store,
            service,
            job_id: job.id,
            saved: RefCell::new(saved),
            updating: Cell::new(false),
        });
        // Sort GTK rows in place rather than reparenting focused controls.
        if let Some(list) = controls.list_box() {
            // Keep comparisons O(1), even for a long album checklist. Object
            // identities remain stable because rows are sorted, not recreated.
            let row_indices = controls
                .albums
                .iter()
                .enumerate()
                .map(|(index, album)| (album.row.as_ptr() as usize, index))
                .collect::<HashMap<_, _>>();
            let weak = Rc::downgrade(&controls);
            list.set_sort_func(move |left, right| {
                let Some(controls) = weak.upgrade() else {
                    return gtk::Ordering::Equal;
                };
                let a = row_indices
                    .get(&(left.as_ptr() as usize))
                    .map(|index| &controls.albums[*index]);
                let b = row_indices
                    .get(&(right.as_ptr() as usize))
                    .map(|index| &controls.albums[*index]);
                match (a, b) {
                    (None, None) => gtk::Ordering::Equal,
                    (None, Some(_)) => gtk::Ordering::Smaller,
                    (Some(_), None) => gtk::Ordering::Larger,
                    (Some(a), Some(b)) => b
                        .check
                        .is_active()
                        .cmp(&a.check.is_active())
                        .then_with(|| a.sort_title.cmp(&b.sort_title))
                        .then_with(|| a.relative_album.cmp(&b.relative_album))
                        .into(),
                }
            });
        }
        controls.refresh();
        controls.set_editable(true);
        for album in &controls.albums {
            let weak = Rc::downgrade(&controls);
            album.check.connect_toggled(move |_| {
                let Some(controls) = weak.upgrade() else {
                    return;
                };
                if controls.updating.get() {
                    return;
                }
                let selected = controls
                    .albums
                    .iter()
                    .filter(|album| album.check.is_active())
                    .map(|album| album.relative_album.clone())
                    .collect();
                controls.save(selected);
            });
        }
        let weak = Rc::downgrade(&controls);
        controls.all.connect_active_notify(move |switch| {
            let Some(controls) = weak.upgrade() else {
                return;
            };
            if controls.updating.get() {
                return;
            }
            let selected = if switch.is_active() {
                controls
                    .albums
                    .iter()
                    .map(|album| album.relative_album.clone())
                    .collect()
            } else {
                BTreeSet::new()
            };
            controls.save(selected);
        });
        let weak = Rc::downgrade(&controls);
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            let Some(controls) = weak.upgrade() else {
                return gtk::glib::ControlFlow::Break;
            };
            if controls.expander.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            controls.update_status();
            gtk::glib::ControlFlow::Continue
        });
        controls
    }

    pub(super) fn set_editable(&self, editable: bool) {
        self.all.set_sensitive(editable && !self.albums.is_empty());
        for album in &self.albums {
            album.check.set_sensitive(editable);
        }
    }

    fn list_box(&self) -> Option<gtk::ListBox> {
        self.all_row
            .parent()
            .and_then(|parent| parent.downcast().ok())
    }

    fn save(&self, selected: BTreeSet<String>) {
        // A bulk action makes one atomic write, not one write per checkbox.
        let values = selected.iter().cloned().collect::<Vec<_>>();
        let result = self.service.accept_upload_albums(self.job_id, &values);
        if result.is_ok() {
            *self.saved.borrow_mut() = selected;
        }
        // On failure restore the saved selection, master switch and row order.
        self.refresh();
        self.expander.set_subtitle(&match &result {
            Ok(_) => trf(
                "setting.sync.upload_albums_selected",
                &[("count", &values.len().to_string())],
            ),
            Err(error) => trf(
                "setting.sync.upload_albums_failed",
                &[("error", &error.to_string())],
            ),
        });
        if result.is_ok() {
            self.update_status();
        }
    }

    fn update_status(&self) {
        let count = self.saved.borrow().len().to_string();
        let job = self.store.get_job(self.job_id).ok().flatten();
        let pending = self.store.pending_change(self.job_id).ok().flatten();
        let subtitle = if let Some(error) = job.and_then(|job| job.last_error) {
            trf(
                if pending.is_some() {
                    "setting.sync.changes_apply_failed"
                } else {
                    "setting.sync.failed"
                },
                &[("error", &error)],
            )
        } else if pending.is_some() {
            trf("setting.sync.upload_albums_applying", &[("count", &count)])
        } else {
            trf("setting.sync.upload_albums_selected", &[("count", &count)])
        };
        self.expander.set_subtitle(&subtitle);
    }

    fn refresh(&self) {
        self.updating.set(true);
        let saved = self.saved.borrow();
        let selected = self
            .albums
            .iter()
            .filter(|album| saved.contains(&album.relative_album))
            .count();
        let total = self.albums.len();
        for album in &self.albums {
            album
                .check
                .set_active(saved.contains(&album.relative_album));
        }
        self.all.set_active(total > 0 && selected == total);
        self.all_row.set_subtitle(&if total == 0 {
            tr("setting.sync.upload_albums_empty")
        } else if selected == 0 {
            tr("setting.sync.upload_albums_select_all_none")
        } else if selected == total {
            trf(
                "setting.sync.upload_albums_select_all_done",
                &[("total", &total.to_string())],
            )
        } else {
            trf(
                "setting.sync.upload_albums_select_all_partial",
                &[
                    ("selected", &selected.to_string()),
                    ("total", &total.to_string()),
                ],
            )
        });
        self.updating.set(false);
        if let Some(list) = self.list_box() {
            list.invalidate_sort();
        }
    }
}
