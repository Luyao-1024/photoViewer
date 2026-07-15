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
use crate::core::telemetry::OperationTrace;
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
    trace: OperationTrace,
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
        let trace_for_worker = trace.clone();
        let move_result = gtk::gio::spawn_blocking(move || {
            let stage = trace_for_worker.stage("filesystem_batch");
            stage.record("detail", format!("{backend:?}"));
            stage.record("item_count", items_for_worker.len());
            move_items_to_backend(items_for_worker, backend, &trace_for_worker)
        })
        .await;

        let Ok(batch) = move_result else {
            crate::core::telemetry::log_error(
                &trace,
                "filesystem_worker",
                "blocking filesystem worker failed",
            );
            rollback_items(&db_actor, &items, &trace).await;
            show_error_dialog(&parent, &tr("trash.move_failed"));
            return;
        };

        commit_moved(&db_actor, batch.moved.clone(), trace.clone()).await;
        let moved_ids = ids_for_items(&batch.moved);
        if !moved_ids.is_empty() {
            let callback_stage = trace.stage("ui_callback");
            callback_stage.record("item_count", moved_ids.len());
            on_moved(moved_ids);
        }

        if batch.failed.is_empty() {
            return;
        }

        crate::core::telemetry::log_warning(
            &trace,
            "filesystem_partial_failure",
            format!(
                "{} of {} items could not be moved",
                batch.failed.len(),
                items.len()
            ),
        );
        if backend == TrashBackend::System {
            prompt_switch_to_app_trash(parent, pool, db_actor, batch.failed, trace, on_moved);
        } else {
            rollback_items(&db_actor, &batch.failed, &trace).await;
            show_error_dialog(&parent, &tr("trash.move_failed"));
        }
    });
}

fn move_items_to_backend(
    items: Vec<MediaItem>,
    backend: TrashBackend,
    trace: &OperationTrace,
) -> MoveBatch {
    let mut moved = Vec::new();
    let mut failed = Vec::new();
    for item in items {
        let stage = trace.stage("filesystem_item");
        stage.record("detail", item.id);
        stage.record("item_count", 1);
        stage.record("bytes", item.file_size);
        match trash::move_to_backend(&item.uri, backend) {
            Ok(()) => moved.push(item),
            Err(err) => {
                crate::core::telemetry::log_warning(
                    trace,
                    "filesystem_item",
                    format!("{}: {err}", item.uri),
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
    trace: OperationTrace,
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
        let trace = trace.clone();
        let on_moved = on_moved.clone();

        glib::spawn_future_local(async move {
            if response != "switch" {
                rollback_items(&db_actor, &failed, &trace).await;
                return;
            }

            let failed_for_worker = failed.clone();
            let result = gtk::gio::spawn_blocking(move || {
                switch_to_app_trash_and_move_failed(&pool, failed_for_worker)
            })
            .await;

            match result {
                Ok(Ok(moved)) => {
                    commit_moved(&db_actor, moved.clone(), trace.clone()).await;
                    let moved_ids = ids_for_items(&moved);
                    if !moved_ids.is_empty() {
                        on_moved(moved_ids);
                    }
                }
                Ok(Err(err)) => {
                    crate::core::telemetry::log_error(&trace, "fallback_filesystem", &err);
                    rollback_items(&db_actor, &failed, &trace).await;
                    show_error_dialog(
                        &parent,
                        &trf(
                            "trash.fallback.switch_failed",
                            &[("error", &err.to_string())],
                        ),
                    );
                }
                Err(err) => {
                    crate::core::telemetry::log_error(
                        &trace,
                        "fallback_worker",
                        format!("{err:?}"),
                    );
                    rollback_items(&db_actor, &failed, &trace).await;
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

async fn commit_moved(db_actor: &DbActorHandle, items: Vec<MediaItem>, trace: OperationTrace) {
    if !items.is_empty() {
        let result = db_actor
            .execute_in_trace(trace.clone(), DbCommand::CommitMovedToTrash { items })
            .await;
        if let Err(error) = result {
            crate::core::telemetry::log_error(&trace, "db_commit", error);
        }
    }
}

async fn rollback_items(db_actor: &DbActorHandle, items: &[MediaItem], trace: &OperationTrace) {
    let ids = ids_for_items(items);
    if !ids.is_empty() {
        if let Err(error) = db_actor
            .execute_in_trace(trace.clone(), DbCommand::RollbackTrashed { ids })
            .await
        {
            crate::core::telemetry::log_error(trace, "db_rollback", error);
        }
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
