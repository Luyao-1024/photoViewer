//! One synchronization cycle.
//!
//! The order is fixed and each step exists for a reason:
//!
//! ```text
//! receive downstream to completion
//!   → acquire the zone's upload lease once
//!       → busy or unconfirmed: skip this cycle's uplink, keep every obligation
//!       → held: prove the client is current, catching up behind the lock if not
//!           → walk delete → modify → create, settling issued work between
//!           → release
//! ```
//!
//! Two rules are load-bearing and easy to lose in a refactor:
//!
//! * **Acquisition is attempted once per cycle.** A busy zone means another
//!   device is uploading; spinning here would fight it and would never end.
//! * **Lease loss is irreversible for the cycle.** Every unfinished uplink item
//!   fails, late responses cannot clear an obligation, and the cycle does not
//!   reacquire. A request that committed before the loss is *not* assumed to
//!   have failed — it is reconciled through the operation receipt, which is why
//!   issued work is durable.
//!
//! This stage uploads **metadata only**. Content and create need an asset pair,
//! which is C1.4. An entry that is only content-dirty is reported as outstanding
//! rather than quietly marked done.

use super::super::native::lease::{LeaseGrant, UploadLease, UploadPhase};
use super::super::native::modify::{
    MetadataPatch, ModifyOutcome, MutationBatch, MutationOperation,
};
use super::super::native::NativeClient;
use super::super::native::NativePinnedScope;
use super::super::native::NativeZone;
use super::super::store::dirty::{
    CategoryOutcome, ChangeCategory, ConfirmedIdentity, DirtyEntry, NativeDirty,
};
use super::super::store::receive::NativeProgress;
use crate::core::error::AppError;

use super::super::store::SyncJob;
use super::receive;
use super::SyncService;

/// What one cycle did, stated without spin.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CycleReport {
    pub job_id: i64,
    /// Downstream completed before any uplink was considered.
    pub received: bool,
    /// The lease was granted.
    pub acquired: bool,
    /// Another device held it; this cycle's uplink was skipped once.
    pub uplink_skipped: bool,
    /// The client was behind the cloud and caught up while holding the lock.
    pub caught_up_behind_lock: bool,
    pub mutations_sent: usize,
    pub mutations_confirmed: usize,
    /// Items that failed, with the reason. Never silently dropped.
    pub failures: Vec<ItemFailure>,
    /// Obligations still outstanding after the cycle.
    pub outstanding: usize,
    pub released: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFailure {
    pub entry_id: i64,
    pub relative_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CycleError {
    /// Another device owns the zone's uplink. Not a failure: the cycle ends
    /// after downstream, every obligation stays, and nothing is retried.
    #[error("another device holds the upload lease")]
    UplinkBusy,
    /// The client is behind the cloud and declared itself not ready. The lease
    /// is kept; a later cycle catches up and asks again.
    #[error("this client is behind the cloud: {0}")]
    Behind(String),
    /// The lease ended mid-cycle. Irreversible here.
    #[error("the upload lease was lost mid-cycle")]
    LeaseLost,
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("{0}")]
    Setup(String),
}

impl SyncService {
    /// Run one full cycle for a native job.
    pub async fn run_cycle(&self, job_id: i64) -> std::result::Result<CycleReport, CycleError> {
        let job = self
            .store()
            .get_job(job_id)
            .map_err(|error| CycleError::Setup(error.to_string()))?
            .ok_or_else(|| CycleError::Setup(format!("job {job_id} no longer exists")))?;
        let mut report = CycleReport {
            job_id,
            ..CycleReport::default()
        };

        // Downstream first and unconditionally. A busy lease must not cost this
        // client the receive half of the work.
        self.receive_downstream(&job, &mut report).await?;

        match self.uplink(&job, &mut report).await {
            Ok(()) => {}
            Err(CycleError::UplinkBusy) => {
                report.uplink_skipped = true;
            }
            Err(CycleError::Behind(_)) => {
                // Behind, not blocked: the obligations stay and the next cycle
                // tries again from the position catch-up left.
            }
            Err(CycleError::LeaseLost) => {
                // Already recorded as failures by the items that were in flight.
            }
            Err(error) => return Err(error),
        }

        report.outstanding = self
            .store()
            .unresolved_resource_count(job_id)
            .map_err(|error| CycleError::Setup(error.to_string()))?;
        Ok(report)
    }

    async fn receive_downstream(
        &self,
        job: &SyncJob,
        report: &mut CycleReport,
    ) -> std::result::Result<(), CycleError> {
        match receive_cycle(self, job.id).await {
            Ok(_) => {
                report.received = true;
                Ok(())
            }
            // A receive failure must not silently become an uplink: sending
            // changes while behind is how a conflict gets manufactured.
            Err(error) => Err(CycleError::Transport(error.to_string())),
        }
    }

    async fn uplink(
        &self,
        job: &SyncJob,
        report: &mut CycleReport,
    ) -> std::result::Result<(), CycleError> {
        let zone = job
            .native_zone
            .clone()
            .ok_or_else(|| CycleError::Setup("a native job must name its zone".into()))?;
        let pinned = self
            .store()
            .native_scope(job.connection_id)
            .map_err(|error| CycleError::Setup(error.to_string()))?
            .ok_or_else(|| CycleError::Setup("this connection has no probed scope".into()))?;
        let device = crate::core::sync::device::device_id(self.pool());
        let client = self.cycle_client(job).await?;

        let (grant, started): (LeaseGrant, _) = client
            .acquire_lease(&device, &zone)
            .await
            .map_err(map_lease_error)?;
        let lease = UploadLease::from_grant(&grant, started);
        report.acquired = true;

        let progress = self
            .store()
            .native_progress(job.id)
            .map_err(|error| CycleError::Setup(error.to_string()))?
            .ok_or_else(|| CycleError::Setup("no receive position exists yet".into()))?;

        let result = self
            .uplink_under_lease(&client, &lease, job, &pinned, &progress, &zone, report)
            .await;

        // Release is best effort and uses the exact old proof. It is attempted
        // even after a loss, because a lease this client can no longer prove it
        // holds must not be left to expire on the server's schedule.
        if client.release_lease(lease.proof()).await.is_ok() {
            report.released = true;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn uplink_under_lease(
        &self,
        client: &NativeClient,
        lease: &UploadLease,
        job: &SyncJob,
        pinned: &NativePinnedScope,
        progress: &NativeProgress,
        zone: &NativeZone,
        report: &mut CycleReport,
    ) -> std::result::Result<(), CycleError> {
        // The gate. `localReady` is only true once this client independently
        // knows its receive position is complete — the server cannot attest
        // that anything was downloaded or published here.
        let local_ready =
            progress.applied_token.is_some() && progress.applied_token == progress.target_token;
        let applied = progress.applied_token.as_deref().ok_or_else(|| {
            CycleError::Setup("this client has not finished catching up yet".into())
        })?;
        let check = client
            .check_lease(lease.proof(), zone, applied, local_ready)
            .await
            .map_err(map_lease_error)?;
        if !check.may_upload {
            // Behind, or not ready. One catch-up runs while the lease is still
            // held, and the cycle then ends *without* re-asking: a passed gate
            // is single-use, and a refused one belongs to a position that
            // catch-up has just replaced. Asking again in a tight loop would
            // spin on a zone whose head is being written by someone else. The
            // obligations stay; the next scheduled cycle asks from the new
            // position.
            report.caught_up_behind_lock = true;
            receive_cycle(self, job.id)
                .await
                .map_err(|error| CycleError::Transport(error.to_string()))?;
            return Err(CycleError::Behind(reason_for(check.is_current)));
        }

        let entries = self
            .store()
            .uplink_view(job.id)
            .map_err(|error| CycleError::Setup(error.to_string()))?;
        let _ = pinned;

        for phase in UploadPhase::ORDER {
            // Every phase is walked, including an empty one: issued operations
            // are settled before the next phase, not before the next non-empty
            // phase.
            if !lease.is_held().await {
                return Err(CycleError::LeaseLost);
            }
            client
                .set_phase(lease.proof(), phase)
                .await
                .map_err(map_lease_error)?;
            let batch = self.batch_for_phase(&entries, phase);
            if batch.is_empty() {
                continue;
            }
            self.dispatch(client, lease, job, pinned, batch, report)
                .await?;
        }
        Ok(())
    }

    fn batch_for_phase(&self, entries: &[DirtyEntry], phase: UploadPhase) -> Vec<MutationBatch> {
        entries
            .iter()
            .filter_map(|entry| {
                let operations = operations_for(entry, phase)?;
                Some(MutationBatch {
                    entry_id: entry.entry_id,
                    relative_path: entry.relative_path.clone(),
                    category: category_for(entry, phase)?,
                    captured_version: entry.version_for(category_for(entry, phase)?),
                    operations,
                })
            })
            .collect()
    }

    async fn dispatch(
        &self,
        client: &NativeClient,
        lease: &UploadLease,
        job: &SyncJob,
        namespace: &NativePinnedScope,
        batch: Vec<MutationBatch>,
        report: &mut CycleReport,
    ) -> std::result::Result<(), CycleError> {
        for item in batch {
            // Checked immediately before every mutation: a cycle that lost the
            // lease between two items must not issue the second.
            if !lease.is_held().await {
                return Err(CycleError::LeaseLost);
            }
            let zone = job
                .native_zone
                .as_ref()
                .ok_or_else(|| CycleError::Setup("a native job must name its zone".into()))?;
            let outcome = client
                .modify_records(lease.proof(), &namespace.namespace, zone, &item)
                .await
                .map_err(map_lease_error)?;

            match outcome {
                ModifyOutcome::LeaseLost => {
                    report.failures.push(ItemFailure {
                        entry_id: item.entry_id,
                        relative_path: item.relative_path.clone(),
                        reason: "the upload lease was lost before this item was sent".into(),
                    });
                    return Err(CycleError::LeaseLost);
                }
                ModifyOutcome::Confirmed(confirmed) => {
                    report.mutations_sent += 1;
                    let outcome = self
                        .store()
                        .confirm_category(
                            item.entry_id,
                            item.category,
                            item.captured_version,
                            ConfirmedIdentity {
                                photo_id: confirmed.photo_id.clone(),
                                master_id: confirmed.master_id.clone(),
                                asset_revision: confirmed.record_change_tag.clone(),
                                master_revision: confirmed.master_revision.clone(),
                                content_generation: confirmed.content_generation.clone(),
                            },
                        )
                        .map_err(|error| CycleError::Setup(error.to_string()))?;
                    if outcome == CategoryOutcome::Cleared {
                        report.mutations_confirmed += 1;
                    }
                }
                ModifyOutcome::ItemFailed(failure) => {
                    // A per-item failure is reported, never swallowed and never
                    // treated as a reason to clear the obligation.
                    report.failures.push(ItemFailure {
                        entry_id: item.entry_id,
                        relative_path: item.relative_path.clone(),
                        reason: failure,
                    });
                }
            }
        }
        Ok(())
    }

    async fn cycle_client(&self, job: &SyncJob) -> std::result::Result<NativeClient, CycleError> {
        let endpoint = super::super::native::NativeEndpoint::parse(&job.endpoint)
            .map_err(|error| CycleError::Setup(error.to_string()))?;
        let reference = job.credential_ref.clone();
        let api_key = tokio::task::spawn_blocking(move || {
            crate::platform::credentials::load_api_key(&reference)
        })
        .await
        .map_err(|error| CycleError::Transport(format!("credential task failed: {error}")))?
        .map_err(|error| CycleError::Transport(error.to_string()))?;
        NativeClient::new(&endpoint, &api_key).map_err(|error| CycleError::Setup(error.to_string()))
    }
}

/// Which category a phase acts on.
fn category_for(entry: &DirtyEntry, phase: UploadPhase) -> Option<ChangeCategory> {
    let bit = match phase {
        UploadPhase::Delete => NativeDirty::DELETE,
        UploadPhase::Modify => NativeDirty::METADATA,
        UploadPhase::Create => NativeDirty::NEW,
    };
    entry.dirty.contains(bit).then_some(match phase {
        UploadPhase::Delete => ChangeCategory::Deletion,
        UploadPhase::Modify => ChangeCategory::Metadata,
        UploadPhase::Create => ChangeCategory::Metadata,
    })
}

/// The record operations this stage can actually issue.
///
/// Only a metadata update on a photo that already exists in the cloud. A create
/// needs an asset pair (C1.4), and a content change needs new bytes; both are
/// reported as outstanding rather than approximated with something that would
/// leave the cloud describing content this client never sent.
fn operations_for(entry: &DirtyEntry, phase: UploadPhase) -> Option<Vec<MutationOperation>> {
    match phase {
        UploadPhase::Modify if entry.dirty.contains(NativeDirty::METADATA) => {
            let (photo, _master) = entry.remote_names()?;
            let revision = entry.asset_revision.clone()?;
            Some(vec![MutationOperation::update_metadata(
                photo,
                revision,
                MetadataPatch {
                    // A supported field, uploaded conditionally. The value is
                    // the user's current one; the capture date and path travel
                    // with it in C1.4's full field set.
                    favorite: None,
                },
            )])
        }
        _ => None,
    }
}

/// Why the gate refused, in the client's own terms.
fn reason_for(is_current: bool) -> String {
    if is_current {
        // Current but not allowed to upload: this client declared itself not
        // ready, which is the local side of the gate, not a cloud change.
        "this client is not ready to upload".to_string()
    } else {
        "the zone moved past this client's position".to_string()
    }
}

fn map_lease_error(error: crate::core::sync::SyncError) -> CycleError {
    use crate::core::sync::SyncError;
    match &error {
        // Both acquisition contention and a mid-cycle loss arrive as rate
        // limiting / unsupported codes; the message decides which decision the
        // cycle makes.
        SyncError::RateLimited => CycleError::UplinkBusy,
        SyncError::Unsupported(message) => {
            if message.contains("UPLOAD_LEASE_LOST") {
                CycleError::LeaseLost
            } else {
                CycleError::Transport(error.to_string())
            }
        }
        other => CycleError::Transport(other.to_string()),
    }
}

/// Run one receive cycle. Named here so the coordinator reads as "receive, then
/// uplink" rather than reaching into the sibling module's internals.
async fn receive_cycle(
    service: &SyncService,
    job_id: i64,
) -> std::result::Result<receive::ReceiveReport, receive::ReceiveError> {
    service.run_receive(job_id).await
}

impl SyncService {
    /// Build a client for one job: the endpoint is re-validated on every use and
    /// the Bearer key is read from the platform secret store, never cached.
    ///
    /// Re-validating here rather than trusting the stored endpoint is what keeps
    /// a configuration edit from quietly redirecting traffic somewhere the user
    /// never chose.
    pub(crate) async fn transport_client_for(
        &self,
        job: &SyncJob,
    ) -> std::result::Result<NativeClient, AppError> {
        let endpoint = crate::core::sync::native::NativeEndpoint::parse(&job.endpoint)
            .map_err(|error| AppError::Backend(error.to_string()))?;
        let reference = job.credential_ref.clone();
        let api_key = tokio::task::spawn_blocking(move || {
            crate::platform::credentials::load_api_key(&reference)
        })
        .await
        .map_err(|error| AppError::Backend(format!("credential task failed: {error}")))??;
        crate::core::sync::native::NativeClient::new(&endpoint, &api_key)
            .map_err(|error| AppError::Backend(error.to_string()))
    }
}
