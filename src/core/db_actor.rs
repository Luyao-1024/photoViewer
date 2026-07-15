use crate::core::backend::local::LocalBackend;
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::events::{ChangeSource, DomainEvent, DomainEventSender, MediaFields};
use crate::core::identity::MediaId;
use crate::core::media::{MediaItem, NewMediaItem};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;
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
    DeleteLiveByFolder {
        source: ChangeSource,
        folder_path: PathBuf,
    },
    PruneMissingLiveRows {
        roots: Vec<PathBuf>,
        excluded_roots: Vec<PathBuf>,
    },
    SetFavorite {
        ids: Vec<MediaId>,
        is_favorite: bool,
    },
    InsertMediaItem {
        item: NewMediaItem,
    },
    UpdateMediaLocation {
        id: MediaId,
        path: PathBuf,
        folder_path: PathBuf,
    },
    InsertEditedMedia {
        item: NewMediaItem,
    },
    UpdateEditedMedia {
        id: MediaId,
        file_mtime: i64,
        file_size: i64,
        blake3_hash: String,
    },
    SetAlbumOrder {
        ordered: Vec<String>,
    },
    SetAlbumCover {
        folder_path: PathBuf,
        cover_uri: String,
    },
    ClearAllMedia,
    DeleteMediaRows {
        ids: Vec<MediaId>,
    },
    MarkThumbnailsGenerated {
        ids: Vec<MediaId>,
    },
    MarkTrashed {
        ids: Vec<MediaId>,
        trace_id: Option<u64>,
    },
    RestoreTrashed {
        ids: Vec<MediaId>,
    },
    DeleteTrashedRows {
        ids: Vec<MediaId>,
    },
    CommitMovedToTrash {
        items: Vec<MediaItem>,
        trace_id: Option<u64>,
    },
    RollbackTrashed {
        ids: Vec<MediaId>,
    },
    RefreshAlbums {
        source: ChangeSource,
    },
    RefreshAlbumsInternal,
    ReconcileTrash {
        pictures_root: PathBuf,
    },
}

/// Runtime DB write urgency. Higher values run first once the actor reaches
/// its queue; an already-running transaction is never interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DbWritePriority {
    Thumbnail = 10,
    DerivedRefresh = 20,
    StartupScan = 30,
    FilesystemWatcher = 70,
    Trash = 90,
    UserInteractive = 100,
}

impl DbCommand {
    pub fn priority(&self) -> DbWritePriority {
        match self {
            Self::UpsertMediaBatch { source, .. }
            | Self::DeleteLiveByPath { source, .. }
            | Self::DeleteLiveByFolder { source, .. } => match source {
                ChangeSource::StartupScan => DbWritePriority::StartupScan,
                ChangeSource::FilesystemWatcher => DbWritePriority::FilesystemWatcher,
                ChangeSource::UserInteractive => DbWritePriority::UserInteractive,
                ChangeSource::TrashReconcile => DbWritePriority::Trash,
                ChangeSource::ThumbnailWorker => DbWritePriority::Thumbnail,
            },
            Self::PruneMissingLiveRows { .. } => DbWritePriority::StartupScan,
            Self::SetFavorite { .. }
            | Self::InsertMediaItem { .. }
            | Self::UpdateMediaLocation { .. }
            | Self::InsertEditedMedia { .. }
            | Self::UpdateEditedMedia { .. }
            | Self::SetAlbumOrder { .. }
            | Self::SetAlbumCover { .. }
            | Self::ClearAllMedia
            | Self::DeleteMediaRows { .. } => DbWritePriority::UserInteractive,
            Self::MarkThumbnailsGenerated { .. } => DbWritePriority::Thumbnail,
            Self::MarkTrashed { .. }
            | Self::RestoreTrashed { .. }
            | Self::DeleteTrashedRows { .. } => DbWritePriority::Trash,
            Self::CommitMovedToTrash { .. } | Self::RollbackTrashed { .. } => {
                DbWritePriority::Trash
            }
            Self::RefreshAlbums { .. } => DbWritePriority::DerivedRefresh,
            Self::RefreshAlbumsInternal => DbWritePriority::DerivedRefresh,
            Self::ReconcileTrash { .. } => DbWritePriority::Trash,
        }
    }
}

#[derive(Debug)]
pub enum DbCommandResult {
    None,
    Count(usize),
    MediaItems(Vec<MediaItem>),
    RemovedUris(Vec<String>),
}

struct DbEnvelope {
    command: DbCommand,
    reply: oneshot::Sender<Result<DbCommandResult>>,
    enqueued_at: Instant,
}

struct QueuedEnvelope {
    priority: DbWritePriority,
    sequence: u64,
    envelope: DbEnvelope,
}

impl PartialEq for QueuedEnvelope {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl Eq for QueuedEnvelope {}

impl Ord for QueuedEnvelope {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for QueuedEnvelope {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone)]
pub struct DbActorHandle {
    tx: mpsc::Sender<DbEnvelope>,
}

impl DbActorHandle {
    pub async fn execute(&self, command: DbCommand) -> Result<DbCommandResult> {
        let (reply, rx) = oneshot::channel();
        self.send_envelope(command, reply)
            .map_err(|err| AppError::Backend(format!("db actor stopped: {err}")))?;
        rx.await
            .map_err(|err| AppError::Backend(format!("db actor dropped response: {err}")))?
    }

    pub fn execute_blocking(&self, command: DbCommand) -> Result<DbCommandResult> {
        let (reply, rx) = oneshot::channel();
        self.send_envelope(command, reply)
            .map_err(|err| AppError::Backend(format!("db actor stopped: {err}")))?;
        rx.blocking_recv()
            .map_err(|err| AppError::Backend(format!("db actor dropped response: {err}")))?
    }

    pub fn enqueue(&self, command: DbCommand) -> Result<()> {
        let (reply, _rx) = oneshot::channel();
        self.send_envelope(command, reply)
            .map_err(|err| AppError::Backend(format!("db actor stopped: {err}")))
    }

    fn send_envelope(
        &self,
        command: DbCommand,
        reply: oneshot::Sender<Result<DbCommandResult>>,
    ) -> std::result::Result<(), Box<mpsc::SendError<DbEnvelope>>> {
        self.tx
            .send(DbEnvelope {
                command,
                reply,
                enqueued_at: Instant::now(),
            })
            .map_err(Box::new)
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
    let mut queue = BinaryHeap::new();
    let mut sequence = 0_u64;
    loop {
        if queue.is_empty() {
            let Ok(envelope) = rx.recv() else { break };
            queue.push(QueuedEnvelope {
                priority: envelope.command.priority(),
                sequence,
                envelope,
            });
            sequence = sequence.wrapping_add(1);
        }
        while let Ok(envelope) = rx.try_recv() {
            queue.push(QueuedEnvelope {
                priority: envelope.command.priority(),
                sequence,
                envelope,
            });
            sequence = sequence.wrapping_add(1);
        }
        let QueuedEnvelope { envelope, .. } = queue.pop().expect("queue is not empty");
        let command_name = db_command_name(&envelope.command);
        let started = Instant::now();
        let is_trash_command = is_trash_command(&envelope.command);
        let trash_item_count = trash_command_item_count(&envelope.command);
        let trash_trace_id = trash_command_trace_id(&envelope.command);
        let queue_wait_ms = envelope.enqueued_at.elapsed().as_millis() as u64;
        let trash_span = is_trash_command.then(|| {
            tracing::info_span!(
                target: crate::core::log_targets::ALBUMS,
                "trash:db_actor_command",
                command = command_name,
                item_count = trash_item_count,
                queue_wait_ms,
                operation_id = ?trash_trace_id,
            )
        });
        let _trash_entered = trash_span.as_ref().map(tracing::Span::enter);
        tracing::trace!(
            target: crate::core::log_targets::STORAGE,
            "DB_ACTOR_FLOW phase=begin command={}",
            command_name
        );
        let result = execute_command(&pool, &events, envelope.command);
        tracing::trace!(
            target: crate::core::log_targets::STORAGE,
            "DB_ACTOR_FLOW phase=end command={} success={} elapsed_ms={}",
            command_name,
            result.is_ok(),
            started.elapsed().as_millis()
        );
        let _ = envelope.reply.send(result);
    }
}

fn is_trash_command(command: &DbCommand) -> bool {
    matches!(
        command,
        DbCommand::MarkTrashed { .. }
            | DbCommand::RestoreTrashed { .. }
            | DbCommand::DeleteTrashedRows { .. }
            | DbCommand::CommitMovedToTrash { .. }
            | DbCommand::RollbackTrashed { .. }
            | DbCommand::ReconcileTrash { .. }
    )
}

fn trash_command_item_count(command: &DbCommand) -> usize {
    match command {
        DbCommand::MarkTrashed { ids, .. }
        | DbCommand::RestoreTrashed { ids }
        | DbCommand::DeleteTrashedRows { ids }
        | DbCommand::RollbackTrashed { ids } => ids.len(),
        DbCommand::CommitMovedToTrash { items, .. } => items.len(),
        DbCommand::ReconcileTrash { .. } => 0,
        _ => 0,
    }
}

fn trash_command_trace_id(command: &DbCommand) -> Option<u64> {
    match command {
        DbCommand::MarkTrashed { trace_id, .. }
        | DbCommand::CommitMovedToTrash { trace_id, .. } => *trace_id,
        _ => None,
    }
}

fn db_command_name(command: &DbCommand) -> &'static str {
    match command {
        DbCommand::UpsertMediaBatch { .. } => "upsert_media_batch",
        DbCommand::DeleteLiveByPath { .. } => "delete_live_by_path",
        DbCommand::DeleteLiveByFolder { .. } => "delete_live_by_folder",
        DbCommand::PruneMissingLiveRows { .. } => "prune_missing_live_rows",
        DbCommand::SetFavorite { .. } => "set_favorite",
        DbCommand::InsertMediaItem { .. } => "insert_media_item",
        DbCommand::UpdateMediaLocation { .. } => "update_media_location",
        DbCommand::InsertEditedMedia { .. } => "insert_edited_media",
        DbCommand::UpdateEditedMedia { .. } => "update_edited_media",
        DbCommand::SetAlbumOrder { .. } => "set_album_order",
        DbCommand::SetAlbumCover { .. } => "set_album_cover",
        DbCommand::ClearAllMedia => "clear_all_media",
        DbCommand::DeleteMediaRows { .. } => "delete_media_rows",
        DbCommand::MarkThumbnailsGenerated { .. } => "mark_thumbnails_generated",
        DbCommand::MarkTrashed { .. } => "mark_trashed",
        DbCommand::RestoreTrashed { .. } => "restore_trashed",
        DbCommand::DeleteTrashedRows { .. } => "delete_trashed_rows",
        DbCommand::CommitMovedToTrash { .. } => "commit_moved_to_trash",
        DbCommand::RollbackTrashed { .. } => "rollback_trashed",
        DbCommand::RefreshAlbums { .. } => "refresh_albums",
        DbCommand::RefreshAlbumsInternal => "refresh_albums_internal",
        DbCommand::ReconcileTrash { .. } => "reconcile_trash",
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
        DbCommand::DeleteLiveByFolder {
            source,
            folder_path,
        } => {
            let changed = db::delete_live_media_by_folder(pool, &folder_path)?;
            if changed > 0 {
                events.send(DomainEvent::AlbumsChanged {
                    source,
                    affected_folders: vec![folder_path],
                    affected_virtual: Vec::new(),
                    live_count_delta: -(changed as i64),
                });
            }
            Ok(DbCommandResult::Count(changed))
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
        DbCommand::InsertMediaItem { item } | DbCommand::InsertEditedMedia { item } => {
            let id = db::insert_media_item(pool, &item)?;
            Ok(DbCommandResult::MediaItems(vec![db::get_media_item(
                pool, id,
            )?]))
        }
        DbCommand::UpdateMediaLocation {
            id,
            path,
            folder_path,
        } => {
            db::update_media_location(pool, id.get(), &path, &folder_path)?;
            Ok(DbCommandResult::MediaItems(vec![db::get_media_item(
                pool,
                id.get(),
            )?]))
        }
        DbCommand::UpdateEditedMedia {
            id,
            file_mtime,
            file_size,
            blake3_hash,
        } => {
            db::update_media_edit_metadata(pool, id.get(), file_mtime, file_size, &blake3_hash)?;
            Ok(DbCommandResult::MediaItems(vec![db::get_media_item(
                pool,
                id.get(),
            )?]))
        }
        DbCommand::SetAlbumOrder { ordered } => {
            crate::core::albums::set_album_order(pool, &ordered)?;
            Ok(DbCommandResult::None)
        }
        DbCommand::SetAlbumCover {
            folder_path,
            cover_uri,
        } => {
            crate::core::albums::set_album_cover(pool, &folder_path, &cover_uri)?;
            events.send(DomainEvent::AlbumCoverChanged {
                folder_path,
                cover_uri,
            });
            Ok(DbCommandResult::None)
        }
        DbCommand::ClearAllMedia => Ok(DbCommandResult::Count(db::clear_all_media(pool)?)),
        DbCommand::DeleteMediaRows { ids } | DbCommand::DeleteTrashedRows { ids } => {
            for id in ids {
                db::delete_media_item(pool, id.get())?;
            }
            Ok(DbCommandResult::None)
        }
        DbCommand::MarkThumbnailsGenerated { ids } => {
            let ids: Vec<i64> = ids.into_iter().map(MediaId::get).collect();
            db::mark_thumbnails_generated(pool, &ids)?;
            Ok(DbCommandResult::None)
        }
        DbCommand::MarkTrashed { ids, trace_id } => {
            let span = tracing::info_span!(
                target: crate::core::log_targets::ALBUMS,
                "trash:db_mark_rows",
                item_count = ids.len(),
                operation_id = ?trace_id,
            );
            let _entered = span.enter();
            let mut changed = Vec::new();
            for id in ids {
                let item = db::get_media_item(pool, id.get())?;
                db::mark_trashed(pool, id.get())?;
                changed.push(item);
            }
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::RestoreTrashed { ids } => {
            let mut changed = Vec::new();
            for id in ids {
                db::unmark_trashed(pool, id.get())?;
                changed.push(db::get_media_item(pool, id.get())?);
            }
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::CommitMovedToTrash { items, .. } => {
            if !items.is_empty() {
                events.send(DomainEvent::MediaMovedToTrash {
                    source: ChangeSource::UserInteractive,
                    items: items.clone(),
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
        DbCommand::RefreshAlbumsInternal => {
            crate::core::albums::refresh(pool)?;
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
