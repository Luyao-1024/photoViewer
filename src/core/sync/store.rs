use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use rusqlite::{params, params_from_iter, OptionalExtension};

use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand, DbCommandResult};
use crate::core::error::{AppError, Result};

use super::error::{classify_detail, SyncCause};
use super::model::{ProviderKind, SyncDirection, UploadScope};
use super::native::protocol::{NativePinnedScope, NativeZone};
use super::remote_path::{has_empty_segment, has_navigation_segment};

pub(crate) mod accounts;
mod actions;
mod configuration;
pub use actions::{ManagedPhoto, PhotoAction};
pub mod dirty;
pub mod publication;
pub mod receive;
mod reset;
pub(crate) mod runtime;
pub use configuration::{JobEdit, PendingChange};
pub use dirty::{
    note_local_change_in, CategoryOutcome, ChangeCategory, ConfirmedIdentity, DirtyCommand,
    DirtyEntry, DirtyOutcome, EditEventStamp, NativeDirty,
};
pub use publication::{PublicationIntent, ResourceState, ResourceWork};
pub use receive::{
    BootstrapState, InboxEvent, InboxKind, NativeProgress, NativeReceiveCommand,
    NativeReceiveOutcome, TypeMarker, TypeStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSyncJob {
    pub provider_kind: ProviderKind,
    pub endpoint: String,
    pub credential_ref: String,
    pub local_root: PathBuf,
    pub direction: SyncDirection,
    pub upload_scope: UploadScope,
    pub upload_albums: Vec<String>,
    /// The configured zone. Always present: a native job is meaningless
    /// without naming the zone it maps its local root to.
    pub native_zone: Option<NativeZone>,
    /// The capability scope accepted by the last successful probe. Persisted
    /// with the connection so later cycles compare against it instead of
    /// trusting whatever the server announces at that moment.
    pub native_pinned: Option<NativePinnedScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncJob {
    pub id: i64,
    pub connection_id: i64,
    pub provider_kind: ProviderKind,
    pub endpoint: String,
    pub credential_ref: String,
    pub local_root: PathBuf,
    pub direction: SyncDirection,
    pub upload_scope: UploadScope,
    pub native_zone: Option<NativeZone>,
    pub paused: bool,
    pub config_generation: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOverviewStatus {
    Disabled,
    NotConfigured,
    Paused,
    Applying,
    Running,
    Failed,
    Ready,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOverview {
    pub status: SyncOverviewStatus,
    pub job_count: usize,
    pub synced_items: usize,
    /// Historical paths plus the complete discovered scope, deduplicated across jobs.
    pub total_items: usize,
    pub conflict_images: usize,
    /// Why the run stopped, recovered from the job's stored failure detail.
    ///
    /// Set only while a failure is being reported: a clean cycle clears the
    /// detail it stores, so this returns to `None` by itself once the cause is
    /// gone. No column is added for it — see `classify_detail` for why the
    /// stored text is enough to tell a quota rejection from a lost connection.
    pub cause: Option<SyncCause>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudState {
    Synced,
    Off,
}

#[derive(Clone)]
pub struct SyncStore {
    pool: DbPool,
    actor: Option<DbActorHandle>,
}

/// Stored reason for a run a previous process left open. A stable key rather
/// than a translated sentence, so a later locale change never rewrites history.
const INTERRUPTED_RUN: &str = "sync.interrupted_run";

#[derive(Debug)]
pub enum SyncWrite {
    BeginReset {
        token: String,
    },
    FinishReset {
        token: String,
    },
    FailReset {
        token: String,
        error: String,
    },
    FinishRun {
        id: i64,
        generation: i64,
        error: Option<String>,
    },
    ReconcileInterruptedRuns,
    FailPendingEdit {
        id: i64,
        revision: i64,
        error: String,
    },
    QueueEdit {
        id: i64,
        edit: JobEdit,
    },
    ApplyChange {
        id: i64,
        revision: i64,
    },
    CreateJob(Box<NewSyncJob>),
    SetJobPaused {
        id: i64,
        paused: bool,
    },
    DeleteJob {
        id: i64,
    },
    SetUploadAlbums {
        id: i64,
        relative_albums: Vec<String>,
    },
    MarkJobStarted {
        id: i64,
    },
    MarkJobCompleted {
        id: i64,
    },
    MarkJobFailed {
        id: i64,
        error: String,
    },
    /// One dirty-truth write. A single variant keeps the actor match exhaustive.
    Dirty(Box<dirty::DirtyCommand>),
    Runtime(Box<runtime::Command>),
    /// One publication/resource write.
    Publication(Box<publication::PublicationCommand>),
    /// One durable receive write. A single variant rather than one per
    /// operation keeps the actor's priority match exhaustive by construction.
    NativeReceive(Box<NativeReceiveCommand>),
}

#[derive(Debug)]
pub enum SyncWriteResult {
    Changed(bool),
    None,
    Id(i64),
    /// Rows closed by a startup reconciliation of runs a previous process left open.
    ReconciledRuns(usize),
    Native(NativeReceiveOutcome),
    Dirty(DirtyOutcome),
    Publication(publication::PublicationOutcome),
    DeletedJob(Option<String>),
}

impl SyncStore {
    /// Cloud state for media in enabled jobs' selected physical albums.
    pub fn media_cloud_states(&self, media_ids: &[i64]) -> Result<HashMap<i64, CloudState>> {
        self.media_cloud_states_with_sync_enabled(media_ids, crate::core::prefs::sync_enabled())
    }

    /// A global opt-out has no cloud relationship left to report. Connections
    /// stay enabled so saved configuration survives, so the connection-level
    /// filter alone would keep drawing badges for a feature the user turned off.
    fn media_cloud_states_with_sync_enabled(
        &self,
        media_ids: &[i64],
        sync_enabled: bool,
    ) -> Result<HashMap<i64, CloudState>> {
        if !sync_enabled || media_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = vec!["?"; media_ids.len()].join(",");
        let sql = format!(
            "SELECT m.id, MAX(CASE WHEN m.storage_location IN (2,3) AND e.native_photo_id IS NOT NULL
                       THEN 1 ELSE 0 END)
             FROM media_items m
             JOIN sync_jobs j ON substr(m.path, 1, length(rtrim(j.local_root, '/')) + 1)
                                 = rtrim(j.local_root, '/') || '/'
             JOIN sync_connections c ON c.id = j.connection_id
             LEFT JOIN sync_entries e ON e.job_id = j.id
                  AND e.relative_path = substr(m.path, length(rtrim(j.local_root, '/')) + 2)
             WHERE m.id IN ({placeholders}) AND m.media_kind IN ('image', 'video')
               AND m.trashed_at IS NULL AND c.enabled = 1
               AND (j.upload_scope = 'all' OR EXISTS (
                   SELECT 1 FROM sync_job_upload_albums a
                   WHERE a.job_id = j.id AND m.folder_path = CASE
                       WHEN a.relative_album = '' THEN j.local_root
                       ELSE rtrim(j.local_root, '/') || '/' || a.relative_album
                   END
               ))
             GROUP BY m.id"
        );
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(media_ids.iter()), |row| {
            let state = if row.get::<_, i64>(1)? == 1 {
                CloudState::Synced
            } else {
                CloudState::Off
            };
            Ok((row.get(0)?, state))
        })?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()
            .map_err(AppError::from)
    }

    pub fn new(pool: DbPool) -> Self {
        Self { pool, actor: None }
    }

    pub(crate) fn with_optional_actor(pool: DbPool, actor: Option<DbActorHandle>) -> Self {
        Self { pool, actor }
    }

    pub fn with_actor(pool: DbPool, actor: DbActorHandle) -> Self {
        Self {
            pool,
            actor: Some(actor),
        }
    }

    pub(crate) fn commit_local_scan(
        &self,
        items: Vec<crate::core::media::NewMediaItem>,
    ) -> Result<Vec<crate::core::media::MediaItem>> {
        if let Some(actor) = &self.actor {
            match actor.execute_blocking(DbCommand::UpsertMediaBatch {
                source: crate::core::events::ChangeSource::FilesystemWatcher,
                items,
            })? {
                DbCommandResult::MediaItems(items) => Ok(items),
                _ => Err(AppError::Backend("unexpected scan response".into())),
            }
        } else {
            crate::core::db::upsert_media_items_batch(&self.pool, &items)
        }
    }

    pub fn create_job(&self, job: &NewSyncJob) -> Result<SyncJob> {
        validate_new_job(job)?;
        let mut job = job.clone();
        job.local_root = std::fs::canonicalize(&job.local_root)?;
        job.upload_albums = normalize_relative_albums(&job.upload_albums)?;
        let job_id = match self.write(SyncWrite::CreateJob(Box::new(job)))? {
            SyncWriteResult::Id(id) => id,
            SyncWriteResult::None | SyncWriteResult::Changed(_) => {
                return Err(AppError::Backend(
                    "create synchronization job returned no identity".into(),
                ))
            }
            SyncWriteResult::DeletedJob(_) => {
                return Err(AppError::Backend(
                    "create synchronization job returned a delete result".into(),
                ))
            }
            SyncWriteResult::ReconciledRuns(_)
            | SyncWriteResult::Native(_)
            | SyncWriteResult::Dirty(_)
            | SyncWriteResult::Publication(_) => {
                return Err(AppError::Backend(
                    "create synchronization job returned an unrelated result".into(),
                ))
            }
        };
        self.get_job(job_id)?.ok_or_else(|| {
            AppError::Backend("created synchronization job could not be read back".into())
        })
    }

    pub fn list_jobs(&self) -> Result<Vec<SyncJob>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM sync_jobs j
             JOIN sync_connections c ON c.id = j.connection_id
             WHERE c.enabled = 1
             ORDER BY j.id"
        ))?;
        let rows = stmt.query_map([], map_job)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }

    pub fn get_job(&self, id: i64) -> Result<Option<SyncJob>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!(
                "SELECT {JOB_COLUMNS} FROM sync_jobs j
                 JOIN sync_connections c ON c.id = j.connection_id
                 WHERE j.id = ?1"
            ),
            [id],
            map_job,
        )
        .optional()
        .map_err(AppError::from)
    }

    /// The capability scope this connection was accepted against. `None` for a
    /// connection that never completed a probe — exactly the state in which a
    /// native job must not run.
    pub fn native_scope(&self, connection_id: i64) -> Result<Option<NativePinnedScope>> {
        let conn = self.pool.get()?;
        let pinned: Option<String> = conn
            .query_row(
                "SELECT native_pinned_scope FROM sync_connections
                 WHERE id = ?1 AND provider_kind = 'native_syncserver'",
                [connection_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        match pinned {
            // A malformed pinned document is a corrupt connection, not an
            // absent one: reporting it as absent would silently downgrade a
            // native job to "never probed" instead of surfacing the problem.
            Some(document) => serde_json::from_str(&document)
                .map(Some)
                .map_err(configuration::json_error),
            None => Ok(None),
        }
    }

    pub fn set_job_paused(&self, id: i64, paused: bool) -> Result<()> {
        self.write(SyncWrite::SetJobPaused { id, paused })?;
        Ok(())
    }

    pub fn delete_job(&self, id: i64) -> Result<Option<String>> {
        match self.write(SyncWrite::DeleteJob { id })? {
            SyncWriteResult::DeletedJob(credential_ref) => Ok(credential_ref),
            _ => Err(AppError::Backend(
                "database returned an invalid synchronization deletion result".into(),
            )),
        }
    }

    pub fn upload_albums(&self, job_id: i64) -> Result<Vec<String>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT relative_album FROM sync_job_upload_albums
             WHERE job_id = ?1 ORDER BY relative_album",
        )?;
        let rows = stmt.query_map([job_id], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }

    /// Return the persisted, provider-neutral synchronization state used by
    /// compact status surfaces such as the Photos overview.
    pub fn overview(&self) -> Result<SyncOverview> {
        self.overview_with_sync_enabled(crate::core::prefs::sync_enabled())
    }

    fn overview_with_sync_enabled(&self, sync_enabled: bool) -> Result<SyncOverview> {
        let conn = self.pool.get()?;
        let (synced_items, total_items) = conn.query_row(
            "WITH paths(path,synced) AS (
                SELECT rtrim(j.local_root,'/')||'/'||e.relative_path,
                    CASE WHEN e.native_confirmed_generation IS NOT NULL AND e.native_dirty=0
                        AND m.storage_location=3 AND m.thumbnail_ready=1
                        AND NOT EXISTS(SELECT 1 FROM sync_publications p WHERE p.entry_id=e.id AND p.state NOT IN ('complete','superseded'))
                        THEN 1 ELSE 0 END
                FROM sync_entries e JOIN sync_jobs j ON j.id=e.job_id
                JOIN sync_connections c ON c.id=j.connection_id
                LEFT JOIN media_items m ON m.id=e.media_id
                WHERE c.enabled=1 AND e.native_retired=0 AND (
                    e.native_dirty=4 OR e.native_confirmed_generation IS NOT NULL
                    OR (e.native_photo_id IS NOT NULL AND j.direction<>'upload_only')
                    OR (m.trashed_at IS NULL AND m.storage_location<>2 AND
                        (j.upload_scope='all' OR EXISTS(SELECT 1 FROM sync_job_upload_albums a
                            WHERE a.job_id=j.id AND m.folder_path=CASE WHEN a.relative_album='' THEN rtrim(j.local_root,'/') ELSE rtrim(j.local_root,'/')||'/'||a.relative_album END)))
                )
            ) SELECT COUNT(DISTINCT CASE WHEN synced=1 THEN path END),COUNT(DISTINCT path) FROM paths",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as usize,
                    row.get::<_, i64>(1)? as usize,
                ))
            },
        )?;
        let conflict_images = conn.query_row(
            "SELECT COUNT(DISTINCT j.local_root || '/' || e.relative_path)
             FROM sync_conflicts f JOIN sync_entries e ON e.id = f.entry_id
             JOIN sync_jobs j ON j.id = f.job_id
             JOIN sync_connections c ON c.id = j.connection_id
             WHERE c.enabled = 1 AND f.state = 'open' AND
                   (lower(e.relative_path) GLOB '*.jpg' OR
                    lower(e.relative_path) GLOB '*.jpeg' OR
                    lower(e.relative_path) GLOB '*.png' OR
                    lower(e.relative_path) GLOB '*.webp' OR
                    lower(e.relative_path) GLOB '*.heic' OR
                    lower(e.relative_path) GLOB '*.heif' OR
                    lower(e.relative_path) GLOB '*.gif')",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut stmt = conn.prepare(
            "SELECT j.paused, j.last_started_at, j.last_completed_at, j.last_error
             FROM sync_jobs j
             JOIN sync_connections c ON c.id = j.connection_id
             WHERE c.enabled = 1
             ORDER BY j.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? != 0,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let jobs = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let job_count = jobs.len();
        // A job that reports a failure says why in the detail it stored, and
        // that text outlives the process. An empty or blank string is not a
        // failure, so it is filtered once here rather than re-tested — and
        // re-tested differently — at each branch below.
        fn failure(error: &Option<String>) -> Option<&str> {
            error.as_deref().filter(|detail| !detail.trim().is_empty())
        }
        let cause = jobs
            .iter()
            .find_map(|(_, _, _, error)| failure(error).map(classify_detail));
        let failed_any = jobs.iter().any(|(_, _, _, error)| failure(error).is_some());
        if !sync_enabled {
            return Ok(SyncOverview {
                status: SyncOverviewStatus::Disabled,
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }
        if jobs.is_empty() {
            return Ok(SyncOverview {
                status: SyncOverviewStatus::NotConfigured,
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }
        let pending = self.reset_pending()? || conn.query_row("SELECT EXISTS(SELECT 1 FROM sync_job_changes p JOIN sync_jobs j ON j.id=p.job_id JOIN sync_connections c ON c.id=j.connection_id WHERE c.enabled=1)",[],|row|row.get::<_, bool>(0))?;
        if pending {
            return Ok(SyncOverview {
                status: if self.reset_error()?.is_some() || failed_any {
                    SyncOverviewStatus::Failed
                } else {
                    SyncOverviewStatus::Applying
                },
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }
        if jobs.iter().all(|(paused, _, _, _)| *paused) {
            return Ok(SyncOverview {
                status: SyncOverviewStatus::Paused,
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }

        let active = jobs.iter().filter(|(paused, _, _, _)| !paused);
        if active
            .clone()
            .any(|(_, _, _, error)| failure(error).is_some())
        {
            return Ok(SyncOverview {
                status: SyncOverviewStatus::Failed,
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }
        if active.clone().any(|(_, started, completed, _)| {
            started.is_some_and(|started| completed.is_none_or(|completed| started > completed))
        }) {
            return Ok(SyncOverview {
                status: SyncOverviewStatus::Running,
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }
        let cycle_finished = active
            .clone()
            .any(|(_, _, completed, _)| completed.is_some());
        if cycle_finished && synced_items < total_items {
            return Ok(SyncOverview {
                status: SyncOverviewStatus::Running,
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }
        if active
            .clone()
            .any(|(_, _, completed, _)| completed.is_some())
        {
            return Ok(SyncOverview {
                status: SyncOverviewStatus::Completed,
                job_count,
                synced_items,
                total_items,
                conflict_images,
                cause,
            });
        }
        Ok(SyncOverview {
            status: SyncOverviewStatus::Ready,
            job_count,
            synced_items,
            total_items,
            conflict_images,
            cause,
        })
    }

    /// The cause of the failure this job last recorded, if it recorded one.
    ///
    /// Recovered from the stored detail rather than kept beside it: the detail
    /// is what the job already persists, and `classify_detail` already turns it
    /// back into a cause, so the answer needs no column of its own.
    pub fn failure_cause(&self, id: i64) -> Result<Option<SyncCause>> {
        Ok(self
            .get_job(id)?
            .and_then(|job| job.last_error)
            .filter(|detail| !detail.trim().is_empty())
            .map(|detail| classify_detail(&detail)))
    }

    pub fn set_upload_albums(&self, id: i64, relative_albums: &[String]) -> Result<()> {
        let relative_albums = normalize_relative_albums(relative_albums)?;
        self.write(SyncWrite::SetUploadAlbums {
            id,
            relative_albums,
        })?;
        Ok(())
    }

    pub fn mark_job_started(&self, id: i64) -> Result<()> {
        self.write(SyncWrite::MarkJobStarted { id })?;
        Ok(())
    }

    /// Close a run's lifecycle. Only the run that captured this generation may
    /// write, so a superseded worker cannot report the newer run's outcome.
    pub fn finish_run(&self, id: i64, generation: i64, error: Option<&str>) -> Result<()> {
        self.write(SyncWrite::FinishRun {
            id,
            generation,
            error: error.map(str::to_owned),
        })?;
        Ok(())
    }

    /// Close runs a previous process left open. Call once per launch, before any
    /// run is started, so the derived overview status cannot keep reporting a
    /// run that died with an earlier process.
    pub(crate) fn reconcile_interrupted_runs(&self) -> Result<usize> {
        match self.write(SyncWrite::ReconcileInterruptedRuns)? {
            SyncWriteResult::ReconciledRuns(updated) => Ok(updated),
            _ => Ok(0),
        }
    }

    pub(crate) fn fail_pending_edit(&self, id: i64, revision: i64, error: &str) -> Result<()> {
        self.write(SyncWrite::FailPendingEdit {
            id,
            revision,
            error: error.into(),
        })?;
        Ok(())
    }

    pub fn mark_job_completed(&self, id: i64) -> Result<()> {
        self.write(SyncWrite::MarkJobCompleted { id })?;
        Ok(())
    }

    pub fn mark_job_failed(&self, id: i64, error: &str) -> Result<()> {
        self.write(SyncWrite::MarkJobFailed {
            id,
            error: error.into(),
        })?;
        Ok(())
    }

    /// Persist a complete discovered scope before transferring any path.
    /// An obsolete worker cannot replace the inventory of a newer configuration.
    /// Publish part of a discovered scope while discovery is still running.
    ///
    /// This only ever adds. The overview's denominator is the union of the
    /// persisted inventory and proven history, so appending as the walk
    /// progresses makes the scope visible immediately instead of leaving the
    /// overview reading `0/0` for the whole enumeration. Because nothing is
    /// ever removed here, a discovery that fails part-way can only leave a
    /// superset of the previous complete scope — never a smaller one — and the
    /// next complete discovery converges on the exact scope through
    /// [`Self::replace_inventory`].
    ///
    /// Returns whether at least one new row was added.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn write(&self, command: SyncWrite) -> Result<SyncWriteResult> {
        if let Some(actor) = &self.actor {
            return match actor.execute_blocking(DbCommand::Sync(command))? {
                DbCommandResult::Sync(result) => Ok(result),
                _ => Err(AppError::Backend(
                    "database actor returned an invalid synchronization result".into(),
                )),
            };
        }
        execute_write(&self.pool, command)
    }
}

pub(crate) fn execute_write(pool: &DbPool, command: SyncWrite) -> Result<SyncWriteResult> {
    match command {
        SyncWrite::BeginReset { token } => reset::begin(pool, token),
        SyncWrite::FinishReset { token } => reset::finish(pool, token),
        SyncWrite::FailReset { token, error } => reset::fail(pool, token, error),
        SyncWrite::FinishRun {
            id,
            generation,
            error,
        } => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            // A run that has ended must never keep reading as running, whatever
            // became true while it ran. The timestamp records "no run is in
            // flight", which is true unconditionally, so it is written
            // unconditionally: guarding it on `config_generation` let an edit
            // applied mid-run bump the generation, turn this UPDATE into a
            // silent zero-row no-op, and pin the overview to "syncing" forever.
            tx.execute(
                "UPDATE sync_jobs SET last_completed_at=unixepoch() WHERE id=?1",
                [id],
            )?;
            // The error, by contrast, describes one specific configuration. It is
            // only attributable while that configuration is still current and no
            // edit is queued to replace it; otherwise it would blame settings that
            // did not cause it, or report a verdict that is about to be superseded.
            let attributable: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_jobs WHERE id=?1 AND config_generation=?2) \
                        AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=?1)",
                params![id, generation],
                |row| row.get(0),
            )?;
            if attributable {
                match error {
                    Some(error) => {
                        tx.execute(
                            "UPDATE sync_jobs SET last_error=?1 WHERE id=?2",
                            params![error, id],
                        )?;
                    }
                    None => {
                        tx.execute("UPDATE sync_jobs SET last_error=NULL WHERE id=?1", [id])?;
                    }
                }
            }
            tx.commit()?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::ReconcileInterruptedRuns => {
            // A run cannot outlive the process that started it, so at launch any
            // `last_started_at > last_completed_at` is a run this process never
            // began — the previous one was killed, or ended on a path that could
            // not record its completion. Without this, the derived overview status
            // stays "running" across every subsequent launch and nothing ever
            // recovers it. Report it as failed rather than completed: it did not
            // finish, and the retry the failure state carries is the honest way
            // back.
            let conn = pool.get()?;
            let updated = conn.execute(
                "UPDATE sync_jobs \
                    SET last_completed_at=last_started_at, \
                        last_error=?1 \
                  WHERE last_started_at IS NOT NULL \
                    AND (last_completed_at IS NULL OR last_started_at > last_completed_at)",
                [INTERRUPTED_RUN],
            )?;
            Ok(SyncWriteResult::ReconciledRuns(updated))
        }
        SyncWrite::FailPendingEdit {
            id,
            revision,
            error,
        } => {
            pool.get()?.execute("UPDATE sync_jobs SET last_error=?1 WHERE id=?2 AND EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=?2 AND json_extract(payload,'$.revision')=?3)",params![error,id,revision])?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::Runtime(command) => {
            runtime::execute(pool, *command)?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::Dirty(command) => {
            let outcome = dirty::execute(pool, command.as_ref())?;
            Ok(SyncWriteResult::Dirty(outcome))
        }
        SyncWrite::Publication(command) => {
            let outcome = publication::execute(pool, command.as_ref())?;
            Ok(SyncWriteResult::Publication(outcome))
        }
        SyncWrite::NativeReceive(command) => {
            let outcome = receive::execute(pool, command.as_ref())?;
            Ok(SyncWriteResult::Native(outcome))
        }
        SyncWrite::QueueEdit { id, edit } => configuration::queue(pool, id, edit),
        SyncWrite::ApplyChange { id, revision } => configuration::apply(pool, id, revision),
        SyncWrite::CreateJob(job) => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            let connection_id = create_connection(&tx, &job)?;
            let zone = job.native_zone.as_ref().ok_or_else(|| {
                AppError::Backend("a native synchronization job must name its zone".into())
            })?;
            let existing = {
                let mut stmt = tx.prepare(
                    "SELECT j.local_root, c.endpoint, j.native_zone_name, c.native_tenant, c.native_pinned_scope
                     FROM sync_jobs j JOIN sync_connections c ON c.id = j.connection_id",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        PathBuf::from(row.get::<_, String>(0)?),
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (
                existing_local,
                existing_endpoint,
                existing_zone,
                existing_tenant,
                existing_scope,
            ) in existing
            {
                let existing_scope: Option<NativePinnedScope> = existing_scope
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(configuration::json_error)?;
                let same_account = existing_scope
                    .as_ref()
                    .zip(job.native_pinned.as_ref())
                    .is_none_or(|(a, b)| accounts::same_account(a, b));
                if same_account && paths_overlap(&existing_local, &job.local_root) {
                    return Err(AppError::Backend(format!(
                        "synchronization local root overlaps an existing job: {}",
                        existing_local.display()
                    )));
                }
                // One local mapping per endpoint/account/zone; separate users
                // of the same server can map to distinct local roots.
                if existing_endpoint == job.endpoint
                    && existing_zone == zone.zone_name
                    && existing_tenant.as_deref()
                        == job
                            .native_pinned
                            .as_ref()
                            .map(|p| p.namespace.tenant.as_str())
                {
                    return Err(AppError::Backend(format!(
                        "zone {} on {} is already mapped by another task",
                        zone.zone_name, existing_endpoint
                    )));
                }
            }
            tx.execute(
                "INSERT INTO sync_jobs
                 (connection_id, local_root, direction, upload_scope,
                  native_zone_name, native_owner)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    connection_id,
                    job.local_root.to_string_lossy(),
                    job.direction.as_str(),
                    job.upload_scope.as_str(),
                    zone.zone_name,
                    zone.owner,
                ],
            )?;
            let id = tx.last_insert_rowid();
            for relative_album in &job.upload_albums {
                tx.execute(
                    "INSERT INTO sync_job_upload_albums (job_id, relative_album)
                     VALUES (?1, ?2)",
                    params![id, relative_album],
                )?;
            }
            accounts::select_in(&tx, None)?;
            tx.commit()?;
            Ok(SyncWriteResult::Id(id))
        }
        SyncWrite::SetJobPaused { id, paused } => {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_jobs SET paused = ?1 WHERE id = ?2",
                params![paused, id],
            )?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::DeleteJob { id } => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            let (connection_id, credential_ref, paused) = tx
                .query_row(
                    "SELECT c.id, c.credential_ref, j.paused
                     FROM sync_jobs j
                     JOIN sync_connections c ON c.id = j.connection_id
                     WHERE j.id = ?1",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)? != 0,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| AppError::Backend(format!("sync job {id} does not exist")))?;
            if !paused {
                return Err(AppError::Backend(
                    "pause this synchronization task before deleting it".into(),
                ));
            }
            tx.execute("DELETE FROM sync_jobs WHERE id = ?1", [id])?;
            let remaining_jobs: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sync_jobs WHERE connection_id = ?1",
                [connection_id],
                |row| row.get(0),
            )?;
            let orphaned_credential = if remaining_jobs == 0 {
                tx.execute(
                    "DELETE FROM sync_connections WHERE id = ?1",
                    [connection_id],
                )?;
                Some(credential_ref)
            } else {
                None
            };
            accounts::refresh_media_locations_in(&tx)?;
            tx.commit()?;
            Ok(SyncWriteResult::DeletedJob(orphaned_credential))
        }
        SyncWrite::SetUploadAlbums {
            id,
            relative_albums,
        } => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            let updated = tx.execute(
                "UPDATE sync_jobs SET upload_scope = 'selected_albums',
                        config_generation = config_generation + 1
                 WHERE id = ?1",
                [id],
            )?;
            if updated == 0 {
                return Err(AppError::Backend(format!(
                    "synchronization job {id} does not exist"
                )));
            }
            tx.execute("DELETE FROM sync_job_upload_albums WHERE job_id = ?1", [id])?;
            for relative_album in relative_albums {
                tx.execute(
                    "INSERT INTO sync_job_upload_albums (job_id, relative_album)
                     VALUES (?1, ?2)",
                    params![id, relative_album],
                )?;
            }
            tx.commit()?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::MarkJobStarted { id } => {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_jobs SET last_started_at = unixepoch(), last_error = NULL WHERE id = ?1",
                [id],
            )?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::MarkJobCompleted { id } => {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_jobs SET last_completed_at = unixepoch(), last_error = NULL WHERE id = ?1",
                [id],
            )?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::MarkJobFailed { id, error } => {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_jobs SET last_error = ?1 WHERE id = ?2",
                params![error, id],
            )?;
            Ok(SyncWriteResult::None)
        }
    }
}

/// Insert or refresh the connection a job belongs to.
///
/// The `credential_ref` is the identity a connection is reused by. The stored
/// provider kind must agree with the request — there is only one kind today,
/// but the check keeps a future second provider from silently adopting another
/// provider's credential reference and pinned scope.
fn create_connection(tx: &rusqlite::Transaction<'_>, job: &NewSyncJob) -> Result<i64> {
    let existing: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, provider_kind FROM sync_connections WHERE credential_ref = ?1",
            [&job.credential_ref],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((connection_id, stored_kind)) = existing else {
        let pinned = job.native_pinned.as_ref();
        tx.execute(
            "INSERT INTO sync_connections
             (enabled, provider_kind, endpoint, credential_ref,
              native_public_base_url, native_protocol_version, native_service_version,
              native_stage, native_checksum_algorithm, native_tenant, native_container,
              native_environment, native_database, native_database_generation, native_zones,
              native_pinned_scope, native_probed_at)
             VALUES (0, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     CASE WHEN ?16 IS NULL THEN NULL ELSE unixepoch() END)",
            params![
                job.provider_kind.as_str(),
                job.endpoint,
                job.credential_ref,
                pinned.map(|p| p.public_base_url.clone()),
                pinned.map(|p| p.protocol_version),
                pinned.map(|p| p.service_version.clone()),
                pinned.map(|p| p.stage.clone()),
                pinned.map(|p| p.checksum_algorithm.clone()),
                pinned.map(|p| p.namespace.tenant.clone()),
                pinned.map(|p| p.namespace.container.clone()),
                pinned.map(|p| p.namespace.environment.clone()),
                pinned.map(|p| p.namespace.database.clone()),
                pinned.map(|p| p.database_generation.clone()),
                pinned.and_then(|p| serde_json::to_string(&p.zones).ok()),
                pinned.and_then(|p| serde_json::to_string(p).ok()),
                pinned.map(|p| p.public_base_url.clone()),
            ],
        )?;
        return Ok(tx.last_insert_rowid());
    };

    if stored_kind != job.provider_kind.as_str() {
        return Err(AppError::Backend(format!(
            "this credential reference already belongs to a {stored_kind} connection; \
             native connections are created explicitly and are never converted"
        )));
    }
    match &job.native_pinned {
        Some(pinned) => tx.execute(
            "UPDATE sync_connections SET endpoint = ?1,
                    native_public_base_url = ?2, native_protocol_version = ?3,
                    native_service_version = ?4, native_stage = ?5, native_checksum_algorithm = ?6,
                    native_tenant = ?7, native_container = ?8, native_environment = ?9,
                    native_database = ?10, native_database_generation = ?11, native_zones = ?12,
                    native_pinned_scope = ?13, native_probed_at = unixepoch(),
                    native_probe_error = NULL,
                    updated_at = unixepoch()
             WHERE id = ?14",
            params![
                job.endpoint,
                pinned.public_base_url,
                pinned.protocol_version,
                pinned.service_version,
                pinned.stage,
                pinned.checksum_algorithm,
                pinned.namespace.tenant,
                pinned.namespace.container,
                pinned.namespace.environment,
                pinned.namespace.database,
                pinned.database_generation,
                serde_json::to_string(&pinned.zones).map_err(configuration::json_error)?,
                serde_json::to_string(pinned).map_err(configuration::json_error)?,
                connection_id,
            ],
        )?,
        None => tx.execute(
            "UPDATE sync_connections SET endpoint = ?1, updated_at = unixepoch()
             WHERE id = ?2",
            params![job.endpoint, connection_id],
        )?,
    };
    Ok(connection_id)
}

fn validate_new_job(job: &NewSyncJob) -> Result<()> {
    if !job.local_root.is_absolute() {
        return Err(AppError::Backend(
            "synchronization local root must be absolute".into(),
        ));
    }
    if !job.local_root.is_dir() {
        return Err(AppError::Backend(
            "synchronization local root must be an existing directory".into(),
        ));
    }
    normalize_relative_albums(&job.upload_albums)?;
    let endpoint = super::native::NativeEndpoint::parse(&job.endpoint)
        .map_err(|error| AppError::Backend(error.to_string()))?;
    // A native task is only meaningful against a scope a probe accepted.
    let pinned = job.native_pinned.as_ref().ok_or_else(|| {
        AppError::Backend("a native connection must be probed before a task can use it".into())
    })?;
    super::native::validate_zone(pinned, job.native_zone_name()?)
        .map_err(|error| AppError::Backend(error.to_string()))?;
    // A pinned document from another origin would send the Bearer key and the
    // lease proof to a server the user never configured.
    if let Ok(announced) = url::Url::parse(&pinned.public_base_url) {
        let announced = (
            announced.scheme().to_string(),
            announced.host_str().unwrap_or_default().to_string(),
            announced.port(),
        );
        if announced != endpoint.origin() {
            return Err(AppError::Backend(format!(
                "the probed scope belongs to {}, not to the configured endpoint",
                pinned.public_base_url
            )));
        }
    }
    Ok(())
}

impl NewSyncJob {
    fn native_zone_name(&self) -> Result<&str> {
        self.native_zone
            .as_ref()
            .map(|zone| zone.zone_name.as_str())
            .ok_or_else(|| {
                AppError::Backend("a native synchronization task must name its zone".into())
            })
    }
}

fn normalize_relative_albums(albums: &[String]) -> Result<Vec<String>> {
    let mut normalized = BTreeSet::new();
    for album in albums {
        let album = album.trim_matches('/');
        if !album.is_empty() && (has_empty_segment(album) || has_navigation_segment(album)) {
            return Err(AppError::Backend(
                "synchronization upload album is invalid".into(),
            ));
        }
        normalized.insert(album.to_string());
    }
    Ok(normalized.into_iter().collect())
}

fn paths_overlap(left: &std::path::Path, right: &std::path::Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Column order `map_job` decodes. Keep the two in step: the positional reader
/// is deliberate so a job row costs one prepared statement per list call.
const JOB_COLUMNS: &str = "j.id, j.connection_id, c.endpoint, c.credential_ref,
                    j.local_root, j.direction,
                    j.paused, j.config_generation, j.upload_scope, j.last_error,
                    c.provider_kind, j.native_zone_name, j.native_owner";

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncJob> {
    let provider_kind: String = row.get(10)?;
    let provider_kind = ProviderKind::parse(&provider_kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            10,
            rusqlite::types::Type::Text,
            format!("invalid sync provider kind: {provider_kind}").into(),
        )
    })?;
    let direction: String = row.get(5)?;
    let direction = SyncDirection::parse(&direction).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            format!("invalid sync direction: {direction}").into(),
        )
    })?;
    let upload_scope: String = row.get(8)?;
    let upload_scope = UploadScope::parse(&upload_scope).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            format!("invalid upload scope: {upload_scope}").into(),
        )
    })?;
    // A job without a zone is meaningless, so this is an error rather than a
    // default: silently synthesising a zone would point a task at a cloud
    // location the user never chose.
    let zone_name: Option<String> = row.get(11)?;
    let owner: String = row.get(12)?;
    let native_zone = zone_name.map(|zone_name| NativeZone { zone_name, owner });
    Ok(SyncJob {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        provider_kind,
        endpoint: row.get(2)?,
        credential_ref: row.get(3)?,
        local_root: PathBuf::from(row.get::<_, String>(4)?),
        direction,
        upload_scope,
        native_zone,
        paused: row.get::<_, i64>(6)? != 0,
        config_generation: row.get(7)?,
        last_error: row.get(9)?,
    })
}

#[cfg(test)]
pub(crate) mod tests;
