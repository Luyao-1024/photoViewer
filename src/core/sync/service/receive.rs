//! One receive cycle: bootstrap when needed, then a fixed-height delta round.
//!
//! The cycle is deliberately boring. It reads the durable position, asks the
//! server for the next page, saves it, and stops. Everything that makes this
//! safe — resuming mid-page, tolerating a server that moves under us, refusing
//! to claim completeness while work is outstanding — lives in the store's
//! transactions, not here.
//!
//! What this stage does **not** do, and says so: it never downloads a resource,
//! publishes a file, or admits media. It materializes the obligations that
//! those stages will settle, and it leaves `applied` behind `target` while any
//! of them is outstanding. That is the honest C1.2 state.

use std::collections::HashMap;

use super::super::native::protocol::{NativeClient, NativePinnedScope, NativeZone};
use super::super::native::receive::{NativeRecord, ZoneChangesEntry, SUPPORTED_RECORD_TYPES};
use super::super::native::records::{RecordSet, RemoteGraph};
use super::super::store::receive::LocalDigest;
use super::super::store::receive::{InboxEvent, InboxKind, NativeProgress, TypeStatus};
use super::SyncService;
pub(super) mod collisions;

/// What one cycle accomplished. `applied` stays false for the whole of C1.2,
/// because promotion always materializes resource work that C1.4 settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveReport {
    pub job_id: i64,
    pub bootstrapped: bool,
    /// Types completed by this cycle.
    pub bootstrap_types_completed: usize,
    pub pages_received: usize,
    pub events_saved: usize,
    pub photos_projected: usize,
    pub unknown_records: usize,
    pub outstanding_resources: usize,
    pub applied: bool,
    pub target_token: Option<String>,
}

/// A cycle that could not finish, with the reason stated rather than guessed.
#[derive(Debug, thiserror::Error)]
pub enum ReceiveError {
    #[error("job {0} has no configured native zone")]
    NoZone(i64),
    #[error("job {0} has no probed capability scope")]
    NoScope(i64),
    #[error("the server does not support everything this receive cycle needs")]
    CapabilityMissing,
    #[error("the receive position expired; metadata will be rebuilt from a new snapshot")]
    TokenExpired,
    #[error("the remote graph is not a complete set of photos: {0}")]
    IncompleteGraph(String),
    #[error("transport failed: {0}")]
    Transport(String),
}

impl SyncService {
    /// Run one receive cycle for a native job.
    ///
    /// The cycle is idempotent: rerunning it after a failure resumes from the
    /// stored token and re-appends only events the inbox has not seen.
    pub async fn run_receive(
        &self,
        job_id: i64,
    ) -> std::result::Result<ReceiveReport, ReceiveError> {
        match self.receive_once(job_id).await {
            Err(ReceiveError::TokenExpired) => {
                self.store
                    .reset_native_progress(job_id)
                    .map_err(|error| ReceiveError::Transport(error.to_string()))?;
                self.receive_once(job_id).await
            }
            result => result,
        }
    }

    async fn receive_once(&self, job_id: i64) -> std::result::Result<ReceiveReport, ReceiveError> {
        let job = self
            .store()
            .get_job(job_id)
            .map_err(|error| ReceiveError::Transport(error.to_string()))?
            .ok_or(ReceiveError::NoZone(job_id))?;
        let zone = job
            .native_zone
            .clone()
            .ok_or(ReceiveError::NoZone(job_id))?;
        let pinned = self
            .store()
            .native_scope(job.connection_id)
            .map_err(|error| ReceiveError::Transport(error.to_string()))?
            .ok_or(ReceiveError::NoScope(job_id))?;
        let client = self.receive_client(&job, &pinned).await?;

        let scope_digest = scope_digest(&pinned, &job);
        let progress = self
            .store
            .begin_native_receive(
                job_id,
                pinned.namespace.clone(),
                &pinned.database_generation,
                &scope_digest,
                zone.clone(),
            )
            .map_err(|error| ReceiveError::Transport(error.to_string()))?;

        let mut report = ReceiveReport {
            job_id,
            bootstrapped: false,
            bootstrap_types_completed: 0,
            pages_received: 0,
            events_saved: 0,
            photos_projected: 0,
            unknown_records: 0,
            outstanding_resources: 0,
            applied: false,
            target_token: None,
        };

        if progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES) {
            report.bootstrapped = true;
            let snapshot = self
                .bootstrap(&client, &pinned, &zone, &progress, &mut report)
                .await?;
            // Bootstrap pages are written into the same durable inbox as delta
            // events, so a bootstrap interrupted after one type finished resumes
            // without dropping what the earlier cycle already read. Promotion
            // therefore always reads the inbox — one code path, no in-memory
            // inventory that a restart would lose.
            self.promote_from_inbox(job_id, &progress.zone, &snapshot.token, &mut report)
                .await?;
        }

        // The delta round starts from wherever bootstrap left the ingest
        // position, which is why an interrupted bootstrap never loses or
        // double-counts a record.
        self.delta_round(&client, &pinned, &zone, job_id, &mut report)
            .await?;
        Ok(report)
    }

    /// Build the transport for this job: the endpoint is re-validated, the
    /// Bearer key comes from the platform secret store, and the pinned scope
    /// gates whether a receive call may be attempted at all.
    async fn receive_client(
        &self,
        job: &super::super::store::SyncJob,
        pinned: &NativePinnedScope,
    ) -> std::result::Result<NativeClient, ReceiveError> {
        if !super::super::native::receive::receive_is_permitted(&pinned.capabilities) {
            return Err(ReceiveError::CapabilityMissing);
        }
        self.transport_client_for(job)
            .await
            .map_err(|error| ReceiveError::Transport(error.to_string()))
    }

    /// Bootstrap every supported record type at one shared height.
    ///
    /// Returns the number of types that reached completion, the zone token the
    /// snapshot established, and every record the pages returned. Each page is
    /// saved with its marker before the next is read, so an interrupted
    /// bootstrap resumes from that type's continuation marker instead of
    /// re-reading the whole inventory.
    async fn bootstrap(
        &self,
        client: &NativeClient,
        pinned: &NativePinnedScope,
        zone: &NativeZone,
        progress: &NativeProgress,
        report: &mut ReceiveReport,
    ) -> std::result::Result<SnapshotCompletion, ReceiveError> {
        let page_limit = client.page_limit(pinned);
        let mut token = progress.ingest_token.clone();
        let snapshot_token = match &progress.snapshot_token {
            Some(value) => value.clone(),
            None => {
                let snapshot = client.snapshot(zone).await.map_err(map_transport)?;
                self.store
                    .pin_native_snapshot(
                        progress.job_id,
                        &snapshot.snapshot_token,
                        &snapshot.sync_token,
                    )
                    .map_err(|e| ReceiveError::Transport(e.to_string()))?;
                token = Some(snapshot.sync_token);
                snapshot.snapshot_token
            }
        };

        for record_type in SUPPORTED_RECORD_TYPES {
            let marker = progress.bootstrap.marker(record_type);
            if marker.status == TypeStatus::Complete {
                // Its pages were read by an earlier cycle. They are not re-read
                // here: the projection this round promotes is built from what
                // *this* cycle read, and an earlier cycle's pages are already
                // in the projection it wrote. Promoting only what was read
                // would drop them, so a partially-completed bootstrap resumes
                // the unfinished types and leaves the completed ones alone.
                continue;
            }
            let mut continuation = marker.continuation_marker.clone();
            let mut count = marker.record_count;
            loop {
                let page = client
                    .query_records(
                        &pinned.namespace,
                        zone,
                        record_type,
                        page_limit,
                        Some(&snapshot_token),
                        continuation.as_deref(),
                    )
                    .await
                    .map_err(map_transport)?;
                count += page.records.len();
                continuation = page.continuation_marker.clone();
                let finished = continuation.is_none();
                // The records go into the durable inbox before the marker
                // advances. A crash between the two replays this page; the
                // reverse order would advance the marker over records that were
                // never stored.
                let saved = self
                    .store
                    .save_delta_page(
                        progress.job_id,
                        inbox_events(&page.records),
                        &page.sync_token,
                    )
                    .map_err(|error| ReceiveError::Transport(error.to_string()))?;
                report.events_saved += saved;
                self.store
                    .save_bootstrap_page(
                        progress.job_id,
                        record_type,
                        continuation.clone(),
                        count,
                        page.sync_token.as_str(),
                    )
                    .map_err(|error| ReceiveError::Transport(error.to_string()))?;
                report.pages_received += 1;
                if finished {
                    report.bootstrap_types_completed += 1;
                }
                if finished {
                    break;
                }
            }
        }

        let token = token
            .ok_or_else(|| ReceiveError::Transport("bootstrap produced no zone position".into()))?;
        Ok(SnapshotCompletion { token })
    }

    /// One fixed-height delta round: page until `moreComing` is false, saving
    /// each page with its token before reading the next.
    async fn delta_round(
        &self,
        client: &NativeClient,
        pinned: &NativePinnedScope,
        zone: &NativeZone,
        job_id: i64,
        report: &mut ReceiveReport,
    ) -> std::result::Result<(), ReceiveError> {
        let page_limit = client.page_limit(pinned);
        let mut position = self
            .store
            .native_progress(job_id)
            .map_err(|error| ReceiveError::Transport(error.to_string()))?
            .and_then(|progress| progress.ingest_token);

        let mut last_token = String::new();
        let mut more_coming = true;
        while more_coming {
            let entry: ZoneChangesEntry = client
                .changes_zone(&pinned.namespace, zone, position.as_deref(), page_limit)
                .await
                .map_err(map_transport)?;
            let token = entry
                .sync_token
                .clone()
                .ok_or_else(|| ReceiveError::Transport("delta page carried no token".into()))?;
            let events = inbox_events(&entry.records);
            let saved = self
                .store
                .save_delta_page(job_id, events, &token)
                .map_err(|error| ReceiveError::Transport(error.to_string()))?;
            report.pages_received += 1;
            report.events_saved += saved;
            position = Some(token.clone());
            last_token = token;
            more_coming = entry.more_coming;
        }

        // The round is complete at a fixed height; now and only now is the
        // ordered inbox replayed into a projection. Promoting mid-round would
        // expose half an atomic pair.
        let zone_now = self
            .store
            .native_progress(job_id)
            .map_err(|error| ReceiveError::Transport(error.to_string()))?
            .map(|progress| progress.zone)
            .ok_or(ReceiveError::NoZone(job_id))?;
        self.promote_from_inbox(job_id, &zone_now, &last_token, report)
            .await
    }

    /// Replay the durable inbox at the current generation into a validated
    /// projection and promote it at `target`.
    async fn promote_from_inbox(
        &self,
        job_id: i64,
        zone: &NativeZone,
        target_token: &str,
        report: &mut ReceiveReport,
    ) -> std::result::Result<(), ReceiveError> {
        let progress = self
            .store()
            .native_progress(job_id)
            .map_err(|error| ReceiveError::Transport(error.to_string()))?
            .ok_or(ReceiveError::NoScope(job_id))?;
        let events = self
            .store()
            .inbox_events(job_id, &progress.database_generation)
            .map_err(|error| ReceiveError::Transport(error.to_string()))?;
        let set = RecordSet::from_events(&events)
            .map_err(|error| ReceiveError::IncompleteGraph(error.to_string()))?;
        let graph = set
            .to_graph(&super::super::native::receive::ZoneId::from(zone))
            .map_err(|error| ReceiveError::IncompleteGraph(error.to_string()))?;
        self.prepare_local_matches(job_id, &graph).await?;
        let local_digests = self.measure_local_matches(job_id, &graph).await;
        self.preserve_collisions(job_id, &graph, &local_digests)
            .await?;
        self.promote(graph, target_token, local_digests, report)
    }

    /// Hash only equal-size, same-path local candidates with no matching cloud
    /// record identity. Ordinary local files and same-record updates never enter
    /// this comparison. Preparation refreshed size/mtime without hashing first.
    async fn measure_local_matches(
        &self,
        job_id: i64,
        graph: &RemoteGraph,
    ) -> HashMap<String, LocalDigest> {
        let entries = match self.store().local_entries(job_id) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!("cannot list unclaimed local entries: {error}");
                return HashMap::new();
            }
        };
        // The service already states how many bytes the cloud original has. A
        // local file of a different length cannot be that file, so most
        // mismatches cost a comparison rather than a read.
        let wanted: HashMap<&str, _> = graph
            .photos
            .iter()
            .filter(|photo| !photo.is_hidden)
            .map(|photo| (photo.relative_path.as_str(), photo))
            .collect();
        let mut digests: HashMap<String, LocalDigest> = HashMap::new();
        let mut to_measure = Vec::new();
        for entry in entries {
            let Some(photo) = wanted.get(entry.relative_path.as_str()) else {
                continue;
            };
            if entry.photo_id.as_deref() == Some(&photo.photo_id)
                || entry.file_size != photo.original.size
            {
                continue;
            }
            match entry.cached {
                Some(digest) => {
                    digests.insert(
                        entry.relative_path,
                        LocalDigest {
                            entry_id: entry.entry_id,
                            local_version: entry.local_version,
                            digest,
                        },
                    );
                }
                None => to_measure.push(entry),
            }
        }
        if to_measure.is_empty() {
            return digests;
        }
        // The lease is taken on the thread that opens the files, not out here:
        // `publication_io` counts readers per thread, so a lease held across an
        // await would guard the runtime instead of the reads.
        let measured = tokio::task::spawn_blocking(move || {
            let _lease = super::super::publication_io::read().ok();
            to_measure
                .into_iter()
                .filter_map(|entry| {
                    let digest = super::super::content::digest_file(&entry.local_path).ok()?;
                    Some((
                        entry.relative_path,
                        LocalDigest {
                            entry_id: entry.entry_id,
                            local_version: entry.local_version,
                            digest,
                        },
                    ))
                })
                .collect::<HashMap<_, _>>()
        })
        .await;
        match measured {
            Ok(measured) => digests.extend(measured),
            // A measurement that did not finish is not a failure: without the
            // digests those entries simply stay unadopted and conflict, which
            // is the state they were already in.
            Err(error) => tracing::warn!("local digest measurement did not finish: {error}"),
        }
        digests
    }

    fn promote(
        &self,
        graph: RemoteGraph,
        target_token: &str,
        local_digests: HashMap<String, LocalDigest>,
        report: &mut ReceiveReport,
    ) -> std::result::Result<(), ReceiveError> {
        let (photos, unknown, resources, applied) = self
            .store
            .promote_remote_graph(report.job_id, graph, target_token, local_digests)
            .map_err(|error| ReceiveError::Transport(error.to_string()))?;
        report.photos_projected = photos;
        report.unknown_records = unknown;
        report.outstanding_resources = resources;
        report.applied = applied;
        report.target_token = Some(target_token.to_string());
        Ok(())
    }
}

/// What a finished bootstrap established: the zone position the delta stream
/// resumes from. Type completion is reported through the cycle's own report.
struct SnapshotCompletion {
    token: String,
}

/// Ordered events, deduplicated by the inbox's own uniqueness on replay.
fn inbox_events(records: &[NativeRecord]) -> Vec<InboxEvent> {
    records
        .iter()
        .filter_map(|record| {
            let record_type = if record.is_tombstone() {
                record.record_type.clone().unwrap_or_default()
            } else {
                record.record_type.clone()?
            };
            let change_tag = record.record_change_tag.clone()?;
            Some(InboxEvent {
                ordinal: 0,
                record_name: record.record_name.clone(),
                record_type,
                record_change_tag: change_tag,
                kind: if record.is_tombstone() {
                    InboxKind::Deleted
                } else {
                    InboxKind::Updated
                },
                payload: serde_json::to_value(record).unwrap_or(serde_json::Value::Null),
            })
        })
        .collect()
}

/// A stable digest of everything a receive position is only valid for.
///
/// The scope is part of the digest rather than a separate comparison so that
/// "same server, same zone, different download scope" cannot inherit the old
/// completeness claim by accident.
fn scope_digest(pinned: &NativePinnedScope, job: &super::super::store::SyncJob) -> String {
    format!(
        "{}|{}|{}|{}|{}|{:?}|{:?}",
        pinned.namespace.tenant,
        pinned.namespace.container,
        pinned.namespace.environment,
        pinned.database_generation,
        job.local_root.display(),
        job.upload_scope,
        job.native_zone.as_ref().map(|zone| &zone.zone_name),
    )
}

fn map_transport(error: crate::core::sync::SyncError) -> ReceiveError {
    match error {
        crate::core::sync::SyncError::Unsupported(message)
            if message.contains(super::super::native::receive::EXPIRED_SYNC_TOKEN) =>
        {
            ReceiveError::TokenExpired
        }
        other => ReceiveError::Transport(other.to_string()),
    }
}
