//! Native cycle: receive/publication → one lease → gated delete/modify/create.
use super::{receive, SyncService};
use crate::core::{
    error::AppError,
    sync::{
        native::{
            assets::{AssetRole, ReservationRequest, UploadIntent},
            modify::OperationState,
            NativeClient, NativePinnedScope, NativeZone, UploadLease, UploadPhase,
        },
        store::{
            dirty::{DirtyEntry, NativeDirty},
            runtime::{Command, Epoch, Issued},
            SyncJob,
        },
        SyncError,
    },
};
use serde_json::{json, Value};
use std::{future::Future, path::PathBuf, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CycleReport {
    pub job_id: i64,
    pub received: bool,
    pub acquired: bool,
    pub uplink_skipped: bool,
    pub caught_up_behind_lock: bool,
    pub mutations_sent: usize,
    pub mutations_confirmed: usize,
    pub failures: Vec<ItemFailure>,
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
    #[error("another device holds the upload lease")]
    UplinkBusy,
    #[error("this client has unfinished downstream work: {0}")]
    Behind(String),
    #[error("UPLOAD_LEASE_LOST")]
    LeaseLost,
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("{0}")]
    Setup(String),
}
fn setup(e: impl std::fmt::Display) -> CycleError {
    CycleError::Setup(e.to_string())
}
fn transport(e: SyncError) -> CycleError {
    if e.to_string().contains("UPLOAD_LEASE_LOST") {
        CycleError::LeaseLost
    } else {
        CycleError::Transport(e.to_string())
    }
}

/// A drop guard owns heartbeat lifetime even if the caller cancels the cycle.
struct Heartbeat {
    task: tokio::task::JoinHandle<()>,
    lost: tokio::sync::watch::Receiver<bool>,
    watchdog: tokio::task::JoinHandle<()>,
}
impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.task.abort();
        self.watchdog.abort();
    }
}
impl Heartbeat {
    fn start(
        client: NativeClient,
        lease: UploadLease,
        store: crate::core::sync::store::SyncStore,
        epoch: Epoch,
    ) -> Self {
        let (sender, lost) = tokio::sync::watch::channel(false);
        let monitor_lease = lease.clone();
        let monitor_sender = sender.clone();
        let watchdog = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let check_store = store.clone();
                let check_epoch = epoch.clone();
                let current =
                    tokio::task::spawn_blocking(move || check_store.epoch_current(&check_epoch))
                        .await;
                if !monitor_lease.valid_now() || !matches!(current, Ok(Ok(true))) {
                    monitor_lease.mark_lost().await;
                    let _ = monitor_sender.send(true);
                    break;
                }
            }
        });
        let task = tokio::spawn(async move {
            let mut delay = lease.heartbeat_interval();
            loop {
                let Some(deadline) = lease.deadline().await else {
                    let _ = sender.send(true);
                    break;
                };
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    lease.mark_lost().await;
                    let _ = sender.send(true);
                    break;
                }
                tokio::time::sleep(delay.min(remaining)).await;
                if !lease.valid_now() {
                    lease.mark_lost().await;
                    let _ = sender.send(true);
                    break;
                }
                let remaining = lease
                    .deadline()
                    .await
                    .map(|d| d.saturating_duration_since(std::time::Instant::now()))
                    .unwrap_or_default();
                match tokio::time::timeout(remaining, client.renew_lease(lease.proof())).await {
                    Ok(Ok((ttl, started))) => {
                        lease.renew(ttl, started).await;
                        delay = lease.heartbeat_interval();
                    }
                    Ok(Err(SyncError::Offline(_) | SyncError::RateLimited))
                        if lease.valid_now() =>
                    {
                        delay = Duration::from_millis(500);
                    }
                    _ => {
                        lease.mark_lost().await;
                        let _ = sender.send(true);
                        break;
                    }
                }
            }
        });
        Self {
            task,
            lost,
            watchdog,
        }
    }
    async fn guard<T>(
        &self,
        lease: &UploadLease,
        future: impl Future<Output = Result<T, CycleError>>,
    ) -> Result<T, CycleError> {
        if !lease.valid_now() || *self.lost.borrow() {
            return Err(CycleError::LeaseLost);
        }
        let mut lost = self.lost.clone();
        tokio::select! {biased;_ = lost.changed()=>Err(CycleError::LeaseLost),result=future=>{let result=result?;if !lease.valid_now(){Err(CycleError::LeaseLost)}else{Ok(result)}}}
    }
}
impl SyncService {
    pub async fn run_cycle(&self, job_id: i64) -> Result<CycleReport, CycleError> {
        let _slot = self.acquire_run_slot(job_id).await.map_err(setup)?;
        let job = self
            .store()
            .get_job(job_id)
            .map_err(setup)?
            .ok_or_else(|| setup("job was removed"))?;
        let result = self.run_owned_cycle(job_id).await;
        let reason = match &result {
            Err(error) => Some(error.to_string()),
            Ok(report) if !report.failures.is_empty() => Some(
                report
                    .failures
                    .iter()
                    .map(|f| format!("{}: {}", f.relative_path, f.reason))
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Ok(report) if report.uplink_skipped => {
                Some("another device holds the upload lease; retry scheduled".into())
            }
            Ok(report) if report.outstanding > 0 => Some(format!(
                "{} synchronization obligations remain",
                report.outstanding
            )),
            _ => None,
        };
        self.store()
            .finish_run(job_id, job.config_generation, reason.as_deref())
            .map_err(setup)?;
        result
    }
    async fn run_owned_cycle(&self, job_id: i64) -> Result<CycleReport, CycleError> {
        let job = self
            .store()
            .get_job(job_id)
            .map_err(setup)?
            .ok_or_else(|| setup("job was removed"))?;
        self.store().mark_job_started(job_id).map_err(setup)?;
        let epoch = Epoch {
            job: job.id,
            config: job.config_generation,
            cycle: uuid::Uuid::new_v4().to_string(),
        };
        self.store()
            .runtime(Command::Begin(epoch.clone()))
            .map_err(setup)?;
        let mut report = CycleReport {
            job_id,
            ..Default::default()
        };
        let result = self.cycle_inner(&job, &epoch, &mut report).await;
        self.store().runtime(Command::End(epoch)).map_err(setup)?;
        match result {
            Err(CycleError::UplinkBusy) => report.uplink_skipped = true,
            Err(CycleError::LeaseLost) => {
                for entry in self.store().uplink_view(job_id).map_err(setup)? {
                    if !report.failures.iter().any(|f| f.entry_id == entry.entry_id) {
                        report.failures.push(ItemFailure {
                            entry_id: entry.entry_id,
                            relative_path: entry.relative_path,
                            reason: "UPLOAD_LEASE_LOST".into(),
                        });
                    }
                }
            }
            Err(CycleError::Behind(reason)) => {
                report.failures.push(ItemFailure {
                    entry_id: 0,
                    relative_path: String::new(),
                    reason,
                });
            }
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        report.outstanding = self
            .store()
            .unresolved_resource_count(job_id)
            .map_err(setup)?
            + self
                .store()
                .uplink_view(job_id)
                .map_err(setup)?
                .iter()
                .filter(|entry| eligible(self, &job, entry).unwrap_or(true))
                .count();
        Ok(report)
    }
    async fn receive_complete(&self, job: i64) -> Result<receive::ReceiveReport, CycleError> {
        let service = self.clone();
        tokio::task::spawn_blocking(move || service.recover_publications(job))
            .await
            .map_err(|_| setup("publication recovery worker stopped"))?
            .map_err(setup)?;
        let report = self
            .run_receive(job)
            .await
            .map_err(|e| CycleError::Transport(e.to_string()))?;
        let transfers = self.run_transfers(job).await.map_err(setup)?;
        if !transfers.failures.is_empty() {
            return Err(CycleError::Behind(format!(
                "resource/publication work is still unresolved: {}",
                transfers
                    .failures
                    .iter()
                    .map(|(_, reason)| reason.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        Ok(report)
    }
    async fn cycle_inner(
        &self,
        job: &SyncJob,
        epoch: &Epoch,
        report: &mut CycleReport,
    ) -> Result<(), CycleError> {
        self.index_upload_sources(job).await.map_err(setup)?;
        self.store()
            .runtime(Command::Enroll {
                epoch: epoch.clone(),
            })
            .map_err(setup)?;
        // Even when publication is blocked, read-only receipt reconciliation
        // after acquisition can settle an earlier committed local operation.
        match self.receive_complete(job.id).await {
            Ok(_) => report.received = true,
            Err(CycleError::Behind(_)) => report.received = true,
            Err(e) => return Err(e),
        }
        if !self.store().epoch_current(epoch).map_err(setup)? {
            return Err(setup("synchronization configuration was superseded"));
        }
        let client = self.transport_client_for(job).await.map_err(setup)?;
        let pinned = self
            .store()
            .native_scope(job.connection_id)
            .map_err(setup)?
            .ok_or_else(|| setup("connection has no pinned scope"))?;
        let zone = job
            .native_zone
            .as_ref()
            .ok_or_else(|| setup("job has no zone"))?;
        let device = crate::core::sync::device::device_id(self.pool());
        let (grant, started) = match client.acquire_lease(&device, zone).await {
            Ok(grant) => grant,
            Err(SyncError::RateLimited) => return Err(CycleError::UplinkBusy),
            Err(e) => {
                report.uplink_skipped = true;
                report.failures.push(ItemFailure {
                    entry_id: 0,
                    relative_path: String::new(),
                    reason: e.to_string(),
                });
                return Ok(());
            }
        };
        let lease = UploadLease::from_grant(&grant, started);
        report.acquired = true;
        let heartbeat = Heartbeat::start(
            client.clone(),
            lease.clone(),
            self.store().clone(),
            epoch.clone(),
        );
        let result = heartbeat
            .guard(
                &lease,
                self.uplink_held(
                    &client, &lease, &heartbeat, job, epoch, &pinned, zone, report,
                ),
            )
            .await;
        // Abort heartbeat first; an in-flight release can never grant a renewal.
        drop(heartbeat);
        report.released = client.release_lease(lease.proof()).await.unwrap_or(false);
        lease.mark_lost().await;
        result
    }
    #[allow(clippy::too_many_arguments)]
    async fn uplink_held(
        &self,
        client: &NativeClient,
        lease: &UploadLease,
        heartbeat: &Heartbeat,
        job: &SyncJob,
        epoch: &Epoch,
        pinned: &NativePinnedScope,
        zone: &NativeZone,
        report: &mut CycleReport,
    ) -> Result<(), CycleError> {
        for operation in self.store().pending_issued(job.id).map_err(setup)? {
            if operation.generation != pinned.database_generation {
                return Err(CycleError::Behind("an issued operation belongs to an obsolete scope/configuration; reconciliation is required".into()));
            }
            if operation.body.is_null() {
                if operation.config != job.config_generation {
                    self.store()
                        .runtime(Command::RetireIssued {
                            epoch: epoch.clone(),
                            operation,
                            rejected: false,
                        })
                        .map_err(setup)?;
                }
                continue;
            }
            if let OperationState::Committed(response) = client
                .operation_status(&operation.id)
                .await
                .map_err(transport)?
            {
                self.store()
                    .runtime(Command::Confirm {
                        epoch: epoch.clone(),
                        operation,
                        response,
                        lease: lease.clone(),
                    })
                    .map_err(setup)?;
                report.mutations_confirmed += 1;
            } else if operation.config != job.config_generation {
                return Err(CycleError::Behind("an older configuration has an unconfirmed issued operation; its outcome must be reconciled before new writes".into()));
            }
        }
        let mut ready = false;
        for _ in 0..3 {
            self.receive_complete(job.id).await?;
            let progress = self
                .store()
                .native_progress(job.id)
                .map_err(setup)?
                .ok_or_else(|| setup("job has no receive progress"))?;
            let Some(applied) = progress.applied_token.as_deref() else {
                return Err(CycleError::Behind(
                    "local resources/conflicts have not been applied".into(),
                ));
            };
            let local_ready = self
                .store()
                .unresolved_resource_count(job.id)
                .map_err(setup)?
                == 0
                && progress.target_token == progress.applied_token;
            let check = client
                .check_lease(lease.proof(), zone, applied, local_ready)
                .await
                .map_err(transport)?;
            if check.may_upload {
                ready = true;
                break;
            }
            report.caught_up_behind_lock = true;
        }
        if !ready {
            return Err(CycleError::Behind(
                "cloud head did not converge within this bounded catch-up cycle".into(),
            ));
        }
        // Freeze the DB-derived view after the gate. Later user edits remain dirty.
        self.store()
            .runtime(Command::Enroll {
                epoch: epoch.clone(),
            })
            .map_err(setup)?;
        let entries = self.store().uplink_view(job.id).map_err(setup)?;
        for phase in UploadPhase::ORDER {
            if phase != UploadPhase::Delete {
                client
                    .set_phase(lease.proof(), phase)
                    .await
                    .map_err(transport)?;
            }
            let pending = self.store().pending_issued(job.id).map_err(setup)?;
            for operation in pending.iter().filter(|op| op.phase == phase.as_str()) {
                self.execute_issued(
                    client,
                    lease,
                    heartbeat,
                    job,
                    epoch,
                    pinned,
                    zone,
                    operation.clone(),
                    report,
                )
                .await?;
            }
            for entry in &entries {
                if !eligible(self, job, entry)? || !matches_phase(entry, phase) {
                    continue;
                }
                if pending.iter().any(|op| op.entry == entry.entry_id) {
                    continue;
                }
                let current = self.store().uplink_view(job.id).map_err(setup)?;
                if !current.iter().any(|row| {
                    row.entry_id == entry.entry_id && row.local_version == entry.local_version
                }) {
                    continue;
                }
                let phase_result = heartbeat
                    .guard(
                        lease,
                        self.prepare_and_send(
                            client, lease, heartbeat, job, epoch, pinned, zone, entry, phase,
                            report,
                        ),
                    )
                    .await;
                match phase_result {
                    Ok(()) => {}
                    Err(CycleError::LeaseLost) => return Err(CycleError::LeaseLost),
                    Err(error) => report.failures.push(ItemFailure {
                        entry_id: entry.entry_id,
                        relative_path: entry.relative_path.clone(),
                        reason: error.to_string(),
                    }),
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    async fn prepare_and_send(
        &self,
        client: &NativeClient,
        lease: &UploadLease,
        heartbeat: &Heartbeat,
        job: &SyncJob,
        epoch: &Epoch,
        pinned: &NativePinnedScope,
        zone: &NativeZone,
        entry: &DirtyEntry,
        phase: UploadPhase,
        report: &mut CycleReport,
    ) -> Result<(), CycleError> {
        let photo = entry
            .photo_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let master = entry
            .master_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut operation = Issued {
            id: format!("pv-{}", uuid::Uuid::new_v4()),
            job: job.id,
            config: job.config_generation,
            entry: entry.entry_id,
            generation: pinned.database_generation.clone(),
            phase: phase.as_str().into(),
            versions: [
                entry.local_version,
                entry.metadata_version,
                entry.content_version,
                entry.deletion_version,
            ],
            dirty: entry.dirty.bits(),
            photo,
            master,
            content_generation: entry
                .content_generation
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            body: Value::Null,
            preparation: json!({"metadata":self.store().local_metadata(entry.entry_id).map_err(setup)?,"events":{"metadataEvent":entry.metadata_event,"contentEvent":entry.content_event,"deletionEvent":entry.deletion_event}}),
        };
        if phase == UploadPhase::Delete {
            if entry.photo_id.is_none() || entry.master_id.is_none() {
                return Err(setup("unconfirmed local creation requires cancellation/reconciliation before cloud deletion"));
            }
            operation.body = json!({"zoneID":super::super::native::receive::ZoneId::from(zone),"atomic":true,"operations":[{"operationType":"delete","record":{"recordName":operation.photo,"recordChangeTag":entry.asset_revision,"_syncserver":deletion_event(entry)}},{"operationType":"delete","record":{"recordName":operation.master,"recordChangeTag":entry.master_revision,"_syncserver":deletion_event(entry)}}]});
        } else {
            operation.preparation["assetRevision"] = json!(entry.asset_revision);
            operation.preparation["masterRevision"] = json!(entry.master_revision);
            if phase == UploadPhase::Create || entry.dirty.contains(NativeDirty::CONTENT) {
                operation.content_generation = uuid::Uuid::new_v4().to_string();
                let source =
                    super::transfer::safe_destination(&job.local_root, &entry.relative_path)
                        .map_err(setup)?;
                let folder = self.staging_root().join("uploads").join(&operation.id);
                let max = pinned.limits.max_asset_bytes;
                let thumb_max = pinned.limits.thumbnail_max_bytes;
                let prepared=tokio::task::spawn_blocking(move||{
                    let _read=super::super::publication_io::read()?;
                    let snapshot=super::super::content::snapshot_source(&source,&folder,"original",max)?;
                    let (thumbnail,digest)=super::super::content::thumbnail_from(&snapshot,2048,thumb_max)?;
                    let path=folder.join("thumbnail.jpg");std::fs::write(&path,&thumbnail)?;std::fs::File::open(&path)?.sync_all()?;
                    Ok::<_,AppError>(json!({"original":{"path":snapshot.path,"hex":snapshot.digest.hex,"base64":snapshot.digest.base64,"size":snapshot.digest.size},"thumbnail":{"path":path,"hex":digest.hex,"base64":digest.base64,"size":digest.size}}))
                }).await.map_err(|_|setup("snapshot preparation worker stopped"))?.map_err(setup)?;
                operation.preparation["resources"] = prepared;
            }
        }
        let current = self.store().uplink_view(job.id).map_err(setup)?;
        if !current
            .iter()
            .any(|row| row.entry_id == entry.entry_id && row.local_version == entry.local_version)
        {
            return Err(setup("local version changed while preparing the operation"));
        }
        self.store()
            .runtime(Command::Save(epoch.clone(), operation.clone()))
            .map_err(setup)?;
        self.execute_issued(
            client, lease, heartbeat, job, epoch, pinned, zone, operation, report,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute_issued(
        &self,
        client: &NativeClient,
        lease: &UploadLease,
        _heartbeat: &Heartbeat,
        _job: &SyncJob,
        epoch: &Epoch,
        pinned: &NativePinnedScope,
        zone: &NativeZone,
        mut operation: Issued,
        report: &mut CycleReport,
    ) -> Result<(), CycleError> {
        if operation.body.is_null() {
            if operation.preparation.get("resources").is_some() {
                let intent = if operation.phase == "create" {
                    UploadIntent::Create
                } else {
                    UploadIntent::Update
                };
                for (name, role) in [
                    ("original", AssetRole::Original),
                    ("thumbnail", AssetRole::Thumbnail),
                ] {
                    let mut descriptor = None;
                    if let Some(session) =
                        operation.preparation["resources"][name]["session"].as_str()
                    {
                        if let Ok(status) = client.upload_session(session).await {
                            if status.state.is_published() {
                                descriptor = status.asset;
                            }
                        }
                    }
                    if descriptor.is_none() {
                        let resource = &operation.preparation["resources"][name];
                        let digest = super::super::ContentDigest {
                            size: resource["size"]
                                .as_u64()
                                .ok_or_else(|| setup("issued snapshot has no size"))?,
                            hex: resource["hex"]
                                .as_str()
                                .ok_or_else(|| setup("issued snapshot has no digest"))?
                                .into(),
                            base64: resource["base64"]
                                .as_str()
                                .ok_or_else(|| setup("issued snapshot has no digest"))?
                                .into(),
                        };
                        let path = PathBuf::from(
                            resource["path"]
                                .as_str()
                                .ok_or_else(|| setup("issued snapshot has no path"))?,
                        );
                        let reservation = client
                            .reserve_assets(
                                &pinned.namespace,
                                zone,
                                lease.proof(),
                                &operation.master,
                                intent,
                                &[ReservationRequest {
                                    role,
                                    intent,
                                    digest: digest.clone(),
                                }],
                            )
                            .await
                            .map_err(transport)?
                            .into_iter()
                            .next()
                            .ok_or_else(|| setup("resource reservation was empty"))?;
                        operation.preparation["resources"][name]["session"] =
                            json!(reservation.session_id);
                        self.store()
                            .runtime(Command::Save(epoch.clone(), operation.clone()))
                            .map_err(setup)?;
                        descriptor = Some(
                            client
                                .upload_file(&reservation, lease.proof(), &path, &digest)
                                .await
                                .map_err(transport)?,
                        );
                    }
                    operation.preparation["resources"][name]["asset"] =
                        serde_json::to_value(descriptor).map_err(setup)?;
                    self.store()
                        .runtime(Command::Save(epoch.clone(), operation.clone()))
                        .map_err(setup)?;
                }
            }
            operation.body = body_for(&operation, zone)?;
            self.store()
                .runtime(Command::Save(epoch.clone(), operation.clone()))
                .map_err(setup)?;
        }
        let response = match client
            .operation_status(&operation.id)
            .await
            .map_err(transport)?
        {
            OperationState::Committed(response) => response,
            OperationState::Pending(_) => {
                return Err(CycleError::Behind(
                    "an issued atomic operation is still pending".into(),
                ))
            }
            OperationState::Unknown => {
                let mut body = operation.body.clone();
                for op in body["operations"]
                    .as_array_mut()
                    .ok_or_else(|| setup("issued operation has no records"))?
                {
                    if let Some(fields) = op
                        .get_mut("record")
                        .and_then(|record| record.get_mut("fields"))
                        .and_then(Value::as_object_mut)
                    {
                        for (field, role) in [
                            ("resOriginalRes", AssetRole::Original),
                            ("resJPEGThumbRes", AssetRole::Thumbnail),
                        ] {
                            if let Some(value) = fields.get_mut(field) {
                                let id = value["value"]["assetId"].as_str().ok_or_else(|| {
                                    setup("issued resource has no stable identity")
                                })?;
                                let fresh =
                                    client.describe_asset(id, role).await.map_err(transport)?;
                                value["value"]["receipt"] = json!(fresh.receipt);
                                value["value"]["downloadURL"] = json!(fresh.download_url);
                            }
                        }
                    }
                }
                if operation.phase == "modify" && operation.preparation.get("resources").is_none() {
                    let current = client
                        .lookup_current(&pinned.namespace, zone, &[&operation.photo])
                        .await
                        .map_err(transport)?;
                    if metadata_matches(&body, &current) {
                        self.store()
                            .runtime(Command::Confirm {
                                epoch: epoch.clone(),
                                operation,
                                response: current,
                                lease: lease.clone(),
                            })
                            .map_err(setup)?;
                        report.mutations_confirmed += 1;
                        return Ok(());
                    }
                }
                report.mutations_sent += 1;
                match client
                    .modify_exact(lease.proof(), &pinned.namespace, &body, &operation.id)
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        let rejected = match &error {
                            SyncError::PreconditionFailed(reason) => reason.contains(" CONFLICT:"),
                            SyncError::Protocol(reason) => {
                                reason.starts_with("HTTP 400 BAD_REQUEST:")
                            }
                            _ => false,
                        };
                        if rejected {
                            self.store()
                                .runtime(Command::RetireIssued {
                                    epoch: epoch.clone(),
                                    operation: operation.clone(),
                                    rejected: true,
                                })
                                .map_err(setup)?;
                        }
                        return Err(transport(error));
                    }
                }
            }
        };
        // Both monotonic authority and the actor-owned epoch are checked at commit.
        self.store()
            .runtime(Command::Confirm {
                epoch: epoch.clone(),
                operation,
                response,
                lease: lease.clone(),
            })
            .map_err(|e| {
                if e.to_string().contains("UPLOAD_LEASE_LOST") {
                    CycleError::LeaseLost
                } else {
                    setup(e)
                }
            })?;
        report.mutations_confirmed += 1;
        Ok(())
    }
    pub(super) async fn credential_client_for(
        &self,
        job: &SyncJob,
    ) -> Result<NativeClient, AppError> {
        let endpoint = super::super::native::NativeEndpoint::parse(&job.endpoint)
            .map_err(|e| AppError::Backend(e.to_string()))?;
        let reference = job.credential_ref.clone();
        let reader = self.credential_reader.clone();
        let key = tokio::task::spawn_blocking(move || reader(&reference))
            .await
            .map_err(|_| AppError::Backend("credential worker stopped".into()))??;
        let client =
            NativeClient::new(&endpoint, &key).map_err(|e| AppError::Backend(e.to_string()))?;
        Ok(client)
    }
    pub(crate) async fn transport_client_for(
        &self,
        job: &SyncJob,
    ) -> Result<NativeClient, AppError> {
        let client = self.credential_client_for(job).await?;
        let pinned = self
            .store()
            .native_scope(job.connection_id)?
            .ok_or_else(|| AppError::Backend("connection has no pinned identity".into()))?;
        let current = client
            .probe()
            .await
            .map_err(|e| AppError::Backend(e.to_string()))?;
        super::authentication::validate_identity(&pinned, &current)?;
        Ok(client)
    }
}
fn matches_phase(entry: &DirtyEntry, phase: UploadPhase) -> bool {
    if entry.dirty.contains(NativeDirty::DELETE) {
        phase == UploadPhase::Delete
    } else if entry.dirty.contains(NativeDirty::NEW) {
        phase == UploadPhase::Create
    } else {
        phase == UploadPhase::Modify
            && (entry.dirty.contains(NativeDirty::METADATA)
                || entry.dirty.contains(NativeDirty::CONTENT))
    }
}
fn eligible(service: &SyncService, job: &SyncJob, entry: &DirtyEntry) -> Result<bool, CycleError> {
    if entry.dirty.contains(NativeDirty::DELETE) {
        return Ok(true);
    }
    let path = job.local_root.join(&entry.relative_path);
    if !path.is_file() {
        return Ok(false);
    }
    if !std::fs::canonicalize(&path)
        .map_err(setup)?
        .starts_with(std::fs::canonicalize(&job.local_root).map_err(setup)?)
    {
        return Err(setup("local source escaped the configured root"));
    }
    if job.upload_scope == super::super::UploadScope::All {
        return Ok(true);
    }
    let conn = service.pool().get().map_err(setup)?;
    let album = std::path::Path::new(&entry.relative_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_job_upload_albums WHERE job_id=?1 AND relative_album=?2)",
        rusqlite::params![job.id, album],
        |r| r.get(0),
    )
    .map_err(setup)
}
fn deletion_event(entry: &DirtyEntry) -> Value {
    entry
        .deletion_event
        .as_ref()
        .map(|event| json!({"deletionEvent":event}))
        .unwrap_or_else(|| json!({}))
}
fn body_for(operation: &Issued, zone: &NativeZone) -> Result<Value, CycleError> {
    let create = operation.phase == "create";
    let resources = operation.preparation.get("resources");
    let kind = if create { "create" } else { "update" };
    let mut fields = if create || operation.dirty & NativeDirty::METADATA != 0 {
        operation.preparation["metadata"].clone()
    } else {
        json!({})
    };
    if create {
        if let Some(fields) = fields.as_object_mut() {
            fields.retain(|_, value| !value.is_null());
        }
    }
    let mut events = operation.preparation["events"].clone();
    if let Some(map) = events.as_object_mut() {
        map.retain(|key, value| {
            !value.is_null()
                && match key.as_str() {
                    "metadataEvent" => create || operation.dirty & NativeDirty::METADATA != 0,
                    "contentEvent" => resources.is_some(),
                    _ => false,
                }
        });
    }
    let mut photo = json!({"recordName":operation.photo,"recordType":"CPLAsset","fields":fields,"_syncserver":events});
    if !create {
        photo["recordChangeTag"] = operation.preparation["assetRevision"].clone();
    }
    let mut operations = Vec::new();
    if let Some(resources) = resources {
        photo["fields"]["contentGeneration"] =
            json!({"type":"STRING","value":operation.content_generation});
        if create {
            photo["fields"]["masterRef"] =
                json!({"type":"REFERENCE","value":{"recordName":operation.master,"action":"NONE"}});
        }
        let mut original = resources["original"]["asset"].clone();
        original["checksumAlgorithm"] = json!("SHA256");
        let mut thumbnail = resources["thumbnail"]["asset"].clone();
        thumbnail["checksumAlgorithm"] = json!("SHA256");
        let mut master = json!({"recordName":operation.master,"recordType":"CPLMaster","fields":{"contentGeneration":{"type":"STRING","value":operation.content_generation},"thumbnailSourceOriginalSHA256":{"type":"STRING","value":resources["original"]["hex"]},"resOriginalRes":{"type":"ASSETID","value":original},"resJPEGThumbRes":{"type":"ASSETID","value":thumbnail}}});
        if create {
            master["fields"]["photoId"] = json!({"type":"STRING","value":operation.photo});
        } else {
            master["recordChangeTag"] = operation.preparation["masterRevision"].clone();
        }
        operations.push(json!({"operationType":kind,"record":master}));
    }
    if photo["fields"]
        .as_object()
        .is_none_or(|fields| fields.is_empty())
    {
        return Err(setup(
            "an empty metadata patch is not successful synchronization",
        ));
    }
    operations.push(json!({"operationType":kind,"record":photo}));
    Ok(
        json!({"zoneID":super::super::native::receive::ZoneId::from(zone),"atomic":true,"operations":operations}),
    )
}

fn metadata_matches(request: &Value, response: &Value) -> bool {
    let Some(requested) = request["operations"].as_array().and_then(|ops| ops.first()) else {
        return false;
    };
    let Some(record) = response["records"].as_array().and_then(|rows| rows.first()) else {
        return false;
    };
    if record.get("serverErrorCode").is_some()
        || record["recordName"] != requested["record"]["recordName"]
    {
        return false;
    }
    requested["record"]["fields"]
        .as_object()
        .is_some_and(|fields| {
            fields.iter().all(|(name, value)| {
                if value.is_null() {
                    record["fields"].get(name).is_none()
                } else {
                    record["fields"][name] == *value
                }
            })
        })
}
