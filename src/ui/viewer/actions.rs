use super::ViewerPage;
use crate::core::db_actor::DbCommand;
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::repository::MediaRepository;
use crate::ui::toasts;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};

impl ViewerPage {
    pub(super) fn setup_delete_button(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.delete_btn.get().connect_clicked(move |_button| {
            let Some(this) = weak.upgrade() else { return };

            let dialog = adw::AlertDialog::builder()
                .heading(tr("trash.confirm_title"))
                .body(tr("trash.confirm_body_one"))
                .build();
            dialog.add_css_class("glass-alert-dialog");
            dialog.add_response("cancel", &tr("dialog.cancel"));
            dialog.add_response("trash", &tr("dialog.trash"));
            dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let weak2 = this.downgrade();
            dialog.connect_response(None, move |_, response| {
                if response != "trash" {
                    return;
                }
                let Some(this) = weak2.upgrade() else { return };
                let trace = crate::core::telemetry::OperationTrace::start(
                    crate::core::telemetry::TraceChain::Mutation,
                    "move_to_trash",
                );
                let db_actor = match this.imp().db_actor.borrow().as_ref() {
                    Some(actor) => actor.clone(),
                    None => {
                        crate::core::telemetry::log_warning(
                            &trace,
                            "precondition",
                            "DB actor is not initialized",
                        );
                        return;
                    }
                };
                let item = match this.current_media_item() {
                    Some(i) => i,
                    None => return,
                };

                let item_id = item.id;
                let weak_after = this.downgrade();
                glib::spawn_future_local(async move {
                    let prepared = db_actor
                        .execute_in_trace(
                            trace.clone(),
                            DbCommand::MarkTrashed {
                                ids: vec![MediaId::from(item_id)],
                            },
                        )
                        .await;
                    let Ok(crate::core::DbCommandResult::MediaItems(mut items)) = prepared else {
                        if let Some(this) = weak_after.upgrade() {
                            toasts::error(
                                &this.imp().toast_overlay.get(),
                                &tr("viewer.toast.move_to_trash_failed"),
                            );
                        }
                        return;
                    };
                    let Some(item) = items.pop() else {
                        return;
                    };
                    let Some(this) = weak_after.upgrade() else {
                        if let Err(error) = db_actor
                            .execute_in_trace(
                                trace.clone(),
                                DbCommand::RollbackTrashed {
                                    ids: vec![MediaId::from(item_id)],
                                },
                            )
                            .await
                        {
                            crate::core::telemetry::log_error(&trace, "db_rollback", error);
                        }
                        return;
                    };
                    let Some(pool) = this.imp().pool.borrow().as_ref().cloned() else {
                        crate::core::telemetry::log_warning(
                            &trace,
                            "precondition",
                            "database pool is not initialized",
                        );
                        if let Err(error) = db_actor
                            .execute_in_trace(
                                trace.clone(),
                                DbCommand::RollbackTrashed {
                                    ids: vec![MediaId::from(item_id)],
                                },
                            )
                            .await
                        {
                            crate::core::telemetry::log_error(&trace, "db_rollback", error);
                        }
                        toasts::error(
                            &this.imp().toast_overlay.get(),
                            &tr("viewer.toast.move_to_trash_failed"),
                        );
                        return;
                    };
                    let weak_for_callback = this.downgrade();
                    crate::ui::trash_fallback::move_marked_items_with_fallback(
                        &this,
                        pool,
                        db_actor,
                        vec![item],
                        trace,
                        move |moved_ids| {
                            if !moved_ids.iter().any(|id| id.get() == item_id) {
                                return;
                            }
                            if let Some(this) = weak_for_callback.upgrade() {
                                this.remove_deleted_item(item_id);
                                if let Some(cb) = this.imp().trashed_cb.borrow().clone() {
                                    cb(item_id);
                                }
                                let undo_weak = this.downgrade();
                                toasts::success_with_action(
                                    &this.imp().toast_overlay.get(),
                                    &tr("viewer.toast.moved_to_trash"),
                                    &tr("viewer.toast.undo"),
                                    move || {
                                        if let Some(this) = undo_weak.upgrade() {
                                            this.restore_deleted_item(item_id);
                                        }
                                    },
                                );
                            }
                        },
                    );
                });
            });
            dialog.present(&this);
        });
    }

    fn remove_deleted_item(&self, item_id: i64) {
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            self.fire_nav(super::NAV_POP);
            return;
        };
        let deleted_index = super::navigation::find_media_index_by_id(&list, item_id)
            .unwrap_or_else(|| {
                self.imp()
                    .current_index
                    .get()
                    .min(list.n_items().saturating_sub(1))
            });
        if deleted_index < list.n_items() {
            list.remove(deleted_index);
        }

        match super::navigation::next_index_after_deleted_item(deleted_index, list.n_items()) {
            Some(next) => self.show_at(next),
            None => self.fire_nav(super::NAV_POP),
        }
    }

    /// Undo the delete: move the file back out of the trash, clear the DB mark,
    /// then re-insert the row into the live list the viewer is browsing.
    ///
    /// The list is only touched once the file is really back, so a failed undo
    /// cannot leave a tile pointing at a missing photo. Writing through the DB
    /// actor is what emits the domain events other pages listen to; going
    /// straight to the pool would fix this view and desynchronise the rest of
    /// the library. Afterwards the viewer shows the restored photo — pressing
    /// Undo means "I wanted that one", and landing anywhere else reads as a miss.
    pub(super) fn restore_deleted_item(&self, item_id: i64) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let db_actor = self.imp().db_actor.borrow().clone();
        let list = self.imp().media_list.borrow().as_ref().cloned();
        let delete_btn = self.imp().delete_btn.get().downgrade();
        let weak_after = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                MediaRepository::new(pool)
                    .restore_batch(&[MediaId::from(item_id)], db_actor.as_ref())
            })
            .await;
            if let Some(btn) = delete_btn.upgrade() {
                btn.set_sensitive(true);
            }
            let Some(this) = weak_after.upgrade() else {
                return;
            };
            let batch = match result {
                Ok(batch) if batch.failures.is_empty() => batch,
                Ok(batch) => {
                    tracing::warn!(
                        "ViewerPage: undo move-to-trash failed: {:?}",
                        batch.failures
                    );
                    toasts::error(
                        &this.imp().toast_overlay.get(),
                        &tr("viewer.toast.restore_failed"),
                    );
                    return;
                }
                Err(error) => {
                    tracing::warn!("ViewerPage: undo move-to-trash worker failed: {error:?}");
                    toasts::error(
                        &this.imp().toast_overlay.get(),
                        &tr("viewer.toast.restore_failed"),
                    );
                    return;
                }
            };
            let Some(list) = list else {
                return;
            };
            for item in batch.mutation.changed_items {
                crate::ui::media_list::insert_media_item_sorted(&list, item);
            }
            if let Some(index) = super::navigation::find_media_index_by_id(&list, item_id) {
                this.show_at(index);
            }
        });
    }

    pub(super) fn setup_favorite_button(&self) {
        crate::ui::grid_css::assert_installed();

        let imp = self.imp();
        imp.favorite_btn
            .get()
            .set_icon_name(crate::ui::favorite_icon::NAME);
        imp.favorite_btn.get().add_css_class("viewer-favorite-btn");
        imp.favorite_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.favorite")));
        self.refresh_favorite_button(false);
        self.watch_toolbar_icon_size();

        let weak = self.downgrade();
        imp.favorite_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            if this.imp().db_actor.borrow().is_none() {
                tracing::warn!("ViewerPage: Favorite pressed but DB actor not set");
                return;
            }
            let Some(item_id) = this.current_media_item().map(|item| item.id) else {
                return;
            };
            let next_state = !this.imp().is_favorite.get();
            this.apply_favorite_state(item_id, next_state, true);
        });
    }

    /// Keep the cloud badge the same size as the toolbar icon beside it.
    ///
    /// `sync_badge` is a bare `GtkImage`, so it takes GTK's default icon size
    /// (16 px) while a button's icon follows whatever the header resolves — and
    /// that is not always 16: themes and libadwaita versions ship their own
    /// header icon size. The badge then renders a third smaller than the heart
    /// on the other side of the same bar. Pinning a number in CSS or the
    /// template would only move the disagreement to the next theme, so read the
    /// resolved size off the favourite button's icon and copy it across.
    ///
    /// Re-run on every map rather than once at construction: the icon size is
    /// a resolved style property, and a header that has never been mapped has
    /// not resolved it yet. Mapping is also what re-runs after the viewer is
    /// popped and pushed again.
    fn watch_toolbar_icon_size(&self) {
        let weak = self.downgrade();
        self.imp().header_bar.get().connect_map(move |_| {
            let Some(this) = weak.upgrade() else { return };
            this.sync_badge_to_toolbar_icon_size();
        });
    }

    fn sync_badge_to_toolbar_icon_size(&self) {
        let imp = self.imp();
        let Some(toolbar_icon) = imp
            .favorite_btn
            .get()
            .child()
            .and_then(|child| child.downcast::<gtk::Image>().ok())
        else {
            return;
        };
        let (_, natural, _, _) = toolbar_icon.measure(gtk::Orientation::Horizontal, -1);
        let badge = imp.sync_badge.get();
        if natural > 0 && badge.pixel_size() != natural {
            badge.set_pixel_size(natural);
        }
    }

    /// Write a favorite state and announce it.
    ///
    /// `announce` adds the undo action, and the undo re-enters with it off: a
    /// rollback that offers another rollback is a toast chain, not an undo.
    pub(super) fn apply_favorite_state(&self, item_id: i64, next_state: bool, announce: bool) {
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            return;
        };
        let button = self.imp().favorite_btn.get();
        button.set_sensitive(false);
        let button_weak = button.downgrade();
        let token = self.imp().current_token.get();
        let weak_after = self.downgrade();
        glib::spawn_future_local(async move {
            let db_result = db_actor
                .execute(DbCommand::SetFavorite {
                    ids: vec![MediaId::from(item_id)],
                    is_favorite: next_state,
                })
                .await
                .map(|_| ());
            if let Some(button) = button_weak.upgrade() {
                button.set_sensitive(true);
            }
            let Some(this) = weak_after.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            match db_result {
                Ok(()) => {
                    this.refresh_favorite_button(next_state);
                    if let Some(cb) = this.imp().favorite_state_cb.borrow().clone() {
                        cb(item_id, next_state);
                    }
                    if announce {
                        let undo_weak = this.downgrade();
                        toasts::success_with_action(
                            &this.imp().toast_overlay.get(),
                            &tr(if next_state {
                                "viewer.toast.favorited"
                            } else {
                                "viewer.toast.unfavorited"
                            }),
                            &tr("viewer.toast.undo"),
                            move || {
                                if let Some(this) = undo_weak.upgrade() {
                                    this.apply_favorite_state(item_id, !next_state, false);
                                }
                            },
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("ViewerPage: Toggle favorite failed: {e}");
                    toasts::error(
                        &this.imp().toast_overlay.get(),
                        &format!("{}: {e}", tr("viewer.toast.favorite_update_failed")),
                    );
                }
            }
        });
    }

    fn refresh_favorite_button(&self, is_favorite: bool) {
        self.imp().is_favorite.set(is_favorite);
        let button = self.imp().favorite_btn.get();
        if is_favorite {
            button.add_css_class("favorite-active");
            button.set_tooltip_text(Some(&tr("viewer.button.favorite_active")));
        } else {
            button.remove_css_class("favorite-active");
            button.set_tooltip_text(Some(&tr("viewer.button.favorite")));
        }
    }

    pub(super) fn sync_favorite_state(&self, item_id: i64) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            self.refresh_favorite_button(false);
            return;
        };

        let token = self.imp().current_token.get();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let result = MediaRepository::new(pool)
                .favorite_state(&[MediaId::from(item_id)])
                .map(|summary| summary.has_favorite);
            let _ = tx.send((result, token));
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok((result, token_expected)) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token_expected {
                return;
            }
            match result {
                Ok(is_favorite) => this.refresh_favorite_button(is_favorite),
                Err(e) => {
                    tracing::warn!("ViewerPage: failed to read favorite state: {e}");
                    this.refresh_favorite_button(false);
                }
            }
        });
    }
}
