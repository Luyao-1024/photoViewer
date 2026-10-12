//! Durable desired edits are separate from the active file-operation context.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PendingChange {
    pub revision: i64,
    pub upload_albums: Option<Vec<String>>,
    #[serde(default)]
    pub direction: Option<SyncDirection>,
    #[serde(default)]
    pub etag_is_sha256: Option<bool>,
    pub remote_root: Option<String>,
    pub delete: bool,
    pub conflict: Option<(SyncConflict, String)>,
}

#[derive(Debug, Clone)]
pub enum JobEdit {
    Direction(SyncDirection),
    EtagIsSha256(bool),
    UploadAlbums(Vec<String>),
    RemoteRoot(String),
    Delete,
    Conflict(SyncConflict, String),
}

fn json_error(error: impl std::fmt::Display) -> AppError {
    AppError::Backend(format!("invalid pending sync configuration: {error}"))
}

fn read(conn: &rusqlite::Connection, id: i64) -> Result<Option<PendingChange>> {
    let value: Option<String> = conn
        .query_row(
            "SELECT payload FROM sync_job_changes WHERE job_id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|value| serde_json::from_str(&value).map_err(json_error))
        .transpose()
}

impl SyncStore {
    pub fn pending_change(&self, id: i64) -> Result<Option<PendingChange>> {
        let conn = self.pool.get()?;
        read(&conn, id)
    }

    pub fn desired_upload_albums(&self, id: i64) -> Result<Vec<String>> {
        if let Some(albums) = self
            .pending_change(id)?
            .and_then(|change| change.upload_albums)
        {
            Ok(albums)
        } else {
            self.upload_albums(id)
        }
    }

    pub fn desired_job(&self, id: i64) -> Result<Option<SyncJob>> {
        let Some(mut job) = self.get_job(id)? else {
            return Ok(None);
        };
        if let Some(change) = self.pending_change(id)? {
            if let Some(enabled) = change.etag_is_sha256 {
                job.etag_is_sha256 = enabled;
            }
            if let Some(direction) = change.direction {
                job.direction = direction;
            }
            if let Some(root) = change.remote_root {
                job.remote_root = root;
            }
            if change.upload_albums.is_some() {
                job.upload_scope = UploadScope::SelectedAlbums;
            }
            // The active generation stays separate; callers can query the pending
            // revision to show applying status rather than claiming completion.
        }
        Ok(Some(job))
    }

    pub fn queue_edit(&self, id: i64, edit: JobEdit) -> Result<bool> {
        let edit = match edit {
            JobEdit::UploadAlbums(albums) => {
                JobEdit::UploadAlbums(normalize_relative_albums(&albums)?)
            }
            JobEdit::RemoteRoot(root) => JobEdit::RemoteRoot(validate_remote_root(&root)?),
            other => other,
        };
        match self.write(SyncWrite::QueueEdit { id, edit })? {
            SyncWriteResult::Changed(changed) => Ok(changed),
            _ => Err(AppError::Backend(
                "unexpected configuration acceptance result".into(),
            )),
        }
    }

    pub(crate) fn apply_change(&self, id: i64, revision: i64) -> Result<SyncWriteResult> {
        self.write(SyncWrite::ApplyChange { id, revision })
    }
}

pub(super) fn queue(pool: &DbPool, id: i64, edit: JobEdit) -> Result<SyncWriteResult> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let (connection, root, scope, generation) = tx.query_row(
        "SELECT connection_id, remote_root, upload_scope, config_generation FROM sync_jobs WHERE id=?1",
        [id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)),
    ).optional()?.ok_or_else(|| AppError::Backend(format!("sync job {id} no longer exists")))?;
    let mut pending = read(&tx, id)?.unwrap_or_default();
    if pending.delete {
        return match edit {
            JobEdit::Delete => Ok(SyncWriteResult::Changed(false)),
            _ => Err(AppError::Backend(
                "this synchronization task is being removed".into(),
            )),
        };
    }
    match edit {
        JobEdit::EtagIsSha256(enabled) => {
            let current: bool = tx.query_row(
                "SELECT etag_is_sha256 FROM sync_jobs WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            if pending.etag_is_sha256.unwrap_or(current) == enabled {
                return Ok(SyncWriteResult::Changed(false));
            }
            pending.etag_is_sha256 = Some(enabled);
        }
        JobEdit::Direction(direction) => {
            if direction == SyncDirection::DownloadOnly {
                return Err(AppError::Backend(
                    "download-only is not a supported synchronization strategy".into(),
                ));
            }
            let current: String =
                tx.query_row("SELECT direction FROM sync_jobs WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
            if pending
                .direction
                .map_or(current == direction.as_str(), |value| value == direction)
            {
                return Ok(SyncWriteResult::Changed(false));
            }
            pending.direction = Some(direction);
            // A choice made under the other strategy must be reviewed again.
            pending.conflict = None;
        }
        JobEdit::UploadAlbums(albums) => {
            let effective = if let Some(albums) = &pending.upload_albums {
                albums.clone()
            } else {
                let mut stmt = tx.prepare("SELECT relative_album FROM sync_job_upload_albums WHERE job_id=?1 ORDER BY relative_album")?;
                let rows = stmt.query_map([id], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            if (pending.upload_albums.is_some() || scope == "selected_albums")
                && effective == albums
            {
                return Ok(SyncWriteResult::Changed(false));
            }
            if let Some((conflict, choice)) = &pending.conflict {
                let album = conflict
                    .relative_path
                    .rsplit_once('/')
                    .map_or("", |(parent, _)| parent);
                if choice != "use_remote" && !albums.iter().any(|value| value == album) {
                    pending.conflict = None;
                }
            }
            pending.upload_albums = Some(albums);
        }
        JobEdit::RemoteRoot(new_root) => {
            if pending.remote_root.as_ref().unwrap_or(&root) == &new_root {
                return Ok(SyncWriteResult::Changed(false));
            }
            validate_other_roots(&tx, connection, id, &new_root)?;
            pending.remote_root = Some(new_root);
            // A conflict choice belongs to its old root, never the new namespace.
            pending.conflict = None;
        }
        JobEdit::Delete => {
            pending.delete = true;
            pending.conflict = None;
        }
        JobEdit::Conflict(conflict, choice) => {
            let current = tx
                .query_row(
                    "SELECT state FROM sync_conflicts WHERE id=?1 AND job_id=?2",
                    params![conflict.id, id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            if current.as_deref() != Some("open")
                || pending.remote_root.as_ref().is_some_and(|new| new != &root)
            {
                return Err(AppError::Backend(
                    "conflict changed or its cloud folder is being replaced".into(),
                ));
            }
            if pending.conflict.as_ref() == Some(&(conflict.clone(), choice.clone())) {
                return Ok(SyncWriteResult::Changed(false));
            }
            pending.conflict = Some((conflict, choice));
        }
    }
    pending.revision = pending.revision.max(generation) + 1;
    let payload = serde_json::to_string(&pending).map_err(json_error)?;
    tx.execute("INSERT INTO sync_job_changes(job_id,payload) VALUES(?1,?2) ON CONFLICT(job_id) DO UPDATE SET payload=excluded.payload", params![id,payload])?;
    tx.execute("UPDATE sync_jobs SET last_error=NULL WHERE id=?1", [id])?;
    tx.commit()?;
    Ok(SyncWriteResult::Changed(true))
}

pub(super) fn validate_other_roots(
    conn: &rusqlite::Connection,
    connection: i64,
    id: i64,
    root: &str,
) -> Result<()> {
    let mut stmt = conn.prepare("SELECT j.remote_root,c.payload FROM sync_jobs j LEFT JOIN sync_job_changes c ON c.job_id=j.id WHERE j.connection_id=?1 AND j.id!=?2")?;
    let rows = stmt.query_map(params![connection, id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (active, payload) = row?;
        let desired: Option<PendingChange> = payload
            .map(|value| serde_json::from_str(&value).map_err(json_error))
            .transpose()?;
        if remote_paths_overlap(&active, root)
            || desired
                .and_then(|c| c.remote_root)
                .is_some_and(|other| remote_paths_overlap(&other, root))
        {
            return Err(AppError::Backend(
                "the selected cloud folder overlaps another task on this connection".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn apply(pool: &DbPool, id: i64, revision: i64) -> Result<SyncWriteResult> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    if super::reset::pending(&tx)? {
        return Ok(SyncWriteResult::Changed(false));
    }
    let Some(pending) = read(&tx, id)? else {
        return Ok(SyncWriteResult::Changed(false));
    };
    if pending.revision != revision {
        return Ok(SyncWriteResult::Changed(false));
    }
    let (connection, root, paused) = tx.query_row(
        "SELECT connection_id,remote_root,paused FROM sync_jobs WHERE id=?1",
        [id],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        },
    )?;
    if !paused {
        return Err(AppError::Backend(
            "internal configuration transition is not quiescent".into(),
        ));
    }
    if pending.delete {
        drop(tx);
        drop(conn);
        return super::execute_write(pool, SyncWrite::DeleteJob { id });
    }
    if let Some(new_root) = &pending.remote_root {
        if new_root != &root {
            validate_other_roots(&tx, connection, id, new_root)?;
            let unfinished: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_tasks WHERE job_id=?1 AND state NOT IN ('succeeded','superseded','cancelled'))", [id], |r| r.get(0))?;
            if unfinished {
                return Err(AppError::Backend("finish recovery of the old cloud folder before applying its pending replacement".into()));
            }
            tx.execute("DELETE FROM sync_tasks WHERE job_id=?1", [id])?;
            tx.execute("DELETE FROM sync_entries WHERE job_id=?1", [id])?;
            tx.execute("UPDATE sync_jobs SET remote_root=?1,last_started_at=NULL,last_completed_at=NULL WHERE id=?2",params![new_root,id])?;
        }
    }
    if let Some(albums) = &pending.upload_albums {
        tx.execute("DELETE FROM sync_job_upload_albums WHERE job_id=?1", [id])?;
        for album in albums {
            tx.execute(
                "INSERT INTO sync_job_upload_albums(job_id,relative_album) VALUES(?1,?2)",
                params![id, album],
            )?;
        }
        tx.execute(
            "UPDATE sync_jobs SET upload_scope='selected_albums' WHERE id=?1",
            [id],
        )?;
    }
    if let Some(direction) = pending.direction {
        if direction == SyncDirection::UploadOnly {
            // Backup's explicit local-authoritative choice supersedes old
            // bidirectional choices. Failed archival is reported as a task error.
            tx.execute("UPDATE sync_conflicts SET state='resolved',resolution='strategy_backup',resolved_at=unixepoch() WHERE job_id=?1 AND state='open'", [id])?;
            tx.execute("UPDATE sync_entries SET remote_fingerprint=NULL,remote_size=NULL WHERE job_id=?1 AND local_fingerprint IS NOT NULL", [id])?;
        }
        tx.execute(
            "UPDATE sync_jobs SET direction=?1 WHERE id=?2",
            params![direction.as_str(), id],
        )?;
    }
    if let Some(enabled) = pending.etag_is_sha256 {
        tx.execute(
            "UPDATE sync_jobs SET etag_is_sha256=?1 WHERE id=?2",
            params![enabled, id],
        )?;
    }
    tx.execute("UPDATE sync_jobs SET config_generation=?1,paused=0,last_error=NULL,last_started_at=NULL,last_completed_at=NULL WHERE id=?2",params![revision,id])?;
    tx.execute("DELETE FROM sync_job_changes WHERE job_id=?1", [id])?;
    tx.commit()?;
    Ok(SyncWriteResult::Changed(true))
}
