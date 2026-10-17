//! Receive cursors and per-photo metadata application. Incomplete wire pairs
//! are temporarily stored on their existing sync entries, then discarded.
//!
//! These tables are **not** local-change truth. They record what the cloud has
//! said and what this client still owes it. Each page persists incomplete photo
//! fields and its ingest position together. Complete metadata goes directly into
//! media_items; file and thumbnail readiness control presentation.
//!
//! The token ladder is deliberately four distinct positions. `snapshot_token`
//! pins a full read at one height; `ingest_token` is where the delta stream
//! resumes; `target_token` is the height of the newest complete projection;
//! `applied_token` is the height this client has actually finished catching up
//! to. A client may only advance `applied` once the projection at `target` has
//! no unresolved resource or conflict work — otherwise a fully received but
//! un-downloaded photo would report itself as synchronized.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::core::db::DbPool;
use crate::core::error::{AppError, Result};

use super::super::content::ContentDigest;
use super::super::native::protocol::{NativeNamespace, NativeZone};
use super::super::native::receive::NativeRecord;
use super::super::native::records::RemoteGraph;
use super::{SyncStore, SyncWrite, SyncWriteResult};
use serde_json::{json, Value};

/// Per-record-type bootstrap progress. One type or one page is never a
/// complete inventory, so each type carries its own marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeMarker {
    pub status: TypeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_marker: Option<String>,
    pub record_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeStatus {
    Pending,
    Complete,
}

/// Which types a bootstrap still owes, and where each one resumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapState {
    /// True once a snapshot token has been accepted. While false, no listing
    /// may be read and no deletion may be inferred.
    #[serde(default)]
    pub started: bool,
    #[serde(default)]
    pub types: BTreeMap<String, TypeMarker>,
}

impl BootstrapState {
    pub fn marker(&self, record_type: &str) -> TypeMarker {
        self.types.get(record_type).cloned().unwrap_or(TypeMarker {
            status: TypeStatus::Pending,
            continuation_marker: None,
            record_count: 0,
        })
    }

    /// True only when every supported type has finished. Unknown types the
    /// server holds are deliberately not in this list: retaining them is not
    /// the same as having enumerated them.
    pub fn is_complete(&self, supported: &[&str]) -> bool {
        self.started
            && supported.iter().all(|name| {
                self.types
                    .get(*name)
                    .is_some_and(|marker| marker.status == TypeStatus::Complete)
            })
    }
}

/// Everything needed to resume this job's receive position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeProgress {
    pub job_id: i64,
    pub zone: NativeZone,
    pub namespace: NativeNamespace,
    pub database_generation: String,
    pub scope_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_token: Option<String>,
    #[serde(default)]
    pub bootstrap: BootstrapState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_token: Option<String>,
    #[serde(default)]
    pub discovery_complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingest_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_token: Option<String>,
}

impl NativeProgress {
    /// A scope, namespace or generation change invalidates every previous
    /// claim: the old applied position says nothing about the new scope, and
    /// inheriting it is exactly the "old progress proves new work" mistake.
    pub fn matches(&self, namespace: &NativeNamespace, generation: &str, scope: &str) -> bool {
        self.namespace == *namespace
            && self.database_generation == generation
            && self.scope_digest == scope
    }

    pub fn needs_bootstrap(&self, supported: &[&str]) -> bool {
        self.snapshot_token.is_none() || !self.bootstrap.is_complete(supported)
    }

    /// Whether `applied` may advance to `target`. Both must exist, be from the
    /// same round, and have no unresolved work behind them.
    pub fn can_apply(&self, unresolved: usize) -> bool {
        self.target_token.is_some()
            && self.applied_token != self.target_token
            && unresolved == 0
            && self
                .bootstrap
                .is_complete(&super::super::native::receive::SUPPORTED_RECORD_TYPES)
    }
}

/// One ordered receive event, exactly as received.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEvent {
    pub ordinal: i64,
    pub record_name: String,
    pub record_type: String,
    pub record_change_tag: String,
    pub kind: InboxKind,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Created,
    Updated,
    Deleted,
}

/// A write against the durable receive state. All of them run in one
/// transaction so a crash can never leave a token ahead of the rows it claims.
#[derive(Debug)]
pub enum NativeReceiveCommand {
    /// Ensure a progress row exists for this job's current namespace, scope and
    /// generation, resetting it if any of those changed.
    Begin {
        job_id: i64,
        namespace: NativeNamespace,
        generation: String,
        scope_digest: String,
        zone: NativeZone,
    },
    /// One query page: advance that type's marker and record its sync token.
    PinSnapshot {
        job_id: i64,
        snapshot_token: String,
        zone_sync_token: String,
    },
    SaveBootstrapPage {
        job_id: i64,
        record_type: String,
        continuation_marker: Option<String>,
        record_count: usize,
        zone_sync_token: String,
    },
    /// One delta page: append ordered events and advance the ingest token in the
    /// same transaction.
    SaveDeltaPage {
        job_id: i64,
        events: Vec<InboxEvent>,
        next_token: String,
    },
    /// The final page at a fixed height. Replaces the projection, materializes
    /// resource obligations, and sets the target token.
    Promote {
        job_id: i64,
        graph: RemoteGraph,
        target_token: String,
        /// Versioned SHA-256 observations, keyed by `relativePath`, measured
        /// outside the promotion transaction. Only entries that have never been
        /// published consult it; see `runtime::adoption`.
        local_digests: HashMap<String, LocalDigest>,
    },
    /// Apply only records affected by a completed delta round.
    PromoteDelta {
        job_id: i64,
        graph: RemoteGraph,
        target_token: String,
        local_digests: HashMap<String, LocalDigest>,
    },
    /// Attempt to advance `applied`. Refused while any obligation is
    /// unresolved, which is what keeps a fully received but un-downloaded photo
    /// from reporting itself as synchronized.
    AdvanceApplied { job_id: i64 },
    /// Adopt the server head produced exclusively by this client's confirmed
    /// upload batch. The mutation response already established record identity,
    /// revisions and resource baselines locally.
    AdoptConfirmedHead { job_id: i64, sync_token: String },
    /// Drop the receive position after token expiry. Local dirty, confirmed
    /// facts and verified assets are untouched; only the cloud-side position is
    /// rebuilt.
    Reset { job_id: i64 },
}

/// A comparison result belongs to one entry at one captured local version.
/// A later edit (or replacement entry at the same path) cannot consume it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDigest {
    pub entry_id: i64,
    pub local_version: i64,
    pub digest: ContentDigest,
}

/// A local file tracked by an enabled cloud job.
#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub photo_id: Option<String>,
    pub entry_id: i64,
    pub local_version: i64,
    pub relative_path: String,
    /// The file on disk, as the library recorded it.
    pub local_path: PathBuf,
    pub file_size: u64,
    /// A digest already taken, so an unchanged file is read once and not again.
    pub cached: Option<ContentDigest>,
}

#[derive(Debug)]
pub enum NativeReceiveOutcome {
    Progress(Box<NativeProgress>),
    SavedEvents(usize),
    Promoted {
        photos: usize,
        unknown_records: usize,
        resources: usize,
        /// False while obligations remain, which is the normal C1.2 state.
        applied: bool,
    },
    AdoptedHead,
    Reset,
}

impl super::SyncStore {
    /// Durable progress for this job, or `None` before the first cycle.
    pub fn native_progress(&self, job_id: i64) -> Result<Option<NativeProgress>> {
        let conn = self.pool.get()?;
        read_progress(&conn, job_id)
    }

    /// Only incomplete/unapplied photo fragments, removed after application.
    pub fn pending_records(&self, job: i64) -> Result<Vec<NativeRecord>> {
        let conn = self.pool.get()?;
        let mut stmt=conn.prepare("SELECT native_pending_master,native_pending_asset FROM sync_entries WHERE job_id=?1 AND (native_pending_asset IS NOT NULL OR (native_pending_master IS NOT NULL AND native_resource_baseline IS NOT NULL)) ORDER BY id")?;
        let rows = stmt.query_map([job], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        })?;
        let mut records: Vec<NativeRecord> = Vec::new();
        for row in rows {
            let (master, asset) = row?;
            for raw in [master, asset].into_iter().flatten() {
                records.push(serde_json::from_str(&raw).map_err(json_error)?);
            }
        }
        drop(stmt);
        drop(conn);
        // A local edit/delete can invalidate an owned pending download without
        // any new server event. A received rename may also await confirmation
        // of newer local content before placement. Re-evaluate only those local
        // obligations from their already confirmed baseline; do not list cloud data.
        let invalidated = {
            let conn = self.pool.get()?;
            let mut stmt=conn.prepare("SELECT DISTINCT p.photo_id FROM sync_publications p JOIN sync_entries e ON e.id=p.entry_id WHERE p.job_id=?1 AND p.state='pending' AND e.native_local_version<>json_extract(p.captured_versions,'$.local')
                UNION SELECT native_photo_id FROM sync_entries WHERE job_id=?1 AND native_dirty=0 AND native_photo_id IS NOT NULL AND relative_path<>json_extract(native_resource_baseline,'$.relativePath')")?;
            let rows = stmt.query_map([job], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut names = records
            .iter()
            .map(|r| r.record_name.clone())
            .collect::<std::collections::HashSet<_>>();
        for photo in invalidated {
            for record in self.current_pair_records(job, &photo)? {
                if names.insert(record.record_name.clone()) {
                    records.push(record);
                }
            }
        }
        Ok(records)
    }

    pub fn current_remote_photo(
        &self,
        job: i64,
        photo: &str,
    ) -> Result<Option<super::super::RemotePhoto>> {
        let conn = self.pool.get()?;
        current_remote_photo_in(&conn, job, photo)?
            .map(|raw| serde_json::from_str(&raw).map_err(json_error))
            .transpose()
    }

    pub fn current_pair_records(&self, job: i64, photo: &str) -> Result<Vec<NativeRecord>> {
        let conn = self.pool.get()?;
        let Some(raw) = current_remote_photo_in(&conn, job, photo)? else {
            return Ok(Vec::new());
        };
        let photo: super::super::RemotePhoto = serde_json::from_str(&raw).map_err(json_error)?;
        let revisions: (Option<String>,Option<String>)=conn.query_row(
            "SELECT native_asset_revision,native_master_revision FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2",
            params![job,photo.photo_id],|r|Ok((r.get(0)?,r.get(1)?)),
        )?;
        let Some(asset_revision) = revisions.0 else {
            return Ok(Vec::new());
        };
        let Some(master_revision) = revisions.1 else {
            return Ok(Vec::new());
        };
        [json!({"recordName":photo.master_id,"recordType":"CPLMaster","recordChangeTag":master_revision,"fields":{
            "photoId":{"type":"STRING","value":photo.photo_id},"contentGeneration":{"type":"STRING","value":photo.content_generation},
            "thumbnailSourceOriginalSHA256":{"type":"STRING","value":photo.thumbnail_source_sha256},
            "resOriginalRes":{"type":"ASSETID","value":photo.original},"resJPEGThumbRes":{"type":"ASSETID","value":photo.thumbnail}}}),
         json!({"recordName":photo.photo_id,"recordType":"CPLAsset","recordChangeTag":asset_revision,"fields":{
            "masterRef":{"type":"REFERENCE","value":{"recordName":photo.master_id,"action":"NONE"}},
            "contentGeneration":{"type":"STRING","value":photo.content_generation},"relativePath":{"type":"STRING","value":photo.relative_path},
            "isFavorite":{"type":"INT64","value":photo.is_favorite as i64},"isHidden":{"type":"INT64","value":photo.is_hidden as i64},
            "assetDate":{"type":"TIMESTAMP","value":photo.asset_date_ms}},"_syncserver":photo.events})]
            .into_iter().map(|v|serde_json::from_value(v).map_err(json_error)).collect()
    }

    pub fn photo_id_for_record(&self, job: i64, name: &str) -> Result<Option<String>> {
        let conn = self.pool.get()?;
        conn.query_row("SELECT COALESCE(native_photo_id,json_extract(native_pending_asset,'$.recordName'),json_extract(native_pending_master,'$.fields.photoId.value')) FROM sync_entries WHERE job_id=?1 AND (native_photo_id=?2 OR native_master_id=?2 OR json_extract(native_pending_asset,'$.recordName')=?2 OR json_extract(native_pending_master,'$.recordName')=?2) LIMIT 1",params![job,name],|r|r.get(0)).optional().map(|v|v.flatten()).map_err(Into::into)
    }

    /// Resource work this job still owes, across every generation. Counted
    /// rather than filtered by job state so a download failure stays visible.
    pub fn unresolved_resource_count(&self, job_id: i64) -> Result<usize> {
        let conn = self.pool.get()?;
        let mut conn = conn;
        let tx = conn.transaction()?;
        unresolved_in_tx(&tx, job_id)
    }

    /// The projection this job currently exposes, at `target`.
    #[doc(hidden)]
    pub fn remote_graph(&self, job: i64, _generation: &str) -> Result<RemoteGraph> {
        let conn = self.pool.get()?;
        let mut stmt=conn.prepare("SELECT native_resource_baseline FROM sync_entries WHERE job_id=?1 AND native_resource_baseline IS NOT NULL")?;
        let rows = stmt.query_map([job], |r| r.get::<_, String>(0))?;
        let mut graph = RemoteGraph::default();
        for raw in rows {
            graph
                .photos
                .push(serde_json::from_str(&raw?).map_err(json_error)?);
        }
        Ok(graph)
    }
}

fn json_error(error: impl std::fmt::Display) -> AppError {
    super::configuration::json_error(error)
}

/// The stored progress row, read column by column rather than by position:
/// the token ladder is the heart of this module and a positional read here
/// would silently cross two of them.
fn read_progress(conn: &rusqlite::Connection, job_id: i64) -> Result<Option<NativeProgress>> {
    let row: Option<ProgressRow> = conn
        .query_row(
            "SELECT job_id, zone_name, owner, tenant, container, environment, database,
                    database_generation, scope_digest, snapshot_token, bootstrap_state,
                    discovery_token, discovery_complete, ingest_token, applied_token,
                    target_token
             FROM sync_native_progress WHERE job_id = ?1",
            [job_id],
            |row| {
                Ok(ProgressRow {
                    job_id: row.get(0)?,
                    zone_name: row.get(1)?,
                    owner: row.get(2)?,
                    tenant: row.get(3)?,
                    container: row.get(4)?,
                    environment: row.get(5)?,
                    database: row.get(6)?,
                    database_generation: row.get(7)?,
                    scope_digest: row.get(8)?,
                    snapshot_token: row.get(9)?,
                    bootstrap_state: row.get(10)?,
                    discovery_token: row.get(11)?,
                    discovery_complete: row.get(12)?,
                    ingest_token: row.get(13)?,
                    applied_token: row.get(14)?,
                    target_token: row.get(15)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let bootstrap = match &row.bootstrap_state {
        Some(value) => serde_json::from_str(value).map_err(json_error)?,
        None => BootstrapState::default(),
    };
    Ok(Some(NativeProgress {
        job_id: row.job_id,
        zone: NativeZone {
            zone_name: row.zone_name,
            owner: row.owner,
        },
        namespace: NativeNamespace {
            tenant: row.tenant,
            container: row.container,
            environment: row.environment,
            database: row.database,
        },
        database_generation: row.database_generation,
        scope_digest: row.scope_digest,
        snapshot_token: row.snapshot_token,
        bootstrap,
        discovery_token: row.discovery_token,
        discovery_complete: row.discovery_complete != 0,
        ingest_token: row.ingest_token,
        applied_token: row.applied_token,
        target_token: row.target_token,
    }))
}

#[derive(Debug)]
struct ProgressRow {
    job_id: i64,
    zone_name: String,
    owner: String,
    tenant: String,
    container: String,
    environment: String,
    database: String,
    database_generation: String,
    scope_digest: String,
    snapshot_token: Option<String>,
    bootstrap_state: Option<String>,
    discovery_token: Option<String>,
    discovery_complete: i64,
    ingest_token: Option<String>,
    applied_token: Option<String>,
    target_token: Option<String>,
}

pub(super) fn execute(
    pool: &DbPool,
    command: &NativeReceiveCommand,
) -> Result<NativeReceiveOutcome> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let outcome = match command {
        NativeReceiveCommand::Begin {
            job_id,
            namespace,
            generation,
            scope_digest,
            zone,
        } => {
            let existing = read_progress(&tx, *job_id)?;
            let reset = existing
                .as_ref()
                .is_some_and(|progress| !progress.matches(namespace, generation, scope_digest));
            if reset {
                // A changed scope, namespace or generation invalidates every
                // claim about the old one. Clearing is deliberate: keeping the
                // old applied token would prove completeness for work that was
                // never in scope.
                tx.execute(
                    "UPDATE sync_native_progress SET snapshot_token = NULL,
                            bootstrap_state = NULL, discovery_token = NULL,
                            discovery_complete = 0, ingest_token = NULL,
                            applied_token = NULL, target_token = NULL, updated_at = unixepoch()
                     WHERE job_id = ?1",
                    [job_id],
                )?;
                tx.execute("UPDATE sync_entries SET native_pending_master=NULL,native_pending_asset=NULL WHERE job_id=?1",[job_id])?;
            }
            tx.execute(
                "INSERT INTO sync_native_progress
                    (job_id, zone_name, owner, tenant, container, environment, database,
                     database_generation, scope_digest, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, unixepoch())
                 ON CONFLICT(job_id, zone_name, owner) DO UPDATE SET
                    tenant = excluded.tenant, container = excluded.container,
                    environment = excluded.environment, database = excluded.database,
                    database_generation = excluded.database_generation,
                    scope_digest = excluded.scope_digest, updated_at = unixepoch()",
                params![
                    job_id,
                    zone.zone_name,
                    zone.owner,
                    namespace.tenant,
                    namespace.container,
                    namespace.environment,
                    namespace.database,
                    generation,
                    scope_digest,
                ],
            )?;
            let mut progress = read_progress(&tx, *job_id)?.expect("just written");
            if reset {
                progress.bootstrap = BootstrapState::default();
                progress.snapshot_token = None;
                progress.ingest_token = None;
                progress.applied_token = None;
                progress.target_token = None;
            }
            NativeReceiveOutcome::Progress(Box::new(progress))
        }
        NativeReceiveCommand::PinSnapshot {
            job_id,
            snapshot_token,
            zone_sync_token,
        } => {
            let progress = require_progress(&tx, *job_id)?;
            if progress
                .snapshot_token
                .as_ref()
                .is_some_and(|old| old != snapshot_token)
            {
                return Err(AppError::Backend(
                    "cannot replace an in-progress bootstrap snapshot".into(),
                ));
            }
            tx.execute(
                "UPDATE sync_native_progress SET snapshot_token=?2,ingest_token=?3 WHERE job_id=?1",
                params![job_id, snapshot_token, zone_sync_token],
            )?;
            NativeReceiveOutcome::Reset
        }
        NativeReceiveCommand::SaveBootstrapPage {
            job_id,
            record_type,
            continuation_marker,
            record_count,
            zone_sync_token,
        } => {
            let mut progress = require_progress(&tx, *job_id)?;
            progress.bootstrap.started = true;
            let _ = zone_sync_token; // A zone position is never a listing snapshot.
            progress.bootstrap.types.insert(
                record_type.clone(),
                TypeMarker {
                    status: if continuation_marker.is_some() {
                        TypeStatus::Pending
                    } else {
                        TypeStatus::Complete
                    },
                    continuation_marker: continuation_marker.clone(),
                    record_count: *record_count,
                },
            );
            persist_bootstrap(&tx, *job_id, &progress)?;
            NativeReceiveOutcome::Progress(Box::new(progress))
        }
        NativeReceiveCommand::SaveDeltaPage {
            job_id,
            events,
            next_token,
        } => {
            let progress = require_progress(&tx, *job_id)?;
            let saved = stage_records(&tx, &progress, events)?;
            // The token moves in the same transaction as the rows it covers, so
            // a crash can only ever replay, never skip.
            tx.execute(
                "UPDATE sync_native_progress SET ingest_token = ?1, updated_at = unixepoch()
                 WHERE job_id = ?2",
                params![next_token, job_id],
            )?;
            NativeReceiveOutcome::SavedEvents(saved)
        }
        NativeReceiveCommand::Promote {
            job_id,
            graph,
            target_token,
            local_digests,
        } => promote(&tx, *job_id, graph, target_token, local_digests, true)?,
        NativeReceiveCommand::PromoteDelta {
            job_id,
            graph,
            target_token,
            local_digests,
        } => promote(&tx, *job_id, graph, target_token, local_digests, false)?,
        NativeReceiveCommand::AdvanceApplied { job_id } => {
            let unresolved = unresolved_in_tx(&tx, *job_id)?;
            let progress = require_progress(&tx, *job_id)?;
            if !progress.can_apply(unresolved) {
                return Err(AppError::Backend(format!(
                    "cannot advance applied position: {unresolved} unresolved items remain"
                )));
            }
            tx.execute(
                "UPDATE sync_native_progress SET applied_token = target_token, updated_at = unixepoch()
                 WHERE job_id = ?1",
                [job_id],
            )?;
            NativeReceiveOutcome::Reset
        }
        NativeReceiveCommand::AdoptConfirmedHead { job_id, sync_token } => {
            let unresolved = unresolved_in_tx(&tx, *job_id)?;
            if unresolved != 0 {
                return Err(AppError::Backend(format!(
                    "cannot adopt a confirmed upload head: {unresolved} unresolved items remain"
                )));
            }
            let issued: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sync_tasks WHERE job_id = ?1 AND state = 'issued'",
                [*job_id],
                |row| row.get(0),
            )?;
            if issued != 0 {
                return Err(AppError::Backend(
                    "cannot adopt a confirmed upload head while issued operations remain".into(),
                ));
            }
            tx.execute(
                "UPDATE sync_native_progress
                    SET ingest_token = ?2, target_token = ?2, applied_token = ?2,
                        updated_at = unixepoch()
                  WHERE job_id = ?1",
                params![job_id, sync_token],
            )?;
            NativeReceiveOutcome::AdoptedHead
        }
        NativeReceiveCommand::Reset { job_id } => {
            // Token expiry rebuilds the cloud-side position only. Local dirty
            // flags, confirmed identity and verified assets are deliberately
            // untouched — and nothing is deleted from a partial listing.
            tx.execute(
                "UPDATE sync_native_progress SET snapshot_token = NULL, bootstrap_state = NULL,
                        ingest_token = NULL, target_token = NULL, applied_token = NULL,
                        updated_at = unixepoch()
                 WHERE job_id = ?1",
                [job_id],
            )?;
            tx.execute("UPDATE sync_entries SET native_pending_master=NULL,native_pending_asset=NULL WHERE job_id=?1",[job_id])?;
            NativeReceiveOutcome::Reset
        }
    };
    tx.commit()?;
    Ok(outcome)
}

fn require_progress(tx: &rusqlite::Transaction<'_>, job_id: i64) -> Result<NativeProgress> {
    read_progress(tx, job_id)?.ok_or_else(|| {
        AppError::Backend("synchronization receive position does not exist yet".into())
    })
}

fn persist_bootstrap(
    tx: &rusqlite::Transaction<'_>,
    job_id: i64,
    progress: &NativeProgress,
) -> Result<()> {
    tx.execute(
        "UPDATE sync_native_progress
            SET bootstrap_state = ?1,
                snapshot_token = COALESCE(snapshot_token, ?2),
                updated_at = unixepoch()
          WHERE job_id = ?3",
        params![
            serde_json::to_string(&progress.bootstrap).map_err(json_error)?,
            progress.snapshot_token,
            job_id,
        ],
    )?;
    Ok(())
}

pub(super) fn current_remote_photo_in(
    conn: &rusqlite::Connection,
    job: i64,
    photo: &str,
) -> Result<Option<String>> {
    let pending:Option<String>=conn.query_row(
        "SELECT json_extract(p.captured_versions,'$.projection') FROM sync_publications p WHERE p.job_id=?1 AND p.photo_id=?2 AND p.state IN ('pending','publishing') ORDER BY p.id DESC LIMIT 1",
        params![job,photo],|r|r.get(0),
    ).optional()?.flatten();
    if pending.is_some() {
        return Ok(pending);
    }
    conn.query_row(
        "SELECT native_resource_baseline FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2",
        params![job, photo],
        |r| r.get(0),
    )
    .optional()
    .map(|v| v.flatten())
    .map_err(Into::into)
}

pub(super) fn record_revision_in(
    conn: &rusqlite::Connection,
    job: i64,
    name: &str,
) -> Result<Option<String>> {
    conn.query_row("SELECT CASE WHEN json_extract(native_pending_asset,'$.recordName')=?2 THEN json_extract(native_pending_asset,'$.recordChangeTag') WHEN json_extract(native_pending_master,'$.recordName')=?2 THEN json_extract(native_pending_master,'$.recordChangeTag') WHEN native_photo_id=?2 THEN native_asset_revision ELSE native_master_revision END FROM sync_entries WHERE job_id=?1 AND (native_photo_id=?2 OR native_master_id=?2 OR json_extract(native_pending_asset,'$.recordName')=?2 OR json_extract(native_pending_master,'$.recordName')=?2) ORDER BY native_photo_id IS NOT NULL DESC LIMIT 1",params![job,name],|r|r.get(0)).optional().map(|v|v.flatten()).map_err(Into::into)
}

fn stage_records(
    tx: &rusqlite::Transaction<'_>,
    progress: &NativeProgress,
    events: &[InboxEvent],
) -> Result<usize> {
    let mut saved = 0;
    for event in events {
        let record: NativeRecord =
            serde_json::from_value(event.payload.clone()).map_err(json_error)?;
        let value = &event.payload;
        let asset = record.record_type.as_deref() == Some("CPLAsset");
        let master = record.record_type.as_deref() == Some("CPLMaster");
        let existing:Option<(i64,Option<String>)>=tx.query_row(
            "SELECT id,native_photo_id FROM sync_entries WHERE job_id=?1 AND (native_photo_id=?2 OR native_master_id=?2 OR json_extract(native_pending_asset,'$.recordName')=?2 OR json_extract(native_pending_master,'$.recordName')=?2) LIMIT 1",
            params![progress.job_id,record.record_name],|r|Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        let photo_name = if asset {
            Some(record.record_name.as_str())
        } else {
            value["fields"]["photoId"]["value"].as_str()
        };
        if !asset && !master && existing.is_none() {
            continue;
        }
        let path = value["fields"]["relativePath"]["value"].as_str();
        let mut entry = existing.map(|r| r.0);
        if entry.is_none() {
            if let Some(photo) = photo_name {
                entry=tx.query_row("SELECT id FROM sync_entries WHERE job_id=?1 AND (native_photo_id=?2 OR json_extract(native_pending_asset,'$.recordName')=?2 OR json_extract(native_pending_master,'$.fields.photoId.value')=?2) LIMIT 1",params![progress.job_id,photo],|r|r.get(0)).optional()?;
            }
        }
        if let Some(path) = path {
            if !super::super::native::records::is_safe_relative_path(path) {
                return Err(AppError::Backend("unsafe remote photo path".into()));
            }
            let local:Option<i64>=tx.query_row("SELECT id FROM sync_entries WHERE job_id=?1 AND relative_path=?2 AND (native_photo_id IS NULL OR native_photo_id=?3)",params![progress.job_id,path,photo_name],|r|r.get(0)).optional()?;
            if let Some(local) = local {
                if let Some(old) = entry.filter(|old| *old != local) {
                    tx.execute("UPDATE sync_entries SET native_pending_master=COALESCE(native_pending_master,(SELECT native_pending_master FROM sync_entries WHERE id=?2)) WHERE id=?1",params![local,old])?;
                    tx.execute("DELETE FROM sync_entries WHERE id=?1 AND native_photo_id IS NULL AND relative_path LIKE '.photoviewer-sync-pending/%'",[old])?;
                }
                entry = Some(local);
            }
        }
        let entry = match entry {
            Some(entry) => entry,
            None => {
                let placeholder = format!(
                    ".photoviewer-sync-pending/{}",
                    photo_name.unwrap_or(&record.record_name)
                );
                let relative = path.unwrap_or(&placeholder);
                let occupied:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_entries WHERE job_id=?1 AND relative_path=?2)",params![progress.job_id,relative],|r|r.get(0))?;
                tx.execute(
                    "INSERT INTO sync_entries(job_id,relative_path) VALUES(?1,?2)",
                    params![
                        progress.job_id,
                        if occupied { &placeholder } else { relative }
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };
        if let Some(path) = path {
            tx.execute("UPDATE sync_entries SET relative_path=?2 WHERE id=?1 AND relative_path LIKE '.photoviewer-sync-pending/%' AND NOT EXISTS(SELECT 1 FROM sync_entries other WHERE other.job_id=?3 AND other.relative_path=?2 AND other.id<>?1)",params![entry,path,progress.job_id])?;
        }
        let column = if asset
            || record.is_tombstone()
                && tx
                    .query_row(
                        "SELECT native_photo_id=?2 FROM sync_entries WHERE id=?1",
                        params![entry, record.record_name],
                        |r| r.get::<_, Option<bool>>(0),
                    )?
                    .unwrap_or(false)
        {
            "native_pending_asset"
        } else {
            "native_pending_master"
        };
        tx.execute(
            &format!("UPDATE sync_entries SET {column}=?2 WHERE id=?1"),
            params![entry, serde_json::to_string(&record).map_err(json_error)?],
        )?;
        saved += 1;
        if asset {
            admit_asset_metadata(tx, progress.job_id, entry, &record)?;
        }
    }
    Ok(saved)
}

fn admit_asset_metadata(
    tx: &rusqlite::Transaction<'_>,
    job: i64,
    entry: i64,
    record: &NativeRecord,
) -> Result<()> {
    let deleting: bool = tx.query_row(
        "SELECT native_dirty=4 FROM sync_entries WHERE id=?1",
        [entry],
        |r| r.get(0),
    )?;
    if deleting {
        return Ok(());
    }
    let fields = record
        .fields
        .as_ref()
        .ok_or_else(|| AppError::Backend("photo metadata has no fields".into()))?;
    let field = |name: &str| {
        fields
            .get(name)
            .map(|v| v["value"].clone())
            .unwrap_or(Value::Null)
    };
    let relative = field("relativePath");
    let Some(relative) = relative.as_str() else {
        return Ok(());
    };
    let root: String =
        tx.query_row("SELECT local_root FROM sync_jobs WHERE id=?1", [job], |r| {
            r.get(0)
        })?;
    let path = std::path::Path::new(&root).join(relative);
    let uri = crate::core::file_uri::from_path(&path);
    let linked:Option<(i64,i64,String)>=tx.query_row(
        "SELECT m.id,m.storage_location,m.uri FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",
        [entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    if let Some((media, location, old_uri)) = linked {
        // Keep the identity of an existing local file through publication. A
        // metadata rename must not insert another row at its destination URI.
        if location != 2 {
            return Ok(());
        }
        if old_uri != uri {
            let occupied: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM media_items WHERE uri=?1 AND id<>?2)",
                params![uri, media],
                |r| r.get(0),
            )?;
            if occupied {
                return Ok(());
            }
            tx.execute("UPDATE media_items SET uri=?2,path=?3,folder_path=?4,thumbnail_ready=0 WHERE id=?1",params![media,uri,path.to_string_lossy(),path.parent().map(|p|p.to_string_lossy().into_owned())])?;
            tx.execute("UPDATE sync_entries SET relative_path=?2 WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM sync_entries other WHERE other.job_id=?3 AND other.relative_path=?2 AND other.id<>?1)",params![entry,relative,job])?;
        }
    }
    let mime = crate::core::media::mime_from_extension(&path).unwrap_or("image/jpeg");
    tx.execute("INSERT INTO media_items(uri,path,folder_path,mime_type,media_kind,file_mtime,file_mtime_ns,file_size,blake3_hash,indexed_at,storage_location,thumbnail_ready,is_favorite,taken_at) VALUES(?1,?2,?3,?4,?5,0,0,0,'',unixepoch(),2,0,?6,?7) ON CONFLICT(uri) DO NOTHING",params![uri,path.to_string_lossy(),path.parent().map(|p|p.to_string_lossy().into_owned()).unwrap_or(root),mime,crate::core::db::media_kind_db_value(mime),field("isFavorite").as_i64().unwrap_or(0),field("assetDate").as_i64().map(|ms|ms.div_euclid(1000))])?;
    tx.execute("UPDATE sync_entries SET media_id=COALESCE(media_id,(SELECT id FROM media_items WHERE uri=?2)) WHERE id=?1",params![entry,uri])?;
    tx.execute(
        "UPDATE media_items SET is_favorite=?2,taken_at=?3 WHERE uri=?1 AND storage_location=2 AND NOT EXISTS(SELECT 1 FROM sync_entries e WHERE e.media_id=media_items.id AND e.native_metadata_version>e.native_acked_metadata_version)",
        params![
            uri,
            field("isFavorite").as_i64().unwrap_or(0),
            field("assetDate").as_i64().map(|ms| ms.div_euclid(1000))
        ],
    )?;
    // Receipt is not confirmation of a local/cloud identity match. The pending
    // Asset already names this photo; projection binds IDs after collision checks.
    Ok(())
}

fn promote(
    tx: &rusqlite::Transaction<'_>,
    job_id: i64,
    graph: &RemoteGraph,
    target_token: &str,
    local_digests: &HashMap<String, LocalDigest>,
    _replace: bool,
) -> Result<NativeReceiveOutcome> {
    let progress = require_progress(tx, job_id)?;
    let generation = progress.database_generation.clone();

    for photo in &graph.photos {
        let content = super::runtime::project_publication(tx, job_id, photo, local_digests)?;
        // A collision may have moved the previous local occupant aside. Admit
        // the remote metadata at its final path immediately, before downloads.
        let entry: Option<i64> = tx
            .query_row(
                "SELECT id FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2",
                params![job_id, photo.photo_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(entry) = entry {
            let metadata:NativeRecord=serde_json::from_value(json!({"recordName":photo.photo_id,"recordType":"CPLAsset","fields":{
                "relativePath":{"type":"STRING","value":photo.relative_path},
                "masterRef":{"type":"REFERENCE","value":{"recordName":photo.master_id,"action":"NONE"}},
                "isFavorite":{"type":"INT64","value":photo.is_favorite as i64},
                "assetDate":{"type":"TIMESTAMP","value":photo.asset_date_ms}
            }})).map_err(json_error)?;
            admit_asset_metadata(tx, job_id, entry, &metadata)?;
        }
        tx.execute("UPDATE media_items SET mime_type=?3,media_kind=?4,width=?5,height=?6,file_size=?7 WHERE id=(SELECT media_id FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2) AND storage_location=2",params![job_id,photo.photo_id,photo.original.mime_type,crate::core::db::media_kind_db_value(&photo.original.mime_type),photo.original.width,photo.original.height,photo.original.size as i64])?;
        let asset_revision = record_revision_in(tx, job_id, &photo.photo_id)?;
        let master_revision = record_revision_in(tx, job_id, &photo.master_id)?;
        tx.execute("UPDATE sync_entries SET native_asset_revision=COALESCE(?3,native_asset_revision),native_master_revision=COALESCE(?4,native_master_revision) WHERE job_id=?1 AND native_photo_id=?2",params![job_id,photo.photo_id,asset_revision,master_revision])?;
        tx.execute("UPDATE sync_entries SET native_pending_master=NULL,native_pending_asset=NULL WHERE job_id=?1 AND native_photo_id=?2",params![job_id,photo.photo_id])?;
        tx.execute("DELETE FROM sync_entries WHERE job_id=?1 AND native_photo_id IS NULL AND relative_path LIKE '.photoviewer-sync-pending/%' AND (json_extract(native_pending_asset,'$.recordName')=?2 OR json_extract(native_pending_master,'$.fields.photoId.value')=?2)",params![job_id,photo.photo_id])?;
        if content == super::runtime::ProjectedContent::Local {
            tx.execute("UPDATE sync_native_resources SET record_name=NULL WHERE job_id=?1 AND record_name=?2 AND NOT EXISTS(SELECT 1 FROM sync_publications p WHERE p.job_id=?1 AND p.photo_id=?2 AND p.state IN ('pending','publishing'))",params![job_id,photo.photo_id])?;
            continue;
        }
        for resource in photo.resources() {
            tx.execute(
                "INSERT INTO sync_native_resources
                    (job_id, generation, asset_id, role, record_name, expected_sha256,
                     expected_size, state, verified_cache)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    CASE WHEN EXISTS(SELECT 1 FROM sync_entries e WHERE e.job_id=?1 AND e.native_photo_id=?5 AND (json_extract(e.native_resource_baseline,'$.original.assetId')=?3 OR json_extract(e.native_resource_baseline,'$.thumbnail.assetId')=?3) AND NOT EXISTS(SELECT 1 FROM sync_publications p WHERE p.job_id=?1 AND p.state NOT IN ('complete','superseded') AND (p.original_asset_id=?3 OR p.thumbnail_asset_id=?3))) THEN 'verified' ELSE 'pending' END,
                    -- When the entry's confirmed baseline already names this
                    -- asset, the bytes are the library file's — adoption proved
                    -- it, and it is recorded here so the settled row points at
                    -- the file that holds them rather than at staging, which
                    -- adoption has no reason to write.
                    (SELECT m.uri FROM sync_entries e
                       JOIN media_items m ON m.path = rtrim((SELECT j.local_root FROM sync_jobs j WHERE j.id=?1), '/') || '/' || e.relative_path
                      WHERE e.job_id = ?1 AND e.native_photo_id = ?5 AND m.trashed_at IS NULL
                      LIMIT 1))
                 ON CONFLICT(job_id, generation, asset_id, role) DO UPDATE SET
                    record_name = excluded.record_name,
                    expected_sha256 = excluded.expected_sha256,
                    expected_size = excluded.expected_size,
                    -- An entry's baseline can acquire an asset *after* this row
                    -- was written: adoption proves the library file already is
                    -- that asset. Refreshing the derived state is what stops the
                    -- transfer stage downloading a copy of a file sitting in the
                    -- library — and, since an outstanding obligation is what
                    -- holds the applied position back, what stops the job
                    -- reporting itself as synchronizing forever.
                    --
                    -- Guarded on the entry owing nothing upward. A dirty entry's
                    -- bytes are about to be uploaded, which is what staging is
                    -- for; settling its resources would leave the publish stage
                    -- with a verified row and no file to read. A clean entry has
                    -- no publication to publish, so it can never reach that code.
                    --
                    -- `staging_path` is dropped along with it. A settled-by-
                    -- adoption row has nothing staged, so keeping the old path
                    -- would leave the database pointing at a file that only
                    -- exists until someone cleans the staging directory.
                    state = CASE WHEN excluded.state = 'verified' AND 1 = (SELECT (e.native_dirty = 0
                    AND NOT EXISTS(SELECT 1 FROM sync_publications p
                                    WHERE p.entry_id = e.id AND p.state NOT IN ('complete','superseded')))
                FROM sync_entries e
               WHERE e.job_id = excluded.job_id AND e.native_photo_id = excluded.record_name
               LIMIT 1)
                                 THEN 'verified' ELSE sync_native_resources.state END,
                    verified_cache = CASE WHEN excluded.state = 'verified' AND 1 = (SELECT (e.native_dirty = 0
                    AND NOT EXISTS(SELECT 1 FROM sync_publications p
                                    WHERE p.entry_id = e.id AND p.state NOT IN ('complete','superseded')))
                FROM sync_entries e
               WHERE e.job_id = excluded.job_id AND e.native_photo_id = excluded.record_name
               LIMIT 1)
                                 THEN (SELECT m.uri FROM sync_entries e
                                        JOIN sync_jobs j ON j.id = e.job_id
                                        JOIN media_items m
                                          ON m.path = rtrim(j.local_root, '/') || '/' || e.relative_path
                                       WHERE e.job_id = excluded.job_id AND e.native_photo_id = excluded.record_name
                                         AND m.trashed_at IS NULL
                                       LIMIT 1)
                                 ELSE sync_native_resources.verified_cache END,
                    staging_path = CASE WHEN excluded.state = 'verified' AND 1 = (SELECT (e.native_dirty = 0
                    AND NOT EXISTS(SELECT 1 FROM sync_publications p
                                    WHERE p.entry_id = e.id AND p.state NOT IN ('complete','superseded')))
                FROM sync_entries e
               WHERE e.job_id = excluded.job_id AND e.native_photo_id = excluded.record_name
               LIMIT 1)
                                 THEN NULL ELSE sync_native_resources.staging_path END",
                params![
                    job_id,
                    generation,
                    resource.asset_id,
                    resource.role,
                    resource.record_name,
                    resource.expected_sha256,
                    resource.expected_size as i64,
                ],
            )?;
        }
    }
    // Only an explicit server tombstone can retire a confirmed identity.
    // A missing row in a snapshot or delta is never a deletion notification.
    for name in &graph.tombstones {
        let entry: Option<(i64, i64)> = tx
            .query_row(
                "SELECT id,native_dirty FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2",
                params![job_id, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((entry, dirty)) = entry {
            if dirty == super::dirty::NativeDirty::DELETE as i64 {
                // An explicit cloud tombstone proves the local deletion already
                // converged. Unknown issued results keep their evidence until
                // the ordinary callback reconciles them.
                tx.execute("DELETE FROM sync_entries WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND state='issued')",[entry])?;
            } else if dirty == 0 {
                tx.execute("DELETE FROM media_items WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1) AND storage_location=2",[entry])?;
                tx.execute("UPDATE media_items SET storage_location=1 WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1) AND storage_location=3",[entry])?;
                super::actions::retire(tx, entry)?;
            } else {
                tx.execute("INSERT INTO sync_conflicts(job_id,entry_id,kind,native_detail) VALUES(?1,?2,'native',?3) ON CONFLICT(entry_id) WHERE state='open' DO UPDATE SET native_detail=excluded.native_detail",params![job_id,entry,serde_json::json!({"reason":"remote photo was deleted while local changes remain","remote":null}).to_string()])?;
            }
            tx.execute("UPDATE sync_publications SET state='superseded' WHERE entry_id=?1 AND state='pending'",[entry])?;
        }
        tx.execute("UPDATE sync_native_resources SET record_name=NULL WHERE job_id=?1 AND generation=?2 AND record_name=?3",params![job_id,generation,name])?;
        tx.execute("UPDATE sync_entries SET native_pending_master=NULL,native_pending_asset=NULL WHERE job_id=?1 AND (native_photo_id=?2 OR native_master_id=?2 OR json_extract(native_pending_asset,'$.recordName')=?2 OR json_extract(native_pending_master,'$.recordName')=?2)",params![job_id,name])?;
    }

    // Asset IDs can be shared by multiple photos. Adoption of one local file
    // does not provide staging bytes for another photo that still has to be
    // placed. Resolve this after the complete graph, independently of order.
    tx.execute("UPDATE sync_native_resources SET state='pending',verified_cache=NULL WHERE job_id=?1 AND generation=?2 AND staging_path IS NULL AND EXISTS(SELECT 1 FROM sync_publications p WHERE p.job_id=sync_native_resources.job_id AND p.state IN ('pending','publishing') AND ((sync_native_resources.role='original' AND p.original_asset_id=sync_native_resources.asset_id) OR (sync_native_resources.role='thumbnail' AND p.thumbnail_asset_id=sync_native_resources.asset_id)) AND EXISTS(SELECT 1 FROM sync_entries e WHERE e.job_id=sync_native_resources.job_id AND e.native_photo_id IS NOT NULL AND e.native_photo_id<>sync_native_resources.record_name AND (json_extract(e.native_resource_baseline,'$.original.assetId')=sync_native_resources.asset_id OR json_extract(e.native_resource_baseline,'$.thumbnail.assetId')=sync_native_resources.asset_id)))",params![job_id,generation])?;
    let resources = unresolved_in_tx(tx, job_id)?;

    tx.execute(
        "UPDATE sync_native_progress SET target_token = ?1, updated_at = unixepoch() WHERE job_id = ?2",
        params![target_token, job_id],
    )?;

    // The gate is evaluated against the position this promotion just installed,
    // not the one read before it: `can_apply` requires a target to exist, and
    // reading it early would make an empty zone look permanently behind.
    let mut promoted = read_progress(tx, job_id)?.expect("progress exists");
    promoted.target_token = Some(target_token.to_string());
    let applied = promoted.can_apply(resources);
    if applied {
        tx.execute(
            "UPDATE sync_native_progress SET applied_token = target_token WHERE job_id = ?1",
            [job_id],
        )?;
    }
    Ok(NativeReceiveOutcome::Promoted {
        photos: graph.photo_count(),
        unknown_records: graph.records.len(),
        resources,
        applied,
    })
}

fn unresolved_in_tx(tx: &rusqlite::Transaction<'_>, job_id: i64) -> Result<usize> {
    let resources: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sync_native_resources WHERE job_id = ?1 AND generation=(SELECT database_generation FROM sync_native_progress WHERE job_id=?1) AND record_name IS NOT NULL AND state NOT IN ('verified','retained')",
        [job_id],
        |row| row.get(0),
    )?;
    let conflicts: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sync_conflicts WHERE job_id = ?1 AND state = 'open'",
        [job_id],
        |row| row.get(0),
    )?;
    let publications: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sync_publications WHERE job_id=?1 AND state NOT IN ('complete','superseded')",
        [job_id],
        |row| row.get(0),
    )?;
    let fragments:i64=tx.query_row("SELECT COUNT(*) FROM sync_entries WHERE job_id=?1 AND (native_pending_master IS NOT NULL OR native_pending_asset IS NOT NULL)",[job_id],|r|r.get(0))?;
    Ok((resources + conflicts + publications + fragments) as usize)
}

#[cfg(test)]
mod tests;

impl SyncStore {
    fn write_receive(&self, command: NativeReceiveCommand) -> Result<NativeReceiveOutcome> {
        match self.write(SyncWrite::NativeReceive(Box::new(command)))? {
            SyncWriteResult::Native(outcome) => Ok(outcome),
            _ => Err(AppError::Backend(
                "synchronization receive write returned an unexpected result".into(),
            )),
        }
    }

    pub fn begin_native_receive(
        &self,
        job_id: i64,
        namespace: NativeNamespace,
        generation: &str,
        scope_digest: &str,
        zone: NativeZone,
    ) -> Result<NativeProgress> {
        match self.write_receive(NativeReceiveCommand::Begin {
            job_id,
            namespace,
            generation: generation.to_string(),
            scope_digest: scope_digest.to_string(),
            zone,
        })? {
            NativeReceiveOutcome::Progress(progress) => Ok(*progress),
            _ => Err(AppError::Backend(
                "beginning a receive position returned no progress".into(),
            )),
        }
    }

    pub fn pin_native_snapshot(
        &self,
        job_id: i64,
        snapshot_token: &str,
        zone_sync_token: &str,
    ) -> Result<()> {
        self.write_receive(NativeReceiveCommand::PinSnapshot {
            job_id,
            snapshot_token: snapshot_token.into(),
            zone_sync_token: zone_sync_token.into(),
        })?;
        Ok(())
    }

    pub fn save_bootstrap_page(
        &self,
        job_id: i64,
        record_type: &str,
        continuation_marker: Option<String>,
        record_count: usize,
        zone_sync_token: &str,
    ) -> Result<NativeProgress> {
        match self.write_receive(NativeReceiveCommand::SaveBootstrapPage {
            job_id,
            record_type: record_type.to_string(),
            continuation_marker,
            record_count,
            zone_sync_token: zone_sync_token.to_string(),
        })? {
            NativeReceiveOutcome::Progress(progress) => Ok(*progress),
            _ => Err(AppError::Backend(
                "saving a bootstrap page returned no progress".into(),
            )),
        }
    }

    pub fn save_delta_page(
        &self,
        job_id: i64,
        events: Vec<InboxEvent>,
        next_token: &str,
    ) -> Result<usize> {
        match self.write_receive(NativeReceiveCommand::SaveDeltaPage {
            job_id,
            events,
            next_token: next_token.to_string(),
        })? {
            NativeReceiveOutcome::SavedEvents(saved) => Ok(saved),
            _ => Err(AppError::Backend(
                "saving a delta page returned no event count".into(),
            )),
        }
    }

    /// Local files this job covers that have never been published.
    ///
    /// These are the only entries that can be adopted rather than conflicted,
    /// and they are the only ones worth measuring: an entry that already holds
    /// a cloud identity has nothing to prove.
    ///
    /// The library row is found by **path**, not through `sync_entries.media_id`.
    /// That association is a presentation convenience and it is not always
    /// populated: an entry first created by a cloud projection has no media row
    /// to point at, and enrolment's `ON CONFLICT DO NOTHING` never backfills
    /// one. Cloud identity remains the record ID; the relative path only locates
    /// a candidate local file for receive comparison.
    pub fn unclaimed_local_entries(&self, job: i64) -> Result<Vec<LocalEntry>> {
        Ok(self
            .local_entries(job)?
            .into_iter()
            .filter(|entry| entry.photo_id.is_none())
            .collect())
    }

    pub fn local_entries(&self, job: i64) -> Result<Vec<LocalEntry>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT e.relative_path, m.path, m.file_size, e.native_local_digest, e.id, e.native_local_version, e.native_photo_id
             FROM sync_entries e
             JOIN sync_jobs j ON j.id = e.job_id
             JOIN sync_connections c ON c.id=j.connection_id AND c.enabled=1
             JOIN media_items m
               ON m.path = rtrim(j.local_root, '/') || '/' || e.relative_path
              AND m.trashed_at IS NULL AND m.storage_location<>2
             WHERE e.job_id = ?1",
        )?;
        let rows = stmt
            .query_map([job], |row| {
                Ok(LocalEntry {
                    photo_id: row.get(6)?,
                    entry_id: row.get(4)?,
                    local_version: row.get(5)?,
                    relative_path: row.get(0)?,
                    local_path: PathBuf::from(row.get::<_, String>(1)?),
                    file_size: row.get::<_, i64>(2)? as u64,
                    cached: row
                        .get::<_, Option<String>>(3)?
                        .and_then(|raw| serde_json::from_str::<LocalDigest>(&raw).ok())
                        .filter(|cached| {
                            Some(cached.entry_id) == row.get(4).ok()
                                && Some(cached.local_version) == row.get(5).ok()
                        })
                        .map(|cached| cached.digest),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn promote_remote_delta(
        &self,
        job_id: i64,
        graph: RemoteGraph,
        target_token: &str,
        local_digests: HashMap<String, LocalDigest>,
    ) -> Result<(usize, usize, usize, bool)> {
        match self.write_receive(NativeReceiveCommand::PromoteDelta {
            job_id,
            graph,
            target_token: target_token.into(),
            local_digests,
        })? {
            NativeReceiveOutcome::Promoted {
                photos,
                unknown_records,
                resources,
                applied,
            } => Ok((photos, unknown_records, resources, applied)),
            _ => Err(AppError::Backend(
                "applying a remote delta returned an unexpected result".into(),
            )),
        }
    }

    pub fn promote_remote_graph(
        &self,
        job_id: i64,
        graph: RemoteGraph,
        target_token: &str,
        local_digests: HashMap<String, LocalDigest>,
    ) -> Result<(usize, usize, usize, bool)> {
        match self.write_receive(NativeReceiveCommand::Promote {
            job_id,
            graph,
            target_token: target_token.to_string(),
            local_digests,
        })? {
            NativeReceiveOutcome::Promoted {
                photos,
                unknown_records,
                resources,
                applied,
            } => Ok((photos, unknown_records, resources, applied)),
            _ => Err(AppError::Backend(
                "promoting a remote graph returned an unexpected result".into(),
            )),
        }
    }

    pub fn advance_applied_token(&self, job_id: i64) -> Result<bool> {
        match self.write_receive(NativeReceiveCommand::AdvanceApplied { job_id })? {
            NativeReceiveOutcome::Reset => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn adopt_confirmed_head(&self, job_id: i64, sync_token: &str) -> Result<()> {
        match self.write_receive(NativeReceiveCommand::AdoptConfirmedHead {
            job_id,
            sync_token: sync_token.to_string(),
        })? {
            NativeReceiveOutcome::AdoptedHead => Ok(()),
            _ => Err(AppError::Backend(
                "adopting a confirmed upload head returned an unexpected result".into(),
            )),
        }
    }

    pub fn reset_native_progress(&self, job_id: i64) -> Result<()> {
        self.write_receive(NativeReceiveCommand::Reset { job_id })?;
        Ok(())
    }
}
