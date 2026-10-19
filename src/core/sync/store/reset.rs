//! An explicit global opt-out forgets history while keeping saved configuration.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResetIntent {
    pub token: String,
    pub error: Option<String>,
}

fn read(conn: &rusqlite::Connection) -> Result<Option<ResetIntent>> {
    conn.query_row(
        "SELECT token,last_error FROM sync_reset_state WHERE id=1",
        [],
        |r| {
            Ok(ResetIntent {
                token: r.get(0)?,
                error: r.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub(super) fn pending(conn: &rusqlite::Connection) -> Result<bool> {
    Ok(read(conn)?.is_some())
}

pub(super) fn local_changes_enabled(conn: &rusqlite::Connection) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT value <> '0' FROM settings WHERE key='native_sync_local_changes_enabled'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(true))
}

impl SyncStore {
    pub(crate) fn resume_local_changes(&self) -> Result<()> {
        let conn = self.pool.get()?;
        let resume = !local_changes_enabled(&conn)? && !pending(&conn)?;
        drop(conn);
        if resume {
            self.write(SyncWrite::ResumeLocalChanges)?;
        }
        Ok(())
    }

    pub fn reset_pending(&self) -> Result<bool> {
        Ok(self.reset_intent()?.is_some())
    }

    pub fn reset_error(&self) -> Result<Option<String>> {
        Ok(self.reset_intent()?.and_then(|intent| intent.error))
    }

    pub(crate) fn reset_local_roots(&self) -> Result<BTreeSet<PathBuf>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare("SELECT DISTINCT local_root FROM sync_jobs")?;
        let roots = stmt.query_map([], |row| Ok(PathBuf::from(row.get::<_, String>(0)?)))?;
        roots
            .collect::<rusqlite::Result<BTreeSet<_>>>()
            .map_err(AppError::from)
    }

    pub(crate) fn reset_intent(&self) -> Result<Option<ResetIntent>> {
        let conn = self.pool.get()?;
        read(&conn)
    }

    pub(crate) fn begin_reset(&self) -> Result<()> {
        self.write(SyncWrite::BeginReset {
            token: uuid::Uuid::new_v4().to_string(),
        })?;
        Ok(())
    }

    pub(crate) fn finish_reset(&self, token: &str) -> Result<bool> {
        match self.write(SyncWrite::FinishReset {
            token: token.into(),
        })? {
            SyncWriteResult::Changed(changed) => Ok(changed),
            _ => Err(AppError::Backend("unexpected sync reset result".into())),
        }
    }

    pub(crate) fn fail_reset(&self, token: &str, error: &str) -> Result<()> {
        self.write(SyncWrite::FailReset {
            token: token.into(),
            error: error.into(),
        })?;
        Ok(())
    }
}

pub(super) fn begin(pool: &DbPool, token: String) -> Result<SyncWriteResult> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    tx.execute("INSERT INTO sync_reset_state(id,token,last_error) VALUES(1,?1,NULL) ON CONFLICT(id) DO UPDATE SET token=excluded.token,last_error=NULL",[token])?;
    tx.execute("INSERT INTO settings(key,value) VALUES('native_sync_local_changes_enabled','0') ON CONFLICT(key) DO UPDATE SET value=excluded.value", [])?;
    // Monotonic generations also stop runs that do not monitor the UI switch.
    tx.execute(
        "UPDATE sync_jobs SET config_generation=config_generation+1",
        [],
    )?;
    tx.commit()?;
    Ok(SyncWriteResult::None)
}

pub(super) fn fail(pool: &DbPool, token: String, error: String) -> Result<SyncWriteResult> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE sync_reset_state SET last_error=?2 WHERE token=?1",
        params![token, error],
    )?;
    tx.commit()?;
    Ok(SyncWriteResult::None)
}

pub(super) fn finish(pool: &DbPool, token: String) -> Result<SyncWriteResult> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    if read(&tx)?.is_none_or(|intent| intent.token != token) {
        return Ok(SyncWriteResult::Changed(false));
    }
    let changes = {
        let mut stmt = tx.prepare("SELECT job_id FROM sync_job_changes ORDER BY job_id")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for id in changes {
        let Some(change) = super::state::read_change(&tx, id)? else {
            continue;
        };
        if change.delete {
            // Honor an explicit task removal, but keep the account/credential.
            tx.execute("DELETE FROM sync_jobs WHERE id=?1", [id])?;
            continue;
        }
        if let Some(direction) = change.direction {
            tx.execute(
                "UPDATE sync_jobs SET direction=?1 WHERE id=?2",
                params![direction.as_str(), id],
            )?;
        }
        if let Some(albums) = change.upload_albums {
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
        tx.execute(
            "UPDATE sync_jobs SET config_generation=max(config_generation,?1) WHERE id=?2",
            params![change.revision, id],
        )?;
        // Pending conflict actions are intentionally not replayed.
    }
    tx.execute_batch(
        "DELETE FROM media_items WHERE storage_location=2;
        UPDATE media_items SET storage_location=1 WHERE storage_location=3;
        DELETE FROM sync_publications;
        DELETE FROM sync_native_resources;
        DELETE FROM sync_native_progress;
        DELETE FROM sync_bootstrap_types;
        DELETE FROM sync_tasks;
        DELETE FROM sync_conflicts;
        DELETE FROM sync_entries;
        DELETE FROM sync_records;
        DELETE FROM sync_job_changes;
        UPDATE sync_jobs SET paused=0,config_generation=config_generation+1,
            last_started_at=NULL,last_completed_at=NULL,last_error=NULL;",
    )?;
    tx.execute("DELETE FROM sync_reset_state WHERE token=?1", [token])?;
    tx.commit()?;
    Ok(SyncWriteResult::Changed(true))
}

/// Resume tracking only after opt-in has crossed the durable reset barrier.
pub(super) fn resume_local_changes(pool: &DbPool) -> Result<SyncWriteResult> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    if !pending(&tx)? {
        tx.execute("UPDATE settings SET value='1' WHERE key='native_sync_local_changes_enabled' AND value='0'", [])?;
    }
    tx.commit()?;
    Ok(SyncWriteResult::None)
}
