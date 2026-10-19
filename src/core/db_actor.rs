use crate::core::backend::local::MissingLiveMedia;
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::events::{ChangeSource, DomainEvent, DomainEventSender, MediaFields};
use crate::core::identity::MediaId;
use crate::core::media::{MediaItem, NewMediaItem};
use crate::core::telemetry::{log_error, OperationTrace, TraceChain};
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
    DeleteLiveByPaths {
        source: ChangeSource,
        paths: Vec<PathBuf>,
    },
    DeleteLiveByFolder {
        source: ChangeSource,
        folder_path: PathBuf,
    },
    PruneMissingLiveRows {
        missing: Vec<MissingLiveMedia>,
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
        item: NewMediaItem,
    },
    SetAlbumOrder {
        ordered: Vec<String>,
    },
    SetAlbumCover {
        folder_path: PathBuf,
        cover_uri: String,
    },
    ResetLibraryDatabase,
    ResetAccountData,
    DeleteMediaRows {
        ids: Vec<MediaId>,
    },
    MarkThumbnailsGenerated {
        ids: Vec<MediaId>,
    },
    InvalidateThumbnailMarks {
        ids: Vec<MediaId>,
    },
    ReconcileThumbnailMarks {
        items: Vec<MediaItem>,
        ready: bool,
    },
    MarkTrashed {
        ids: Vec<MediaId>,
    },
    RestoreTrashed {
        ids: Vec<MediaId>,
    },
    DeleteTrashedRows {
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
    RefreshAlbumsInternal,
    ReconcileTrash {
        plan: crate::core::trash::TrashReconcilePlan,
    },
    Sync(crate::core::sync::store::SyncWrite),
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
            | Self::DeleteLiveByPaths { source, .. }
            | Self::DeleteLiveByFolder { source, .. } => match source {
                ChangeSource::StartupScan => DbWritePriority::StartupScan,
                ChangeSource::FilesystemWatcher => DbWritePriority::FilesystemWatcher,
                ChangeSource::UserInteractive => DbWritePriority::UserInteractive,
                ChangeSource::TrashReconcile => DbWritePriority::Trash,
                ChangeSource::ThumbnailWorker => DbWritePriority::Thumbnail,
                ChangeSource::Synchronization => DbWritePriority::FilesystemWatcher,
            },
            Self::PruneMissingLiveRows { .. } => DbWritePriority::StartupScan,
            Self::SetFavorite { .. }
            | Self::InsertMediaItem { .. }
            | Self::UpdateMediaLocation { .. }
            | Self::InsertEditedMedia { .. }
            | Self::UpdateEditedMedia { .. }
            | Self::SetAlbumOrder { .. }
            | Self::SetAlbumCover { .. }
            | Self::ResetLibraryDatabase
            | Self::ResetAccountData
            | Self::DeleteMediaRows { .. } => DbWritePriority::UserInteractive,
            Self::MarkThumbnailsGenerated { .. }
            | Self::InvalidateThumbnailMarks { .. }
            | Self::ReconcileThumbnailMarks { .. } => DbWritePriority::Thumbnail,
            Self::MarkTrashed { .. }
            | Self::RestoreTrashed { .. }
            | Self::DeleteTrashedRows { .. } => DbWritePriority::Trash,
            Self::CommitMovedToTrash { .. } | Self::RollbackTrashed { .. } => {
                DbWritePriority::Trash
            }
            Self::RefreshAlbums { .. } => DbWritePriority::DerivedRefresh,
            Self::RefreshAlbumsInternal => DbWritePriority::DerivedRefresh,
            Self::ReconcileTrash { .. } => DbWritePriority::Trash,
            Self::Sync(
                crate::core::sync::store::SyncWrite::ResumeLocalChanges
                | crate::core::sync::store::SyncWrite::BeginReset { .. }
                | crate::core::sync::store::SyncWrite::FinishReset { .. }
                | crate::core::sync::store::SyncWrite::FailReset { .. }
                | crate::core::sync::store::SyncWrite::CreateJob(_)
                | crate::core::sync::store::SyncWrite::QueueEdit { .. }
                | crate::core::sync::store::SyncWrite::ApplyChange { .. },
            )
            | Self::Sync(crate::core::sync::store::SyncWrite::SetJobPaused { .. })
            | Self::Sync(crate::core::sync::store::SyncWrite::DeleteJob { .. })
            | Self::Sync(crate::core::sync::store::SyncWrite::SetUploadAlbums { .. }) => {
                DbWritePriority::UserInteractive
            }
            // The remaining sync writes are engine bookkeeping rather than user intent,
            // so they run below a filesystem-watcher burst. Listed exhaustively on purpose:
            // a catch-all `Self::Sync(_)` would silently demote any variant added later,
            // with no compiler error. Adding a `SyncWrite` variant now fails to compile
            // here, which is the point — classify it deliberately.
            Self::Sync(
                crate::core::sync::store::SyncWrite::FinishRun { .. }
                | crate::core::sync::store::SyncWrite::ReconcileInterruptedRuns
                | crate::core::sync::store::SyncWrite::FailPendingEdit { .. }
                | crate::core::sync::store::SyncWrite::MarkJobStarted { .. }
                | crate::core::sync::store::SyncWrite::MarkJobCompleted { .. }
                | crate::core::sync::store::SyncWrite::MarkJobFailed { .. }
                | crate::core::sync::store::SyncWrite::NativeReceive(_)
                | crate::core::sync::store::SyncWrite::Dirty(_)
                | crate::core::sync::store::SyncWrite::Publication(_)
                | crate::core::sync::store::SyncWrite::Runtime(_),
            ) => DbWritePriority::StartupScan,
        }
    }
}

#[derive(Debug)]
pub enum DbCommandResult {
    None,
    Count(usize),
    MediaItems(Vec<MediaItem>),
    RemovedUris(Vec<String>),
    Sync(crate::core::sync::store::SyncWriteResult),
}

struct DbEnvelope {
    command: DbCommand,
    reply: DbReply,
    enqueued_at: Instant,
    /// The caller's operation (for example a user mutation or scan).
    trace: OperationTrace,
    /// A database-only projection of a non-database caller trace. This keeps
    /// `-T database` useful even when the caller selected another chain.
    database_trace: Option<OperationTrace>,
}

enum DbReply {
    Async(oneshot::Sender<Result<DbCommandResult>>),
    Blocking(mpsc::SyncSender<Result<DbCommandResult>>),
}

impl DbReply {
    fn send(self, result: Result<DbCommandResult>) {
        match self {
            Self::Async(reply) => {
                let _ = reply.send(result);
            }
            Self::Blocking(reply) => {
                let _ = reply.send(result);
            }
        }
    }
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
    pool: DbPool,
    /// Clone of the sender the actor thread owns. Producers that do not run a
    /// database command (the startup scan, for example) still need to publish
    /// domain events, and they must not reach into the actor's queue for it.
    events: DomainEventSender,
}

impl DbActorHandle {
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Publish a domain event without going through a database command. Safe
    /// off the main thread; see `DomainEventSender::send` for the backpressure
    /// contract.
    pub fn emit(&self, event: DomainEvent) {
        self.events.send(event);
    }

    pub async fn execute(&self, command: DbCommand) -> Result<DbCommandResult> {
        let trace = OperationTrace::start(TraceChain::Database, db_command_name(&command));
        self.execute_with_traces(trace, None, command).await
    }

    /// Submit a command as one stage of an already-running operation trace.
    ///
    /// Use this for a cross-thread workflow such as edit, move, import, or
    /// restore. Calls that do not have a parent flow should use [`Self::execute`]
    /// and receive their own `database` operation automatically.
    pub async fn execute_in_trace(
        &self,
        trace: OperationTrace,
        command: DbCommand,
    ) -> Result<DbCommandResult> {
        let database_trace = (trace.chain() != TraceChain::Database)
            .then(|| OperationTrace::start(TraceChain::Database, db_command_name(&command)));
        self.execute_with_traces(trace, database_trace, command)
            .await
    }

    async fn execute_with_traces(
        &self,
        trace: OperationTrace,
        database_trace: Option<OperationTrace>,
        command: DbCommand,
    ) -> Result<DbCommandResult> {
        let (reply, rx) = oneshot::channel();
        if let Err(err) = self.send_envelope(
            command,
            DbReply::Async(reply),
            trace.clone(),
            database_trace.clone(),
        ) {
            let error = AppError::Backend(format!("db actor stopped: {err}"));
            log_error(&trace, "enqueue", &error);
            if let Some(database_trace) = &database_trace {
                log_error(database_trace, "enqueue", &error);
            }
            return Err(error);
        }
        match rx.await {
            Ok(result) => result,
            Err(err) => {
                let error = AppError::Backend(format!("db actor dropped response: {err}"));
                log_error(&trace, "response", &error);
                if let Some(database_trace) = &database_trace {
                    log_error(database_trace, "response", &error);
                }
                Err(error)
            }
        }
    }

    pub fn execute_blocking(&self, command: DbCommand) -> Result<DbCommandResult> {
        let trace = OperationTrace::start(TraceChain::Database, db_command_name(&command));
        self.execute_blocking_with_traces(trace, None, command)
    }

    pub fn execute_blocking_in_trace(
        &self,
        trace: OperationTrace,
        command: DbCommand,
    ) -> Result<DbCommandResult> {
        let database_trace = (trace.chain() != TraceChain::Database)
            .then(|| OperationTrace::start(TraceChain::Database, db_command_name(&command)));
        self.execute_blocking_with_traces(trace, database_trace, command)
    }

    fn execute_blocking_with_traces(
        &self,
        trace: OperationTrace,
        database_trace: Option<OperationTrace>,
        command: DbCommand,
    ) -> Result<DbCommandResult> {
        let (reply, rx) = mpsc::sync_channel(1);
        if let Err(err) = self.send_envelope(
            command,
            DbReply::Blocking(reply),
            trace.clone(),
            database_trace.clone(),
        ) {
            let error = AppError::Backend(format!("db actor stopped: {err}"));
            log_error(&trace, "enqueue", &error);
            if let Some(database_trace) = &database_trace {
                log_error(database_trace, "enqueue", &error);
            }
            return Err(error);
        }
        match rx.recv() {
            Ok(result) => result,
            Err(err) => {
                let error = AppError::Backend(format!("db actor dropped response: {err}"));
                log_error(&trace, "response", &error);
                if let Some(database_trace) = &database_trace {
                    log_error(database_trace, "response", &error);
                }
                Err(error)
            }
        }
    }

    pub fn enqueue(&self, command: DbCommand) -> Result<()> {
        let (reply, _rx) = oneshot::channel();
        let trace = OperationTrace::start(TraceChain::Database, db_command_name(&command));
        if let Err(err) = self.send_envelope(command, DbReply::Async(reply), trace.clone(), None) {
            let error = AppError::Backend(format!("db actor stopped: {err}"));
            log_error(&trace, "enqueue", &error);
            return Err(error);
        }
        Ok(())
    }

    fn send_envelope(
        &self,
        command: DbCommand,
        reply: DbReply,
        trace: OperationTrace,
        database_trace: Option<OperationTrace>,
    ) -> std::result::Result<(), Box<mpsc::SendError<DbEnvelope>>> {
        self.tx
            .send(DbEnvelope {
                command,
                reply,
                enqueued_at: Instant::now(),
                trace,
                database_trace,
            })
            .map_err(Box::new)
    }
}

pub fn start_db_actor(pool: DbPool, events: DomainEventSender) -> DbActorHandle {
    let (tx, rx) = mpsc::channel::<DbEnvelope>();
    let handle_pool = pool.clone();
    let handle_events = events.clone();
    std::thread::Builder::new()
        .name("photo-viewer-db-actor".into())
        .spawn(move || run_db_actor(pool, events, rx))
        .unwrap_or_else(|error| {
            tracing::error!(
                target: crate::core::log_targets::STORAGE,
                error = %error,
                "database actor thread failed to start"
            );
            panic!("failed to spawn DB actor thread: {error}");
        });
    DbActorHandle {
        tx,
        pool: handle_pool,
        events: handle_events,
    }
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
        let queue_wait_ms = envelope.enqueued_at.elapsed().as_millis() as u64;
        let item_count = db_command_item_count(&envelope.command);
        let stage = envelope.trace.stage("db_actor_execute");
        let database_stage = envelope
            .database_trace
            .as_ref()
            .map(|trace| trace.stage("db_actor_execute"));
        for stage in std::iter::once(&stage).chain(database_stage.iter()) {
            stage.record("detail", command_name);
            stage.record("item_count", item_count);
            stage.record("queue_wait_ms", queue_wait_ms);
        }
        let result = execute_command(&pool, &events, envelope.command);
        if let Err(error) = &result {
            log_error(&envelope.trace, "db_actor_execute", error);
            if let Some(database_trace) = &envelope.database_trace {
                log_error(database_trace, "db_actor_execute", error);
            }
        }
        envelope.reply.send(result);
    }
}

fn db_command_item_count(command: &DbCommand) -> usize {
    match command {
        DbCommand::UpsertMediaBatch { items, .. } => items.len(),
        DbCommand::SetFavorite { ids, .. }
        | DbCommand::DeleteMediaRows { ids }
        | DbCommand::MarkThumbnailsGenerated { ids }
        | DbCommand::InvalidateThumbnailMarks { ids }
        | DbCommand::MarkTrashed { ids }
        | DbCommand::RestoreTrashed { ids }
        | DbCommand::DeleteTrashedRows { ids }
        | DbCommand::RollbackTrashed { ids } => ids.len(),
        DbCommand::CommitMovedToTrash { items }
        | DbCommand::ReconcileThumbnailMarks { items, .. } => items.len(),
        _ => 0,
    }
}

fn db_command_name(command: &DbCommand) -> &'static str {
    match command {
        DbCommand::UpsertMediaBatch { .. } => "upsert_media_batch",
        DbCommand::DeleteLiveByPath { .. } => "delete_live_by_path",
        DbCommand::DeleteLiveByPaths { .. } => "delete_live_by_paths",
        DbCommand::DeleteLiveByFolder { .. } => "delete_live_by_folder",
        DbCommand::PruneMissingLiveRows { .. } => "prune_missing_live_rows",
        DbCommand::SetFavorite { .. } => "set_favorite",
        DbCommand::InsertMediaItem { .. } => "insert_media_item",
        DbCommand::UpdateMediaLocation { .. } => "update_media_location",
        DbCommand::InsertEditedMedia { .. } => "insert_edited_media",
        DbCommand::UpdateEditedMedia { .. } => "update_edited_media",
        DbCommand::SetAlbumOrder { .. } => "set_album_order",
        DbCommand::SetAlbumCover { .. } => "set_album_cover",
        DbCommand::ResetLibraryDatabase => "reset_library_database",
        DbCommand::ResetAccountData => "reset_account_data",
        DbCommand::DeleteMediaRows { .. } => "delete_media_rows",
        DbCommand::MarkThumbnailsGenerated { .. } => "mark_thumbnails_generated",
        DbCommand::InvalidateThumbnailMarks { .. } => "invalidate_thumbnail_marks",
        DbCommand::ReconcileThumbnailMarks { .. } => "reconcile_thumbnail_marks",
        DbCommand::MarkTrashed { .. } => "mark_trashed",
        DbCommand::RestoreTrashed { .. } => "restore_trashed",
        DbCommand::DeleteTrashedRows { .. } => "delete_trashed_rows",
        DbCommand::CommitMovedToTrash { .. } => "commit_moved_to_trash",
        DbCommand::RollbackTrashed { .. } => "rollback_trashed",
        DbCommand::RefreshAlbums { .. } => "refresh_albums",
        DbCommand::RefreshAlbumsInternal => "refresh_albums_internal",
        DbCommand::ReconcileTrash { .. } => "reconcile_trash",
        DbCommand::Sync(_) => "sync_state_write",
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
            let uri = crate::core::file_uri::from_path(&path);
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
        DbCommand::DeleteLiveByPaths { source, paths } => {
            let removed = db::delete_live_media_by_paths(
                pool,
                &paths,
                db::DeletionEvidence::ParentMustSurvive,
            )?;
            if !removed.is_empty() {
                events.send(DomainEvent::MediaRemoved {
                    source,
                    ids: Vec::new(),
                    uris: removed.clone(),
                });
                events.send(DomainEvent::AlbumsChanged {
                    source,
                    affected_folders: paths
                        .iter()
                        .filter_map(|path| path.parent().map(PathBuf::from))
                        .collect(),
                    affected_virtual: Vec::new(),
                    live_count_delta: -(removed.len() as i64),
                });
            }
            Ok(DbCommandResult::RemovedUris(removed))
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
        DbCommand::PruneMissingLiveRows { missing } => {
            let by_id: std::collections::HashMap<i64, MissingLiveMedia> = missing
                .into_iter()
                .map(|item| (item.id.get(), item))
                .collect();
            let ids: Vec<i64> = by_id.keys().copied().collect();
            let removed_ids =
                db::delete_media_by_ids_returning(pool, &ids, db::DeletionEvidence::PruneVerified)?;
            let removed: Vec<String> = removed_ids
                .iter()
                .filter_map(|id| by_id.get(id).map(|item| item.uri.clone()))
                .collect();
            if !removed.is_empty() {
                events.send(DomainEvent::MediaRemoved {
                    source: ChangeSource::StartupScan,
                    ids: Vec::new(),
                    uris: removed.clone(),
                });
                events.send(DomainEvent::AlbumsChanged {
                    source: ChangeSource::StartupScan,
                    affected_folders: removed_ids
                        .iter()
                        .filter_map(|id| by_id.get(id).map(|item| item.folder_path.clone()))
                        .collect(),
                    affected_virtual: Vec::new(),
                    live_count_delta: -(removed.len() as i64),
                });
            }
            Ok(DbCommandResult::RemovedUris(removed))
        }
        DbCommand::SetFavorite { ids, is_favorite } => {
            let mut changed = Vec::new();
            // The user's favorite toggle and the synchronization obligation it
            // creates are written in one transaction. Splitting them would let
            // a crash leave a changed photo that the cloud never learns about,
            // or an obligation for a change that never happened.
            let mut conn = pool.get()?;
            let stamp = crate::core::sync::store::EditEventStamp {
                edited_at_ms: crate::core::sync::device::now_millis(),
                source: crate::core::sync::device::device_id(pool),
                event_id: crate::core::sync::device::new_event_id(),
            };
            let tx = conn.transaction()?;
            for id in &ids {
                db::set_media_favorite(&tx, id.get(), is_favorite)?;
                crate::core::sync::store::note_local_change_in(
                    &tx,
                    id.get(),
                    crate::core::sync::store::ChangeCategory::Metadata,
                    &stamp,
                )?;
            }
            tx.commit()?;
            for id in &ids {
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
        DbCommand::InsertEditedMedia { item } => {
            let changed = db::upsert_media_items_batch(pool, &[item])?;
            events.send(DomainEvent::MediaUpserted {
                source: ChangeSource::UserInteractive,
                items: changed.clone(),
            });
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::InsertMediaItem { item } => {
            let id = db::insert_media_item(pool, &item)?;
            let changed = vec![db::get_media_item(pool, id)?];
            events.send(DomainEvent::MediaUpserted {
                source: ChangeSource::UserInteractive,
                items: changed.clone(),
            });
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::UpdateMediaLocation {
            id,
            path,
            folder_path,
        } => {
            db::update_media_location(pool, id.get(), &path, &folder_path)?;
            let changed = vec![db::get_media_item(pool, id.get())?];
            events.send(DomainEvent::MediaUpdated {
                source: ChangeSource::UserInteractive,
                items: changed.clone(),
                fields: MediaFields::LOCATION,
            });
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::UpdateEditedMedia { id, item } => {
            db::update_media_edit_metadata(pool, id.get(), &item)?;
            let changed = vec![db::get_media_item(pool, id.get())?];
            events.send(DomainEvent::MediaUpdated {
                source: ChangeSource::UserInteractive,
                items: changed.clone(),
                fields: MediaFields {
                    metadata: true,
                    thumbnail: true,
                    attributes: true,
                    ..MediaFields::LOCATION
                },
            });
            events.send(DomainEvent::AlbumsDirty {
                source: ChangeSource::UserInteractive,
            });
            Ok(DbCommandResult::MediaItems(changed))
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
        DbCommand::ResetLibraryDatabase => {
            Ok(DbCommandResult::Count(db::reset_library_database(pool)?))
        }
        DbCommand::ResetAccountData => Ok(DbCommandResult::Count(db::reset_account_data(pool)?)),
        DbCommand::DeleteTrashedRows { ids } => {
            db::commit_permanent_deletions(
                pool,
                &ids.into_iter().map(MediaId::get).collect::<Vec<_>>(),
            )?;
            events.send(DomainEvent::TrashChanged {
                source: ChangeSource::UserInteractive,
            });
            Ok(DbCommandResult::None)
        }
        DbCommand::DeleteMediaRows { ids } => {
            for id in ids {
                db::delete_media_item(pool, id.get())?;
            }
            Ok(DbCommandResult::None)
        }
        DbCommand::InvalidateThumbnailMarks { ids } => {
            db::invalidate_thumbnail_marks(
                pool,
                &ids.into_iter().map(MediaId::get).collect::<Vec<_>>(),
            )?;
            Ok(DbCommandResult::None)
        }
        DbCommand::ReconcileThumbnailMarks { items, ready } => {
            db::reconcile_thumbnail_marks(pool, &items, ready)?;
            Ok(DbCommandResult::None)
        }
        DbCommand::MarkThumbnailsGenerated { ids } => {
            let ids: Vec<i64> = ids.into_iter().map(MediaId::get).collect();
            db::mark_thumbnails_generated(pool, &ids)?;
            Ok(DbCommandResult::None)
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
        DbCommand::RestoreTrashed { ids } => {
            let raw = ids.iter().map(|id| id.get()).collect::<Vec<_>>();
            db::restore_trashed_metadata(pool, &raw)?;
            let changed = raw
                .into_iter()
                .map(|id| db::get_media_item(pool, id))
                .collect::<Result<Vec<_>>>()?;
            events.send(DomainEvent::MediaRestored {
                source: ChangeSource::UserInteractive,
                items: changed.clone(),
            });
            events.send(DomainEvent::TrashChanged {
                source: ChangeSource::UserInteractive,
            });
            Ok(DbCommandResult::MediaItems(changed))
        }
        DbCommand::CommitMovedToTrash { items } => {
            db::commit_trash_metadata(pool, &items.iter().map(|item| item.id).collect::<Vec<_>>())?;
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
        DbCommand::ReconcileTrash { plan } => {
            crate::core::trash::commit_trash_reconcile(pool, plan)?;
            events.send(DomainEvent::TrashChanged {
                source: ChangeSource::TrashReconcile,
            });
            Ok(DbCommandResult::None)
        }
        DbCommand::Sync(command) => {
            let mut changed_metadata = Vec::new();
            if let crate::core::sync::store::SyncWrite::NativeReceive(receive) = &command {
                if let crate::core::sync::store::NativeReceiveCommand::Promote {
                    job_id,
                    graph,
                    ..
                }
                | crate::core::sync::store::NativeReceiveCommand::PromoteDelta {
                    job_id,
                    graph,
                    ..
                } = receive.as_ref()
                {
                    let conn = pool.get()?;
                    for photo in &graph.photos {
                        let row=conn.query_row("SELECT m.id FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.job_id=?1 AND e.native_photo_id=?2 AND (m.is_favorite<>?3 OR m.taken_at IS NOT ?4)",rusqlite::params![job_id,photo.photo_id,photo.is_favorite,photo.asset_date_ms.map(|ms|ms.div_euclid(1000))],|r|r.get::<_,i64>(0));
                        if let Ok(id) = row {
                            changed_metadata.push(id);
                        }
                    }
                }
            }
            let admitted_uri = match &command {
                crate::core::sync::store::SyncWrite::Runtime(command) => match command.as_ref() {
                    crate::core::sync::store::runtime::Command::Admit { item, .. } => {
                        let visible = db::get_media_item_by_uri(pool, &item.uri)?
                            .is_some_and(|old| old.can_display());
                        Some((item.uri.clone(), visible))
                    }
                    crate::core::sync::store::runtime::Command::ThumbnailReady {
                        media, ..
                    } => db::get_media_item(pool, *media)
                        .ok()
                        .map(|item| (item.uri.clone(), item.can_display())),
                    _ => None,
                },
                _ => None,
            };
            let cloud_state_changed = matches!(
                &command,
                crate::core::sync::store::SyncWrite::BeginReset { .. }
                    | crate::core::sync::store::SyncWrite::FinishReset { .. }
                    | crate::core::sync::store::SyncWrite::FailReset { .. }
                    | crate::core::sync::store::SyncWrite::CreateJob(_)
                    | crate::core::sync::store::SyncWrite::DeleteJob { .. }
                    | crate::core::sync::store::SyncWrite::SetUploadAlbums { .. }
                    | crate::core::sync::store::SyncWrite::SetJobPaused { .. }
                    | crate::core::sync::store::SyncWrite::QueueEdit { .. }
                    | crate::core::sync::store::SyncWrite::ApplyChange { .. }
                    | crate::core::sync::store::SyncWrite::FailPendingEdit { .. }
                    | crate::core::sync::store::SyncWrite::MarkJobCompleted { .. }
                    | crate::core::sync::store::SyncWrite::MarkJobFailed { .. }
            );
            let cloud_state_changed = cloud_state_changed
                || matches!(&command,
                crate::core::sync::store::SyncWrite::Runtime(command) if matches!(command.as_ref(), crate::core::sync::store::runtime::Command::SelectAccount { .. } | crate::core::sync::store::runtime::Command::ClearAccount));
            let lifecycle_changed = match &command {
                crate::core::sync::store::SyncWrite::MarkJobStarted { .. }
                | crate::core::sync::store::SyncWrite::FinishRun { .. } => true,
                crate::core::sync::store::SyncWrite::Runtime(command) => matches!(
                    command.as_ref(),
                    crate::core::sync::store::runtime::Command::UserAction { .. }
                        | crate::core::sync::store::runtime::Command::Confirm { .. }
                ),
                _ => false,
            };
            let visibility_changed = matches!(
                &command,
                crate::core::sync::store::SyncWrite::DeleteJob { .. }
                    | crate::core::sync::store::SyncWrite::FinishReset { .. }
            ) || matches!(&command,crate::core::sync::store::SyncWrite::Runtime(c)
                    if matches!(c.as_ref(),crate::core::sync::store::runtime::Command::SelectAccount {job:Some(_)} | crate::core::sync::store::runtime::Command::ClearAccount));
            let configuration_only = matches!(
                &command,
                crate::core::sync::store::SyncWrite::SetJobPaused { .. }
                    | crate::core::sync::store::SyncWrite::SetUploadAlbums { .. }
                    | crate::core::sync::store::SyncWrite::QueueEdit { .. }
                    | crate::core::sync::store::SyncWrite::ApplyChange { .. }
                    | crate::core::sync::store::SyncWrite::FailPendingEdit { .. }
            );
            let result = crate::core::sync::store::execute_write(pool, command)?;
            let cloud_state_changed = cloud_state_changed
                && !matches!(
                    result,
                    crate::core::sync::store::SyncWriteResult::Changed(false)
                );
            if visibility_changed {
                events.send(DomainEvent::LiveCountDirty);
                events.send(DomainEvent::AlbumsDirty {
                    source: ChangeSource::Synchronization,
                });
            }
            let updated = changed_metadata
                .into_iter()
                .filter_map(|id| db::get_media_item(pool, id).ok())
                .collect::<Vec<_>>();
            if !updated.is_empty() {
                events.send(DomainEvent::MediaUpdated {
                    source: ChangeSource::Synchronization,
                    items: updated,
                    fields: MediaFields {
                        favorite: true,
                        metadata: true,
                        ..MediaFields::FAVORITE
                    },
                });
                events.send(DomainEvent::AlbumsDirty {
                    source: ChangeSource::Synchronization,
                });
            }
            if lifecycle_changed {
                events.send(DomainEvent::SyncStateDirty);
            }
            if let Some((uri, was_visible)) = admitted_uri {
                if let Some(item) = db::get_media_item_by_uri(pool, &uri)? {
                    events.send(if was_visible {
                        DomainEvent::MediaUpdated {
                            source: ChangeSource::Synchronization,
                            items: vec![item],
                            fields: MediaFields {
                                favorite: false,
                                metadata: false,
                                location: true,
                                trash: false,
                                thumbnail: true,
                                attributes: false,
                            },
                        }
                    } else {
                        DomainEvent::MediaUpserted {
                            source: ChangeSource::Synchronization,
                            items: vec![item],
                        }
                    });
                    events.send(DomainEvent::AlbumsDirty {
                        source: ChangeSource::Synchronization,
                    });
                    events.send(DomainEvent::SyncStateDirty);
                }
            }
            if cloud_state_changed {
                events.send(if configuration_only {
                    DomainEvent::SyncConfigurationDirty
                } else {
                    DomainEvent::SyncStateDirty
                });
            }
            Ok(DbCommandResult::Sync(result))
        }
    }
}

#[cfg(test)]
mod tests;
