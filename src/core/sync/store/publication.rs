//! Resource work and publication intent.
//!
//! Two tables carry everything the transfer stages need to be restartable:
//!
//! * `sync_native_resources` — one row per photo role per generation, holding
//!   the size and digest the cloud promised and how far this client got. It is
//!   an *obligation*, not a queue of things to try: a row is pending from the
//!   moment a projection names it.
//! * `sync_publications` — one row per photo whose pair has to become usable
//!   locally, holding the captured local versions and the last complete file.
//!
//! The invariant everything else rests on: **a photo becomes visible only when
//! both of its roles are verified and the file it names is on disk.** Nothing
//! here writes `media_items`; admission goes through the ordinary media write
//! path once — and only once — the pair is proven.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::core::db::DbPool;
use crate::core::error::{AppError, Result};

use super::{SyncStore, SyncWrite, SyncWriteResult};

/// Where one resource stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    /// Promised by a projection, not yet fetched.
    Pending,
    /// Bytes are in owned staging and match the declared digest.
    Staged,
    /// The file is published and the role's local artifact (cache entry) exists.
    Verified,
    /// The promise cannot be met as stated. Never silently retried as if new.
    Failed,
}

impl ResourceState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Staged => "staged",
            Self::Verified => "verified",
            Self::Failed => "failed",
        }
    }
}

/// One resource the cloud owes this client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceWork {
    pub job_id: i64,
    pub generation: String,
    pub asset_id: String,
    pub role: String,
    pub record_name: Option<String>,
    pub expected_sha256: Option<String>,
    pub expected_size: Option<i64>,
    pub state: ResourceState,
    pub staging_path: Option<String>,
    pub verified_cache: Option<String>,
}

/// A photo whose pair must become usable locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationIntent {
    pub id: i64,
    pub publication_id: String,
    pub job_id: i64,
    pub entry_id: Option<i64>,
    pub photo_id: String,
    pub master_id: Option<String>,
    pub original_asset_id: Option<String>,
    pub thumbnail_asset_id: Option<String>,
    pub state: String,
    pub original_staging: Option<String>,
    pub thumbnail_staging: Option<String>,
}

#[derive(Debug)]
pub enum PublicationCommand {
    /// Read the obligations a generation owes, original before thumbnail.
    Outstanding { job_id: i64 },
    /// Read the publications this job has not finished.
    Intents { job_id: i64 },
    /// Record that a role's bytes are staged and verified.
    Stage {
        job_id: i64,
        asset_id: String,
        role: String,
        staging_path: String,
    },
    /// Record that a role is published and its local artifact exists.
    Verify {
        job_id: i64,
        asset_id: String,
        role: String,
        verified_cache: String,
    },
    Fail {
        job_id: i64,
        asset_id: String,
        role: String,
        error: String,
    },
    /// Open or refresh the intent for a photo whose pair is now owed.
    Begin {
        job_id: i64,
        entry_id: Option<i64>,
        publication_id: String,
        photo_id: String,
        master_id: Option<String>,
        original_asset_id: Option<String>,
        thumbnail_asset_id: Option<String>,
    },
    /// Ask whether a publication's pair is fully verified. Read-only: the
    /// answer does not change the intent, so a caller that then fails to admit
    /// can try again next cycle.
    Ready { publication_id: String },
    /// Mark a publication complete. Only valid after admission succeeded —
    /// marking earlier would lose the photo if admission then failed.
    Complete { publication_id: String },
}

#[derive(Debug)]
pub enum PublicationOutcome {
    Outstanding(Vec<ResourceWork>),
    Intents(Vec<PublicationIntent>),
    Done(bool),
}

impl SyncStore {
    fn write_publication(&self, command: PublicationCommand) -> Result<PublicationOutcome> {
        match self.write(SyncWrite::Publication(Box::new(command)))? {
            SyncWriteResult::Publication(outcome) => Ok(outcome),
            _ => Err(AppError::Backend(
                "a publication write returned an unexpected result".into(),
            )),
        }
    }

    /// Resource obligations this job still owes, in the order they should be
    /// attempted. A photo's original comes before its thumbnail: the thumbnail
    /// is generated from the original, and publishing it first would leave a
    /// visible tile backed by a file nothing else agrees with.
    pub fn outstanding_resources(&self, job_id: i64) -> Result<Vec<ResourceWork>> {
        if self.native_progress(job_id)?.is_none() {
            return Err(AppError::Backend("no receive position exists yet".into()));
        }
        match self.write_publication(PublicationCommand::Outstanding { job_id })? {
            PublicationOutcome::Outstanding(work) => Ok(work),
            _ => Err(AppError::Backend(
                "reading outstanding resources returned an unexpected result".into(),
            )),
        }
    }

    pub fn stage_resource(
        &self,
        job_id: i64,
        asset_id: &str,
        role: &str,
        staging_path: &str,
    ) -> Result<()> {
        self.write_publication(PublicationCommand::Stage {
            job_id,
            asset_id: asset_id.into(),
            role: role.into(),
            staging_path: staging_path.into(),
        })?;
        Ok(())
    }

    pub fn verify_resource(
        &self,
        job_id: i64,
        asset_id: &str,
        role: &str,
        verified_cache: &str,
    ) -> Result<()> {
        self.write_publication(PublicationCommand::Verify {
            job_id,
            asset_id: asset_id.into(),
            role: role.into(),
            verified_cache: verified_cache.into(),
        })?;
        Ok(())
    }

    pub fn fail_resource(
        &self,
        job_id: i64,
        asset_id: &str,
        role: &str,
        error: &str,
    ) -> Result<()> {
        self.write_publication(PublicationCommand::Fail {
            job_id,
            asset_id: asset_id.into(),
            role: role.into(),
            error: error.into(),
        })?;
        Ok(())
    }

    pub fn begin_publication(
        &self,
        job_id: i64,
        entry_id: Option<i64>,
        publication_id: &str,
        photo_id: &str,
        master_id: Option<&str>,
        original_asset_id: Option<&str>,
        thumbnail_asset_id: Option<&str>,
    ) -> Result<()> {
        self.write_publication(PublicationCommand::Begin {
            job_id,
            entry_id,
            publication_id: publication_id.into(),
            photo_id: photo_id.into(),
            master_id: master_id.map(str::to_string),
            original_asset_id: original_asset_id.map(str::to_string),
            thumbnail_asset_id: thumbnail_asset_id.map(str::to_string),
        })?;
        Ok(())
    }

    /// Whether the pair is fully verified. Read-only, so a failed admission
    /// can be retried.
    pub fn publication_ready(&self, publication_id: &str) -> Result<bool> {
        match self.write_publication(PublicationCommand::Ready {
            publication_id: publication_id.into(),
        })? {
            PublicationOutcome::Done(ready) => Ok(ready),
            _ => Err(AppError::Backend(
                "checking a publication returned an unexpected result".into(),
            )),
        }
    }

    /// Mark a publication complete, refusing unless both roles verified.
    ///
    /// Only call this once admission succeeded: a completed-but-unadmitted
    /// photo would never be revisited.
    pub fn complete_publication(&self, publication_id: &str) -> Result<bool> {
        match self.write_publication(PublicationCommand::Complete {
            publication_id: publication_id.into(),
        })? {
            PublicationOutcome::Done(complete) => Ok(complete),
            _ => Err(AppError::Backend(
                "completing a publication returned an unexpected result".into(),
            )),
        }
    }

    pub fn pending_publications(&self, job_id: i64) -> Result<Vec<PublicationIntent>> {
        match self.write_publication(PublicationCommand::Intents { job_id })? {
            PublicationOutcome::Intents(intents) => Ok(intents),
            _ => Err(AppError::Backend(
                "reading pending publications returned an unexpected result".into(),
            )),
        }
    }
}

pub(super) fn execute(pool: &DbPool, command: &PublicationCommand) -> Result<PublicationOutcome> {
    match command {
        PublicationCommand::Outstanding { job_id } => {
            let conn = pool.get()?;
            let generation: Option<String> = conn
                .query_row(
                    "SELECT database_generation FROM sync_native_progress WHERE job_id = ?1",
                    [job_id],
                    |row| row.get(0),
                )
                .optional()?;
            let mut obligations = Vec::new();
            if generation.is_some() {
                let mut stmt = conn.prepare(
                    "SELECT job_id, generation, asset_id, role, record_name, expected_sha256,
                            expected_size, state, staging_path, verified_cache
                     FROM sync_native_resources
                     WHERE job_id = ?1 AND state IN ('pending', 'staged')
                     ORDER BY record_name,
                              CASE role WHEN 'original' THEN 0 ELSE 1 END",
                )?;
                let rows = stmt.query_map([job_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                    ))
                })?;
                for row in rows {
                    let (
                        job_id,
                        generation,
                        asset_id,
                        role,
                        record_name,
                        expected_sha256,
                        expected_size,
                        state,
                        staging_path,
                        verified_cache,
                    ) = row?;
                    obligations.push(ResourceWork {
                        job_id,
                        generation,
                        asset_id,
                        role,
                        record_name,
                        expected_sha256,
                        expected_size,
                        state: parse_state(&state),
                        staging_path,
                        verified_cache,
                    });
                }
            }
            Ok(PublicationOutcome::Outstanding(obligations))
        }
        PublicationCommand::Intents { job_id } => {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT id, publication_id, job_id, entry_id, photo_id, master_id,
                        original_asset_id, thumbnail_asset_id, state,
                        original_staging, thumbnail_staging
                 FROM sync_publications
                 WHERE job_id = ?1 AND state <> 'complete'
                 ORDER BY id",
            )?;
            let rows = stmt.query_map([job_id], |row| {
                Ok(PublicationIntent {
                    id: row.get(0)?,
                    publication_id: row.get(1)?,
                    job_id: row.get(2)?,
                    entry_id: row.get(3)?,
                    photo_id: row.get(4)?,
                    master_id: row.get(5)?,
                    original_asset_id: row.get(6)?,
                    thumbnail_asset_id: row.get(7)?,
                    state: row.get(8)?,
                    original_staging: row.get(9)?,
                    thumbnail_staging: row.get(10)?,
                })
            })?;
            let mut intents = Vec::new();
            for row in rows {
                intents.push(row?);
            }
            Ok(PublicationOutcome::Intents(intents))
        }
        PublicationCommand::Stage {
            job_id,
            asset_id,
            role,
            staging_path,
        } => {
            let updated = set_resource_state(pool, *job_id, asset_id, role, ResourceState::Staged)?;
            if updated == 0 {
                return Err(AppError::Backend(format!(
                    "no resource obligation for {asset_id}/{role} on job {job_id}"
                )));
            }
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_native_resources SET staging_path = ?1, updated_at = unixepoch()
                 WHERE job_id = ?2 AND asset_id = ?3 AND role = ?4",
                params![staging_path, job_id, asset_id, role],
            )?;
            Ok(PublicationOutcome::Done(true))
        }
        PublicationCommand::Verify {
            job_id,
            asset_id,
            role,
            verified_cache,
        } => {
            let updated =
                set_resource_state(pool, *job_id, asset_id, role, ResourceState::Verified)?;
            if updated == 0 {
                return Err(AppError::Backend(format!(
                    "no resource obligation for {asset_id}/{role} on job {job_id}"
                )));
            }
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_native_resources SET verified_cache = ?1, updated_at = unixepoch()
                 WHERE job_id = ?2 AND asset_id = ?3 AND role = ?4",
                params![verified_cache, job_id, asset_id, role],
            )?;
            Ok(PublicationOutcome::Done(true))
        }
        PublicationCommand::Fail {
            job_id,
            asset_id,
            role,
            error,
        } => {
            set_resource_state(pool, *job_id, asset_id, role, ResourceState::Failed)?;
            let conn = pool.get()?;
            conn.execute(
                "UPDATE sync_native_resources SET staging_path = NULL, updated_at = unixepoch()
                 WHERE job_id = ?1 AND asset_id = ?2 AND role = ?3",
                params![job_id, asset_id, role],
            )?;
            let _ = error;
            Ok(PublicationOutcome::Done(true))
        }
        PublicationCommand::Begin {
            job_id,
            entry_id,
            publication_id,
            photo_id,
            master_id,
            original_asset_id,
            thumbnail_asset_id,
        } => {
            let conn = pool.get()?;
            // Re-opening an existing intent is idempotent: a retry after a
            // restart must not create a second one for the same photo.
            conn.execute(
                "INSERT INTO sync_publications
                    (publication_id, job_id, entry_id, photo_id, master_id,
                     original_asset_id, thumbnail_asset_id, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending')
                 ON CONFLICT(publication_id) DO UPDATE SET
                    entry_id = excluded.entry_id,
                    original_asset_id = excluded.original_asset_id,
                    thumbnail_asset_id = excluded.thumbnail_asset_id,
                    updated_at = unixepoch()",
                params![
                    publication_id,
                    job_id,
                    entry_id,
                    photo_id,
                    master_id,
                    original_asset_id,
                    thumbnail_asset_id,
                ],
            )?;
            Ok(PublicationOutcome::Done(true))
        }
        PublicationCommand::Ready { publication_id } => {
            let conn = pool.get()?;
            Ok(PublicationOutcome::Done(publication_is_ready(
                &conn,
                publication_id,
            )?))
        }
        PublicationCommand::Complete { publication_id } => {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            // Re-checked inside the marking transaction: the state that made it
            // ready must still hold when it is committed as complete.
            if !publication_is_ready(&tx, publication_id)? {
                return Ok(PublicationOutcome::Done(false));
            }
            tx.execute(
                "UPDATE sync_publications SET state = 'complete', updated_at = unixepoch()
                  WHERE publication_id = ?1",
                [publication_id],
            )?;
            tx.commit()?;
            Ok(PublicationOutcome::Done(true))
        }
    }
}

/// The gate. A photo's pair is ready when both of its roles are verified for
/// the generation the intent belongs to — which is what stops a half-downloaded
/// pair from becoming a visible tile.
fn publication_is_ready(conn: &rusqlite::Connection, publication_id: &str) -> Result<bool> {
    let job_id: i64 = conn.query_row(
        "SELECT job_id FROM sync_publications WHERE publication_id = ?1",
        [publication_id],
        |row| row.get(0),
    )?;
    let generation: String = conn.query_row(
        "SELECT database_generation FROM sync_native_progress WHERE job_id = ?1",
        [job_id],
        |row| row.get(0),
    )?;
    let pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sync_native_resources
          WHERE job_id = ?1 AND generation = ?2 AND record_name IS NOT NULL
            AND state <> 'verified'",
        params![job_id, generation],
        |row| row.get(0),
    )?;
    Ok(pending == 0)
}

fn set_resource_state(
    pool: &DbPool,
    job_id: i64,
    asset_id: &str,
    role: &str,
    state: ResourceState,
) -> Result<usize> {
    let conn = pool.get()?;
    Ok(conn.execute(
        "UPDATE sync_native_resources SET state = ?1, updated_at = unixepoch()
          WHERE job_id = ?2 AND asset_id = ?3 AND role = ?4",
        params![state.as_str(), job_id, asset_id, role],
    )?)
}

fn parse_state(value: &str) -> ResourceState {
    match value {
        "staged" => ResourceState::Staged,
        "verified" => ResourceState::Verified,
        "failed" => ResourceState::Failed,
        _ => ResourceState::Pending,
    }
}

#[cfg(test)]
mod tests;
