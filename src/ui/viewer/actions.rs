use super::ViewerPage;
use crate::core::db_actor::DbCommand;
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::repository::MediaRepository;
use crate::ui::toasts;
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
                let db_actor = match this.imp().db_actor.borrow().as_ref() {
                    Some(actor) => actor.clone(),
                    None => {
                        tracing::warn!(
                            target: crate::core::log_targets::VIEWER,
                            "TRASH_TRACE viewer_delete_no_actor"
                        );
                        return;
                    }
                };
                let item = match this.current_media_item() {
                    Some(i) => i,
                    None => return,
                };

                let item_id = item.id;
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "TRASH_TRACE viewer_delete_requested id={} uri={}",
                    item.id,
                    item.uri
                );
                let trash_trace = crate::ui::trash_fallback::TrashMoveTrace::begin("viewer", 1);
                let weak_after = this.downgrade();
                glib::spawn_future_local(async move {
                    let prepared = db_actor
                        .execute(DbCommand::MarkTrashed {
                            ids: vec![MediaId::from(item_id)],
                            trace_id: Some(trash_trace.operation_id()),
                        })
                        .await;
                    let Ok(crate::core::DbCommandResult::MediaItems(mut items)) = prepared else {
                        tracing::warn!(
                            target: crate::core::log_targets::VIEWER,
                            "TRASH_TRACE viewer_mark_failed id={item_id}"
                        );
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
                        let _ = db_actor
                            .execute(DbCommand::RollbackTrashed {
                                ids: vec![MediaId::from(item_id)],
                            })
                            .await;
                        return;
                    };
                    let Some(pool) = this.imp().pool.borrow().as_ref().cloned() else {
                        let _ = db_actor
                            .execute(DbCommand::RollbackTrashed {
                                ids: vec![MediaId::from(item_id)],
                            })
                            .await;
                        toasts::error(
                            &this.imp().toast_overlay.get(),
                            &tr("viewer.toast.move_to_trash_failed"),
                        );
                        return;
                    };
                    trash_trace.marked(1);

                    let weak_for_callback = this.downgrade();
                    crate::ui::trash_fallback::move_marked_items_with_fallback(
                        &this,
                        pool,
                        db_actor,
                        vec![item],
                        trash_trace,
                        move |moved_ids| {
                            if !moved_ids.iter().any(|id| id.get() == item_id) {
                                return;
                            }
                            if let Some(this) = weak_for_callback.upgrade() {
                                toasts::success(
                                    &this.imp().toast_overlay.get(),
                                    &tr("viewer.toast.moved_to_trash"),
                                );
                                this.remove_deleted_item(item_id);
                                if let Some(cb) = this.imp().trashed_cb.borrow().clone() {
                                    cb(item_id);
                                }
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

    pub(super) fn setup_favorite_button(&self) {
        crate::ui::grid_css::assert_installed();

        let imp = self.imp();
        imp.favorite_btn.get().add_css_class("viewer-favorite-btn");
        imp.favorite_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.favorite")));
        self.refresh_favorite_button(false);

        let weak = self.downgrade();
        imp.favorite_btn.get().connect_clicked(move |button| {
            let Some(this) = weak.upgrade() else { return };
            let db_actor = match this.imp().db_actor.borrow().as_ref() {
                Some(actor) => actor.clone(),
                None => {
                    tracing::warn!("ViewerPage: Favorite pressed but DB actor not set");
                    return;
                }
            };
            let item_id = match this.current_media_item() {
                Some(i) => i.id,
                None => return,
            };

            let next_state = !this.imp().is_favorite.get();
            button.set_sensitive(false);
            let button_weak = button.downgrade();
            let token = this.imp().current_token.get();
            let weak_after = this.downgrade();
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
                if let Some(this) = weak_after.upgrade() {
                    if this.imp().current_token.get() != token {
                        return;
                    }
                    match db_result {
                        Ok(()) => {
                            this.refresh_favorite_button(next_state);
                            if let Some(cb) = this.imp().favorite_state_cb.borrow().clone() {
                                cb(item_id, next_state);
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
                }
            });
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
