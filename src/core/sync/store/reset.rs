//! An explicit global opt-out forgets history while keeping saved configuration.
use super::*;
use serde::{Deserialize, Serialize};

const RESET_KEY: &str = "sync_reset_pending";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResetIntent {
    pub token: String,
    pub error: Option<String>,
}

fn encode_error(error: impl std::fmt::Display) -> AppError {
    AppError::Backend(format!("invalid sync reset intent: {error}"))
}

fn read(conn: &rusqlite::Connection) -> Result<Option<ResetIntent>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            [RESET_KEY],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|json| serde_json::from_str(&json).map_err(encode_error))
        .transpose()
}

pub(super) fn pending(conn: &rusqlite::Connection) -> Result<bool> {
    Ok(read(conn)?.is_some())
}

impl SyncStore {
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
    let intent =
        serde_json::to_string(&ResetIntent { token, error: None }).map_err(encode_error)?;
    tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![RESET_KEY, intent])?;
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
    if let Some(mut intent) = read(&tx)? {
        if intent.token == token {
            intent.error = Some(error);
            tx.execute(
                "UPDATE settings SET value=?1 WHERE key=?2",
                params![
                    serde_json::to_string(&intent).map_err(encode_error)?,
                    RESET_KEY
                ],
            )?;
        }
    }
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
        let mut stmt = tx.prepare("SELECT job_id,payload FROM sync_job_changes ORDER BY job_id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, json) in changes {
        let change: PendingChange = serde_json::from_str(&json).map_err(encode_error)?;
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
        DELETE FROM sync_tasks;
        DELETE FROM sync_conflicts;
        DELETE FROM sync_entries;
        DELETE FROM sync_job_changes;
        UPDATE sync_jobs SET paused=0,config_generation=config_generation+1,
            last_started_at=NULL,last_completed_at=NULL,last_error=NULL;",
    )?;
    tx.execute("DELETE FROM settings WHERE key=?1", [RESET_KEY])?;
    tx.commit()?;
    Ok(SyncWriteResult::Changed(true))
}
