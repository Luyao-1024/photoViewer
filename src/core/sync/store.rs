use std::collections::BTreeSet;
use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};

use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand, DbCommandResult};
use crate::core::error::{AppError, Result};

use super::model::{Fingerprint, Revision, RevisionStrength, SyncDirection, UploadScope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSyncJob {
    pub endpoint: String,
    pub username: String,
    pub credential_ref: String,
    pub local_root: PathBuf,
    pub remote_root: String,
    pub direction: SyncDirection,
    pub upload_scope: UploadScope,
    pub upload_albums: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncJob {
    pub id: i64,
    pub connection_id: i64,
    pub endpoint: String,
    pub username: String,
    pub credential_ref: String,
    pub local_root: PathBuf,
    pub remote_root: String,
    pub direction: SyncDirection,
    pub upload_scope: UploadScope,
    pub propagate_deletes: bool,
    pub paused: bool,
    pub config_generation: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEntry {
    pub id: i64,
    pub relative_path: String,
    pub local: Option<Fingerprint>,
    pub local_mtime_ns: Option<i64>,
    pub remote: Option<Fingerprint>,
    pub remote_revision: Option<String>,
    pub remote_revision_weak: bool,
    pub baseline: Option<Fingerprint>,
    pub baseline_remote_revision: Option<String>,
    pub state: String,
    pub generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConflict {
    pub id: i64,
    pub job_id: i64,
    pub entry_id: i64,
    pub relative_path: String,
    pub kind: String,
    pub local_fingerprint: Option<String>,
    pub remote_fingerprint: Option<String>,
    pub remote_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTask {
    pub operation_id: String,
    pub job_id: i64,
    pub entry_id: i64,
    pub relative_path: String,
    pub action: String,
    pub state: String,
    pub expected_revision: Option<String>,
    pub expected_weak: bool,
    pub artifact_path: PathBuf,
    pub artifact_hash: String,
    pub config_generation: i64,
    pub entry_generation: i64,
}

#[derive(Clone)]
pub struct SyncStore {
    pool: DbPool,
    actor: Option<DbActorHandle>,
}

#[derive(Debug)]
pub enum SyncWrite {
    CreateJob(NewSyncJob),
    SetJobPaused {
        id: i64,
        paused: bool,
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
    UpsertObservation {
        job_id: i64,
        relative_path: String,
        local: Option<Fingerprint>,
        local_mtime_ns: Option<i64>,
        remote: Option<Fingerprint>,
        remote_revision: Option<String>,
        remote_revision_weak: bool,
        state: String,
    },
    CommitBaseline {
        entry_id: i64,
        fingerprint: Fingerprint,
        remote_revision: Option<String>,
    },
    RecordConflict {
        job_id: i64,
        entry_id: i64,
        kind: String,
        local: Option<Fingerprint>,
        remote: Option<Fingerprint>,
        remote_revision: Option<String>,
    },
    PrepareTask {
        operation_id: String,
        job_id: i64,
        entry_id: i64,
        action: String,
        expected_revision: Option<String>,
        expected_weak: bool,
        artifact_path: PathBuf,
        artifact_hash: String,
        config_generation: i64,
        entry_generation: i64,
    },
    SetTaskState {
        operation_id: String,
        state: String,
        error: Option<String>,
    },
    ResolveConflict {
        conflict_id: i64,
        entry_id: i64,
        resolution: String,
        fingerprint: Fingerprint,
        remote_revision: Option<String>,
    },
}

#[derive(Debug)]
pub enum SyncWriteResult {
    None,
    Id(i64),
}

impl SyncStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool, actor: None }
    }

    pub fn with_actor(pool: DbPool, actor: DbActorHandle) -> Self {
        Self {
            pool,
            actor: Some(actor),
        }
    }

    pub fn create_job(&self, job: &NewSyncJob) -> Result<SyncJob> {
        validate_new_job(job)?;
        let mut job = job.clone();
        job.local_root = std::fs::canonicalize(&job.local_root)?;
        job.remote_root = job.remote_root.trim_matches('/').to_string();
        job.upload_albums = normalize_relative_albums(&job.upload_albums)?;
        let job_id = match self.write(SyncWrite::CreateJob(job))? {
            SyncWriteResult::Id(id) => id,
            SyncWriteResult::None => {
                return Err(AppError::Backend(
                    "create synchronization job returned no identity".into(),
                ))
            }
        };
        self.get_job(job_id)?.ok_or_else(|| {
            AppError::Backend("created synchronization job could not be read back".into())
        })
    }

    pub fn list_jobs(&self) -> Result<Vec<SyncJob>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT j.id, j.connection_id, c.endpoint, c.username, c.credential_ref,
                    j.local_root, j.remote_root, j.direction, j.propagate_deletes,
                    j.paused, j.config_generation, j.upload_scope, j.last_error
             FROM sync_jobs j
             JOIN sync_connections c ON c.id = j.connection_id
             WHERE c.enabled = 1
             ORDER BY j.id",
        )?;
        let rows = stmt.query_map([], map_job)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }

    pub fn get_job(&self, id: i64) -> Result<Option<SyncJob>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT j.id, j.connection_id, c.endpoint, c.username, c.credential_ref,
                    j.local_root, j.remote_root, j.direction, j.propagate_deletes,
                    j.paused, j.config_generation, j.upload_scope, j.last_error
             FROM sync_jobs j JOIN sync_connections c ON c.id = j.connection_id
             WHERE j.id = ?1",
            [id],
            map_job,
        )
        .optional()
        .map_err(AppError::from)
    }

    pub fn set_job_paused(&self, id: i64, paused: bool) -> Result<()> {
        self.write(SyncWrite::SetJobPaused { id, paused })?;
        Ok(())
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

    pub fn entries(&self, job_id: i64) -> Result<Vec<StoredEntry>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, relative_path, local_fingerprint, local_size, local_mtime_ns,
                    remote_fingerprint, remote_size, remote_revision, remote_revision_weak,
                    baseline_fingerprint, baseline_size, baseline_remote_revision,
                    state, entry_generation
             FROM sync_entries WHERE job_id = ?1 ORDER BY relative_path",
        )?;
        let rows = stmt.query_map([job_id], |row| {
            Ok(StoredEntry {
                id: row.get(0)?,
                relative_path: row.get(1)?,
                local: fingerprint(row.get(2)?, row.get(3)?),
                local_mtime_ns: row.get(4)?,
                remote: fingerprint(row.get(5)?, row.get(6)?),
                remote_revision: row.get(7)?,
                remote_revision_weak: row.get::<_, i64>(8)? != 0,
                baseline: fingerprint(row.get(9)?, row.get(10)?),
                baseline_remote_revision: row.get(11)?,
                state: row.get(12)?,
                generation: row.get(13)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }

    pub fn open_conflicts(&self, job_id: i64) -> Result<Vec<SyncConflict>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.job_id, c.entry_id, e.relative_path, c.kind,
                    c.local_fingerprint, c.remote_fingerprint, c.remote_revision
             FROM sync_conflicts c
             JOIN sync_entries e ON e.id = c.entry_id
             WHERE c.job_id = ?1 AND c.state = 'open'
             ORDER BY e.relative_path",
        )?;
        let rows = stmt.query_map([job_id], |row| {
            Ok(SyncConflict {
                id: row.get(0)?,
                job_id: row.get(1)?,
                entry_id: row.get(2)?,
                relative_path: row.get(3)?,
                kind: row.get(4)?,
                local_fingerprint: row.get(5)?,
                remote_fingerprint: row.get(6)?,
                remote_revision: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }

    pub fn get_open_conflict(&self, conflict_id: i64) -> Result<Option<SyncConflict>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT c.id, c.job_id, c.entry_id, e.relative_path, c.kind,
                    c.local_fingerprint, c.remote_fingerprint, c.remote_revision
             FROM sync_conflicts c
             JOIN sync_entries e ON e.id = c.entry_id
             WHERE c.id = ?1 AND c.state = 'open'",
            [conflict_id],
            |row| {
                Ok(SyncConflict {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    entry_id: row.get(2)?,
                    relative_path: row.get(3)?,
                    kind: row.get(4)?,
                    local_fingerprint: row.get(5)?,
                    remote_fingerprint: row.get(6)?,
                    remote_revision: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(AppError::from)
    }

    pub fn unfinished_tasks(&self, job_id: i64) -> Result<Vec<StoredTask>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT t.operation_id, t.job_id, t.entry_id, e.relative_path,
                    t.action, t.state, t.expected_revision, t.expected_weak,
                    t.artifact_path, t.artifact_hash, t.config_generation,
                    t.entry_generation
             FROM sync_tasks t
             JOIN sync_entries e ON e.id = t.entry_id
             WHERE t.job_id = ?1 AND t.state IN ('prepared', 'reconciling')
             ORDER BY t.id",
        )?;
        let rows = stmt.query_map([job_id], |row| {
            Ok(StoredTask {
                operation_id: row.get(0)?,
                job_id: row.get(1)?,
                entry_id: row.get(2)?,
                relative_path: row.get(3)?,
                action: row.get(4)?,
                state: row.get(5)?,
                expected_revision: row.get(6)?,
                expected_weak: row.get::<_, i64>(7)? != 0,
                artifact_path: PathBuf::from(row.get::<_, String>(8)?),
                artifact_hash: row.get(9)?,
                config_generation: row.get(10)?,
                entry_generation: row.get(11)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_observation(
        &self,
        job_id: i64,
        relative_path: &str,
        local: Option<&Fingerprint>,
        local_mtime_ns: Option<i64>,
        remote: Option<&Fingerprint>,
        remote_revision: Option<&str>,
        remote_revision_weak: bool,
        state: &str,
    ) -> Result<i64> {
        match self.write(SyncWrite::UpsertObservation {
            job_id,
            relative_path: relative_path.into(),
            local: local.cloned(),
            local_mtime_ns,
            remote: remote.cloned(),
            remote_revision: remote_revision.map(str::to_string),
            remote_revision_weak,
            state: state.into(),
        })? {
            SyncWriteResult::Id(id) => Ok(id),
            SyncWriteResult::None => Err(AppError::Backend(
                "upsert synchronization observation returned no identity".into(),
            )),
        }
    }

    pub fn commit_baseline(
        &self,
        entry_id: i64,
        fingerprint: &Fingerprint,
        remote_revision: Option<&str>,
    ) -> Result<()> {
        self.write(SyncWrite::CommitBaseline {
            entry_id,
            fingerprint: fingerprint.clone(),
            remote_revision: remote_revision.map(str::to_string),
        })?;
        Ok(())
    }

    pub fn record_conflict(
        &self,
        job_id: i64,
        entry_id: i64,
        kind: &str,
        local: Option<&Fingerprint>,
        remote: Option<&Fingerprint>,
        remote_revision: Option<&str>,
    ) -> Result<()> {
        self.write(SyncWrite::RecordConflict {
            job_id,
            entry_id,
            kind: kind.into(),
            local: local.cloned(),
            remote: remote.cloned(),
            remote_revision: remote_revision.map(str::to_string),
        })?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_task(
        &self,
        operation_id: &str,
        job_id: i64,
        entry_id: i64,
        action: &str,
        expected_revision: Option<&Revision>,
        artifact_path: &std::path::Path,
        artifact_hash: &str,
        config_generation: i64,
        entry_generation: i64,
    ) -> Result<()> {
        self.write(SyncWrite::PrepareTask {
            operation_id: operation_id.into(),
            job_id,
            entry_id,
            action: action.into(),
            expected_revision: expected_revision.map(|revision| revision.value.clone()),
            expected_weak: expected_revision
                .is_some_and(|revision| revision.strength == RevisionStrength::Weak),
            artifact_path: artifact_path.to_path_buf(),
            artifact_hash: artifact_hash.into(),
            config_generation,
            entry_generation,
        })?;
        Ok(())
    }

    pub fn set_task_state(
        &self,
        operation_id: &str,
        state: &str,
        error: Option<&str>,
    ) -> Result<()> {
        self.write(SyncWrite::SetTaskState {
            operation_id: operation_id.into(),
            state: state.into(),
            error: error.map(str::to_string),
        })?;
        Ok(())
    }

    pub fn resolve_conflict(
        &self,
        conflict_id: i64,
        entry_id: i64,
        resolution: &str,
        fingerprint: &Fingerprint,
        remote_revision: Option<&str>,
    ) -> Result<()> {
        self.write(SyncWrite::ResolveConflict {
            conflict_id,
            entry_id,
            resolution: resolution.into(),
            fingerprint: fingerprint.clone(),
            remote_revision: remote_revision.map(str::to_string),
        })?;
        Ok(())
    }

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
        SyncWrite::CreateJob(job) => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            let connection_id = match tx
                .query_row(
                    "SELECT id FROM sync_connections WHERE credential_ref = ?1",
                    [&job.credential_ref],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
            {
                Some(id) => id,
                None => {
                    tx.execute(
                        "INSERT INTO sync_connections
                         (provider_kind, endpoint, username, credential_ref)
                         VALUES ('webdav', ?1, ?2, ?3)",
                        params![job.endpoint, job.username, job.credential_ref],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            tx.execute(
                "UPDATE sync_connections SET endpoint = ?1, username = ?2,
                        updated_at = unixepoch() WHERE id = ?3",
                params![job.endpoint, job.username, connection_id],
            )?;
            let existing = {
                let mut stmt =
                    tx.prepare("SELECT connection_id, local_root, remote_root FROM sync_jobs")?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        PathBuf::from(row.get::<_, String>(1)?),
                        row.get::<_, String>(2)?,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (existing_connection, existing_local, existing_remote) in existing {
                if paths_overlap(&existing_local, &job.local_root) {
                    return Err(AppError::Backend(format!(
                        "synchronization local root overlaps an existing job: {}",
                        existing_local.display()
                    )));
                }
                if existing_connection == connection_id
                    && remote_paths_overlap(&existing_remote, &job.remote_root)
                {
                    return Err(AppError::Backend(format!(
                        "synchronization remote root overlaps an existing job: {existing_remote}"
                    )));
                }
            }
            tx.execute(
                "INSERT INTO sync_jobs
                 (connection_id, local_root, remote_root, direction, upload_scope)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    connection_id,
                    job.local_root.to_string_lossy(),
                    job.remote_root,
                    job.direction.as_str(),
                    job.upload_scope.as_str(),
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
        SyncWrite::UpsertObservation {
            job_id,
            relative_path,
            local,
            local_mtime_ns,
            remote,
            remote_revision,
            remote_revision_weak,
            state,
        } => {
            let conn = pool.get()?;
            conn.execute(
                "INSERT INTO sync_entries
                 (job_id, relative_path, local_fingerprint, local_size, local_mtime_ns,
                  remote_fingerprint, remote_size, remote_revision, remote_revision_weak, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(job_id, relative_path) DO UPDATE SET
                   local_fingerprint = excluded.local_fingerprint,
                   local_size = excluded.local_size,
                   local_mtime_ns = excluded.local_mtime_ns,
                   remote_fingerprint = excluded.remote_fingerprint,
                   remote_size = excluded.remote_size,
                   remote_revision = excluded.remote_revision,
                   remote_revision_weak = excluded.remote_revision_weak,
                   state = excluded.state,
                   entry_generation = sync_entries.entry_generation + 1,
                   updated_at = unixepoch()",
                params![
                    job_id,
                    relative_path,
                    local.as_ref().map(|f| f.blake3.as_str()),
                    local.as_ref().map(|f| f.size as i64),
                    local_mtime_ns,
                    remote.as_ref().map(|f| f.blake3.as_str()),
                    remote.as_ref().map(|f| f.size as i64),
                    remote_revision,
                    remote_revision_weak,
                    state,
                ],
            )?;
            let id = conn.query_row(
                "SELECT id FROM sync_entries WHERE job_id = ?1 AND relative_path = ?2",
                params![job_id, relative_path],
                |row| row.get(0),
            )?;
            Ok(SyncWriteResult::Id(id))
        }
        SyncWrite::CommitBaseline {
            entry_id,
            fingerprint,
            remote_revision,
        } => {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_entries SET
                        local_fingerprint = ?1, local_size = ?2,
                        remote_fingerprint = ?1, remote_size = ?2,
                        remote_revision = ?3,
                        baseline_fingerprint = ?1, baseline_size = ?2,
                        baseline_remote_revision = ?3, state = 'synced',
                        updated_at = unixepoch() WHERE id = ?4",
                params![
                    fingerprint.blake3,
                    fingerprint.size as i64,
                    remote_revision,
                    entry_id
                ],
            )?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::RecordConflict {
            job_id,
            entry_id,
            kind,
            local,
            remote,
            remote_revision,
        } => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE sync_entries SET state = 'conflict', updated_at = unixepoch() WHERE id = ?1",
                [entry_id],
            )?;
            let updated = tx.execute(
                "UPDATE sync_conflicts SET kind = ?3, local_fingerprint = ?4,
                        remote_fingerprint = ?5, remote_revision = ?6
                 WHERE job_id = ?1 AND entry_id = ?2 AND state = 'open'",
                params![
                    job_id,
                    entry_id,
                    kind,
                    local.as_ref().map(|f| f.blake3.as_str()),
                    remote.as_ref().map(|f| f.blake3.as_str()),
                    remote_revision
                ],
            )?;
            if updated == 0 {
                tx.execute(
                    "INSERT INTO sync_conflicts
                     (job_id, entry_id, kind, local_fingerprint, remote_fingerprint, remote_revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        job_id,
                        entry_id,
                        kind,
                        local.as_ref().map(|f| f.blake3.as_str()),
                        remote.as_ref().map(|f| f.blake3.as_str()),
                        remote_revision
                    ],
                )?;
            }
            tx.commit()?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::PrepareTask {
            operation_id,
            job_id,
            entry_id,
            action,
            expected_revision,
            expected_weak,
            artifact_path,
            artifact_hash,
            config_generation,
            entry_generation,
        } => {
            let conn = pool.get()?;
            conn.execute(
                "INSERT INTO sync_tasks
                 (operation_id, job_id, entry_id, action, state, expected_revision,
                  expected_weak, artifact_path, artifact_hash, config_generation,
                  entry_generation)
                 VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    operation_id,
                    job_id,
                    entry_id,
                    action,
                    expected_revision,
                    expected_weak,
                    artifact_path.to_string_lossy(),
                    artifact_hash,
                    config_generation,
                    entry_generation,
                ],
            )?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::SetTaskState {
            operation_id,
            state,
            error,
        } => {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_tasks SET state = ?1, last_error = ?2,
                        updated_at = unixepoch() WHERE operation_id = ?3",
                params![state, error, operation_id],
            )?;
            Ok(SyncWriteResult::None)
        }
        SyncWrite::ResolveConflict {
            conflict_id,
            entry_id,
            resolution,
            fingerprint,
            remote_revision,
        } => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            let updated = tx.execute(
                "UPDATE sync_conflicts SET state = 'resolved', resolution = ?1,
                        resolved_at = unixepoch()
                 WHERE id = ?2 AND entry_id = ?3 AND state = 'open'",
                params![resolution, conflict_id, entry_id],
            )?;
            if updated != 1 {
                return Err(AppError::Backend(
                    "synchronization conflict is no longer open".into(),
                ));
            }
            tx.execute(
                "UPDATE sync_entries SET
                        local_fingerprint = ?1, local_size = ?2,
                        remote_fingerprint = ?1, remote_size = ?2,
                        remote_revision = ?3,
                        baseline_fingerprint = ?1, baseline_size = ?2,
                        baseline_remote_revision = ?3, state = 'synced',
                        updated_at = unixepoch() WHERE id = ?4",
                params![
                    fingerprint.blake3,
                    fingerprint.size as i64,
                    remote_revision,
                    entry_id
                ],
            )?;
            tx.commit()?;
            Ok(SyncWriteResult::None)
        }
    }
}

fn validate_new_job(job: &NewSyncJob) -> Result<()> {
    if !job.local_root.is_absolute() {
        return Err(AppError::Backend(
            "synchronization local root must be absolute".into(),
        ));
    }
    if job.remote_root.contains("..") {
        return Err(AppError::Backend(
            "synchronization remote root cannot contain parent traversal".into(),
        ));
    }
    if !job.local_root.is_dir() {
        return Err(AppError::Backend(
            "synchronization local root must be an existing directory".into(),
        ));
    }
    if job.remote_root.trim_matches('/').is_empty()
        || job
            .remote_root
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return Err(AppError::Backend(
            "synchronization remote root is invalid".into(),
        ));
    }
    normalize_relative_albums(&job.upload_albums)?;
    let endpoint = url::Url::parse(&job.endpoint)
        .map_err(|error| AppError::Backend(format!("invalid WebDAV endpoint: {error}")))?;
    if endpoint.scheme() != "https" && endpoint.host_str() != Some("localhost") {
        return Err(AppError::Backend(
            "WebDAV endpoint must use HTTPS (HTTP is allowed only for localhost tests)".into(),
        ));
    }
    Ok(())
}

fn normalize_relative_albums(albums: &[String]) -> Result<Vec<String>> {
    let mut normalized = BTreeSet::new();
    for album in albums {
        let album = album.trim_matches('/');
        if !album.is_empty()
            && album
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
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

fn remote_paths_overlap(left: &str, right: &str) -> bool {
    let left = left.trim_matches('/');
    let right = right.trim_matches('/');
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncJob> {
    let direction: String = row.get(7)?;
    let direction = SyncDirection::parse(&direction).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Text,
            format!("invalid sync direction: {direction}").into(),
        )
    })?;
    let upload_scope: String = row.get(11)?;
    let upload_scope = UploadScope::parse(&upload_scope).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            11,
            rusqlite::types::Type::Text,
            format!("invalid upload scope: {upload_scope}").into(),
        )
    })?;
    Ok(SyncJob {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        endpoint: row.get(2)?,
        username: row.get(3)?,
        credential_ref: row.get(4)?,
        local_root: PathBuf::from(row.get::<_, String>(5)?),
        remote_root: row.get(6)?,
        direction,
        upload_scope,
        propagate_deletes: row.get::<_, i64>(8)? != 0,
        paused: row.get::<_, i64>(9)? != 0,
        config_generation: row.get(10)?,
        last_error: row.get(12)?,
    })
}

fn fingerprint(hash: Option<String>, size: Option<i64>) -> Option<Fingerprint> {
    Some(Fingerprint {
        blake3: hash?,
        size: u64::try_from(size?).ok()?,
    })
}

#[cfg(test)]
mod tests;
