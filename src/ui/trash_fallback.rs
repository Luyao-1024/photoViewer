use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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

/// Correlates every phase of one user-initiated move-to-trash operation.
///
/// The filesystem worker and DB actor run on different threads, so a tracing
/// span alone cannot carry this context across the whole operation. Keep this
/// small explicit ID on every performance event instead.
#[derive(Debug, Clone, Copy)]
pub struct TrashMoveTrace {
    operation_id: u64,
    requested_at: Instant,
}

static NEXT_TRASH_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

impl TrashMoveTrace {
    pub fn begin(source: &'static str, requested_count: usize) -> Self {
        let trace = Self {
            operation_id: NEXT_TRASH_OPERATION_ID.fetch_add(1, Ordering::Relaxed),
            requested_at: Instant::now(),
        };
        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            operation_id = trace.operation_id,
            source,
            requested_count,
            "TRASH_PERF phase=requested"
        );
        trace
    }

    pub fn marked(&self, marked_count: usize) {
        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            operation_id = self.operation_id,
            marked_count,
            elapsed_ms = self.requested_at.elapsed().as_millis() as u64,
            "TRASH_PERF phase=db_marked"
        );
    }

    pub fn operation_id(self) -> u64 {
        self.operation_id
    }

    fn elapsed_ms(self) -> u64 {
        self.requested_at.elapsed().as_millis() as u64
    }
}

pub fn move_marked_items_with_fallback<W, F>(
    parent: &W,
    pool: DbPool,
    db_actor: DbActorHandle,
    items: Vec<MediaItem>,
    trace: TrashMoveTrace,
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
        let filesystem_wait_started = Instant::now();
        let operation_id = trace.operation_id();
        let move_result = gtk::gio::spawn_blocking(move || {
            let span = tracing::info_span!(
                target: crate::core::log_targets::ALBUMS,
                "trash:filesystem_batch",
                operation_id,
                backend = ?backend,
                item_count = items_for_worker.len(),
            );
            let _entered = span.enter();
            move_items_to_backend(items_for_worker, backend, operation_id)
        })
        .await;

        let Ok(batch) = move_result else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                operation_id,
                elapsed_ms = filesystem_wait_started.elapsed().as_millis() as u64,
                "TRASH_PERF phase=filesystem_worker_failed"
            );
            rollback_items(&db_actor, &items).await;
            show_error_dialog(&parent, &tr("trash.move_failed"));
            return;
        };

        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            operation_id,
            backend = ?backend,
            moved_count = batch.moved.len(),
            failed_count = batch.failed.len(),
            elapsed_ms = filesystem_wait_started.elapsed().as_millis() as u64,
            total_elapsed_ms = trace.elapsed_ms(),
            "TRASH_PERF phase=filesystem_complete"
        );

        commit_moved(&db_actor, batch.moved.clone(), trace).await;
        let moved_ids = ids_for_items(&batch.moved);
        if !moved_ids.is_empty() {
            let callback_started = Instant::now();
            on_moved(moved_ids);
            tracing::info!(
                target: crate::core::log_targets::ALBUMS,
                operation_id,
                elapsed_ms = callback_started.elapsed().as_millis() as u64,
                total_elapsed_ms = trace.elapsed_ms(),
                "TRASH_PERF phase=ui_callback_complete"
            );
        }

        if batch.failed.is_empty() {
            tracing::info!(
                target: crate::core::log_targets::ALBUMS,
                operation_id,
                total_elapsed_ms = trace.elapsed_ms(),
                "TRASH_PERF phase=complete"
            );
            return;
        }

        if backend == TrashBackend::System {
            prompt_switch_to_app_trash(parent, pool, db_actor, batch.failed, trace, on_moved);
        } else {
            rollback_items(&db_actor, &batch.failed).await;
            show_error_dialog(&parent, &tr("trash.move_failed"));
        }
    });
}

fn move_items_to_backend(
    items: Vec<MediaItem>,
    backend: TrashBackend,
    operation_id: u64,
) -> MoveBatch {
    let mut moved = Vec::new();
    let mut failed = Vec::new();
    for item in items {
        let span = tracing::info_span!(
            target: crate::core::log_targets::ALBUMS,
            "trash:filesystem_item",
            operation_id,
            media_id = item.id,
            bytes = item.file_size,
            backend = ?backend,
            outcome = tracing::field::Empty,
        );
        let _entered = span.enter();
        match trash::move_to_backend(&item.uri, backend) {
            Ok(()) => {
                span.record("outcome", "moved");
                moved.push(item);
            }
            Err(err) => {
                span.record("outcome", "failed");
                tracing::warn!(
                    "failed to move {} to {backend:?} trash backend: {err}",
                    item.uri,
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
    trace: TrashMoveTrace,
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
        let trace = trace;
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
                    commit_moved(&db_actor, moved.clone(), trace).await;
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

async fn commit_moved(db_actor: &DbActorHandle, items: Vec<MediaItem>, trace: TrashMoveTrace) {
    if !items.is_empty() {
        let started = Instant::now();
        let result = db_actor
            .execute(DbCommand::CommitMovedToTrash {
                items,
                trace_id: Some(trace.operation_id()),
            })
            .await;
        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            operation_id = trace.operation_id(),
            success = result.is_ok(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            total_elapsed_ms = trace.elapsed_ms(),
            "TRASH_PERF phase=db_commit_complete"
        );
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
