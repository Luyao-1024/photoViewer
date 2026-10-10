use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

mod configuration;
use configuration::{JobKey, GLOBAL_RUN_EPOCH};

use crate::config;
use crate::core::backend::local::LocalBackend;
use crate::core::error::{AppError, Result};

use super::local::{self, LocalEntry};
use super::model::{
    Baseline, EntrySnapshot, Observation, PlanAction, Revision, RevisionStrength, UploadScope,
};
use super::planner;
use super::provider::{RemoteEntry, SyncProvider, TransferProgress, WriteCondition};
use super::store::{StoredEntry, StoredTask, SyncConflict, SyncJob, SyncStore};
use super::webdav::WebDavProvider;

static OPERATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static OPERATION_SESSION: OnceLock<uuid::Uuid> = OnceLock::new();
static ACTIVE_JOBS: OnceLock<Mutex<HashSet<JobKey>>> = OnceLock::new();
static LIVE_PROGRESS: Mutex<Option<LiveProgressSession>> = Mutex::new(None);

/// What the live progress label should describe right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncLivePhase {
    /// Run started; remote enumeration and planning have not produced counts yet.
    Preparing,
    Downloading,
    Uploading,
}

/// In-memory progress of the current home-pull run. Never persisted; a run
/// reports it so the Photos overview can show real fractions instead of a
/// bare "synchronizing" label. While `transfer_active` is set, the byte
/// counters describe the file currently streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncLiveProgress {
    pub phase: SyncLivePhase,
    pub downloaded: usize,
    pub download_total: usize,
    pub uploaded: usize,
    pub upload_total: usize,
    pub transfer_active: bool,
    pub current_bytes: u64,
    pub current_total: u64,
}

/// The progress state plus the transport byte-count sink of the active run.
struct LiveProgressSession {
    state: SyncLiveProgress,
    transfer: Option<Arc<TransferProgress>>,
}

impl SyncLiveProgress {
    fn preparing() -> Self {
        Self {
            phase: SyncLivePhase::Preparing,
            downloaded: 0,
            download_total: 0,
            uploaded: 0,
            upload_total: 0,
            transfer_active: false,
            current_bytes: 0,
            current_total: 0,
        }
    }

    fn add_totals(&mut self, downloads: usize, uploads: usize) {
        self.download_total += downloads;
        self.upload_total += uploads;
    }

    fn begin_transfer(&mut self, phase: SyncLivePhase, expected_total: u64) {
        self.phase = phase;
        self.transfer_active = true;
        self.current_bytes = 0;
        self.current_total = expected_total;
    }

    fn finish_transfer(&mut self) {
        self.transfer_active = false;
        self.current_bytes = 0;
        self.current_total = 0;
    }

    /// Count one resolved planned transfer. Clamped to the planned total so
    /// unusual double transfers (verification followed by replacement) can
    /// never display "done beyond total".
    fn complete_download(&mut self) {
        if self.downloaded < self.download_total {
            self.downloaded += 1;
        }
        self.finish_transfer();
    }

    fn complete_upload(&mut self) {
        if self.uploaded < self.upload_total {
            self.uploaded += 1;
        }
        self.finish_transfer();
    }
}

/// Snapshot of the live synchronization progress, `None` when no run is
/// active. While a transfer streams, the byte counters are overlaid from the
/// transport sink so the label advances between per-file completions.
pub fn live_progress() -> Option<SyncLiveProgress> {
    let guard = LIVE_PROGRESS.lock().ok()?;
    let session = guard.as_ref()?;
    let mut snapshot = session.state;
    if snapshot.transfer_active {
        if let Some(transfer) = session.transfer.as_ref() {
            let (bytes, total) = transfer.snapshot();
            snapshot.current_bytes = bytes;
            snapshot.current_total = total;
        }
    }
    Some(snapshot)
}

fn live_progress_begin_session() {
    if let Ok(mut slot) = LIVE_PROGRESS.lock() {
        if slot.is_some() && any_job_active() {
            return;
        }
        *slot = Some(LiveProgressSession {
            state: SyncLiveProgress::preparing(),
            transfer: None,
        });
    }
}

/// Close the progress session only when no job is running. A duplicate
/// trigger closing its own cycle must not blind the still-running run that
/// owns the session.
fn live_progress_end_session() {
    if any_job_active() {
        return;
    }
    if let Ok(mut slot) = LIVE_PROGRESS.lock() {
        *slot = None;
    }
}

fn any_job_active() -> bool {
    ACTIVE_JOBS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|active| !active.is_empty())
        .unwrap_or(true)
}

fn job_is_active(job_id: &JobKey) -> bool {
    ACTIVE_JOBS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|active| active.contains(job_id))
        .unwrap_or(false)
}

fn live_progress_add_totals(downloads: usize, uploads: usize) {
    if let Ok(Some(session)) = LIVE_PROGRESS.lock().as_deref_mut() {
        session.state.add_totals(downloads, uploads);
    }
}

fn live_progress_attach_transfer(transfer: Arc<TransferProgress>) {
    if let Ok(Some(session)) = LIVE_PROGRESS.lock().as_deref_mut() {
        session.transfer = Some(transfer);
    }
}

fn live_progress_begin(phase: SyncLivePhase, expected_total: u64) {
    if let Ok(Some(session)) = LIVE_PROGRESS.lock().as_deref_mut() {
        session.state.begin_transfer(phase, expected_total);
        if let Some(transfer) = session.transfer.as_ref() {
            transfer.start(expected_total);
        }
    }
}

fn live_progress_complete_download() {
    if let Ok(Some(session)) = LIVE_PROGRESS.lock().as_deref_mut() {
        session.state.complete_download();
    }
}

fn live_progress_complete_upload() {
    if let Ok(Some(session)) = LIVE_PROGRESS.lock().as_deref_mut() {
        session.state.complete_upload();
    }
}

struct ActiveJobGuard(JobKey);

impl ActiveJobGuard {
    fn acquire(job_id: JobKey) -> Result<Self> {
        let active = ACTIVE_JOBS.get_or_init(|| Mutex::new(HashSet::new()));
        let mut active = active
            .lock()
            .map_err(|_| AppError::Backend("synchronization job lock is poisoned".into()))?;
        if !active.insert(job_id.clone()) {
            return Err(AppError::Backend(
                "synchronization job is already running".into(),
            ));
        }
        Ok(Self(job_id))
    }
}

impl Drop for ActiveJobGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_JOBS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            active.remove(&self.0);
        }
    }
}

#[derive(Debug, Clone)]
pub struct SyncCredentials {
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    UseLocal,
    UseRemote,
    KeepBoth,
}

impl ConflictResolution {
    fn as_str(self) -> &'static str {
        match self {
            Self::UseLocal => "use_local",
            Self::UseRemote => "use_remote",
            Self::KeepBoth => "keep_both",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub uploaded: usize,
    pub downloaded: usize,
    pub verified: usize,
    pub unchanged: usize,
    pub conflicts: usize,
}

#[derive(Clone)]
pub struct SyncService {
    store: SyncStore,
    pool: crate::core::db::DbPool,
    actor: Option<crate::core::db_actor::DbActorHandle>,
    staging_root: PathBuf,
}

impl SyncService {
    pub fn new(pool: crate::core::db::DbPool) -> Self {
        Self {
            store: SyncStore::new(pool.clone()),
            pool,
            actor: None,
            staging_root: config::data_dir().join("sync-staging"),
        }
    }

    pub fn with_actor(
        pool: crate::core::db::DbPool,
        actor: crate::core::db_actor::DbActorHandle,
    ) -> Self {
        Self {
            store: SyncStore::with_actor(pool.clone(), actor.clone()),
            pool,
            actor: Some(actor),
            staging_root: config::data_dir().join("sync-staging"),
        }
    }

    pub fn store(&self) -> &SyncStore {
        &self.store
    }

    pub async fn list_remote_collections(&self, job_id: i64) -> Result<Vec<String>> {
        let job = self
            .store
            .get_job(job_id)?
            .ok_or_else(|| AppError::Backend("synchronization job no longer exists".into()))?;
        let provider = self.provider_for_configuration(&job).await?;
        provider.probe().await.map_err(provider_error)?;

        let mut pending = VecDeque::from([String::new()]);
        let mut visited = HashSet::new();
        let mut collections = BTreeSet::new();
        while let Some(parent) = pending.pop_front() {
            if !visited.insert(parent.clone()) {
                continue;
            }
            for entry in provider
                .list_children(&parent)
                .await
                .map_err(provider_error)?
            {
                if !entry.is_collection {
                    continue;
                }
                let collection = entry.key.trim_matches('/').to_string();
                if collection.is_empty()
                    || collection
                        .split('/')
                        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
                {
                    continue;
                }
                collections.insert(collection.clone());
                pending.push_back(collection);
                if collections.len() > 10_000 {
                    return Err(AppError::Backend(
                        "cloud folder listing exceeds the 10000 collection safety limit".into(),
                    ));
                }
            }
        }
        Ok(collections.into_iter().collect())
    }

    fn cleanup_staging_artifacts(&self, artifacts: Vec<PathBuf>) {
        let Ok(staging_root) = self.staging_root.canonicalize() else {
            return;
        };
        for artifact in artifacts {
            let (Some(name), Some(parent)) = (
                artifact.file_name(),
                artifact.parent().and_then(|path| path.canonicalize().ok()),
            ) else {
                continue;
            };
            if !parent.starts_with(&staging_root) {
                continue;
            }
            if let Err(error) = std::fs::remove_file(parent.join(name)) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %artifact.display(), "could not remove abandoned sync artifact: {error}");
                }
            }
        }
    }

    pub async fn run_job(&self, job_id: i64, credentials: SyncCredentials) -> Result<RunSummary> {
        let job = self
            .store
            .get_job(job_id)?
            .ok_or_else(|| AppError::Backend("synchronization job no longer exists".into()))?;
        let progress = Arc::new(TransferProgress::default());
        let provider = Arc::new(
            WebDavProvider::new(&job.endpoint, job.username.clone(), credentials.password)
                .map_err(provider_error)?
                .with_transfer_progress(progress.clone()),
        );
        self.run_job_with_provider(job_id, provider, Some(progress))
            .await
    }

    async fn run_job_with_provider(
        &self,
        job_id: i64,
        provider: Arc<dyn SyncProvider>,
        progress: Option<Arc<TransferProgress>>,
    ) -> Result<RunSummary> {
        ensure_webdav_sync_enabled()?;
        let _active = self.acquire_run_slot(job_id).await?;
        let job = self
            .store
            .get_job(job_id)?
            .ok_or_else(|| AppError::Backend("synchronization job no longer exists".into()))?;
        if job.paused || self.store.pending_change(job_id)?.is_some() {
            return Err(AppError::Backend(
                "synchronization configuration is applying".into(),
            ));
        }
        self.store.mark_job_started(job_id)?;
        let result = self
            .run_with_provider_inner(&job, provider, true, progress)
            .await;
        // Close the lifecycle on *every* outcome. The old code deliberately
        // skipped the completion whenever a configuration edit was pending or
        // sync had been switched off mid-run, which left `last_started_at`
        // ahead of `last_completed_at` with nothing able to close it: the
        // overview then reported "syncing" on every later launch, forever.
        // `finish_run` still decides what remains attributable to write (the
        // error is generation-guarded); only the fact that the run ended is
        // unconditional.
        let failure = result.as_ref().err().map(ToString::to_string);
        if let Err(error) = self
            .store
            .finish_run(job_id, job.config_generation, failure.as_deref())
        {
            tracing::warn!(
                job_id,
                "could not record that the synchronization run ended: {error}"
            );
        }
        result
    }

    pub async fn run_saved_job(&self, job_id: i64) -> Result<RunSummary> {
        if let Some(provider) = self.registered_provider(job_id)? {
            return self.run_job_with_provider(job_id, provider, None).await;
        }
        ensure_webdav_sync_enabled()?;
        let job = self
            .store
            .get_job(job_id)?
            .ok_or_else(|| AppError::Backend(format!("sync job {job_id} does not exist")))?;
        let reference = job.credential_ref;
        let password =
            tokio::task::spawn_blocking(move || crate::platform::credentials::load(&reference))
                .await
                .map_err(|error| AppError::Backend(format!("credential task failed: {error}")))??;
        self.run_job(job_id, SyncCredentials { password }).await
    }

    /// Close runs that a previous process left open.
    ///
    /// A run cannot outlive the process that started it, so any job whose
    /// `last_started_at` is still ahead of `last_completed_at` at launch is
    /// describing a run this process never began. Without this the overview's
    /// derived `Running` status is unrecoverable: it is recomputed from those
    /// two persisted columns on every launch, so a single interrupted run
    /// pins the compact status to "syncing" for every later launch, with no
    /// failure recorded and therefore no retry offered. Call this once per
    /// launch, before starting any run.
    pub fn reconcile_interrupted_runs(&self) -> Result<usize> {
        self.store.reconcile_interrupted_runs()
    }

    pub async fn trigger_saved_jobs_once(&self) -> Result<()> {
        ensure_webdav_sync_enabled()?;
        for job in self.store.list_jobs()? {
            self.request_job_sync(job.id)?;
        }
        Ok(())
    }

    async fn resolve_with_provider(
        &self,
        job: &SyncJob,
        conflict: SyncConflict,
        resolution: ConflictResolution,
        provider: Arc<dyn SyncProvider>,
    ) -> Result<()> {
        let _active = ActiveJobGuard::acquire(self.job_key(job.id)?)?;
        provider.probe().await.map_err(provider_error)?;
        let current_conflict = self
            .store
            .get_open_conflict(conflict.id)?
            .filter(|current| current == &conflict)
            .ok_or_else(|| {
                AppError::Backend(
                    "synchronization conflict changed; review the current versions again".into(),
                )
            })?;
        let upload_albums = self
            .store
            .desired_upload_albums(job.id)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        if !upload_allowed(
            job.upload_scope,
            &upload_albums,
            &current_conflict.relative_path,
        ) && resolution != ConflictResolution::UseRemote
        {
            return Err(AppError::Backend(
                "this album is not selected for upload; use the cloud version or select the album first"
                    .into(),
            ));
        }
        let key = remote_key(&job.remote_root, &current_conflict.relative_path)?;
        let local_path = local::destination(&job.local_root, &current_conflict.relative_path)?;
        let local_fingerprint = optional_local_fingerprint(&local_path)?;
        if local_fingerprint
            .as_ref()
            .map(|value| value.blake3.as_str())
            != current_conflict.local_fingerprint.as_deref()
        {
            return Err(conflict_changed());
        }
        let remote = provider.stat(&key).await.map_err(provider_error)?;
        if remote
            .as_ref()
            .and_then(|entry| entry.revision.as_ref())
            .map(|revision| revision.value.as_str())
            != current_conflict.remote_revision.as_deref()
        {
            return Err(conflict_changed());
        }

        if let Some(pending) = self.store.pending_change(job.id)? {
            if pending
                .remote_root
                .as_ref()
                .is_some_and(|root| root != &job.remote_root)
                || pending.conflict.as_ref().is_none_or(|(choice, action)| {
                    choice != &current_conflict || action != resolution.as_str()
                })
            {
                return Err(AppError::Backend(
                    "conflict choice was superseded by a newer edit".into(),
                ));
            }
        }
        match resolution {
            ConflictResolution::UseLocal => {
                self.resolve_use_local(
                    job,
                    &current_conflict,
                    provider.as_ref(),
                    &key,
                    &local_path,
                    local_fingerprint,
                    remote,
                )
                .await
            }
            ConflictResolution::UseRemote => {
                self.resolve_use_remote(
                    job,
                    &current_conflict,
                    provider.as_ref(),
                    &local_path,
                    local_fingerprint,
                    remote,
                )
                .await
            }
            ConflictResolution::KeepBoth => {
                self.resolve_keep_both(
                    job,
                    &current_conflict,
                    provider.as_ref(),
                    &key,
                    &local_path,
                    local_fingerprint,
                    remote,
                )
                .await
            }
        }
    }

    #[cfg(test)]
    async fn run_with_provider(
        &self,
        job: &SyncJob,
        provider: Arc<dyn SyncProvider>,
    ) -> Result<RunSummary> {
        self.run_with_provider_inner(job, provider, false, None)
            .await
    }

    async fn run_with_provider_inner(
        &self,
        job: &SyncJob,
        provider: Arc<dyn SyncProvider>,
        monitor_global_switch: bool,
        transfer_progress: Option<Arc<TransferProgress>>,
    ) -> Result<RunSummary> {
        let run_epoch = GLOBAL_RUN_EPOCH.load(Ordering::Acquire);
        if self.sync_should_stop(
            job.id,
            job.config_generation,
            monitor_global_switch,
            run_epoch,
        )? {
            return Ok(RunSummary::default());
        }
        provider.probe().await.map_err(provider_error)?;
        if self.sync_should_stop(
            job.id,
            job.config_generation,
            monitor_global_switch,
            run_epoch,
        )? {
            return Ok(RunSummary::default());
        }
        provider
            .ensure_collection(job.remote_root.trim_matches('/'))
            .await
            .map_err(provider_error)?;
        if self.sync_should_stop(
            job.id,
            job.config_generation,
            monitor_global_switch,
            run_epoch,
        )? {
            return Ok(RunSummary::default());
        }
        self.recover_unfinished_tasks(job, provider.as_ref(), monitor_global_switch, run_epoch)
            .await?;
        if self.sync_should_stop(
            job.id,
            job.config_generation,
            monitor_global_switch,
            run_epoch,
        )? {
            return Ok(RunSummary::default());
        }
        let protected_uploads = self
            .store
            .unfinished_tasks(job.id)?
            .into_iter()
            .filter(|task| task.action.starts_with("upload_"))
            .map(|task| task.relative_path)
            .collect::<BTreeSet<_>>();
        let remote_entries = discover_remote(provider.as_ref(), &job.remote_root).await?;
        if self.sync_should_stop(
            job.id,
            job.config_generation,
            monitor_global_switch,
            run_epoch,
        )? {
            return Ok(RunSummary::default());
        }
        // Cloud discovery determines download scope only. Upload permissions
        // remain the user's persisted choice, even for cloud-backed albums.
        let current_job = self
            .store
            .get_job(job.id)?
            .ok_or_else(|| AppError::Backend(format!("sync job {} no longer exists", job.id)))?;
        if current_job.config_generation != job.config_generation {
            return Ok(RunSummary::default());
        }
        let job = current_job;
        let upload_albums = self
            .store
            .desired_upload_albums(job.id)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let local_entries = local::scan_matching(&job.local_root, |relative_path| {
            remote_entries.contains_key(relative_path)
                || upload_allowed(job.upload_scope, &upload_albums, relative_path)
        })?;
        let stored_entries = self
            .store
            .entries(job.id)?
            .into_iter()
            .map(|entry| (entry.relative_path.clone(), entry))
            .collect::<BTreeMap<_, _>>();

        let mut paths = BTreeSet::new();
        paths.extend(local_entries.keys().cloned());
        paths.extend(remote_entries.keys().cloned());
        paths.extend(stored_entries.keys().cloned());

        // Pre-plan every path once so the live progress label can show real
        // fractions ("downloaded X of Y") before the first transfer finishes.
        let mut planned_downloads = 0usize;
        let mut planned_uploads = 0usize;
        for relative_path in &paths {
            if protected_uploads.contains(relative_path) {
                continue;
            }
            let upload_ok = upload_allowed(job.upload_scope, &upload_albums, relative_path);
            match plan_entry(
                &job,
                relative_path,
                observed_local(local_entries.get(relative_path)),
                observed_remote(
                    remote_entries.get(relative_path),
                    stored_entries.get(relative_path),
                ),
                stored_entries.get(relative_path),
                upload_ok,
            ) {
                PlanAction::UploadNew | PlanAction::UploadReplace { .. } => planned_uploads += 1,
                PlanAction::DownloadNew
                | PlanAction::DownloadReplace
                | PlanAction::VerifyContent => planned_downloads += 1,
                _ => {}
            }
        }
        live_progress_add_totals(planned_downloads, planned_uploads);
        if let Some(transfer) = transfer_progress {
            live_progress_attach_transfer(transfer);
        }

        let mut summary = RunSummary::default();
        for relative_path in paths {
            if self.sync_should_stop(
                job.id,
                job.config_generation,
                monitor_global_switch,
                run_epoch,
            )? {
                break;
            }
            if protected_uploads.contains(&relative_path) {
                continue;
            }
            let local = local_entries.get(&relative_path);
            let remote = remote_entries.get(&relative_path);
            let stored = stored_entries.get(&relative_path);
            self.reconcile_one(
                &job,
                provider.as_ref(),
                &relative_path,
                local,
                remote,
                stored,
                upload_allowed(job.upload_scope, &upload_albums, &relative_path),
                &mut summary,
            )
            .await?;
        }
        Ok(summary)
    }

    fn sync_should_stop(
        &self,
        job_id: i64,
        expected_generation: i64,
        monitor_global_switch: bool,
        run_epoch: u64,
    ) -> Result<bool> {
        if monitor_global_switch
            && (!crate::core::prefs::webdav_sync_enabled()
                || GLOBAL_RUN_EPOCH.load(Ordering::Acquire) != run_epoch)
        {
            return Ok(true);
        }
        let job = self
            .store
            .get_job(job_id)?
            .ok_or_else(|| AppError::Backend(format!("sync job {job_id} no longer exists")))?;
        Ok(job.paused
            || job.config_generation != expected_generation
            || self.store.pending_change(job_id)?.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_use_local(
        &self,
        job: &SyncJob,
        conflict: &SyncConflict,
        provider: &dyn SyncProvider,
        key: &str,
        local_path: &Path,
        local_fingerprint: Option<super::model::Fingerprint>,
        remote: Option<RemoteEntry>,
    ) -> Result<()> {
        let fingerprint = local_fingerprint.ok_or_else(|| {
            AppError::Backend("the selected local version no longer exists".into())
        })?;
        let expected = remote
            .and_then(|entry| entry.revision)
            .filter(|revision| revision.strength == RevisionStrength::Strong)
            .ok_or_else(|| {
                AppError::Backend(
                    "the server did not provide a strong ETag, so the remote file cannot be replaced safely"
                        .into(),
                )
            })?;
        let operation_id = next_operation_id(job.id);
        let local_entry = LocalEntry {
            relative_path: conflict.relative_path.clone(),
            absolute_path: local_path.to_path_buf(),
            fingerprint: fingerprint.clone(),
            modified_ns: 0,
        };
        let snapshot = self.create_upload_snapshot(job, &local_entry, &operation_id)?;
        let snapshot_fingerprint = local::fingerprint(&snapshot)?;
        if snapshot_fingerprint != fingerprint {
            remove_file_if_exists(&snapshot)?;
            return Err(conflict_changed());
        }
        self.store.prepare_task(
            &operation_id,
            job.id,
            conflict.entry_id,
            "resolve_use_local",
            Some(&expected),
            &snapshot,
            &snapshot_fingerprint.blake3,
            job.config_generation,
            self.entry_generation(job.id, conflict.entry_id)?,
        )?;
        let uploaded = match provider
            .upload(key, &snapshot, WriteCondition::ReplaceIf(expected))
            .await
        {
            Ok(uploaded) => uploaded,
            Err(error) => {
                self.store.set_task_state(
                    &operation_id,
                    "reconciling",
                    Some(&error.to_string()),
                )?;
                return Err(provider_error(error));
            }
        };
        let revision = uploaded
            .revision
            .as_ref()
            .map(|revision| revision.value.as_str());
        self.store.resolve_conflict(
            conflict.id,
            conflict.entry_id,
            ConflictResolution::UseLocal.as_str(),
            &snapshot_fingerprint,
            revision,
        )?;
        let current = local::fingerprint(local_path)?;
        if current != snapshot_fingerprint {
            self.store.upsert_observation(
                job.id,
                &conflict.relative_path,
                Some(&current),
                None,
                Some(&snapshot_fingerprint),
                revision,
                uploaded
                    .revision
                    .as_ref()
                    .is_some_and(|value| value.strength == RevisionStrength::Weak),
                "pending",
            )?;
        }
        self.store
            .set_task_state(&operation_id, "succeeded", None)?;
        remove_file_if_exists(&snapshot)
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_use_remote(
        &self,
        job: &SyncJob,
        conflict: &SyncConflict,
        provider: &dyn SyncProvider,
        local_path: &Path,
        local_fingerprint: Option<super::model::Fingerprint>,
        remote: Option<RemoteEntry>,
    ) -> Result<()> {
        let remote = remote.ok_or_else(|| {
            AppError::Backend("the selected cloud version no longer exists".into())
        })?;
        let expected = remote.revision.as_ref().ok_or_else(|| {
            AppError::Backend(
                "the server did not provide a version identifier for the selected cloud file"
                    .into(),
            )
        })?;
        let staged = self
            .download_to_staging(job, provider, &conflict.relative_path, Some(expected))
            .await?;
        let fingerprint = local::fingerprint(&staged)?;
        if conflict
            .remote_fingerprint
            .as_deref()
            .is_some_and(|recorded| recorded != fingerprint.blake3)
        {
            remove_file_if_exists(&staged)?;
            return Err(conflict_changed());
        }
        self.ensure_remote_revision(provider, job, conflict, expected)
            .await?;
        if optional_local_fingerprint(local_path)? != local_fingerprint {
            remove_file_if_exists(&staged)?;
            return Err(conflict_changed());
        }
        let operation_id = operation_id_from_artifact(&staged)?;
        self.store.prepare_task(
            &operation_id,
            job.id,
            conflict.entry_id,
            "resolve_use_remote",
            Some(expected),
            &staged,
            &fingerprint.blake3,
            job.config_generation,
            self.entry_generation(job.id, conflict.entry_id)?,
        )?;
        let backup = if local_fingerprint.is_some() {
            Some(local::atomic_publish_replace(
                &staged,
                local_path,
                &operation_id,
            )?)
        } else {
            local::atomic_publish_new(&staged, local_path)?;
            None
        };
        let commit = (|| {
            self.upsert_downloaded_media(local_path)?;
            self.store.resolve_conflict(
                conflict.id,
                conflict.entry_id,
                ConflictResolution::UseRemote.as_str(),
                &fingerprint,
                Some(&expected.value),
            )
        })();
        if let Err(error) = commit {
            self.store
                .set_task_state(&operation_id, "reconciling", Some(&error.to_string()))?;
            return Err(error);
        }
        if let Some(backup) = backup {
            remove_file_if_exists(&backup)?;
        }
        self.store
            .set_task_state(&operation_id, "succeeded", None)?;
        remove_file_if_exists(&staged)
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_keep_both(
        &self,
        job: &SyncJob,
        conflict: &SyncConflict,
        provider: &dyn SyncProvider,
        original_key: &str,
        local_path: &Path,
        local_fingerprint: Option<super::model::Fingerprint>,
        remote: Option<RemoteEntry>,
    ) -> Result<()> {
        let local_fingerprint = local_fingerprint.ok_or_else(|| {
            AppError::Backend("the local conflict version no longer exists".into())
        })?;
        let remote = remote.ok_or_else(|| {
            AppError::Backend("the cloud conflict version no longer exists".into())
        })?;
        let expected = remote
            .revision
            .as_ref()
            .filter(|revision| revision.strength == RevisionStrength::Strong)
            .ok_or_else(|| {
                AppError::Backend(
                    "the server did not provide a strong ETag, so both versions cannot be finalized safely"
                        .into(),
                )
            })?;
        let remote_artifact = self
            .download_to_staging(job, provider, &conflict.relative_path, Some(expected))
            .await?;
        let remote_fingerprint = local::fingerprint(&remote_artifact)?;
        if conflict
            .remote_fingerprint
            .as_deref()
            .is_some_and(|recorded| recorded != remote_fingerprint.blake3)
        {
            remove_file_if_exists(&remote_artifact)?;
            return Err(conflict_changed());
        }
        self.ensure_remote_revision(provider, job, conflict, expected)
            .await?;
        if optional_local_fingerprint(local_path)?.as_ref() != Some(&local_fingerprint) {
            remove_file_if_exists(&remote_artifact)?;
            return Err(conflict_changed());
        }

        let local_operation = next_operation_id(job.id);
        let local_entry = LocalEntry {
            relative_path: conflict.relative_path.clone(),
            absolute_path: local_path.to_path_buf(),
            fingerprint: local_fingerprint.clone(),
            modified_ns: 0,
        };
        let local_artifact = self.create_upload_snapshot(job, &local_entry, &local_operation)?;
        if local::fingerprint(&local_artifact)? != local_fingerprint {
            remove_file_if_exists(&remote_artifact)?;
            remove_file_if_exists(&local_artifact)?;
            return Err(conflict_changed());
        }

        let copy_relative = conflict_copy_path(&conflict.relative_path, conflict.id)?;
        let copy_key = remote_key(&job.remote_root, &copy_relative)?;
        let copy_local = local::destination(&job.local_root, &copy_relative)?;
        let copy_operation = operation_id_from_artifact(&remote_artifact)?;
        self.store.prepare_task(
            &copy_operation,
            job.id,
            conflict.entry_id,
            "resolve_keep_both_copy",
            None,
            &remote_artifact,
            &remote_fingerprint.blake3,
            job.config_generation,
            self.entry_generation(job.id, conflict.entry_id)?,
        )?;
        let uploaded_copy = self
            .upload_new_or_verify(
                provider,
                &copy_key,
                &remote_artifact,
                &remote_fingerprint,
                job,
            )
            .await?;
        publish_new_or_verify(&remote_artifact, &copy_local, &remote_fingerprint)?;
        self.upsert_downloaded_media(&copy_local)?;
        self.store
            .set_task_state(&copy_operation, "succeeded", None)?;
        remove_file_if_exists(&remote_artifact)?;

        if optional_local_fingerprint(local_path)?.as_ref() != Some(&local_fingerprint) {
            return Err(conflict_changed());
        }
        self.store.prepare_task(
            &local_operation,
            job.id,
            conflict.entry_id,
            "resolve_keep_both_original",
            Some(expected),
            &local_artifact,
            &local_fingerprint.blake3,
            job.config_generation,
            self.entry_generation(job.id, conflict.entry_id)?,
        )?;
        let uploaded_original = match provider
            .upload(
                original_key,
                &local_artifact,
                WriteCondition::ReplaceIf(expected.clone()),
            )
            .await
        {
            Ok(uploaded) => uploaded,
            Err(error) => {
                self.store.set_task_state(
                    &local_operation,
                    "reconciling",
                    Some(&error.to_string()),
                )?;
                return Err(provider_error(error));
            }
        };
        let original_revision = uploaded_original
            .revision
            .as_ref()
            .map(|revision| revision.value.as_str());
        let copy_entry = self.store.upsert_observation(
            job.id,
            &copy_relative,
            Some(&remote_fingerprint),
            None,
            Some(&remote_fingerprint),
            uploaded_copy
                .revision
                .as_ref()
                .map(|revision| revision.value.as_str()),
            uploaded_copy
                .revision
                .as_ref()
                .is_some_and(|revision| revision.strength == RevisionStrength::Weak),
            "synced",
        )?;
        self.store.commit_baseline(
            copy_entry,
            &remote_fingerprint,
            uploaded_copy
                .revision
                .as_ref()
                .map(|revision| revision.value.as_str()),
        )?;
        self.store.resolve_conflict(
            conflict.id,
            conflict.entry_id,
            ConflictResolution::KeepBoth.as_str(),
            &local_fingerprint,
            original_revision,
        )?;
        self.store
            .set_task_state(&local_operation, "succeeded", None)?;
        remove_file_if_exists(&local_artifact)
    }

    async fn ensure_remote_revision(
        &self,
        provider: &dyn SyncProvider,
        job: &SyncJob,
        conflict: &SyncConflict,
        expected: &Revision,
    ) -> Result<()> {
        let key = remote_key(&job.remote_root, &conflict.relative_path)?;
        let current = provider.stat(&key).await.map_err(provider_error)?;
        if current
            .and_then(|entry| entry.revision)
            .as_ref()
            .is_some_and(|revision| revision == expected)
        {
            Ok(())
        } else {
            Err(conflict_changed())
        }
    }

    async fn upload_new_or_verify(
        &self,
        provider: &dyn SyncProvider,
        key: &str,
        artifact: &Path,
        expected_fingerprint: &super::model::Fingerprint,
        job: &SyncJob,
    ) -> Result<RemoteEntry> {
        match provider
            .upload(key, artifact, WriteCondition::CreateOnly)
            .await
        {
            Ok(uploaded) => Ok(uploaded),
            Err(super::provider::ProviderError::PreconditionFailed(_)) => {
                let existing = provider
                    .stat(key)
                    .await
                    .map_err(provider_error)?
                    .ok_or_else(conflict_changed)?;
                let relative = relative_remote_key(job.remote_root.trim_matches('/'), key)?;
                let downloaded = self
                    .download_to_staging(job, provider, &relative, existing.revision.as_ref())
                    .await?;
                let fingerprint = local::fingerprint(&downloaded)?;
                remove_file_if_exists(&downloaded)?;
                if &fingerprint == expected_fingerprint {
                    Ok(existing)
                } else {
                    Err(AppError::Backend(format!(
                        "the conflict copy path is already occupied by different content: {key}"
                    )))
                }
            }
            Err(error) => Err(provider_error(error)),
        }
    }

    fn entry_generation(&self, job_id: i64, entry_id: i64) -> Result<i64> {
        self.store
            .entries(job_id)?
            .into_iter()
            .find(|entry| entry.id == entry_id)
            .map(|entry| entry.generation)
            .ok_or_else(|| AppError::Backend("synchronization entry no longer exists".into()))
    }

    fn upsert_downloaded_media(&self, target: &Path) -> Result<()> {
        if let Some(item) = LocalBackend::new_item_from_path(target)? {
            if let Some(actor) = &self.actor {
                actor.execute_blocking(crate::core::db_actor::DbCommand::UpsertMediaBatch {
                    source: crate::core::events::ChangeSource::Synchronization,
                    items: vec![item],
                })?;
            } else {
                LocalBackend::new(self.pool.clone()).upsert(&item)?;
            }
        }
        Ok(())
    }

    async fn recover_unfinished_tasks(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        monitor_global_switch: bool,
        run_epoch: u64,
    ) -> Result<()> {
        for task in self.store.unfinished_tasks(job.id)? {
            if self.sync_should_stop(
                job.id,
                job.config_generation,
                monitor_global_switch,
                run_epoch,
            )? {
                break;
            }
            if task.config_generation > job.config_generation {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("task belongs to an older synchronization configuration"),
                )?;
                continue;
            }
            if task.action.starts_with("resolve_") {
                self.recover_resolution_task(job, provider, &task).await?;
            } else if task.action.starts_with("upload_") {
                self.recover_upload_task(job, provider, &task).await?;
            } else if task.action.starts_with("download_") {
                self.recover_download_task(job, provider, &task).await?;
            } else {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("unknown synchronization recovery action"),
                )?;
            }
        }
        Ok(())
    }

    async fn recover_upload_task(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        task: &StoredTask,
    ) -> Result<()> {
        let artifact = match verified_task_artifact(task)? {
            Some(fingerprint) => fingerprint,
            None => {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("upload recovery artifact is missing or changed"),
                )?;
                return Ok(());
            }
        };
        let key = remote_key(&job.remote_root, &task.relative_path)?;
        let mut partial_fingerprint = None;
        let mut partial_revision = None;
        if let Some(remote) = provider.stat(&key).await.map_err(provider_error)? {
            let downloaded = self
                .download_to_staging(job, provider, &task.relative_path, remote.revision.as_ref())
                .await?;
            let remote_fingerprint = local::fingerprint(&downloaded)?;
            if remote_fingerprint == artifact {
                remove_file_if_exists(&downloaded)?;
                if !self.restore_recovered_local(job, task, &artifact, None)? {
                    return Ok(());
                }
                self.finish_recovered_transfer(job, task, &artifact, remote.revision.as_ref())?;
                self.resolve_recovered_conflicts(job, task, &artifact, remote.revision.as_ref())?;
                remove_file_if_exists(&task.artifact_path)?;
                return Ok(());
            }
            if task.action == "upload_new"
                && is_strict_file_prefix(&downloaded, &task.artifact_path)?
                && remote
                    .revision
                    .as_ref()
                    .is_some_and(|revision| revision.strength == RevisionStrength::Strong)
            {
                partial_fingerprint = Some(remote_fingerprint);
                partial_revision = remote.revision;
            } else if task.action == "upload_new" {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("remote content is not a verified upload prefix with a strong ETag"),
                )?;
                remove_file_if_exists(&downloaded)?;
                return Ok(());
            }
            remove_file_if_exists(&downloaded)?;
        }

        let upload_albums = self
            .store
            .desired_upload_albums(job.id)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let scope = if self
            .store
            .pending_change(job.id)?
            .is_some_and(|change| change.upload_albums.is_some())
        {
            UploadScope::SelectedAlbums
        } else {
            job.upload_scope
        };
        if !upload_allowed(scope, &upload_albums, &task.relative_path) {
            // A proven completed write above may be committed, but recovery
            // cannot bypass a subsequent opt-out. Keep the snapshot and protect
            // this path from downloading a partial remote file over the original.
            if self.restore_recovered_local(job, task, &artifact, partial_fingerprint.as_ref())? {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("album is not selected for upload; preserved snapshot awaits explicit selection"),
                )?;
            }
            return Ok(());
        }

        let condition = if task.action == "upload_new" {
            partial_revision.map_or(WriteCondition::CreateOnly, WriteCondition::ReplaceIf)
        } else {
            let expected = task.expected_revision.as_ref().ok_or_else(|| {
                AppError::Backend("replacement recovery has no expected remote revision".into())
            })?;
            if task.expected_weak {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("weak ETag cannot authorize upload recovery"),
                )?;
                return Ok(());
            }
            WriteCondition::ReplaceIf(Revision::etag(expected.clone()))
        };
        match provider.upload(&key, &task.artifact_path, condition).await {
            Ok(uploaded) => {
                let verified = self
                    .download_to_staging(
                        job,
                        provider,
                        &task.relative_path,
                        uploaded.revision.as_ref(),
                    )
                    .await?;
                let matches = local::fingerprint(&verified)? == artifact;
                remove_file_if_exists(&verified)?;
                if !matches {
                    self.store.set_task_state(
                        &task.operation_id,
                        "blocked",
                        Some("remote content differs after recovery upload"),
                    )?;
                    return Ok(());
                }
                if !self.restore_recovered_local(
                    job,
                    task,
                    &artifact,
                    partial_fingerprint.as_ref(),
                )? {
                    return Ok(());
                }
                self.finish_recovered_transfer(job, task, &artifact, uploaded.revision.as_ref())?;
                self.resolve_recovered_conflicts(job, task, &artifact, uploaded.revision.as_ref())?;
                remove_file_if_exists(&task.artifact_path)?;
            }
            Err(error) => {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some(&format!("upload recovery requires reconciliation: {error}")),
                )?;
            }
        }
        Ok(())
    }

    fn restore_recovered_local(
        &self,
        job: &SyncJob,
        task: &StoredTask,
        artifact: &super::model::Fingerprint,
        partial: Option<&super::model::Fingerprint>,
    ) -> Result<bool> {
        let target = local::destination(&job.local_root, &task.relative_path)?;
        match optional_local_fingerprint(&target)? {
            Some(current) if current == *artifact => {}
            Some(current)
                if partial == Some(&current)
                    || is_strict_file_prefix(&target, &task.artifact_path)? =>
            {
                let backup = local::atomic_publish_replace(
                    &task.artifact_path,
                    &target,
                    &task.operation_id,
                )?;
                remove_file_if_exists(&backup)?;
                self.upsert_downloaded_media(&target)?;
            }
            None => {
                local::atomic_publish_new(&task.artifact_path, &target)?;
                self.upsert_downloaded_media(&target)?;
            }
            Some(_) => {
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("local content differs from the upload snapshot and remote prefix"),
                )?;
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn resolve_recovered_conflicts(
        &self,
        job: &SyncJob,
        task: &StoredTask,
        artifact: &super::model::Fingerprint,
        revision: Option<&Revision>,
    ) -> Result<()> {
        for conflict in self.store.open_conflicts(job.id)? {
            if conflict.entry_id == task.entry_id {
                self.store.resolve_conflict(
                    conflict.id,
                    task.entry_id,
                    "recovered_upload",
                    artifact,
                    revision.map(|value| value.value.as_str()),
                )?;
            }
        }
        Ok(())
    }

    async fn recover_download_task(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        task: &StoredTask,
    ) -> Result<()> {
        let target = local::destination(&job.local_root, &task.relative_path)?;
        let current = optional_local_fingerprint(&target)?;
        if current
            .as_ref()
            .is_some_and(|fingerprint| fingerprint.blake3 == task.artifact_hash)
        {
            let key = remote_key(&job.remote_root, &task.relative_path)?;
            let remote = provider.stat(&key).await.map_err(provider_error)?;
            let version_matches = remote
                .as_ref()
                .and_then(|entry| entry.revision.as_ref())
                .map(|revision| revision.value.as_str())
                == task.expected_revision.as_deref();
            if version_matches {
                let fingerprint = current.expect("checked above");
                self.upsert_downloaded_media(&target)?;
                self.finish_recovered_transfer(
                    job,
                    task,
                    &fingerprint,
                    remote.as_ref().and_then(|entry| entry.revision.as_ref()),
                )?;
                remove_file_if_exists(&task.artifact_path)?;
                return Ok(());
            }
        }

        if task.artifact_path.is_file() {
            self.store.set_task_state(
                &task.operation_id,
                "superseded",
                Some("download had not been published; a fresh reconciliation will replace it"),
            )?;
            remove_file_if_exists(&task.artifact_path)?;
        } else {
            self.store.set_task_state(
                &task.operation_id,
                "blocked",
                Some("download publication result is ambiguous and requires reconciliation"),
            )?;
        }
        Ok(())
    }

    async fn recover_resolution_task(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        task: &StoredTask,
    ) -> Result<()> {
        let key = remote_key(&job.remote_root, &task.relative_path)?;
        let remote = provider.stat(&key).await.map_err(provider_error)?;
        if let (Some(artifact), Some(remote)) = (verified_task_artifact(task)?, remote) {
            let downloaded = self
                .download_to_staging(job, provider, &task.relative_path, remote.revision.as_ref())
                .await?;
            let actual = local::fingerprint(&downloaded)?;
            remove_file_if_exists(&downloaded)?;
            if artifact == actual {
                // The write is proven, but resolving the conflict also requires
                // the original user choice and any companion-copy state. Keep
                // the evidence and ask the user to review the still-open row.
                self.store.set_task_state(
                    &task.operation_id,
                    "blocked",
                    Some("conflict write completed; review the conflict before finalizing"),
                )?;
                return Ok(());
            }
        }
        self.store.set_task_state(
            &task.operation_id,
            "blocked",
            Some("conflict resolution was interrupted; review current versions again"),
        )?;
        Ok(())
    }

    fn finish_recovered_transfer(
        &self,
        job: &SyncJob,
        task: &StoredTask,
        fingerprint: &super::model::Fingerprint,
        revision: Option<&Revision>,
    ) -> Result<()> {
        self.store.commit_baseline(
            task.entry_id,
            fingerprint,
            revision.map(|value| value.value.as_str()),
        )?;
        let local_path = local::destination(&job.local_root, &task.relative_path)?;
        if let Some(current) = optional_local_fingerprint(&local_path)? {
            if current != *fingerprint {
                self.store.upsert_observation(
                    job.id,
                    &task.relative_path,
                    Some(&current),
                    None,
                    Some(fingerprint),
                    revision.map(|value| value.value.as_str()),
                    revision.is_some_and(|value| value.strength == RevisionStrength::Weak),
                    "pending",
                )?;
            }
        }
        self.store
            .set_task_state(&task.operation_id, "succeeded", None)
    }

    #[allow(clippy::too_many_arguments)]
    async fn reconcile_one(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        local_entry: Option<&LocalEntry>,
        remote_entry: Option<&RemoteEntry>,
        stored: Option<&StoredEntry>,
        upload_allowed: bool,
        summary: &mut RunSummary,
    ) -> Result<()> {
        let local_observation = observed_local(local_entry);
        let remote_observation = observed_remote(remote_entry, stored);
        let mut action = plan_entry(
            job,
            relative_path,
            local_observation,
            remote_observation.clone(),
            stored,
            upload_allowed,
        );

        let entry_id = self.store.upsert_observation(
            job.id,
            relative_path,
            local_entry.map(|entry| &entry.fingerprint),
            local_entry.map(|entry| entry.modified_ns),
            remote_observation.fingerprint(),
            remote_entry.and_then(|entry| entry.revision.as_ref().map(|r| r.value.as_str())),
            remote_entry
                .and_then(|entry| entry.revision.as_ref())
                .is_some_and(|revision| revision.strength == RevisionStrength::Weak),
            action_state(&action),
        )?;

        if action == PlanAction::VerifyContent {
            let remote = remote_entry
                .ok_or_else(|| AppError::Backend("verification requires a remote object".into()))?;
            live_progress_begin(SyncLivePhase::Downloading, remote.size);
            let verified = self
                .download_to_staging(job, provider, relative_path, remote.revision.as_ref())
                .await?;
            let remote_fingerprint = local::fingerprint(&verified)?;
            if local_entry.is_some_and(|entry| entry.fingerprint == remote_fingerprint) {
                self.store.commit_baseline(
                    entry_id,
                    &remote_fingerprint,
                    remote
                        .revision
                        .as_ref()
                        .map(|revision| revision.value.as_str()),
                )?;
                remove_file_if_exists(&verified)?;
                summary.verified += 1;
                live_progress_complete_download();
                tracing::info!(
                    target: crate::core::log_targets::STORAGE,
                    job_id = job.id,
                    path = %relative_path,
                    bytes = remote.size,
                    "verified cloud copy matches local"
                );
                return Ok(());
            }
            remove_file_if_exists(&verified)?;
            action = if upload_allowed {
                // Verification consumed this path's planned download and
                // resolved it as a conflict; count it so the label advances.
                live_progress_complete_download();
                PlanAction::Conflict(super::model::ConflictKind::InitialContentMismatch)
            } else {
                // The replacement below re-downloads and counts on publish.
                PlanAction::DownloadReplace
            };
        }

        match action {
            PlanAction::Noop => {
                if let (Some(local), Some(remote)) = (local_entry, remote_entry) {
                    if remote_observation
                        .fingerprint()
                        .is_some_and(|fingerprint| fingerprint == &local.fingerprint)
                    {
                        self.store.commit_baseline(
                            entry_id,
                            &local.fingerprint,
                            remote
                                .revision
                                .as_ref()
                                .map(|revision| revision.value.as_str()),
                        )?;
                    }
                }
                summary.unchanged += 1;
            }
            PlanAction::UploadNew | PlanAction::UploadReplace { .. } => {
                let local = local_entry
                    .ok_or_else(|| AppError::Backend("upload plan has no local source".into()))?;
                let operation_id = next_operation_id(job.id);
                let snapshot_path = self.create_upload_snapshot(job, local, &operation_id)?;
                let snapshot_fingerprint = local::fingerprint(&snapshot_path)?;
                if snapshot_fingerprint != local.fingerprint {
                    remove_file_if_exists(&snapshot_path)?;
                    return Err(AppError::Backend(format!(
                        "local file changed before upload: {}",
                        local.absolute_path.display()
                    )));
                }
                let (condition, expected, task_action) = match action {
                    PlanAction::UploadNew => (WriteCondition::CreateOnly, None, "upload_new"),
                    PlanAction::UploadReplace { expected } => (
                        WriteCondition::ReplaceIf(expected.clone()),
                        Some(expected),
                        "upload_replace",
                    ),
                    _ => unreachable!(),
                };
                self.store.prepare_task(
                    &operation_id,
                    job.id,
                    entry_id,
                    task_action,
                    expected.as_ref(),
                    &snapshot_path,
                    &snapshot_fingerprint.blake3,
                    job.config_generation,
                    stored.map_or(1, |entry| entry.generation.saturating_add(1)),
                )?;
                let key = remote_key(&job.remote_root, relative_path)?;
                live_progress_begin(SyncLivePhase::Uploading, snapshot_fingerprint.size);
                let uploaded = match provider.upload(&key, &snapshot_path, condition).await {
                    Ok(uploaded) => uploaded,
                    Err(error) => {
                        self.store.set_task_state(
                            &operation_id,
                            "reconciling",
                            Some(&error.to_string()),
                        )?;
                        return Err(provider_error(error));
                    }
                };
                let current = local::fingerprint(&local.absolute_path)?;
                if let Err(error) = self.store.commit_baseline(
                    entry_id,
                    &snapshot_fingerprint,
                    uploaded
                        .revision
                        .as_ref()
                        .map(|revision| revision.value.as_str()),
                ) {
                    self.store.set_task_state(
                        &operation_id,
                        "reconciling",
                        Some(&error.to_string()),
                    )?;
                    return Err(error);
                }
                if current != snapshot_fingerprint {
                    self.store.upsert_observation(
                        job.id,
                        relative_path,
                        Some(&current),
                        None,
                        Some(&snapshot_fingerprint),
                        uploaded
                            .revision
                            .as_ref()
                            .map(|revision| revision.value.as_str()),
                        uploaded
                            .revision
                            .as_ref()
                            .is_some_and(|revision| revision.strength == RevisionStrength::Weak),
                        "pending",
                    )?;
                }
                self.store
                    .set_task_state(&operation_id, "succeeded", None)?;
                remove_file_if_exists(&snapshot_path)?;
                summary.uploaded += 1;
                live_progress_complete_upload();
                tracing::info!(
                    target: crate::core::log_targets::STORAGE,
                    job_id = job.id,
                    path = %relative_path,
                    bytes = snapshot_fingerprint.size,
                    "uploaded to cloud"
                );
            }
            PlanAction::DownloadNew | PlanAction::DownloadReplace => {
                let remote = remote_entry.ok_or_else(|| {
                    AppError::Backend("download plan has no remote source".into())
                })?;
                live_progress_begin(SyncLivePhase::Downloading, remote.size);
                let staged = self
                    .download_to_staging(job, provider, relative_path, remote.revision.as_ref())
                    .await?;
                let fingerprint = local::fingerprint(&staged)?;
                let operation_id = staged
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| AppError::Backend("invalid staging artifact name".into()))?
                    .to_string();
                self.store.prepare_task(
                    &operation_id,
                    job.id,
                    entry_id,
                    if action == PlanAction::DownloadNew {
                        "download_new"
                    } else {
                        "download_replace"
                    },
                    remote.revision.as_ref(),
                    &staged,
                    &fingerprint.blake3,
                    job.config_generation,
                    stored.map_or(1, |entry| entry.generation.saturating_add(1)),
                )?;
                let target = local::destination(&job.local_root, relative_path)?;
                let backup = match action {
                    PlanAction::DownloadNew => {
                        local::atomic_publish_new(&staged, &target)?;
                        None
                    }
                    PlanAction::DownloadReplace => {
                        if let Some(local) = local_entry {
                            let current = local::fingerprint(&local.absolute_path)?;
                            if current != local.fingerprint {
                                remove_file_if_exists(&staged)?;
                                return Err(AppError::Backend(format!(
                                    "local file changed during download: {}",
                                    target.display()
                                )));
                            }
                        }
                        Some(local::atomic_publish_replace(
                            &staged,
                            &target,
                            &operation_id,
                        )?)
                    }
                    _ => unreachable!(),
                };
                let commit: Result<()> = (|| {
                    if let Some(item) = LocalBackend::new_item_from_path(&target)? {
                        if let Some(actor) = &self.actor {
                            actor.execute_blocking(
                                crate::core::db_actor::DbCommand::UpsertMediaBatch {
                                    source: crate::core::events::ChangeSource::Synchronization,
                                    items: vec![item],
                                },
                            )?;
                        } else {
                            LocalBackend::new(self.pool.clone()).upsert(&item)?;
                        }
                    }
                    self.store.commit_baseline(
                        entry_id,
                        &fingerprint,
                        remote
                            .revision
                            .as_ref()
                            .map(|revision| revision.value.as_str()),
                    )?;
                    Ok(())
                })();
                if let Err(error) = commit {
                    self.store.set_task_state(
                        &operation_id,
                        "reconciling",
                        Some(&error.to_string()),
                    )?;
                    if let Some(backup) = &backup {
                        let failed = backup.with_extension("downloaded-uncommitted");
                        let preserve = std::fs::rename(&target, &failed);
                        let restore = std::fs::rename(backup, &target);
                        if let (Err(preserve), Err(restore)) = (preserve, restore) {
                            return Err(AppError::Backend(format!(
                                "download commit failed: {error}; preserving new file failed: {preserve}; restoring old file failed: {restore}; backup remains at {}",
                                backup.display()
                            )));
                        }
                    }
                    return Err(error);
                }
                if let Some(backup) = backup {
                    remove_file_if_exists(&backup)?;
                }
                self.store
                    .set_task_state(&operation_id, "succeeded", None)?;
                remove_file_if_exists(&staged)?;
                summary.downloaded += 1;
                live_progress_complete_download();
                tracing::info!(
                    target: crate::core::log_targets::STORAGE,
                    job_id = job.id,
                    path = %relative_path,
                    bytes = remote.size,
                    "downloaded from cloud"
                );
            }
            PlanAction::Conflict(kind) => {
                self.store.record_conflict(
                    job.id,
                    entry_id,
                    &format!("{kind:?}"),
                    local_entry.map(|entry| &entry.fingerprint),
                    remote_observation.fingerprint(),
                    remote_entry
                        .and_then(|entry| entry.revision.as_ref().map(|r| r.value.as_str())),
                )?;
                summary.conflicts += 1;
            }
            PlanAction::WaitForCompleteObservation => {}
            PlanAction::DeleteLocal | PlanAction::DeleteRemote { .. } => {
                return Err(AppError::Backend(
                    "automatic deletion is not enabled in the first synchronization release".into(),
                ));
            }
            PlanAction::VerifyContent => unreachable!(),
        }
        Ok(())
    }

    async fn download_to_staging(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        expected: Option<&Revision>,
    ) -> Result<PathBuf> {
        let job_dir = self.staging_root.join(job.id.to_string());
        std::fs::create_dir_all(&job_dir)?;
        let path = job_dir.join(format!("{}.download", next_operation_id(job.id)));
        let key = remote_key(&job.remote_root, relative_path)?;
        match provider.download(&key, expected, &path).await {
            Ok(_) => Ok(path),
            Err(error) => {
                let _ = remove_file_if_exists(&path);
                Err(provider_error(error))
            }
        }
    }

    fn create_upload_snapshot(
        &self,
        job: &SyncJob,
        local: &LocalEntry,
        operation_id: &str,
    ) -> Result<PathBuf> {
        let job_dir = self.staging_root.join(job.id.to_string());
        std::fs::create_dir_all(&job_dir)?;
        let target = job_dir.join(format!("{operation_id}.upload"));
        let mut source = std::fs::File::open(&local.absolute_path)?;
        let mut destination = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        std::io::copy(&mut source, &mut destination)?;
        destination.sync_all()?;
        Ok(target)
    }
}

fn ensure_webdav_sync_enabled() -> Result<()> {
    if crate::core::prefs::webdav_sync_enabled() {
        Ok(())
    } else {
        Err(AppError::Backend(
            "WebDAV synchronization is disabled in Settings".into(),
        ))
    }
}

fn optional_local_fingerprint(path: &Path) -> Result<Option<super::model::Fingerprint>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::Backend(format!(
            "refusing to synchronize through a symbolic link: {}",
            path.display()
        ))),
        Ok(metadata) if metadata.is_file() => local::fingerprint(path).map(Some),
        Ok(_) => Err(AppError::Backend(format!(
            "synchronization path is not a regular file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn is_strict_file_prefix(candidate: &Path, complete: &Path) -> Result<bool> {
    let mut prefix = std::fs::File::open(candidate)?;
    let mut source = std::fs::File::open(complete)?;
    let prefix_size = prefix.metadata()?.len();
    if prefix_size >= source.metadata()?.len() {
        return Ok(false);
    }
    let mut left = [0u8; 64 * 1024];
    let mut right = [0u8; 64 * 1024];
    loop {
        let read = prefix.read(&mut left)?;
        if read == 0 {
            return Ok(true);
        }
        source.read_exact(&mut right[..read])?;
        if left[..read] != right[..read] {
            return Ok(false);
        }
    }
}

fn operation_id_from_artifact(path: &Path) -> Result<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::Backend("invalid synchronization artifact name".into()))
}

fn conflict_copy_path(relative_path: &str, conflict_id: i64) -> Result<String> {
    let path = Path::new(relative_path);
    let parent = path.parent().and_then(Path::to_str).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Backend("conflict file name is not valid UTF-8".into()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Backend("conflict file has no supported extension".into()))?;
    let name = format!("{stem}.cloud-conflict-{conflict_id}.{extension}");
    Ok(if parent.is_empty() {
        name
    } else {
        format!("{parent}/{name}")
    })
}

fn publish_new_or_verify(
    staged: &Path,
    target: &Path,
    fingerprint: &super::model::Fingerprint,
) -> Result<()> {
    if target.exists() {
        if optional_local_fingerprint(target)?.as_ref() == Some(fingerprint) {
            return Ok(());
        }
        return Err(AppError::Backend(format!(
            "the conflict copy path is already occupied by different content: {}",
            target.display()
        )));
    }
    local::atomic_publish_new(staged, target)
}

fn verified_task_artifact(task: &StoredTask) -> Result<Option<super::model::Fingerprint>> {
    let Some(fingerprint) = optional_local_fingerprint(&task.artifact_path)? else {
        return Ok(None);
    };
    if fingerprint.blake3 != task.artifact_hash {
        return Ok(None);
    }
    Ok(Some(fingerprint))
}

fn conflict_changed() -> AppError {
    AppError::Backend(
        "the local or cloud version changed after the conflict was recorded; synchronize again and review the new conflict"
            .into(),
    )
}

async fn discover_remote(
    provider: &dyn SyncProvider,
    configured_root: &str,
) -> Result<BTreeMap<String, RemoteEntry>> {
    let root = configured_root.trim_matches('/').to_string();
    let mut queue = VecDeque::from([root.clone()]);
    let mut visited = BTreeSet::new();
    let mut result = BTreeMap::new();
    while let Some(collection) = queue.pop_front() {
        if !visited.insert(collection.clone()) {
            continue;
        }
        let entries = provider
            .list_children(&collection)
            .await
            .map_err(provider_error)?;
        for entry in entries {
            if entry.is_collection {
                relative_remote_key(&root, &entry.key)?;
                queue.push_back(entry.key.trim_matches('/').to_string());
                continue;
            }
            let relative = relative_remote_key(&root, &entry.key)?;
            if crate::core::media::is_supported_media_path(Path::new(&relative)) {
                result.insert(relative, entry);
                if result.len() > 200_000 {
                    return Err(AppError::Backend(
                        "remote synchronization listing exceeds the 200000 object safety limit"
                            .into(),
                    ));
                }
            }
        }
    }
    Ok(result)
}

fn stored_baseline(stored: &StoredEntry) -> Option<Baseline> {
    Some(Baseline {
        fingerprint: stored.baseline.clone()?,
        local_revision: None,
        remote_revision: stored.baseline_remote_revision.clone().map(Revision::etag),
    })
}

fn observed_local(local_entry: Option<&LocalEntry>) -> Observation {
    local_entry.map_or(Observation::Absent, |entry| Observation::Present {
        fingerprint: Some(entry.fingerprint.clone()),
        revision: None,
        size: entry.fingerprint.size,
        modified_unix: Some(entry.modified_ns / 1_000_000_000),
    })
}

fn observed_remote(
    remote_entry: Option<&RemoteEntry>,
    stored: Option<&StoredEntry>,
) -> Observation {
    remote_entry.map_or(Observation::Absent, |entry| {
        let known_fingerprint = stored.and_then(|stored| {
            if stored.remote_revision.as_deref()
                == entry
                    .revision
                    .as_ref()
                    .map(|revision| revision.value.as_str())
            {
                stored.remote.clone()
            } else {
                None
            }
        });
        Observation::Present {
            fingerprint: known_fingerprint,
            revision: entry.revision.clone(),
            size: entry.size,
            modified_unix: entry.modified_unix,
        }
    })
}

fn plan_entry(
    job: &SyncJob,
    relative_path: &str,
    local_observation: Observation,
    remote_observation: Observation,
    stored: Option<&StoredEntry>,
    upload_allowed: bool,
) -> PlanAction {
    let snapshot = EntrySnapshot {
        relative_path: relative_path.into(),
        local: local_observation,
        remote: remote_observation,
        baseline: stored.and_then(stored_baseline),
        direction: job.direction,
        propagate_deletes: job.propagate_deletes,
    };
    if upload_allowed {
        planner::plan(&snapshot)
    } else {
        planner::plan_remote_authoritative(&snapshot)
    }
}

fn remote_key(root: &str, relative: &str) -> Result<String> {
    if relative
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(AppError::Backend(
            "invalid relative synchronization path".into(),
        ));
    }
    let root = root.trim_matches('/');
    Ok(if root.is_empty() {
        relative.into()
    } else {
        format!("{root}/{relative}")
    })
}

fn relative_remote_key(root: &str, key: &str) -> Result<String> {
    let key = key.trim_matches('/');
    if root.is_empty() {
        return Ok(key.into());
    }
    key.strip_prefix(root)
        .and_then(|relative| relative.strip_prefix('/'))
        .map(str::to_string)
        .ok_or_else(|| AppError::Backend(format!("remote object escaped configured root: {key}")))
}

fn upload_allowed(
    scope: UploadScope,
    selected_albums: &BTreeSet<String>,
    relative_path: &str,
) -> bool {
    if scope == UploadScope::All {
        return true;
    }
    let relative_album = relative_path
        .rsplit_once('/')
        .map_or("", |(album, _)| album);
    selected_albums.contains(relative_album)
}

fn next_operation_id(job_id: i64) -> String {
    let sequence = OPERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let session = OPERATION_SESSION.get_or_init(uuid::Uuid::new_v4);
    format!("{job_id}-{session}-{sequence}")
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn action_state(action: &PlanAction) -> &'static str {
    match action {
        PlanAction::Noop => "synced",
        PlanAction::Conflict(_) => "conflict",
        PlanAction::WaitForCompleteObservation => "unknown",
        PlanAction::VerifyContent => "verifying",
        _ => "pending",
    }
}

fn provider_error(error: impl std::fmt::Display) -> AppError {
    AppError::Backend(format!("WebDAV synchronization failed: {error}"))
}

#[cfg(test)]
mod tests;
