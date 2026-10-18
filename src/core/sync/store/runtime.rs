//! Actor-owned cycle and issued-operation evidence. Dirty remains on sync_entries.
use super::dirty::NativeDirty;
use super::{SyncStore, SyncWrite, SyncWriteResult};
use crate::core::{
    db::DbPool,
    error::{AppError, Result},
    sync::{content::ContentDigest, native::UploadLease},
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Epoch {
    pub job: i64,
    pub config: i64,
    pub cycle: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issued {
    pub id: String,
    pub job: i64,
    pub config: i64,
    pub entry: i64,
    pub generation: String,
    pub phase: String,
    pub versions: [i64; 4],
    pub dirty: u8,
    pub photo: String,
    pub master: String,
    pub content_generation: String,
    pub body: Value,
    #[serde(default)]
    pub preparation: Value,
}

#[derive(Debug)]
pub enum Command {
    ThumbnailReady {
        publication: String,
        media: i64,
        mtime_ns: i64,
        asset: String,
    },
    DiscardPendingDeletions {
        job: i64,
    },
    ConfirmAbsentDeletion {
        epoch: Epoch,
        entry: i64,
        version: i64,
        lease: UploadLease,
    },
    ClearAccount,
    SelectAccount {
        job: Option<i64>,
    },
    AcceptAuthentication {
        connection: i64,
        expected: Box<super::super::NativePinnedScope>,
        updated: Box<super::super::NativePinnedScope>,
    },
    RetireIssued {
        epoch: Epoch,
        operation: Issued,
        rejected: bool,
        /// The service's own reason for a rejection, kept verbatim.
        ///
        /// Without it a refusal is filed as an unexplained failure, which is
        /// indistinguishable from a transient one and gets retried against a
        /// service that has already given its final answer.
        reason: String,
    },
    UserAction {
        job: i64,
        entry: i64,
        version: i64,
        conflict: Option<String>,
        action: super::PhotoAction,
        recovery: Option<String>,
    },
    UploadFailed {
        epoch: Epoch,
        entry: i64,
        reason: String,
    },
    Begin(Epoch),
    End(Epoch),
    Save(Epoch, Issued),
    Enroll {
        epoch: Epoch,
    },
    ObserveLocal {
        job: i64,
        paths: Vec<String>,
    },
    BeginCollision {
        id: String,
        capture: Value,
        details: Value,
    },
    FinishCollision {
        id: String,
    },
    AbortCollision {
        id: String,
    },
    ForgetIdentityForAdoption {
        job: i64,
        entry: i64,
        version: i64,
    },
    Confirm {
        epoch: Epoch,
        operation: Issued,
        response: Value,
        lease: UploadLease,
    },
    Publishing {
        id: String,
        details: Value,
    },
    Rollback {
        id: String,
    },
    CleanPublication {
        id: String,
    },
    Admit {
        id: String,
        photo: super::super::RemotePhoto,
        item: Box<crate::core::media::NewMediaItem>,
    },
    Conflict {
        job: i64,
        entry: i64,
        detail: String,
    },
}

fn invalid(message: &str) -> AppError {
    AppError::Backend(message.to_string())
}
fn valid_epoch(tx: &Transaction<'_>, epoch: &Epoch) -> Result<()> {
    let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_jobs j JOIN sync_connections c ON c.id=j.connection_id WHERE j.id=?1 AND j.config_generation=?2 AND j.native_cycle_id=?3 AND j.paused=0 AND c.enabled=1 AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=j.id))",params![epoch.job,epoch.config,epoch.cycle],|r|r.get(0))?;
    if !valid {
        return Err(invalid(
            "synchronization configuration/cycle was superseded",
        ));
    }
    Ok(())
}

impl SyncStore {
    pub(crate) fn upload_error(&self, job: i64, entry: i64) -> Result<Option<String>> {
        self.pool
            .get()?
            .query_row(
                "SELECT native_upload_error FROM sync_entries WHERE job_id=?1 AND id=?2",
                params![job, entry],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
    pub(crate) fn runtime(&self, command: Command) -> Result<()> {
        match self.write(SyncWrite::Runtime(Box::new(command)))? {
            SyncWriteResult::None => Ok(()),
            _ => Err(invalid("unexpected runtime write response")),
        }
    }
    pub(crate) fn epoch_current(&self, epoch: &Epoch) -> Result<bool> {
        let conn = self.pool.get()?;
        conn.query_row("SELECT EXISTS(SELECT 1 FROM sync_jobs j JOIN sync_connections c ON c.id=j.connection_id WHERE j.id=?1 AND j.config_generation=?2 AND j.native_cycle_id=?3 AND j.paused=0 AND c.enabled=1 AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=j.id) AND NOT EXISTS(SELECT 1 FROM sync_reset_state WHERE id=1))",params![epoch.job,epoch.config,epoch.cycle],|r|r.get(0)).map_err(Into::into)
    }
    pub(crate) fn pending_issued(&self, job: i64) -> Result<Vec<Issued>> {
        let conn = self.pool.get()?;
        let ids = {
            let mut stmt=conn.prepare("SELECT operation_id FROM sync_tasks WHERE job_id=?1 AND state='issued' ORDER BY id")?;
            let rows = stmt
                .query_map([job], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        ids.into_iter()
            .map(|id| {
                super::issued::read(&conn, &id)?
                    .ok_or_else(|| invalid("issued operation has no captured state"))
            })
            .collect()
    }
    pub(crate) fn local_metadata(&self, entry: i64) -> Result<Value> {
        let conn = self.pool.get()?;
        let (favorite,date,path,trashed):(Option<i64>,Option<i64>,String,bool)=conn.query_row("SELECT m.is_favorite,m.taken_at,e.relative_path,m.trashed_at IS NOT NULL FROM sync_entries e LEFT JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[entry],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
        let baseline_ms = super::records::load_photo(&conn, "baseline", &entry.to_string())?
            .and_then(|photo| photo.asset_date_ms);
        let date_ms = date.map(|seconds| {
            baseline_ms
                .filter(|ms| ms.div_euclid(1000) == seconds)
                .unwrap_or_else(|| seconds.saturating_mul(1000))
        });
        Ok(
            json!({"isTrashed":{"type":"INT64","value":trashed as i64},"isFavorite":{"type":"INT64","value":favorite.unwrap_or(0)},"assetDate":date_ms.map(|ms|json!({"type":"TIMESTAMP","value":ms})),"relativePath":{"type":"STRING","value":path}}),
        )
    }
}

pub(super) fn execute(pool: &DbPool, command: Command) -> Result<()> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    match command {
        Command::ThumbnailReady {
            publication,
            media,
            mtime_ns,
            asset,
        } => {
            // Resource bytes and all cache sizes were installed before this
            // command. Ignore a late download for a replaced photo/version.
            tx.execute("UPDATE media_items SET thumbnail_ready=1,thumbnail_generated_at=unixepoch() WHERE id=?1 AND file_mtime_ns=?2 AND EXISTS(SELECT 1 FROM sync_publications p JOIN sync_entries e ON e.id=p.entry_id JOIN sync_jobs j ON j.id=p.job_id JOIN sync_connections c ON c.id=j.connection_id WHERE p.publication_id=?3 AND p.thumbnail_asset_id=?4 AND p.state IN ('pending','publishing') AND e.media_id=media_items.id AND e.native_dirty<>4 AND c.enabled=1 AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=j.id))",params![media,mtime_ns,publication,asset])?;
        }
        Command::DiscardPendingDeletions { job } => {
            let pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_entries WHERE job_id=?1 AND native_dirty=4)",
                [job],
                |r| r.get(0),
            )?;
            if pending {
                // Account switching explicitly discards these local obligations,
                // including uncertain evidence; returning starts fresh discovery.
                tx.execute("DELETE FROM sync_tasks WHERE entry_id IN (SELECT id FROM sync_entries WHERE job_id=?1 AND native_dirty=4)",[job])?;
                tx.execute("DELETE FROM sync_publications WHERE entry_id IN (SELECT id FROM sync_entries WHERE job_id=?1 AND native_dirty=4)",[job])?;
                tx.execute(
                    "DELETE FROM sync_entries WHERE job_id=?1 AND native_dirty=4",
                    [job],
                )?;
                super::records::clear_job_pending(&tx, job)?;
                super::state::clear_bootstrap(&tx, job)?;
                tx.execute("UPDATE sync_native_progress SET snapshot_token=NULL,bootstrap_started=0,ingest_token=NULL,target_token=NULL,applied_token=NULL,discovery_token=NULL,discovery_complete=0 WHERE job_id=?1",[job])?;
            }
        }
        Command::ConfirmAbsentDeletion {
            epoch,
            entry,
            version,
            lease,
        } => {
            valid_epoch(&tx, &epoch)?;
            if !lease.valid_now() {
                return Err(invalid("UPLOAD_LEASE_LOST"));
            }
            let removed=tx.execute("DELETE FROM sync_entries WHERE id=?1 AND job_id=?2 AND native_dirty=4 AND native_local_version=?3 AND native_photo_id IS NULL AND native_master_id IS NULL AND NOT EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND (state='issued' OR (state='confirmed' AND action='create')))",params![entry,epoch.job,version])?;
            if removed != 1 {
                return Err(invalid(
                    "an unconfirmed cloud operation still owns the deletion",
                ));
            }
        }
        Command::ClearAccount => {
            tx.execute("UPDATE sync_connections SET enabled=0", [])?;
            super::accounts::refresh_media_locations_in(&tx)?;
            tx.execute("INSERT INTO settings(key,value) VALUES('native_sync_account','0') ON CONFLICT(key) DO UPDATE SET value=excluded.value", [])?;
        }
        Command::SelectAccount { job } => super::accounts::select_in(&tx, job)?,
        Command::AcceptAuthentication {
            connection,
            expected,
            updated,
        } => {
            let current = super::state::read_scope(&tx, connection)?
                .ok_or_else(|| invalid("connection has no pinned identity"))?;
            let before_user = current
                .authentication
                .as_ref()
                .and_then(|a| a.user.as_ref())
                .map(|u| u.id.as_str());
            let after_user = updated
                .authentication
                .as_ref()
                .and_then(|a| a.user.as_ref())
                .map(|u| u.id.as_str());
            if current != *expected
                || before_user.is_some_and(|id| Some(id) != after_user)
                || current.public_base_url != updated.public_base_url
                || current.namespace != updated.namespace
                || current.database_generation != updated.database_generation
            {
                return Err(invalid("connection changed during login"));
            }
            super::state::write_scope(&tx, connection, &updated)?;
        }
        Command::RetireIssued {
            epoch,
            operation,
            rejected,
            reason,
        } => {
            valid_epoch(&tx, &epoch)?;
            if !rejected && (!operation.body.is_null() || operation.config == epoch.config) {
                return Err(invalid("an uncertain issued mutation cannot be discarded"));
            }
            if super::issued::read(&tx, &operation.id)?.as_ref() != Some(&operation) {
                return Err(invalid("issued evidence changed before retirement"));
            }
            let changed=tx.execute("UPDATE sync_tasks SET state='failed',last_error=?3 WHERE operation_id=?1 AND job_id=?2 AND state='issued'",params![operation.id,epoch.job,if rejected{reason.as_str()}else{"configuration superseded preparation before mutation"}])?;
            if changed != 1 {
                return Err(invalid("issued evidence changed before retirement"));
            }
        }
        Command::UserAction {
            job,
            entry,
            version,
            conflict,
            action,
            recovery,
        } => {
            super::actions::apply(&tx, job, entry, version, conflict, action, recovery)?;
        }
        Command::UploadFailed {
            epoch,
            entry,
            reason,
        } => {
            valid_epoch(&tx, &epoch)?;
            let changed = tx.execute(
                "UPDATE sync_entries SET native_upload_error=?3 WHERE job_id=?1 AND id=?2",
                params![epoch.job, entry, reason],
            )?;
            if changed != 1 {
                return Err(invalid(
                    "upload failure belongs to another job or missing entry",
                ));
            }
        }
        Command::Begin(epoch) => {
            // The caller holds a process-wide job guard. A previous process's
            // epoch is never authority for a newly issued callback.
            let count=tx.execute("UPDATE sync_jobs SET native_cycle_id=?3 WHERE id=?1 AND config_generation=?2 AND paused=0 AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=?1)",params![epoch.job,epoch.config,epoch.cycle])?;
            if count != 1 {
                return Err(invalid("job no longer accepts this synchronization cycle"));
            }
            tx.execute(
                "UPDATE sync_entries SET native_upload_error=NULL WHERE job_id=?1 AND native_upload_error IS NOT NULL",
                [epoch.job],
            )?;
        }
        Command::ObserveLocal { job, paths } => {
            for relative in paths {
                tx.execute("INSERT INTO sync_entries(job_id,media_id,relative_path,native_dirty) SELECT j.id,m.id,?2,1 FROM sync_jobs j JOIN sync_connections c ON c.id=j.connection_id JOIN media_items m ON m.path=rtrim(j.local_root,'/')||'/'||?2 AND m.trashed_at IS NULL AND m.storage_location<>2 WHERE j.id=?1 AND c.enabled=1 ON CONFLICT(job_id,relative_path) DO UPDATE SET media_id=COALESCE(sync_entries.media_id,excluded.media_id)",params![job,relative])?;
            }
        }
        Command::BeginCollision {
            id,
            capture,
            details,
        } => {
            check_publication(&tx, &capture)?;
            let entry = capture["entry"]
                .as_i64()
                .ok_or_else(|| invalid("collision has no entry"))?;
            let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND state='issued') OR EXISTS(SELECT 1 FROM sync_publications WHERE entry_id=?1 AND state='publishing')",[entry],|r|r.get(0))?;
            if busy {
                return Err(invalid(
                    "an issued operation still owns the local collision",
                ));
            }
            tx.execute("INSERT INTO sync_publications(publication_id,job_id,entry_id,photo_id,state) VALUES(?1,?2,?3,?4,'publishing')",params![id,capture["job"].as_i64(),entry,capture["photo"].as_str()])?;
            super::recovery::set_capture(&tx, &id, &capture)?;
            super::recovery::set_details(&tx, &id, Some(&details))?;
        }
        Command::FinishCollision { id } => {
            let capture = super::recovery::capture(&tx, &id)?
                .ok_or_else(|| invalid("missing collision capture"))?;
            let details = super::recovery::details(&tx, &id)?
                .ok_or_else(|| invalid("missing collision details"))?;
            check_publication(&tx, &capture)?;
            let entry = capture["entry"]
                .as_i64()
                .ok_or_else(|| invalid("collision has no entry"))?;
            let source = details["source"]
                .as_str()
                .ok_or_else(|| invalid("collision has no source"))?;
            let preserved = details["preserved"]
                .as_str()
                .ok_or_else(|| invalid("collision has no preserved path"))?;
            let media:i64=tx.query_row("SELECT m.id FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1 AND m.path=?2",params![entry,source],|r|r.get(0))?;
            let path = std::path::Path::new(preserved);
            tx.execute(
                "UPDATE media_items SET path=?2,uri=?3,folder_path=?4 WHERE id=?1",
                params![
                    media,
                    preserved,
                    crate::core::file_uri::from_path(path),
                    path.parent()
                        .ok_or_else(|| invalid("collision has no parent"))?
                        .to_string_lossy()
                ],
            )?;
            let entries = {
                let mut stmt=tx.prepare("SELECT e.id,j.local_root FROM sync_entries e JOIN sync_jobs j ON j.id=e.job_id WHERE e.media_id=?1")?;
                let rows = stmt
                    .query_map([media], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (entry, root) in entries {
                if let Ok(relative) = path.strip_prefix(root) {
                    tx.execute("UPDATE sync_entries SET relative_path=?2,native_local_version=native_local_version+1,native_digest_entry_id=NULL,native_digest_version=NULL,native_digest_sha256=NULL,native_digest_size=NULL WHERE id=?1",params![entry,relative.to_string_lossy()])?;
                    tx.execute("UPDATE sync_conflicts SET state='resolved',resolution='local path preserved with a numeric suffix',resolved_at=unixepoch() WHERE entry_id=?1 AND state='open' AND kind='native' AND native_reason IN ('another local/cloud identity owns this path','cloud content at this path differs from the local file')",[entry])?;
                }
            }
            tx.execute(
                "UPDATE sync_publications SET state='complete' WHERE publication_id=?1",
                [&id],
            )?;
            super::recovery::set_details(&tx, &id, None)?;
        }
        Command::AbortCollision { id } => {
            tx.execute(
                "UPDATE sync_publications SET state='superseded' WHERE publication_id=?1",
                [&id],
            )?;
            super::recovery::set_details(&tx, &id, None)?;
        }
        Command::ForgetIdentityForAdoption {
            job,
            entry,
            version,
        } => {
            let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND state='issued') OR EXISTS(SELECT 1 FROM sync_publications WHERE entry_id=?1 AND state='publishing')",[entry],|r|r.get(0))?;
            if busy {
                return Err(invalid("an issued operation still owns the identity"));
            }
            let changed=tx.execute("UPDATE sync_entries SET native_photo_id=NULL,native_master_id=NULL,native_has_baseline=0,native_asset_revision=NULL,native_master_revision=NULL,native_content_generation=NULL,native_confirmed_generation=NULL,native_retired=0 WHERE id=?1 AND job_id=?2 AND native_local_version=?3",params![entry,job,version])?;
            if changed != 1 {
                return Err(invalid("local edit superseded identity adoption"));
            }
            super::records::set_baseline_document(&tx, entry, None)?;
        }
        Command::Enroll { epoch } => {
            valid_epoch(&tx, &epoch)?;
            let (root, scope): (String, String) = tx.query_row(
                "SELECT local_root,upload_scope FROM sync_jobs WHERE id=?1",
                [epoch.job],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let rows = {
                let mut statement=tx.prepare("SELECT id,path FROM media_items WHERE trashed_at IS NULL AND storage_location<>2 AND substr(path,1,length(rtrim(?1,'/'))+1)=rtrim(?1,'/')||'/'")?;
                let rows = statement
                    .query_map([&root], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (media, path) in rows {
                let relative = std::path::Path::new(&path)
                    .strip_prefix(&root)
                    .map_err(|_| invalid("local media escaped the upload root"))?
                    .to_string_lossy()
                    .to_string();
                let album = std::path::Path::new(&relative)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let selected:bool=scope=="all" || tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_job_upload_albums WHERE job_id=?1 AND relative_album=?2)",params![epoch.job,album],|r|r.get(0))?;
                if selected {
                    tx.execute("INSERT INTO sync_entries(job_id,media_id,relative_path,native_dirty) VALUES(?1,?2,?3,1) ON CONFLICT(job_id,relative_path) DO NOTHING",params![epoch.job,media,relative])?;
                }
            }
        }
        Command::End(epoch) => {
            tx.execute(
                "UPDATE sync_jobs SET native_cycle_id=NULL WHERE id=?1 AND native_cycle_id=?2",
                params![epoch.job, epoch.cycle],
            )?;
        }
        Command::Save(epoch, issued) => {
            valid_epoch(&tx, &epoch)?;
            if issued.job != epoch.job || issued.config != epoch.config {
                return Err(invalid("issued operation configuration mismatch"));
            }
            let changed=tx.execute("INSERT INTO sync_tasks(operation_id,job_id,entry_id,action,state,config_generation,entry_generation) VALUES(?1,?2,?3,?4,'issued',?5,?6) ON CONFLICT(operation_id) DO UPDATE SET updated_at=unixepoch() WHERE sync_tasks.state='issued' AND sync_tasks.job_id=excluded.job_id",params![issued.id,issued.job,issued.entry,issued.phase,issued.config,issued.versions[0]])?;
            if changed != 1 {
                return Err(invalid("issued operation cannot replace settled evidence"));
            }
            super::issued::write(&tx, &issued)?;
        }
        Command::Confirm {
            epoch,
            operation,
            response,
            lease,
        } => {
            valid_epoch(&tx, &epoch)?;
            if !lease.valid_now() {
                return Err(invalid("UPLOAD_LEASE_LOST"));
            }
            let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_tasks WHERE operation_id=?1 AND job_id=?2 AND entry_id=?3 AND state='issued')",params![operation.id,epoch.job,operation.entry],|r|r.get(0))?;
            if !matches {
                return Err(invalid("no issued operation to confirm"));
            }
            let stored = super::issued::read(&tx, &operation.id)?
                .ok_or_else(|| invalid("no issued operation to confirm"))?;
            if stored.generation != operation.generation
                || stored.versions != operation.versions
                || stored.body != operation.body
            {
                return Err(invalid(
                    "operation confirmation does not match issued evidence",
                ));
            }
            let generation: String = tx.query_row(
                "SELECT database_generation FROM sync_native_progress WHERE job_id=?1",
                [epoch.job],
                |r| r.get(0),
            )?;
            if generation != operation.generation {
                return Err(invalid("operation belongs to another database generation"));
            }
            let records = response
                .get("records")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("mutation returned no records"))?;
            let expected = operation.body["operations"]
                .as_array()
                .ok_or_else(|| invalid("issued operation contains no operations"))?;
            if records.len() != expected.len() {
                return Err(invalid("mutation did not confirm the whole atomic batch"));
            }
            for expected in expected {
                let name = expected["record"]["recordName"]
                    .as_str()
                    .ok_or_else(|| invalid("issued record has no identity"))?;
                let result = records
                    .iter()
                    .find(|r| r["recordName"].as_str() == Some(name))
                    .ok_or_else(|| invalid("atomic mutation confirmed another record"))?;
                if result.get("serverErrorCode").is_some()
                    || result["recordChangeTag"].as_str().is_none_or(str::is_empty)
                {
                    return Err(invalid("atomic mutation contains an unconfirmed record"));
                }
                if operation.phase == "delete" && result["deleted"] != true {
                    return Err(invalid("delete response is not a tombstone"));
                }
            }
            if operation.phase == "delete" {
                tx.execute("UPDATE sync_entries SET native_retired=CASE WHEN native_local_version=?2 THEN 1 ELSE 0 END,
                    native_photo_id=NULL,native_master_id=NULL,native_asset_revision=NULL,native_master_revision=NULL,
                    native_content_generation=NULL,native_has_baseline=0,
                    native_confirmed_generation=NULL,native_dirty=CASE WHEN native_local_version=?2 THEN 0 ELSE 1 END,
                    native_acked_deletion_version=MAX(native_acked_deletion_version,?3) WHERE id=?1",
                    params![operation.entry,operation.versions[0],operation.versions[3]])?;
                super::records::set_baseline_document(&tx, operation.entry, None)?;
            } else {
                let revision = |name: &str| {
                    records
                        .iter()
                        .find(|r| r["recordName"].as_str() == Some(name))
                        .and_then(|r| r["recordChangeTag"].as_str())
                };
                let metadata_sent =
                    operation.phase == "create" || operation.preparation["sendMetadata"] == true;
                let content_sent =
                    operation.phase == "create" || operation.preparation.get("resources").is_some();
                tx.execute("UPDATE sync_entries SET native_retired=0,native_photo_id=?2,native_master_id=?3,
                    native_asset_revision=COALESCE(?4,native_asset_revision),native_master_revision=COALESCE(?5,native_master_revision),
                    native_content_generation=?6,native_confirmed_generation=?7,
                    native_acked_metadata_version=MAX(native_acked_metadata_version,?8),
                    native_acked_content_version=MAX(native_acked_content_version,?9) WHERE id=?1",
                    params![operation.entry,operation.photo,operation.master,revision(&operation.photo),revision(&operation.master),
                        operation.content_generation,generation,
                        if metadata_sent {operation.versions[1]} else {0},
                        if content_sent {operation.versions[2]} else {0}])?;
                tx.execute("UPDATE sync_entries SET native_dirty=CASE
                    WHEN native_dirty=4 THEN 4
                    WHEN native_local_version<>?2 OR native_metadata_version>native_acked_metadata_version
                        OR native_content_version>native_acked_content_version THEN 2
                    ELSE 0 END WHERE id=?1", params![operation.entry,operation.versions[0]])?;
            }
            tx.execute("UPDATE sync_conflicts SET state='resolved',resolution='confirmed issued operation',resolved_at=unixepoch() WHERE entry_id=?1 AND kind='native' AND state='open' AND native_reason IN ('remote change intersects an unconfirmed local edit; preserve both versions','another local/cloud identity owns this path')",[operation.entry])?;
            if operation.phase != "delete" {
                let mut set = super::super::native::records::RecordSet::default();
                if records.len() == 2 {
                    for record in records {
                        set.apply(
                            serde_json::from_value(record.clone())
                                .map_err(|_| invalid("invalid confirmed photo record"))?,
                        );
                    }
                    let zone: String = tx.query_row(
                        "SELECT native_zone_name FROM sync_jobs WHERE id=?1",
                        [epoch.job],
                        |r| r.get(0),
                    )?;
                    let zone = super::super::native::receive::ZoneId::from(
                        &super::super::NativeZone::new(zone),
                    );
                    let graph = set.to_graph(&zone).map_err(|_| {
                        invalid("confirmed photo pair violates the server contract")
                    })?;
                    let photo = graph
                        .photos
                        .first()
                        .ok_or_else(|| invalid("confirmed pair contains no photo"))?;
                    super::records::set_baseline_document(
                        &tx,
                        operation.entry,
                        Some(
                            &serde_json::to_string(&photo)
                                .map_err(|_| invalid("invalid confirmed photo"))?,
                        ),
                    )?;
                    // Reuse exact owned upload bytes in later metadata-only rounds.
                    for (name, role, asset) in [
                        ("original", "original", &photo.original),
                        ("thumbnail", "thumbnail", &photo.thumbnail),
                    ] {
                        if let Some(path) =
                            operation.preparation["resources"][name]["path"].as_str()
                        {
                            let hex = crate::core::sync::ContentDigest::hex_from_descriptor_base64(
                                &asset.file_checksum,
                            )
                            .ok_or_else(|| invalid("confirmed asset has an invalid digest"))?;
                            tx.execute("INSERT INTO sync_native_resources(job_id,generation,asset_id,role,record_name,expected_sha256,expected_size,staging_path,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'verified') ON CONFLICT(job_id,generation,asset_id,role) DO UPDATE SET staging_path=excluded.staging_path,state='verified'",params![operation.job,operation.generation,asset.asset_id,role,operation.photo,hex,asset.size as i64,path])?;
                        }
                    }
                } else {
                    let baseline = super::records::baseline_document(&tx, operation.entry)?;
                    if let Some(baseline) = baseline {
                        let mut photo: super::super::RemotePhoto = serde_json::from_str(&baseline)
                            .map_err(|_| invalid("invalid confirmed resource baseline"))?;
                        let fields = &records[0]["fields"];
                        photo.is_favorite =
                            fields["isFavorite"]["value"].as_i64().unwrap_or(0) != 0;
                        photo.asset_date_ms = fields["assetDate"]["value"].as_i64();
                        if let Some(path) = fields["relativePath"]["value"].as_str() {
                            photo.relative_path = path.into();
                        }
                        if let Some(events) = records[0].get("_syncserver") {
                            photo.events = serde_json::from_value(events.clone())
                                .map_err(|_| invalid("invalid confirmed edit events"))?;
                        }
                        super::records::set_baseline_document(
                            &tx,
                            operation.entry,
                            Some(
                                &serde_json::to_string(&photo)
                                    .map_err(|_| invalid("invalid confirmed photo"))?,
                            ),
                        )?;
                    }
                }
            }
            tx.execute("UPDATE media_items SET storage_location=3,thumbnail_ready=CASE WHEN ?2 THEN 1 ELSE thumbnail_ready END WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1)",params![operation.entry,operation.preparation.get("resources").is_some()])?;
            tx.execute("UPDATE sync_tasks SET state='confirmed',updated_at=unixepoch() WHERE operation_id=?1",[operation.id])?;
            if !lease.valid_now() {
                return Err(invalid("UPLOAD_LEASE_LOST"));
            }
            if operation.phase == "delete" {
                // The row was kept as cloud-only waiting for exactly this
                // confirmation (its local original disappeared while sync was
                // enabled and dirty=4 hid it from the grid). The cloud copy is
                // gone, so the row goes too — unless it became local again in
                // the meantime (restored file) or another live entry still
                // shares it.
                let media: Option<i64> = tx
                    .query_row(
                        "SELECT media_id FROM sync_entries WHERE id=?1 AND native_dirty=0 AND native_retired=1 AND native_local_version=?2",
                        params![operation.entry, operation.versions[0]],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(media) = media {
                    tx.execute(
                        "DELETE FROM media_items WHERE id=?1 AND storage_location=2
                          AND NOT EXISTS(SELECT 1 FROM sync_entries e
                                         WHERE e.media_id=?1 AND e.native_retired=0)",
                        [media],
                    )?;
                }
                tx.execute("DELETE FROM sync_entries WHERE id=?1 AND native_dirty=0 AND native_retired=1 AND native_local_version=?2",
                    params![operation.entry,operation.versions[0]])?;
            }
        }
        Command::CleanPublication { id } => {
            super::recovery::set_details(&tx, &id, None)?;
        }
        Command::Rollback { id } => {
            tx.execute("UPDATE sync_publications SET state='pending' WHERE publication_id=?1 AND state='publishing'",[&id])?;
            super::recovery::set_details(&tx, &id, None)?;
        }
        Command::Publishing { id, details } => {
            let capture = super::recovery::capture(&tx, &id)?
                .ok_or_else(|| invalid("missing publication capture"))?;
            check_publication(&tx, &capture)?;
            tx.execute(
                "UPDATE sync_publications SET state='publishing' WHERE publication_id=?1",
                [&id],
            )?;
            super::recovery::set_details(&tx, &id, Some(&details))?;
        }
        Command::Admit { id, photo, item } => {
            let capture = super::recovery::capture(&tx, &id)?
                .ok_or_else(|| invalid("missing publication capture"))?;
            check_publication(&tx, &capture)?;
            let entry = capture["entry"]
                .as_i64()
                .ok_or_else(|| invalid("publication has no entry"))?;
            let job = capture["job"]
                .as_i64()
                .ok_or_else(|| invalid("publication has no job"))?;
            // A remote metadata rename retains local media identity.
            let previous: Option<i64> = tx.query_row(
                "SELECT media_id FROM sync_entries WHERE id=?1",
                [entry],
                |r| r.get(0),
            )?;
            if let Some(media) = previous {
                tx.execute(
                    "UPDATE media_items SET uri=?2,path=?3,folder_path=?4 WHERE id=?1",
                    params![
                        media,
                        item.uri,
                        item.path.to_string_lossy(),
                        item.folder_path.to_string_lossy()
                    ],
                )?;
            }
            let media = crate::core::db::upsert_media_items_in(&tx, &[*item], Some(&id))?;
            let media = media
                .first()
                .ok_or_else(|| invalid("media admission was blocked"))?;
            tx.execute(
                "UPDATE media_items SET is_favorite=?2 WHERE id=?1",
                params![
                    media.id,
                    if capture["preserveMetadata"] == true {
                        capture["favorite"].as_i64().unwrap_or(0) != 0
                    } else {
                        photo.is_favorite
                    }
                ],
            )?;
            let revision = |name: &str| -> Result<String> {
                super::receive::record_revision_in(&tx, job, name)?
                    .ok_or_else(|| invalid("photo has no confirmed record revision"))
            };
            tx.execute("UPDATE media_items SET storage_location=3,thumbnail_ready=1,thumbnail_generated_at=unixepoch() WHERE id=?1",[media.id])?;
            tx.execute("UPDATE sync_entries SET media_id=?2,native_photo_id=?3,native_master_id=?4,native_asset_revision=?5,native_master_revision=?6,native_content_generation=?7,native_confirmed_generation=(SELECT database_generation FROM sync_native_progress WHERE job_id=?8),relative_path=?9,native_digest_entry_id=NULL,native_digest_version=NULL,native_digest_sha256=NULL,native_digest_size=NULL WHERE id=?1",params![entry,media.id,photo.photo_id,photo.master_id,revision(&photo.photo_id)?,revision(&photo.master_id)?,photo.content_generation,job,if capture["preserveMetadata"] == true { capture["localRelativePath"].as_str().ok_or_else(|| invalid("missing local metadata path"))? } else { &photo.relative_path }])?;
            super::records::set_baseline_document(
                &tx,
                entry,
                Some(
                    &serde_json::to_string(&photo)
                        .map_err(|_| invalid("cannot encode publication baseline"))?,
                ),
            )?;
            for (asset, role) in [
                (&photo.original.asset_id, "original"),
                (&photo.thumbnail.asset_id, "thumbnail"),
            ] {
                let count=tx.execute("UPDATE sync_native_resources SET state='verified',verified_cache=?4 WHERE job_id=?1 AND generation=(SELECT database_generation FROM sync_native_progress WHERE job_id=?1) AND asset_id=?2 AND role=?3 AND state IN ('staged','verified')",params![job,asset,role,media.uri])?;
                if count != 1 {
                    return Err(invalid("publication resource is no longer staged"));
                }
            }
            tx.execute(
                "UPDATE sync_publications SET state='complete' WHERE publication_id=?1",
                [id],
            )?;
        }
        Command::Conflict { job, entry, detail } => {
            super::recovery::write_conflict(&tx, job, entry, &detail, None)?;
        }
    }
    tx.commit()?;
    Ok(())
}
fn check_publication(tx: &Transaction<'_>, capture: &Value) -> Result<()> {
    let job = capture["job"]
        .as_i64()
        .ok_or_else(|| invalid("missing captured job"))?;
    let entry = capture["entry"]
        .as_i64()
        .ok_or_else(|| invalid("missing captured entry"))?;
    let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_entries e JOIN sync_jobs j ON j.id=e.job_id WHERE e.id=?1 AND j.id=?2 AND e.native_local_version=?3 AND j.config_generation=?4 AND e.native_dirty=?5 AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=j.id))",params![entry,job,capture["local"].as_i64(),capture["config"].as_i64(),capture["dirty"].as_i64().unwrap_or(0)],|r|r.get(0))?;
    if !valid {
        return Err(invalid(
            "local edit/configuration superseded the publication",
        ));
    }
    let owner:Option<String>=tx.query_row("SELECT publication_id FROM sync_publications WHERE entry_id=?1 AND capture_projection_present=1 AND state IN ('pending','publishing') ORDER BY id DESC LIMIT 1",[entry],|r|r.get(0)).optional()?;
    let current = owner
        .map(|id| super::recovery::capture(tx, &id))
        .transpose()?
        .flatten()
        .and_then(|capture| capture["projection"].as_str().map(str::to_owned));
    if capture.get("projection").is_some() && current.as_deref() != capture["projection"].as_str() {
        return Err(invalid(
            "a later received version superseded the publication",
        ));
    }
    Ok(())
}

/// Whether this projection owes remote resources or keeps a newer local original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectedContent {
    Remote,
    Local,
}

fn entry_events(
    tx: &Transaction<'_>,
    entry: i64,
) -> Result<super::super::native::receive::EditEvents> {
    super::records::read_events(tx, entry)
}

fn local_event_wins(
    local: Option<&super::super::native::receive::EditEvent>,
    remote: Option<&super::super::native::receive::EditEvent>,
) -> bool {
    // Old imports have no edit event. A real event beats a missing one; if
    // neither side can name an edit time, preserve the pending local edit.
    (local.is_none() && remote.is_none())
        || super::super::native::records::latest_event(local, remote).as_ref() == local
}

fn supersede_pending(tx: &Transaction<'_>, entry: i64) -> Result<()> {
    tx.execute(
        "UPDATE sync_publications SET state='superseded' WHERE entry_id=?1 AND state='pending'",
        [entry],
    )?;
    Ok(())
}

/// Projection and its publication obligation are committed with ingest/target.
pub(super) fn project_publication(
    tx: &Transaction<'_>,
    job: i64,
    photo: &super::super::RemotePhoto,
    local_digests: &HashMap<String, super::receive::LocalDigest>,
) -> Result<ProjectedContent> {
    let mut row:Option<(i64,i64,i64,Option<String>,String)>=tx.query_row("SELECT id,native_local_version,native_dirty,native_photo_id,relative_path FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2",params![job,photo.photo_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    if row.is_none() {
        row=tx.query_row("SELECT id,native_local_version,native_dirty,native_photo_id,relative_path FROM sync_entries WHERE job_id=?1 AND relative_path=?2",params![job,photo.relative_path],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    }
    let (entry, local, dirty, baseline, identity, local_path) = if let Some((
        entry,
        local,
        dirty,
        identity,
        path,
    )) = row
    {
        (
            entry,
            local,
            dirty,
            super::records::baseline_document(tx, entry)?,
            identity,
            path,
        )
    } else {
        tx.execute("INSERT INTO sync_entries(job_id,relative_path,native_photo_id,native_master_id) VALUES(?1,?2,?3,?4)",params![job,photo.relative_path,photo.photo_id,photo.master_id])?;
        (
            {
                let created = tx.last_insert_rowid();
                // Collision preservation moved the local occupant, not the
                // received pair. Move its temporary fragments to the cloud row.
                let old:Option<i64>=tx.query_row("SELECT entry_id FROM sync_records WHERE job_id=?1 AND owner_kind='pending' AND photo_id=?2 AND entry_id<>?3 LIMIT 1",params![job,photo.photo_id,created],|r|r.get(0)).optional()?.flatten();
                if let Some(old) = old {
                    for master in [true, false] {
                        if let Some(raw) = super::records::pending_document(tx, old, master)? {
                            let record = serde_json::from_str(&raw)
                                .map_err(|_| invalid("invalid received record"))?;
                            super::records::set_pending(tx, created, master, Some(&record))?;
                        }
                    }
                    super::records::clear_pending(tx, old)?;
                }
                created
            },
            0,
            0,
            None,
            Some(photo.photo_id.clone()),
            photo.relative_path.clone(),
        )
    };
    // A retained confirmation proves the local/cloud association even when
    // additive schema adoption left this media row at the local-only default.
    // Do this before the identical-baseline fast path; no user obligation or
    // publication is settled by repairing availability.
    tx.execute(
        "UPDATE media_items SET storage_location=3
         WHERE storage_location=1 AND id=(
             SELECT e.media_id FROM sync_entries e
             JOIN sync_jobs j ON j.id=e.job_id
             JOIN sync_connections c ON c.id=j.connection_id
             JOIN sync_native_progress p ON p.job_id=j.id
             WHERE e.id=?1 AND e.native_photo_id=?2 AND e.native_retired=0
               AND e.native_confirmed_generation=p.database_generation AND c.enabled=1
         )",
        params![entry, photo.photo_id],
    )?;
    let dirty = NativeDirty(dirty as u8);
    let (metadata_pending, content_pending): (bool, bool) = tx.query_row(
        "SELECT native_metadata_version>native_acked_metadata_version,native_content_version>native_acked_content_version FROM sync_entries WHERE id=?1",
        [entry], |r| Ok((r.get(0)?,r.get(1)?)),
    )?;
    let encoded = serde_json::to_string(photo).map_err(|_| invalid("invalid projected photo"))?;
    if baseline.as_deref() == Some(&encoded)
        && (local_path == photo.relative_path || metadata_pending)
    {
        if content_pending || dirty.contains(NativeDirty::DELETE) {
            supersede_pending(tx, entry)?;
            return Ok(ProjectedContent::Local);
        }
        return Ok(ProjectedContent::Remote);
    }
    let baseline_photo = baseline
        .as_ref()
        .and_then(|value| serde_json::from_str::<super::super::RemotePhoto>(value).ok());
    let same_metadata = baseline_photo.as_ref().is_some_and(|old| {
        old.is_favorite == photo.is_favorite
            && old.asset_date_ms == photo.asset_date_ms
            && old.relative_path == photo.relative_path
            && old.events.metadata == photo.events.metadata
    });
    let same_content = baseline_photo.as_ref().is_some_and(|old| {
        old.content_generation == photo.content_generation
            && old.original == photo.original
            && old.thumbnail == photo.thumbnail
            && old.events.content == photo.events.content
    });
    // Metadata admission may already have bound the cloud ID. A cloud-only row
    // still has no local bytes for a stale NEW/content obligation to preserve.
    let cloud_only: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_entries e JOIN media_items m ON m.id=e.media_id
          WHERE e.id=?1 AND m.storage_location=2)
          AND NOT EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND state='issued')
          AND NOT EXISTS(SELECT 1 FROM sync_publications WHERE entry_id=?1 AND state='publishing')",
        [entry],
        |r| r.get(0),
    )?;
    let missing_local = if cloud_only && !dirty.contains(NativeDirty::DELETE) {
        true
    } else if identity.is_none() {
        match adoption(tx, job, entry, photo, dirty, local_digests)? {
            Adoption::Bound | Adoption::Deferred => return Ok(ProjectedContent::Remote),
            Adoption::MissingLocal => true,
            Adoption::Divergent => false,
        }
    } else {
        false
    };
    let conflict = if photo.is_hidden {
        Some("remote hidden photo has no supported local representation")
    } else if identity.is_none() && !missing_local {
        Some("cloud content at this path differs from the local file")
    } else if identity.as_deref().is_some_and(|id| id != photo.photo_id) {
        Some("another local/cloud identity owns this path")
    } else {
        None
    };
    if let Some(reason) = conflict {
        super::recovery::write_conflict(tx, job, entry, reason, Some(photo))?;
        return Ok(ProjectedContent::Remote);
    }

    let events = entry_events(tx, entry)?;
    let preserve_metadata = !missing_local
        && metadata_pending
        && (same_metadata
            || local_event_wins(events.metadata.as_ref(), photo.events.metadata.as_ref()));
    // Physical deletion is durable until cloud deletion is confirmed. A newer
    // metadata/content event cannot resurrect the removed file or clear state 4.
    let keep_delete = dirty.contains(NativeDirty::DELETE);
    // NEW/CONTENT on an orphan entry cannot stand in for a local original.
    // Busy operations and explicit deletions were excluded before MissingLocal.
    let keep_content = !missing_local
        && (keep_delete
            || (content_pending || dirty.contains(NativeDirty::NEW))
                && (same_content
                    || local_event_wins(events.content.as_ref(), photo.events.content.as_ref())));
    let settled = NativeDirty(if keep_delete {
        NativeDirty::DELETE
    } else if preserve_metadata || keep_content {
        NativeDirty::MODIFIED
    } else {
        NativeDirty::CLEAN
    });
    // Merely observing the newer remote event settles the losing local category;
    // it does not advance local edit versions. The ordinary publication captures
    // these exact versions, so a subsequent edit still invalidates placement.
    let revision = |name: &str| -> Result<Option<String>> {
        super::receive::record_revision_in(tx, job, name)
    };
    tx.execute(
        "UPDATE sync_entries SET native_photo_id=?2,native_master_id=?3,native_dirty=?4,
         native_asset_revision=COALESCE(?5,native_asset_revision),native_master_revision=COALESCE(?6,native_master_revision),
         native_acked_metadata_version=CASE WHEN ?7 THEN native_acked_metadata_version ELSE native_metadata_version END,
         native_acked_content_version=CASE WHEN ?8 THEN native_acked_content_version ELSE native_content_version END,
         native_acked_deletion_version=CASE WHEN ?9 THEN native_acked_deletion_version ELSE native_deletion_version END
         WHERE id=?1",
        params![entry,photo.photo_id,photo.master_id,settled.bits() as i64,revision(&photo.photo_id)?,revision(&photo.master_id)?,preserve_metadata as i64,keep_content as i64,keep_delete as i64],
    )?;
    // Previously recorded edit/edit disagreements are now ordinary timestamp
    // decisions. Unsupported identities/representations keep their own handling.
    tx.execute("UPDATE sync_conflicts SET state='resolved',resolution='latest edit event',resolved_at=unixepoch() WHERE entry_id=?1 AND state='open' AND kind='native' AND native_reason IN ('remote change intersects an unconfirmed local edit; preserve both versions','cloud content at this path differs from the local file')",[entry])?;

    let needs_placement = missing_local
        || !keep_content
            && (!same_content || (!preserve_metadata && local_path != photo.relative_path));
    if !needs_placement {
        if !preserve_metadata {
            tx.execute("UPDATE media_items SET is_favorite=?2,taken_at=?3 WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1)",params![entry,photo.is_favorite,photo.asset_date_ms.map(|ms|ms.div_euclid(1000))])?;
        }
        tx.execute("UPDATE sync_entries SET native_content_generation=?2,native_confirmed_generation=(SELECT database_generation FROM sync_native_progress WHERE job_id=?3) WHERE id=?1",params![entry,photo.content_generation,job])?;
        super::records::set_baseline_document(tx, entry, Some(&encoded))?;
        supersede_pending(tx, entry)?;
        // The remote losing original is not a download obligation. It is not
        // labelled verified either: the library still holds the local winner.
        return Ok(if keep_content {
            ProjectedContent::Local
        } else {
            ProjectedContent::Remote
        });
    }
    let config: i64 = tx.query_row(
        "SELECT config_generation FROM sync_jobs WHERE id=?1",
        [job],
        |r| r.get(0),
    )?;
    use sha2::{Digest, Sha256};
    let id = format!(
        "publication-{job}-{}-{:x}",
        photo.photo_id,
        Sha256::digest(encoded.as_bytes())
    );
    tx.execute("UPDATE sync_publications SET state='superseded' WHERE job_id=?1 AND photo_id=?2 AND publication_id<>?3 AND state='pending'",params![job,photo.photo_id,id])?;
    let desired:(String,Option<i64>,Option<i64>)=tx.query_row("SELECT e.relative_path,m.is_favorite,m.taken_at FROM sync_entries e LEFT JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let capture = json!({"job":job,"entry":entry,"local":local,"config":config,"dirty":settled.bits(),"contentGeneration":photo.content_generation,"projection":encoded,"preserveMetadata":preserve_metadata,"localRelativePath":desired.0,"favorite":desired.1,"takenAt":desired.2});
    tx.execute("INSERT INTO sync_publications(publication_id,job_id,entry_id,photo_id,master_id,original_asset_id,thumbnail_asset_id) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(publication_id) DO UPDATE SET state=CASE WHEN sync_publications.state='superseded' THEN 'pending' ELSE sync_publications.state END",params![id,job,entry,photo.photo_id,photo.master_id,photo.original.asset_id,photo.thumbnail.asset_id])?;
    super::recovery::set_capture(tx, &id, &capture)?;
    // Library-backed verification has no staging bytes. Any new publication
    // (including a shared asset or rename) must obtain staging bytes again;
    // a different photo's adoption cannot satisfy its placement obligation.
    tx.execute("UPDATE sync_native_resources SET state='pending',verified_cache=NULL WHERE job_id=?1 AND generation=(SELECT database_generation FROM sync_native_progress WHERE job_id=?1) AND ((asset_id=?2 AND role='original') OR (asset_id=?3 AND role='thumbnail')) AND staging_path IS NULL",params![job,photo.original.asset_id,photo.thumbnail.asset_id])?;
    Ok(ProjectedContent::Remote)
}

/// What comparing a local file against a same-path cloud photo concluded.
enum Adoption {
    /// Provably the same bytes. The remote identity is now this entry's.
    Bound,
    /// An operation owns this entry, or the inbox cannot yet name its
    /// revisions. Nothing is written and nothing is reported: this is a state
    /// that resolves itself, not a decision for the user.
    Deferred,
    /// No live media exists to adopt. Download and admit the remote pair.
    MissingLocal,
    /// The bytes differ, or could not be compared. A real conflict.
    Divergent,
}

/// Bind a never-published local file to the cloud photo that already holds it.
///
/// This is the same settlement `PhotoAction::UseCloud` performs, reached
/// without asking, and the reason that is safe is the digest. That action
/// refuses to run unless a recovery copy of the local original has been saved,
/// because discarding local bytes is normally a loss. Here the local bytes and
/// the cloud bytes are known to be the same bytes, so there is nothing to
/// preserve — the copy on disk *is* the cloud copy. That is why the guard can
/// be bypassed on proof and on nothing else.
///
/// It is entirely local. The service has no adopt or import operation, and does
/// not need one: nothing here changes what the cloud holds, it only stops this
/// client from proposing to recreate what is already there.
fn adoption(
    tx: &Transaction<'_>,
    job: i64,
    entry: i64,
    photo: &super::super::RemotePhoto,
    dirty: NativeDirty,
    local_digests: &HashMap<String, super::receive::LocalDigest>,
) -> Result<Adoption> {
    // An issued or in-flight operation is the recovery path's business. Binding
    // the identity underneath it would turn its create into a guaranteed 409
    // against a record that now answers to this entry.
    let busy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND state='issued')
             OR EXISTS(SELECT 1 FROM sync_publications WHERE entry_id=?1 AND state='publishing')",
        [entry],
        |row| row.get(0),
    )?;
    if busy {
        return Ok(Adoption::Deferred);
    }
    // A hidden cloud photo has no local representation to bind to, and a
    // deletion is an explicit intent that adoption would silently cancel.
    if photo.is_hidden || dirty.contains(NativeDirty::DELETE) {
        return Ok(Adoption::Divergent);
    }
    // An entry with no live local file has nothing to compare. Calling that a
    // divergence would be a lie, and worse, a conflict row here would block the
    // very download that would give the photo a local representation. The
    // caller must create the ordinary resource and admission obligations.
    //
    // Resolved by path, for the same reason `unclaimed_local_entries` does it
    // that way: `media_id` is a presentation association that an entry created
    // by a cloud projection simply does not have, and every such entry would
    // otherwise be permanently unmeasurable.
    let media: Option<i64> = tx
        .query_row(
            "SELECT m.id FROM sync_entries e
               JOIN sync_jobs j ON j.id = e.job_id
               JOIN media_items m
                 ON m.path = rtrim(j.local_root, '/') || '/' || e.relative_path
                AND m.trashed_at IS NULL AND m.storage_location<>2
             WHERE e.id = ?1",
            [entry],
            |row| row.get(0),
        )
        .optional()?;
    let Some(media) = media else {
        return Ok(Adoption::MissingLocal);
    };

    // Measuring and caching both capture the entry's local version before the
    // I/O. The read lease ends before this transaction begins: an intervening
    // edit must keep its upload obligation, even when these older bytes match.
    let Some(observation) = local_digests.get(&photo.relative_path) else {
        return Ok(Adoption::Divergent);
    };
    let local_version: i64 = tx.query_row(
        "SELECT native_local_version FROM sync_entries WHERE id=?1",
        [entry],
        |row| row.get(0),
    )?;
    if observation.entry_id != entry || observation.local_version != local_version {
        return Ok(Adoption::Deferred);
    }
    let measured = &observation.digest;
    super::records::set_digest(tx, entry, Some(observation))?;
    let expected = ContentDigest::hex_from_descriptor_base64(&photo.original.file_checksum);
    if Some(measured.hex.as_str()) != expected.as_deref() || measured.size != photo.original.size {
        return Ok(Adoption::Divergent);
    }

    // Without both record revisions this is a half-bound identity, and a half
    // bound identity is worse than none: the next conditional update would go
    // out with a tag this client cannot produce. Wait for the inbox instead.
    let revision = |record: &str| -> Result<Option<String>> {
        super::receive::record_revision_in(tx, job, record)
    };
    let (Some(asset_revision), Some(master_revision)) =
        (revision(&photo.photo_id)?, revision(&photo.master_id)?)
    else {
        return Ok(Adoption::Deferred);
    };

    // A pending metadata bit is a real user edit with no trustworthy cloud
    // counterpart to weigh it against, so the local value stands and the next
    // uplink publishes it. With no such bit there is nothing local to lose, and
    // the cloud is authoritative. The same bit decides both questions: whether
    // the local row is overwritten, and whether the metadata obligation is
    // settled.
    let keep_metadata: bool = tx.query_row(
        "SELECT native_metadata_version>native_acked_metadata_version FROM sync_entries WHERE id=?1",
        [entry], |r| r.get(0),
    )?;
    let generation: String = tx.query_row(
        "SELECT database_generation FROM sync_native_progress WHERE job_id=?1",
        [job],
        |row| row.get(0),
    )?;
    let settled = NativeDirty(if keep_metadata {
        NativeDirty::MODIFIED
    } else {
        NativeDirty::CLEAN
    });
    let encoded = serde_json::to_string(photo).map_err(|_| invalid("invalid projected photo"))?;

    if !keep_metadata {
        tx.execute(
            "UPDATE media_items SET is_favorite=?2,taken_at=?3 WHERE id=?1",
            params![
                media,
                photo.is_favorite,
                photo.asset_date_ms.map(|ms| ms.div_euclid(1000))
            ],
        )?;
    }
    tx.execute(
        "UPDATE media_items SET storage_location=3 WHERE id=?1",
        [media],
    )?;
    tx.execute("UPDATE sync_entries SET media_id=?2,native_photo_id=?3,native_master_id=?4,native_asset_revision=?5,native_master_revision=?6,native_content_generation=?7,native_confirmed_generation=?8,native_dirty=?9,native_retired=0,native_acked_content_version=MAX(native_acked_content_version,native_content_version),native_acked_metadata_version=CASE WHEN ?10 THEN native_acked_metadata_version ELSE MAX(native_acked_metadata_version,native_metadata_version) END,updated_at=unixepoch() WHERE id=?1",params![entry,media,photo.photo_id,photo.master_id,asset_revision,master_revision,photo.content_generation,generation,settled.bits() as i64,keep_metadata])?;
    super::records::set_baseline_document(tx, entry, Some(&encoded))?;
    super::records::set_digest(tx, entry, Some(observation))?;
    tx.execute("UPDATE sync_conflicts SET state='resolved',resolution='same local and cloud bytes',resolved_at=unixepoch() WHERE entry_id=?1 AND state='open' AND kind='native' AND native_reason IN ('another local/cloud identity owns this path','cloud content at this path differs from the local file')",[entry])?;
    // Nothing is owed upward. Whatever publication was proposed against this
    // entry proposed to create what the cloud already has.
    tx.execute(
        "DELETE FROM sync_publications WHERE entry_id=?1 AND state<>'publishing'",
        [entry],
    )?;
    // The resource obligations are settled by the INSERT that follows this call:
    // it reads the baseline written just above, and marks the pair verified
    // against the library file. Writing that here instead would be a no-op — the
    // rows do not exist yet at this point in the promotion.
    Ok(Adoption::Bound)
}

#[cfg(test)]
mod tests;
