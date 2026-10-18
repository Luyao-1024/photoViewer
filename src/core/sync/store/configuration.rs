//! Durable desired edits are separate from the active cycle's context.
use super::*;
use serde::{Deserialize, Serialize};

/// An accepted edit that has not been installed yet. It is written before any
/// quiescence so an exit mid-transition can recover the user's latest choice
/// rather than losing it in memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PendingChange {
    pub revision: i64,
    pub upload_albums: Option<Vec<String>>,
    #[serde(default)]
    pub direction: Option<SyncDirection>,
    pub delete: bool,
}

#[derive(Debug, Clone)]
pub enum JobEdit {
    Direction(SyncDirection),
    UploadAlbums(Vec<String>),
    Delete,
}

/// Protocol/legacy decoding errors. Active SQLite state is relational.
pub(super) fn json_error(error: impl std::fmt::Display) -> AppError {
    AppError::Backend(format!("invalid synchronization JSON document: {error}"))
}

fn read(conn: &rusqlite::Connection, id: i64) -> Result<Option<PendingChange>> {
    super::state::read_change(conn, id)
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

    /// The job as the user will see it once the pending edit is applied.
    ///
    /// The active generation stays separate: a caller that needs "is this still
    /// being applied?" asks for the pending revision rather than reading a
    /// desired value and calling the transition complete.
    pub fn desired_job(&self, id: i64) -> Result<Option<SyncJob>> {
        let Some(mut job) = self.get_job(id)? else {
            return Ok(None);
        };
        if let Some(change) = self.pending_change(id)? {
            if let Some(direction) = change.direction {
                job.direction = direction;
            }
            if change.upload_albums.is_some() {
                job.upload_scope = UploadScope::SelectedAlbums;
            }
        }
        Ok(Some(job))
    }

    pub fn queue_edit(&self, id: i64, edit: JobEdit) -> Result<bool> {
        // Normalize before comparing: a re-submitted value that only differs in
        // order or duplicates is not a change, and must not restart anything.
        let edit = match edit {
            JobEdit::UploadAlbums(albums) => {
                JobEdit::UploadAlbums(normalize_relative_albums(&albums)?)
            }
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
    let (scope, generation) = tx
        .query_row(
            "SELECT upload_scope, config_generation FROM sync_jobs WHERE id=?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::Backend(format!("sync job {id} no longer exists")))?;
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
        JobEdit::Direction(direction) => {
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
            pending.upload_albums = Some(albums);
        }
        JobEdit::Delete => pending.delete = true,
    }
    pending.revision = pending.revision.max(generation) + 1;
    super::state::write_change(&tx, id, &pending)?;
    tx.execute("UPDATE sync_jobs SET last_error=NULL WHERE id=?1", [id])?;
    tx.commit()?;
    Ok(SyncWriteResult::Changed(true))
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
    // A newer edit landed while this transition waited for quiescence; it wins,
    // so the old worker cannot install a superseded configuration.
    if pending.revision != revision {
        return Ok(SyncWriteResult::Changed(false));
    }
    let paused: bool = tx.query_row("SELECT paused FROM sync_jobs WHERE id=?1", [id], |r| {
        r.get(0)
    })?;
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
        tx.execute(
            "UPDATE sync_jobs SET direction=?1 WHERE id=?2",
            params![direction.as_str(), id],
        )?;
    }
    // The configuration generation is the worker's invalidation token: an
    // in-flight cycle that captured the previous value discards its results.
    tx.execute("UPDATE sync_jobs SET config_generation=?1,paused=0,last_error=NULL,last_started_at=NULL,last_completed_at=NULL WHERE id=?2",params![revision,id])?;
    tx.execute("DELETE FROM sync_job_changes WHERE job_id=?1", [id])?;
    tx.commit()?;
    Ok(SyncWriteResult::Changed(true))
}
