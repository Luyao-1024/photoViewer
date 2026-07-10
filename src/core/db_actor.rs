use crate::core::backend::local::LocalBackend;
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::events::{ChangeSource, DomainEvent, DomainEventSender, MediaFields};
use crate::core::identity::MediaId;
use crate::core::media::{MediaItem, NewMediaItem};
use std::path::PathBuf;
use std::sync::mpsc;
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum DbCommand {
    UpsertMediaBatch {
        source: ChangeSource,
        items: Vec<NewMediaItem>,
    },
    DeleteLiveByPath {
        source: ChangeSource,
        path: PathBuf,
    },
    PruneMissingLiveRows {
        roots: Vec<PathBuf>,
        excluded_roots: Vec<PathBuf>,
    },
    SetFavorite {
        ids: Vec<MediaId>,
        is_favorite: bool,
    },
    MarkTrashed {
        ids: Vec<MediaId>,
    },
    CommitMovedToTrash {
        items: Vec<MediaItem>,
    },
    RollbackTrashed {
        ids: Vec<MediaId>,
    },
    RefreshAlbums {
        source: ChangeSource,
    },
    ReconcileTrash {
        pictures_root: PathBuf,
    },
}

#[derive(Debug)]
pub enum DbCommandResult {
    None,
    MediaItems(Vec<MediaItem>),
    RemovedUris(Vec<String>),
}

struct DbEnvelope {
    command: DbCommand,
    reply: oneshot::Sender<Result<DbCommandResult>>,
}

#[derive(Clone)]
pub struct DbActorHandle {
    tx: mpsc::Sender<DbEnvelope>,
}

impl DbActorHandle {
    pub async fn execute(&self, command: DbCommand) -> Result<DbCommandResult> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbEnvelope { command, reply })
            .map_err(|err| AppError::Backend(format!("db actor stopped: {err}")))?;
        rx.await
            .map_err(|err| AppError::Backend(format!("db actor dropped response: {err}")))?
    }

    pub fn execute_blocking(&self, command: DbCommand) -> Result<DbCommandResult> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbEnvelope { command, reply })
            .map_err(|err| AppError::Backend(format!("db actor stopped: {err}")))?;
        rx.blocking_recv()
            .map_err(|err| AppError::Backend(format!("db actor dropped response: {err}")))?
    }

    pub fn enqueue(&self, command: DbCommand) -> Result<()> {
        let (reply, _rx) = oneshot::channel();
        self.tx
            .send(DbEnvelope { command, reply })
            .map_err(|err| AppError::Backend(format!("db actor stopped: {err}")))
    }
}

pub fn start_db_actor(pool: DbPool, events: DomainEventSender) -> DbActorHandle {
    let (tx, rx) = mpsc::channel::<DbEnvelope>();
    std::thread::Builder::new()
        .name("photo-viewer-db-actor".into())
        .spawn(move || run_db_actor(pool, events, rx))
        .expect("failed to spawn DB actor thread");
    DbActorHandle { tx }
}

fn run_db_actor(pool: DbPool, events: DomainEventSender, rx: mpsc::Receiver<DbEnvelope>) {
    while let Ok(envelope) = rx.recv() {
        let result = execute_command(&pool, &events, envelope.command);
        let _ = envelope.reply.send(result);
    }
}

fn execute_command(
    pool: &DbPool,
    events: &DomainEventSender,
    command: DbCommand,
) -> Result<DbCommandResult> {
    match command {
        DbCommand::UpsertMediaBatch { source, items } => {
            let changed = db::upsert_media_items_batch(pool, &items)?;
            if !changed.is_empty() {
                events.send(DomainEvent::MediaUpserted {
                    source,
                    items: changed.clone(),
                });
                events.send(DomainEvent::AlbumsChanged {
                    source,
                    affected_folders: changed
                        .iter()
                        .map(|item| item.folder_path.clone())
                        .collect(),
                    affected_virtual: Vec::new(),
                    live_count_delta: 0,
                });
            }
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::DeleteLiveByPath { source, path } => {
            let uri = format!("file://{}", path.display());
            let changed = db::delete_media_by_path(pool, &path)?;
            if changed > 0 {
                events.send(DomainEvent::MediaRemoved {
                    source,
                    ids: Vec::new(),
                    uris: vec![uri.clone()],
                });
                events.send(DomainEvent::AlbumsChanged {
                    source,
                    affected_folders: path.parent().map(PathBuf::from).into_iter().collect(),
                    affected_virtual: Vec::new(),
                    live_count_delta: -(changed as i64),
                });
                Ok(DbCommandResult::RemovedUris(vec![uri]))
            } else {
                Ok(DbCommandResult::RemovedUris(Vec::new()))
            }
        }
        DbCommand::PruneMissingLiveRows {
            roots,
            excluded_roots,
        } => {
            let backend = LocalBackend::new(pool.clone());
            let removed = backend.prune_missing_live_media_under_roots(&roots, &excluded_roots)?;
            if !removed.is_empty() {
                events.send(DomainEvent::MediaRemoved {
                    source: ChangeSource::StartupScan,
                    ids: Vec::new(),
                    uris: removed.clone(),
                });
                events.send(DomainEvent::AlbumsChanged {
                    source: ChangeSource::StartupScan,
                    affected_folders: roots,
                    affected_virtual: Vec::new(),
                    live_count_delta: -(removed.len() as i64),
                });
            }
            Ok(DbCommandResult::RemovedUris(removed))
        }
        DbCommand::SetFavorite { ids, is_favorite } => {
            let mut changed = Vec::new();
            for id in ids {
                db::set_media_favorite(pool, id.get(), is_favorite)?;
                changed.push(db::get_media_item(pool, id.get())?);
            }
            if !changed.is_empty() {
                events.send(DomainEvent::MediaUpdated {
                    source: ChangeSource::UserInteractive,
                    items: changed.clone(),
                    fields: MediaFields::FAVORITE,
                });
                events.send(DomainEvent::AlbumsChanged {
                    source: ChangeSource::UserInteractive,
                    affected_folders: Vec::new(),
                    affected_virtual: vec!["favorites".into()],
                    live_count_delta: 0,
                });
            }
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::MarkTrashed { ids } => {
            let mut changed = Vec::new();
            for id in ids {
                let item = db::get_media_item(pool, id.get())?;
                db::mark_trashed(pool, id.get())?;
                changed.push(item);
            }
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::CommitMovedToTrash { items } => {
            if !items.is_empty() {
                crate::core::albums::refresh(pool)?;
                events.send(DomainEvent::MediaMovedToTrash {
                    source: ChangeSource::UserInteractive,
                    items: items.clone(),
                });
                events.send(DomainEvent::AlbumsChanged {
                    source: ChangeSource::UserInteractive,
                    affected_folders: items.iter().map(|item| item.folder_path.clone()).collect(),
                    affected_virtual: Vec::new(),
                    live_count_delta: -(items.len() as i64),
                });
            }
            Ok(DbCommandResult::MediaItems(items))
        }
        DbCommand::RollbackTrashed { ids } => {
            let mut changed = Vec::new();
            for id in ids {
                db::unmark_trashed(pool, id.get())?;
                changed.push(db::get_media_item(pool, id.get())?);
            }
            if !changed.is_empty() {
                events.send(DomainEvent::MediaUpdated {
                    source: ChangeSource::UserInteractive,
                    items: changed.clone(),
                    fields: MediaFields::TRASH,
                });
            }
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::RefreshAlbums { source } => {
            crate::core::albums::refresh(pool)?;
            events.send(DomainEvent::AlbumsChanged {
                source,
                affected_folders: Vec::new(),
                affected_virtual: Vec::new(),
                live_count_delta: 0,
            });
            Ok(DbCommandResult::None)
        }
        DbCommand::ReconcileTrash { pictures_root } => {
            crate::core::trash::reconcile_trash(pool, &pictures_root)?;
            events.send(DomainEvent::TrashChanged {
                source: ChangeSource::TrashReconcile,
            });
            Ok(DbCommandResult::None)
        }
    }
}

#[cfg(test)]
mod tests;
