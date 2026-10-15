//! Durable receive state: bootstrap markers, the ordered inbox, and promotion
//! of a validated graph.
//!
//! These tables are **not** local-change truth. They record what the cloud has
//! said and what this client still owes it, which is why a failed download or a
//! crash mid-round loses no work: the next cycle resumes from the tokens and the
//! inbox rather than starting over.
//!
//! The token ladder is deliberately four distinct positions. `snapshot_token`
//! pins a full read at one height; `ingest_token` is where the delta stream
//! resumes; `target_token` is the height of the newest complete projection;
//! `applied_token` is the height this client has actually finished catching up
//! to. A client may only advance `applied` once the projection at `target` has
//! no unresolved resource or conflict work — otherwise a fully received but
//! un-downloaded photo would report itself as synchronized.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::core::db::DbPool;
use crate::core::error::{AppError, Result};

use super::super::native::protocol::{NativeNamespace, NativeZone};
use super::super::native::records::{RemoteGraph, PHOTO_PAYLOAD_MARKER};
use super::{SyncStore, SyncWrite, SyncWriteResult};

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
        self.target_token.is_some() && self.applied_token != self.target_token && unresolved == 0
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
    },
    /// Attempt to advance `applied`. Refused while any obligation is
    /// unresolved, which is what keeps a fully received but un-downloaded photo
    /// from reporting itself as synchronized.
    AdvanceApplied { job_id: i64 },
    /// Drop the receive position after token expiry. Local dirty, confirmed
    /// facts and verified assets are untouched; only the cloud-side position is
    /// rebuilt.
    Reset { job_id: i64 },
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
    Reset,
}

impl super::SyncStore {
    /// Durable progress for this job, or `None` before the first cycle.
    pub fn native_progress(&self, job_id: i64) -> Result<Option<NativeProgress>> {
        let conn = self.pool.get()?;
        read_progress(&conn, job_id)
    }

    /// Ordered inbox events for this job's current generation, oldest first.
    pub fn inbox_events(&self, job_id: i64, generation: &str) -> Result<Vec<InboxEvent>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT ordinal, record_name, record_type, record_change_tag, kind, payload
             FROM sync_native_inbox
             WHERE job_id = ?1 AND generation = ?2
             ORDER BY ordinal",
        )?;
        let rows = stmt.query_map(params![job_id, generation], |row| {
            let payload: Option<String> = row.get(5)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                payload,
            ))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (ordinal, record_name, record_type, record_change_tag, kind, payload) = row?;
            events.push(InboxEvent {
                ordinal,
                record_name,
                record_type,
                record_change_tag,
                kind: match kind.as_str() {
                    "created" => InboxKind::Created,
                    "updated" => InboxKind::Updated,
                    _ => InboxKind::Deleted,
                },
                payload: payload
                    .map(|value| serde_json::from_str(&value))
                    .transpose()
                    .map_err(json_error)?
                    .unwrap_or(serde_json::Value::Null),
            });
        }
        Ok(events)
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
    pub fn remote_graph(&self, job_id: i64, generation: &str) -> Result<RemoteGraph> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT record_name, record_type, payload FROM sync_native_records
             WHERE job_id = ?1 AND generation = ?2 ORDER BY record_name",
        )?;
        let rows = stmt.query_map(params![job_id, generation], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (record_name, record_type, payload) = row?;
            records.push((
                record_name,
                record_type,
                serde_json::from_str(&payload).map_err(json_error)?,
            ));
        }
        Ok(rebuild_graph(&records))
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

/// Rebuild the typed graph from projected rows. The projection is stored as
/// validated photos plus retained unknown records, so this only has to split
/// them back apart.
fn rebuild_graph(rows: &[(String, String, serde_json::Value)]) -> RemoteGraph {
    let mut photos = Vec::new();
    let mut records = Vec::new();
    for (record_name, record_type, payload) in rows {
        if record_type == PHOTO_PAYLOAD_MARKER {
            if let Ok(photo) = serde_json::from_value(payload.clone()) {
                photos.push(photo);
            }
            continue;
        }
        records.push(crate::core::sync::native::records::ProjectedRecord {
            record_name: record_name.clone(),
            record_type: record_type.clone(),
            record_change_tag: payload
                .get("recordChangeTag")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            payload: payload.clone(),
        });
    }
    RemoteGraph {
        photos,
        records,
        tombstones: Vec::new(),
    }
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
                tx.execute("DELETE FROM sync_native_inbox WHERE job_id = ?1", [job_id])?;
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
        NativeReceiveCommand::SaveBootstrapPage {
            job_id,
            record_type,
            continuation_marker,
            record_count,
            zone_sync_token,
        } => {
            let mut progress = require_progress(&tx, *job_id)?;
            progress.bootstrap.started = true;
            if progress.snapshot_token.is_none() {
                progress.snapshot_token = Some(zone_sync_token.clone());
            }
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
            let generation = progress.database_generation.clone();
            let zone_name = progress.zone.zone_name.clone();
            let owner = progress.zone.owner.clone();
            let mut ordinal = next_ordinal(&tx, *job_id, &generation)?;
            let mut saved = 0usize;
            for event in events {
                ordinal += 1;
                // Replay must be idempotent: the same (record, revision) seen
                // twice keeps its first position and is skipped the second time.
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO sync_native_inbox
                        (job_id, generation, zone_name, owner, ordinal, record_name,
                         record_type, record_change_tag, kind, payload)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        job_id,
                        generation,
                        zone_name,
                        owner,
                        ordinal,
                        event.record_name,
                        event.record_type,
                        event.record_change_tag,
                        match event.kind {
                            InboxKind::Created => "created",
                            InboxKind::Updated => "updated",
                            InboxKind::Deleted => "deleted",
                        },
                        serde_json::to_string(&event.payload).map_err(json_error)?,
                    ],
                )?;
                if inserted > 0 {
                    saved += 1;
                }
            }
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
        } => promote(&tx, *job_id, graph, target_token)?,
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
            tx.execute("DELETE FROM sync_native_inbox WHERE job_id = ?1", [job_id])?;
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

fn next_ordinal(tx: &rusqlite::Transaction<'_>, job_id: i64, generation: &str) -> Result<i64> {
    let current: i64 = tx.query_row(
        "SELECT COALESCE(MAX(ordinal), 0) FROM sync_native_inbox WHERE job_id = ?1 AND generation = ?2",
        params![job_id, generation],
        |row| row.get(0),
    )?;
    Ok(current)
}

fn promote(
    tx: &rusqlite::Transaction<'_>,
    job_id: i64,
    graph: &RemoteGraph,
    target_token: &str,
) -> Result<NativeReceiveOutcome> {
    let progress = require_progress(tx, job_id)?;
    let generation = progress.database_generation.clone();
    let zone_name = progress.zone.zone_name.clone();
    let owner = progress.zone.owner.clone();

    // Replace atomically: the projection is only ever read whole.
    tx.execute(
        "DELETE FROM sync_native_records WHERE job_id = ?1 AND generation = ?2",
        params![job_id, generation],
    )?;
    tx.execute(
        "UPDATE sync_native_resources SET record_name=NULL WHERE job_id=?1 AND generation=?2",
        params![job_id, generation],
    )?;
    for photo in &graph.photos {
        tx.execute(
            "INSERT INTO sync_native_records
                (job_id, generation, zone_name, owner, record_name, record_type,
                 record_change_tag, content_generation, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                job_id,
                generation,
                zone_name,
                owner,
                photo.photo_id,
                PHOTO_PAYLOAD_MARKER,
                photo.content_generation,
                photo.content_generation,
                serde_json::to_string(photo).map_err(json_error)?,
            ],
        )?;
        super::runtime::project_publication(tx, job_id, photo)?;
        for resource in photo.resources() {
            tx.execute(
                "INSERT INTO sync_native_resources
                    (job_id, generation, asset_id, role, record_name, expected_sha256,
                     expected_size, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CASE WHEN EXISTS(SELECT 1 FROM sync_entries e WHERE e.job_id=?1 AND e.native_photo_id=?5 AND (json_extract(e.native_resource_baseline,'$.original.assetId')=?3 OR json_extract(e.native_resource_baseline,'$.thumbnail.assetId')=?3)) THEN 'verified' ELSE 'pending' END)
                 ON CONFLICT(job_id, generation, asset_id, role) DO UPDATE SET
                    record_name = excluded.record_name,
                    expected_sha256 = excluded.expected_sha256,
                    expected_size = excluded.expected_size",
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
    for record in &graph.records {
        tx.execute(
            "INSERT INTO sync_native_records
                (job_id, generation, zone_name, owner, record_name, record_type,
                 record_change_tag, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(job_id, generation, zone_name, owner, record_name) DO UPDATE SET
                record_type = excluded.record_type,
                record_change_tag = excluded.record_change_tag,
                payload = excluded.payload,
                updated_at = unixepoch()",
            params![
                job_id,
                generation,
                zone_name,
                owner,
                record.record_name,
                record.record_type,
                record.record_change_tag,
                serde_json::to_string(&record.payload).map_err(json_error)?,
            ],
        )?;
    }

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
        "SELECT COUNT(*) FROM sync_native_resources WHERE job_id = ?1 AND generation=(SELECT database_generation FROM sync_native_progress WHERE job_id=?1) AND record_name IS NOT NULL AND state <> 'verified'",
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
    Ok((resources + conflicts + publications) as usize)
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

    pub fn promote_remote_graph(
        &self,
        job_id: i64,
        graph: RemoteGraph,
        target_token: &str,
    ) -> Result<(usize, usize, usize, bool)> {
        match self.write_receive(NativeReceiveCommand::Promote {
            job_id,
            graph,
            target_token: target_token.to_string(),
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

    pub fn reset_native_progress(&self, job_id: i64) -> Result<()> {
        self.write_receive(NativeReceiveCommand::Reset { job_id })?;
        Ok(())
    }
}
