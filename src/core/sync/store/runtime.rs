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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    AcceptAuthentication {
        connection: i64,
        expected: Box<super::super::NativePinnedScope>,
        updated: Box<super::super::NativePinnedScope>,
    },
    RetireIssued {
        epoch: Epoch,
        operation: Issued,
        rejected: bool,
    },
    UserAction {
        job: i64,
        entry: i64,
        version: i64,
        conflict: Option<String>,
        action: super::PhotoAction,
        recovery: Option<String>,
    },
    Begin(Epoch),
    End(Epoch),
    Save(Epoch, Issued),
    Enroll {
        epoch: Epoch,
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
    pub(crate) fn runtime(&self, command: Command) -> Result<()> {
        match self.write(SyncWrite::Runtime(Box::new(command)))? {
            SyncWriteResult::None => Ok(()),
            _ => Err(invalid("unexpected runtime write response")),
        }
    }
    pub(crate) fn epoch_current(&self, epoch: &Epoch) -> Result<bool> {
        let conn = self.pool.get()?;
        conn.query_row("SELECT EXISTS(SELECT 1 FROM sync_jobs j JOIN sync_connections c ON c.id=j.connection_id WHERE j.id=?1 AND j.config_generation=?2 AND j.native_cycle_id=?3 AND j.paused=0 AND c.enabled=1 AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=j.id) AND NOT EXISTS(SELECT 1 FROM settings WHERE key='sync_reset_pending'))",params![epoch.job,epoch.config,epoch.cycle],|r|r.get(0)).map_err(Into::into)
    }
    pub(crate) fn pending_issued(&self, job: i64) -> Result<Vec<Issued>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT native_capture FROM sync_tasks WHERE job_id=?1 AND state='issued' ORDER BY id",
        )?;
        let rows = stmt
            .query_map([job], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|_| invalid("invalid issued operation evidence"))
            })
            .collect()
    }
    pub(crate) fn local_metadata(&self, entry: i64) -> Result<Value> {
        let conn = self.pool.get()?;
        let (favorite,date,path,baseline):(Option<i64>,Option<i64>,String,Option<String>)=conn.query_row("SELECT m.is_favorite,m.taken_at,e.relative_path,e.native_resource_baseline FROM sync_entries e LEFT JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[entry],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
        let baseline_ms = baseline
            .and_then(|json| serde_json::from_str::<Value>(&json).ok())
            .and_then(|value| value["assetDateMs"].as_i64());
        let date_ms = date.map(|seconds| {
            baseline_ms
                .filter(|ms| ms.div_euclid(1000) == seconds)
                .unwrap_or_else(|| seconds.saturating_mul(1000))
        });
        Ok(
            json!({"isFavorite":{"type":"INT64","value":favorite.unwrap_or(0)},"assetDate":date_ms.map(|ms|json!({"type":"TIMESTAMP","value":ms})),"relativePath":{"type":"STRING","value":path}}),
        )
    }
}

pub(super) fn execute(pool: &DbPool, command: Command) -> Result<()> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    match command {
        Command::AcceptAuthentication {
            connection,
            expected,
            updated,
        } => {
            let before: String = tx.query_row(
                "SELECT native_pinned_scope FROM sync_connections WHERE id=?1",
                [connection],
                |r| r.get(0),
            )?;
            let current: super::super::NativePinnedScope = serde_json::from_str(&before)
                .map_err(|_| invalid("invalid connection identity"))?;
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
            tx.execute("UPDATE sync_connections SET native_pinned_scope=?2,native_probed_at=unixepoch(),native_probe_error=NULL WHERE id=?1",params![connection,serde_json::to_string(&updated).map_err(|_|invalid("invalid connection identity"))?])?;
        }
        Command::RetireIssued {
            epoch,
            operation,
            rejected,
        } => {
            valid_epoch(&tx, &epoch)?;
            if !rejected && (!operation.body.is_null() || operation.config == epoch.config) {
                return Err(invalid("an uncertain issued mutation cannot be discarded"));
            }
            let encoded = serde_json::to_string(&operation)
                .map_err(|_| invalid("invalid issued evidence"))?;
            let changed=tx.execute("UPDATE sync_tasks SET state='failed',last_error=?4 WHERE operation_id=?1 AND job_id=?2 AND state='issued' AND native_capture=?3",params![operation.id,epoch.job,encoded,if rejected {"server rejected the atomic mutation"} else {"configuration superseded preparation before mutation"}])?;
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
        Command::Begin(epoch) => {
            // The caller holds a process-wide job guard. A previous process's
            // epoch is never authority for a newly issued callback.
            let count=tx.execute("UPDATE sync_jobs SET native_cycle_id=?3 WHERE id=?1 AND config_generation=?2 AND paused=0 AND NOT EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=?1)",params![epoch.job,epoch.config,epoch.cycle])?;
            if count != 1 {
                return Err(invalid("job no longer accepts this synchronization cycle"));
            }
        }
        Command::Enroll { epoch } => {
            valid_epoch(&tx, &epoch)?;
            let (root, scope): (String, String) = tx.query_row(
                "SELECT local_root,upload_scope FROM sync_jobs WHERE id=?1",
                [epoch.job],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let rows = {
                let mut statement=tx.prepare("SELECT id,path FROM media_items WHERE trashed_at IS NULL AND substr(path,1,length(rtrim(?1,'/'))+1)=rtrim(?1,'/')||'/'")?;
                let rows = statement
                    .query_map([&root], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            tx.execute("DELETE FROM sync_inventory WHERE job_id=?1", [epoch.job])?;
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
                    tx.execute("INSERT INTO sync_inventory(job_id,relative_path,config_generation) VALUES(?1,?2,?3)",params![epoch.job,relative,epoch.config])?;
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
            let capture = serde_json::to_string(&issued)
                .map_err(|_| invalid("cannot encode issued operation"))?;
            tx.execute("INSERT INTO sync_tasks(operation_id,job_id,entry_id,action,state,config_generation,entry_generation,native_capture) VALUES(?1,?2,?3,?4,'issued',?5,?6,?7) ON CONFLICT(operation_id) DO UPDATE SET native_capture=excluded.native_capture,updated_at=unixepoch() WHERE sync_tasks.state='issued' AND sync_tasks.job_id=excluded.job_id",params![issued.id,issued.job,issued.entry,issued.phase,issued.config,issued.versions[0],capture])?;
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
            let stored: Option<String>=tx.query_row("SELECT native_capture FROM sync_tasks WHERE operation_id=?1 AND job_id=?2 AND entry_id=?3 AND state='issued'",params![operation.id,epoch.job,operation.entry],|r|r.get(0)).optional()?;
            let stored: Issued = serde_json::from_str(
                &stored.ok_or_else(|| invalid("no issued operation to confirm"))?,
            )
            .map_err(|_| invalid("invalid operation evidence"))?;
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
            let (local,metadata,content,deletion):(i64,i64,i64,i64)=tx.query_row("SELECT native_local_version,native_metadata_version,native_content_version,native_deletion_version FROM sync_entries WHERE id=?1 AND job_id=?2",params![operation.entry,epoch.job],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            let versions = [local, metadata, content, deletion];
            let mut clear = 0u8;
            for (bit, index) in [(2u8, 1), (4, 2), (8, 3)] {
                if operation.dirty & bit != 0 && versions[index] == operation.versions[index] {
                    clear |= bit;
                }
            }
            if operation.phase == "create" {
                clear |= 1;
            }
            if operation.phase == "delete" {
                tx.execute("UPDATE sync_entries SET native_retired=CASE WHEN native_local_version=?2 THEN 1 ELSE 0 END WHERE id=?1",params![operation.entry,operation.versions[0]])?;
                tx.execute("UPDATE sync_entries SET native_photo_id=NULL,native_master_id=NULL,native_asset_revision=NULL,native_master_revision=NULL,native_content_generation=NULL,native_resource_baseline=NULL,native_baseline_metadata=NULL,native_confirmed_generation=NULL,native_dirty=(native_dirty & ?2) | CASE WHEN native_local_version<>?3 THEN 1 ELSE 0 END WHERE id=?1",params![operation.entry,!(clear as i64),operation.versions[0]])?;
            } else {
                let revision = |name: &str| {
                    records
                        .iter()
                        .find(|r| r["recordName"].as_str() == Some(name))
                        .and_then(|r| r["recordChangeTag"].as_str())
                };
                tx.execute("UPDATE sync_entries SET native_retired=0,native_photo_id=?2,native_master_id=?3,native_asset_revision=COALESCE(?4,native_asset_revision),native_master_revision=COALESCE(?5,native_master_revision),native_content_generation=?6,native_confirmed_generation=?7,native_dirty=native_dirty & ?8,native_acked_metadata_version=MAX(native_acked_metadata_version,?9),native_acked_content_version=MAX(native_acked_content_version,?10) WHERE id=?1",params![operation.entry,operation.photo,operation.master,revision(&operation.photo),revision(&operation.master),operation.content_generation,generation,!(clear as i64),if operation.dirty&2!=0{operation.versions[1]}else{0},if operation.dirty&4!=0{operation.versions[2]}else{0}])?;
                let fields = operation.body["operations"]
                    .as_array()
                    .and_then(|ops| {
                        ops.iter()
                            .find(|op| op["record"]["recordName"] == operation.photo)
                    })
                    .map(|op| op["record"]["fields"].clone())
                    .unwrap_or(Value::Null);
                tx.execute(
                    "UPDATE sync_entries SET native_baseline_metadata=?2 WHERE id=?1",
                    params![
                        operation.entry,
                        serde_json::to_string(&fields)
                            .map_err(|_| invalid("invalid metadata baseline"))?
                    ],
                )?;
            }
            tx.execute("UPDATE sync_conflicts SET state='resolved',resolution='confirmed issued operation',resolved_at=unixepoch() WHERE entry_id=?1 AND kind='native' AND state='open' AND json_extract(native_detail,'$.reason') IN ('remote change intersects an unconfirmed local edit; preserve both versions','another local/cloud identity owns this path')",[operation.entry])?;
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
                    tx.execute(
                        "UPDATE sync_entries SET native_resource_baseline=?2 WHERE id=?1",
                        params![
                            operation.entry,
                            serde_json::to_string(photo)
                                .map_err(|_| invalid("invalid confirmed photo baseline"))?
                        ],
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
                    let baseline: Option<String> = tx.query_row(
                        "SELECT native_resource_baseline FROM sync_entries WHERE id=?1",
                        [operation.entry],
                        |r| r.get(0),
                    )?;
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
                        tx.execute(
                            "UPDATE sync_entries SET native_resource_baseline=?2 WHERE id=?1",
                            params![
                                operation.entry,
                                serde_json::to_string(&photo)
                                    .map_err(|_| invalid("invalid confirmed resource baseline"))?
                            ],
                        )?;
                    }
                }
            }
            tx.execute("UPDATE sync_tasks SET state='confirmed',updated_at=unixepoch() WHERE operation_id=?1",[operation.id])?;
            if !lease.valid_now() {
                return Err(invalid("UPLOAD_LEASE_LOST"));
            }
        }
        Command::CleanPublication { id } => {
            tx.execute("UPDATE sync_publications SET role_state=NULL,old_snapshot=NULL WHERE publication_id=?1 AND state='complete'",[id])?;
        }
        Command::Rollback { id } => {
            tx.execute("UPDATE sync_publications SET state='pending',role_state=NULL,old_snapshot=NULL WHERE publication_id=?1 AND state='publishing'",[id])?;
        }
        Command::Publishing { id, details } => {
            let captured:String=tx.query_row("SELECT captured_versions FROM sync_publications WHERE publication_id=?1 AND state IN ('pending','publishing')",[&id],|r|r.get(0))?;
            let capture: Value = serde_json::from_str(&captured)
                .map_err(|_| invalid("invalid publication capture"))?;
            check_publication(&tx, &capture)?;
            tx.execute("UPDATE sync_publications SET state='publishing',role_state=?2,old_snapshot=?3 WHERE publication_id=?1",params![id,details.to_string(),if details["hadOld"]==true {Some(json!({"path":details["backup"]}).to_string())}else{None}])?;
        }
        Command::Admit { id, photo, item } => {
            let captured:String=tx.query_row("SELECT captured_versions FROM sync_publications WHERE publication_id=?1 AND state='publishing'",[&id],|r|r.get(0))?;
            let capture: Value = serde_json::from_str(&captured)
                .map_err(|_| invalid("invalid publication capture"))?;
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
                tx.query_row("SELECT record_change_tag FROM sync_native_inbox WHERE job_id=?1 AND record_name=?2 ORDER BY ordinal DESC LIMIT 1",params![job,name],|r|r.get(0)).map_err(Into::into)
            };
            tx.execute("UPDATE sync_entries SET media_id=?2,native_photo_id=?3,native_master_id=?4,native_asset_revision=?5,native_master_revision=?6,native_content_generation=?7,native_confirmed_generation=(SELECT database_generation FROM sync_native_progress WHERE job_id=?8),native_baseline_metadata=?9,native_resource_baseline=?10 WHERE id=?1",params![entry,media.id,photo.photo_id,photo.master_id,revision(&photo.photo_id)?,revision(&photo.master_id)?,photo.content_generation,job,json!({"isFavorite":photo.is_favorite,"assetDate":photo.asset_date_ms,"relativePath":photo.relative_path}).to_string(),serde_json::to_string(&photo).map_err(|_|invalid("cannot encode publication baseline"))?])?;
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
            tx.execute("INSERT INTO sync_conflicts(job_id,entry_id,kind,native_detail) VALUES(?1,?2,'native',?3) ON CONFLICT(entry_id) WHERE state='open' DO UPDATE SET native_detail=excluded.native_detail",params![job,entry,json!({"reason":detail}).to_string()])?;
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
    let current:Option<String>=tx.query_row("SELECT payload FROM sync_native_records WHERE job_id=?1 AND record_type='__photo__' AND record_name=(SELECT native_photo_id FROM sync_entries WHERE id=?2)",params![job,entry],|r|r.get(0)).optional()?;
    if capture.get("projection").is_some() && current.as_deref() != capture["projection"].as_str() {
        return Err(invalid("remote projection superseded the publication"));
    }
    Ok(())
}

/// Projection and its publication obligation are committed with ingest/target.
pub(super) fn project_publication(
    tx: &Transaction<'_>,
    job: i64,
    photo: &super::super::RemotePhoto,
    local_digests: &HashMap<String, ContentDigest>,
) -> Result<()> {
    let row:Option<(i64,i64,i64,Option<String>,Option<String>)>=tx.query_row("SELECT id,native_local_version,native_dirty,native_resource_baseline,native_photo_id FROM sync_entries WHERE job_id=?1 AND (native_photo_id=?2 OR relative_path=?3) ORDER BY native_photo_id IS NOT NULL DESC LIMIT 1",params![job,photo.photo_id,photo.relative_path],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let (entry, local, dirty, baseline, identity) = if let Some(row) = row {
        row
    } else {
        tx.execute("INSERT INTO sync_entries(job_id,relative_path,native_photo_id,native_master_id) VALUES(?1,?2,?3,?4)",params![job,photo.relative_path,photo.photo_id,photo.master_id])?;
        (
            tx.last_insert_rowid(),
            0,
            0,
            None,
            Some(photo.photo_id.clone()),
        )
    };
    let dirty = NativeDirty(dirty as u8);
    let encoded = serde_json::to_string(photo).map_err(|_| invalid("invalid projected photo"))?;
    if baseline.as_deref() == Some(&encoded) {
        return Ok(());
    }
    let baseline_photo = baseline
        .as_ref()
        .and_then(|value| serde_json::from_str::<super::super::RemotePhoto>(value).ok());
    let same_metadata = baseline_photo.as_ref().is_some_and(|old| {
        old.is_favorite == photo.is_favorite
            && old.asset_date_ms == photo.asset_date_ms
            && old.relative_path == photo.relative_path
    });
    let same_content = baseline_photo.as_ref().is_some_and(|old| {
        old.content_generation == photo.content_generation
            && old.original == photo.original
            && old.thumbnail == photo.thumbnail
    });
    // Independent remote metadata/local content edits merge without rewriting bytes.
    let same_path = baseline_photo
        .as_ref()
        .is_some_and(|old| old.relative_path == photo.relative_path);
    // The dirty value is a bitmask whose categories are tracked independently —
    // an edit made while an operation was in flight leaves two bits set at once,
    // and that is a normal state, not an anomaly. Every question here is
    // therefore asked per category. `content_settled` deliberately exempts a
    // new entry: it has no confirmed baseline, and comparing content it never
    // published is not evidence of anything.
    let content_settled = !dirty.contains(NativeDirty::CONTENT) || dirty.contains(NativeDirty::NEW);
    if (content_settled && same_content && same_path || same_content && same_metadata)
        && !photo.is_hidden
    {
        let (asset_revision,master_revision):(String,String)=(tx.query_row("SELECT record_change_tag FROM sync_native_inbox WHERE job_id=?1 AND record_name=?2 ORDER BY ordinal DESC LIMIT 1",params![job,photo.photo_id],|r|r.get(0))?,tx.query_row("SELECT record_change_tag FROM sync_native_inbox WHERE job_id=?1 AND record_name=?2 ORDER BY ordinal DESC LIMIT 1",params![job,photo.master_id],|r|r.get(0))?);
        if !dirty.contains(NativeDirty::METADATA) {
            tx.execute("UPDATE media_items SET is_favorite=?2,taken_at=?3 WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1)",params![entry,photo.is_favorite,photo.asset_date_ms.map(|ms|ms.div_euclid(1000))])?;
        }
        tx.execute("UPDATE sync_entries SET native_resource_baseline=?2,native_asset_revision=?3,native_master_revision=?4 WHERE id=?1",params![entry,encoded,asset_revision,master_revision])?;
        return Ok(());
    }
    // No cloud identity is not a disagreement. It is a local file that has
    // never been published — or a publication whose confirmation was lost, in
    // which case the entry looks identical. Both are settled by comparing the
    // bytes, and only the bytes: the service keys `relativePath` uniquely per
    // zone, so a same-path cloud photo is the same photo and the identity match
    // is a claim. A digest match is the proof of it.
    if identity.is_none() {
        match adoption(tx, job, entry, photo, dirty, local_digests)? {
            Adoption::Bound | Adoption::Deferred => return Ok(()),
            Adoption::Divergent => {}
        }
    }

    let preserve_metadata = dirty.contains(NativeDirty::METADATA) && same_metadata;
    // Hidden is asked first because it is the one reason the user cannot act
    // on: neither resolution applies to a photo with no local representation,
    // so reporting it as a version or path disagreement would offer them
    // choices that all fail.
    let conflict = if photo.is_hidden {
        Some("remote hidden photo has no supported local representation")
    } else if identity.is_none() {
        Some("cloud content at this path differs from the local file")
    } else if identity.as_deref() != Some(&photo.photo_id) {
        Some("another local/cloud identity owns this path")
    } else if !dirty.is_clean() && !preserve_metadata {
        Some("remote change intersects an unconfirmed local edit; preserve both versions")
    } else {
        None
    };
    if let Some(reason) = conflict {
        tx.execute("INSERT INTO sync_conflicts(job_id,entry_id,kind,native_detail) VALUES(?1,?2,'native',?3) ON CONFLICT(entry_id) WHERE state='open' DO UPDATE SET native_detail=excluded.native_detail",params![job,entry,json!({"reason":reason,"remote":photo}).to_string()])?;
        return Ok(());
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
    let capture = json!({"job":job,"entry":entry,"local":local,"config":config,"dirty":dirty.bits(),"contentGeneration":photo.content_generation,"projection":encoded,"preserveMetadata":preserve_metadata,"localRelativePath":desired.0,"favorite":desired.1,"takenAt":desired.2});
    tx.execute("INSERT INTO sync_publications(publication_id,job_id,entry_id,photo_id,master_id,original_asset_id,thumbnail_asset_id,captured_versions) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(publication_id) DO UPDATE SET captured_versions=CASE WHEN sync_publications.state='pending' THEN excluded.captured_versions ELSE sync_publications.captured_versions END",params![id,job,entry,photo.photo_id,photo.master_id,photo.original.asset_id,photo.thumbnail.asset_id,capture.to_string()])?;
    Ok(())
}

/// What comparing a local file against a same-path cloud photo concluded.
enum Adoption {
    /// Provably the same bytes. The remote identity is now this entry's.
    Bound,
    /// An operation owns this entry, or the inbox cannot yet name its
    /// revisions. Nothing is written and nothing is reported: this is a state
    /// that resolves itself, not a decision for the user.
    Deferred,
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
    local_digests: &HashMap<String, ContentDigest>,
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
    // resource and admission paths own that outcome, so this stage stays out.
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
                AND m.trashed_at IS NULL
             WHERE e.id = ?1",
            [entry],
            |row| row.get(0),
        )
        .optional()?;
    let Some(media) = media else {
        return Ok(Adoption::Deferred);
    };

    // The digest is read, never computed here: this function runs inside the
    // promote transaction, and a transaction that hashes a library is a
    // transaction that holds the write lock for the length of a disk sweep.
    // The receive path measures the files first and hands the answers in.
    let (measured, already_kept): (Option<ContentDigest>, bool) = match tx
        .query_row(
            "SELECT native_local_digest FROM sync_entries WHERE id=?1",
            [entry],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
    {
        Some(cached) => match serde_json::from_str(&cached) {
            Ok(digest) => (Some(digest), true),
            Err(_) => (local_digests.get(&photo.relative_path).cloned(), false),
        },
        None => (local_digests.get(&photo.relative_path).cloned(), false),
    };
    let Some(measured) = measured else {
        return Ok(Adoption::Divergent);
    };
    // What the local file hashes to is a fact about the local file, whatever
    // the cloud turns out to hold, so it is kept either way. An entry that ends
    // up in conflict must not re-read its file on every subsequent cycle just
    // to arrive at the same conclusion — and if the cloud copy changes, this
    // cached digest is what the next comparison is made against anyway.
    if !already_kept {
        tx.execute(
            "UPDATE sync_entries SET native_local_digest=?2 WHERE id=?1",
            params![entry, serde_json::to_string(&measured).unwrap_or_default()],
        )?;
    }
    let expected = ContentDigest::hex_from_descriptor_base64(&photo.original.file_checksum);
    if Some(measured.hex.as_str()) != expected.as_deref() || measured.size != photo.original.size {
        return Ok(Adoption::Divergent);
    }

    // Without both record revisions this is a half-bound identity, and a half
    // bound identity is worse than none: the next conditional update would go
    // out with a tag this client cannot produce. Wait for the inbox instead.
    let revision = |record: &str| -> Result<Option<String>> {
        tx.query_row(
            "SELECT record_change_tag FROM sync_native_inbox WHERE job_id=?1 AND record_name=?2 ORDER BY ordinal DESC LIMIT 1",
            params![job, record],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
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
    let keep_metadata = dirty.contains(NativeDirty::METADATA);
    let generation: String = tx.query_row(
        "SELECT database_generation FROM sync_native_progress WHERE job_id=?1",
        [job],
        |row| row.get(0),
    )?;
    let settled = NativeDirty(
        (dirty.bits() & !NativeDirty::NEW & !NativeDirty::CONTENT)
            & if keep_metadata {
                u8::MAX
            } else {
                !NativeDirty::METADATA
            },
    );
    let encoded = serde_json::to_string(photo).map_err(|_| invalid("invalid projected photo"))?;
    let baseline_metadata = json!({"isFavorite":photo.is_favorite,"assetDate":photo.asset_date_ms,"relativePath":photo.relative_path});

    if !keep_metadata {
        tx.execute("UPDATE media_items SET is_favorite=?2,taken_at=?3 WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1)",params![entry,photo.is_favorite,photo.asset_date_ms.map(|ms|ms.div_euclid(1000))])?;
    }
    tx.execute(
        "UPDATE sync_entries SET media_id=?2,native_photo_id=?3,native_master_id=?4,native_asset_revision=?5,native_master_revision=?6,
            native_content_generation=?7,native_confirmed_generation=?8,native_resource_baseline=?9,
            native_baseline_metadata=?10,native_local_digest=?11,native_dirty=?12,native_retired=0,
            native_acked_content_version=MAX(native_acked_content_version,native_content_version),
            native_acked_metadata_version=CASE WHEN ?13 THEN native_acked_metadata_version ELSE MAX(native_acked_metadata_version,native_metadata_version) END,
            updated_at=unixepoch()
         WHERE id=?1",
        params![entry,media,photo.photo_id,photo.master_id,asset_revision,master_revision,photo.content_generation,generation,encoded,baseline_metadata.to_string(),serde_json::to_string(&measured).unwrap_or_default(),settled.bits() as i64,keep_metadata as i64],
    )?;
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
