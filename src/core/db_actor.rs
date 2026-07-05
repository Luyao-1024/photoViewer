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
mod tests {
    use super::*;
    use crate::core::media::MEDIA_SUBKIND_STANDARD;
    use chrono::Utc;

    fn new_item(path: PathBuf) -> NewMediaItem {
        NewMediaItem {
            uri: format!("file://{}", path.display()),
            path: path.clone(),
            folder_path: path.parent().unwrap().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: MEDIA_SUBKIND_STANDARD.into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: String::new(),
        }
    }

    #[tokio::test]
    async fn set_favorite_updates_db_and_emits_precise_event() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
        let id = db::insert_media_item(&pool, &new_item(dir.path().join("a.jpg"))).unwrap();
        let (events, mut rx) = DomainEventSender::new();
        let actor = start_db_actor(pool.clone(), events);

        actor
            .execute(DbCommand::SetFavorite {
                ids: vec![MediaId::from(id)],
                is_favorite: true,
            })
            .await
            .unwrap();

        assert!(db::get_media_item(&pool, id).unwrap().is_favorite);
        match rx.recv().await.unwrap() {
            DomainEvent::MediaUpdated { items, fields, .. } => {
                assert_eq!(items[0].id, id);
                assert!(fields.favorite);
            }
            other => panic!("expected MediaUpdated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_live_by_path_removes_row_and_emits_uri() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.jpg");
        let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
        let uri = format!("file://{}", path.display());
        db::insert_media_item(&pool, &new_item(path.clone())).unwrap();
        let (events, mut rx) = DomainEventSender::new();
        let actor = start_db_actor(pool.clone(), events);

        actor
            .execute(DbCommand::DeleteLiveByPath {
                source: ChangeSource::FilesystemWatcher,
                path,
            })
            .await
            .unwrap();

        assert!(db::list_all_media(&pool).unwrap().is_empty());
        match rx.recv().await.unwrap() {
            DomainEvent::MediaRemoved { uris, .. } => assert_eq!(uris, vec![uri]),
            other => panic!("expected MediaRemoved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn trash_commit_emits_precise_moved_event_after_mark() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
        let id = db::insert_media_item(&pool, &new_item(dir.path().join("a.jpg"))).unwrap();
        let (events, mut rx) = DomainEventSender::new();
        let actor = start_db_actor(pool.clone(), events);

        let prepared = actor
            .execute(DbCommand::MarkTrashed {
                ids: vec![MediaId::from(id)],
            })
            .await
            .unwrap();
        assert!(db::get_media_item(&pool, id).unwrap().trashed_at.is_some());

        let DbCommandResult::MediaItems(items) = prepared else {
            panic!("expected prepared media items");
        };
        actor
            .execute(DbCommand::CommitMovedToTrash {
                items: items.clone(),
            })
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            DomainEvent::MediaMovedToTrash { items, .. } => assert_eq!(items[0].id, id),
            other => panic!("expected MediaMovedToTrash, got {other:?}"),
        }
    }
}
