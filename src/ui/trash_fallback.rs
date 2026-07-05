use std::rc::Rc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};

use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::i18n::{tr, trf};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::prefs::{self, TrashBackend};
use crate::core::trash;

#[derive(Debug)]
struct MoveBatch {
    moved: Vec<MediaItem>,
    failed: Vec<MediaItem>,
}

pub fn move_marked_items_with_fallback<W, F>(
    parent: &W,
    pool: DbPool,
    db_actor: DbActorHandle,
    items: Vec<MediaItem>,
    on_moved: F,
) where
    W: IsA<gtk::Widget> + Clone + 'static,
    F: Fn(Vec<MediaId>) + 'static,
{
    if items.is_empty() {
        return;
    }

    let parent = parent.clone().upcast::<gtk::Widget>();
    let on_moved: Rc<dyn Fn(Vec<MediaId>)> = Rc::new(on_moved);
    glib::spawn_future_local(async move {
        let backend = prefs::trash_backend();
        let items_for_worker = items.clone();
        let move_result =
            gtk::gio::spawn_blocking(move || move_items_to_backend(items_for_worker, backend))
                .await;

        let Ok(batch) = move_result else {
            rollback_items(&db_actor, &items).await;
            show_error_dialog(&parent, &tr("trash.move_failed"));
            return;
        };

        commit_moved(&db_actor, batch.moved.clone()).await;
        let moved_ids = ids_for_items(&batch.moved);
        if !moved_ids.is_empty() {
            on_moved(moved_ids);
        }

        if batch.failed.is_empty() {
            return;
        }

        if backend == TrashBackend::System {
            prompt_switch_to_app_trash(parent, pool, db_actor, batch.failed, on_moved);
        } else {
            rollback_items(&db_actor, &batch.failed).await;
            show_error_dialog(&parent, &tr("trash.move_failed"));
        }
    });
}

fn move_items_to_backend(items: Vec<MediaItem>, backend: TrashBackend) -> MoveBatch {
    let mut moved = Vec::new();
    let mut failed = Vec::new();
    for item in items {
        match trash::move_to_backend(&item.uri, backend) {
            Ok(()) => moved.push(item),
            Err(err) => {
                tracing::warn!(
                    "failed to move {} to {backend:?} trash backend: {err}",
                    item.uri
                );
                failed.push(item);
            }
        }
    }
    MoveBatch { moved, failed }
}

fn prompt_switch_to_app_trash(
    parent: gtk::Widget,
    pool: DbPool,
    db_actor: DbActorHandle,
    failed: Vec<MediaItem>,
    on_moved: Rc<dyn Fn(Vec<MediaId>)>,
) {
    let count = failed.len().to_string();
    let dialog = adw::AlertDialog::builder()
        .heading(tr("trash.fallback.title"))
        .body(trf("trash.fallback.body", &[("count", &count)]))
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("cancel", &tr("dialog.cancel"));
    dialog.add_response("switch", &tr("trash.fallback.switch_to_app"));
    dialog.set_response_appearance("switch", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("switch"));
    dialog.set_close_response("cancel");

    let parent_for_response = parent.clone();
    dialog.connect_response(None, move |_, response| {
        let response = response.to_string();
        let pool = pool.clone();
        let db_actor = db_actor.clone();
        let parent = parent_for_response.clone();
        let failed = failed.clone();
        let on_moved = on_moved.clone();

        glib::spawn_future_local(async move {
            if response != "switch" {
                rollback_items(&db_actor, &failed).await;
                return;
            }

            let failed_for_worker = failed.clone();
            let result = gtk::gio::spawn_blocking(move || {
                switch_to_app_trash_and_move_failed(&pool, failed_for_worker)
            })
            .await;

            match result {
                Ok(Ok(moved)) => {
                    commit_moved(&db_actor, moved.clone()).await;
                    let moved_ids = ids_for_items(&moved);
                    if !moved_ids.is_empty() {
                        on_moved(moved_ids);
                    }
                }
                Ok(Err(err)) => {
                    rollback_items(&db_actor, &failed).await;
                    show_error_dialog(
                        &parent,
                        &trf(
                            "trash.fallback.switch_failed",
                            &[("error", &err.to_string())],
                        ),
                    );
                }
                Err(err) => {
                    rollback_items(&db_actor, &failed).await;
                    show_error_dialog(
                        &parent,
                        &trf(
                            "trash.fallback.switch_failed",
                            &[("error", &format!("{err:?}"))],
                        ),
                    );
                }
            }
        });
    });
    dialog.present(&parent);
}

fn switch_to_app_trash_and_move_failed(
    pool: &DbPool,
    failed: Vec<MediaItem>,
) -> crate::core::Result<Vec<MediaItem>> {
    let excluded = ids_for_items(&failed);
    trash::migrate_trash_backend(pool, TrashBackend::System, TrashBackend::App, &excluded)?;

    let mut moved = Vec::new();
    for item in failed {
        match trash::move_to_app_trash(&item.uri) {
            Ok(()) => moved.push(item),
            Err(err) => {
                for item in moved.iter().rev() {
                    let _ = trash::restore_from_trash(&item.uri);
                }
                let _ = trash::migrate_trash_backend(
                    pool,
                    TrashBackend::App,
                    TrashBackend::System,
                    &[],
                );
                return Err(err);
            }
        }
    }

    if let Err(err) = prefs::set_trash_backend(TrashBackend::App) {
        for item in moved.iter().rev() {
            let _ = trash::restore_from_trash(&item.uri);
        }
        let _ = trash::migrate_trash_backend(pool, TrashBackend::App, TrashBackend::System, &[]);
        return Err(crate::core::error::AppError::Backend(err));
    }

    Ok(moved)
}

async fn commit_moved(db_actor: &DbActorHandle, items: Vec<MediaItem>) {
    if !items.is_empty() {
        let _ = db_actor
            .execute(DbCommand::CommitMovedToTrash { items })
            .await;
    }
}

async fn rollback_items(db_actor: &DbActorHandle, items: &[MediaItem]) {
    let ids = ids_for_items(items);
    if !ids.is_empty() {
        let _ = db_actor.execute(DbCommand::RollbackTrashed { ids }).await;
    }
}

fn ids_for_items(items: &[MediaItem]) -> Vec<MediaId> {
    items
        .iter()
        .map(|item| MediaId::from(item.id))
        .collect::<Vec<_>>()
}

fn show_error_dialog(parent: &gtk::Widget, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("trash.operation_failed"))
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
}
