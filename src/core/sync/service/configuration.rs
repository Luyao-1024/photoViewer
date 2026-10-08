//! Edit acceptance is durable; the worker, never the editor, owns quiescence.
use super::super::store::{JobEdit, SyncWriteResult};
use super::*;
use std::collections::HashMap;
use std::sync::Weak;

pub(super) type JobKey = (PathBuf, i64);
#[derive(Default)]
struct JobWork {
    gate: tokio::sync::Mutex<()>,
    requested: AtomicBool,
    working: AtomicBool,
    provider: Mutex<Option<Weak<dyn SyncProvider>>>,
    deletion: Mutex<Option<bool>>,
}
static WORK: OnceLock<Mutex<HashMap<JobKey, Arc<JobWork>>>> = OnceLock::new();
pub(super) static GLOBAL_RUN_EPOCH: AtomicU64 = AtomicU64::new(0);

impl SyncService {
    pub(super) fn job_key(&self, id: i64) -> Result<JobKey> {
        let conn = self.pool.get()?;
        let path = conn
            .path()
            .ok_or_else(|| AppError::Backend("sync requires a persistent database".into()))?;
        Ok((PathBuf::from(path), id))
    }

    fn job_work(&self, id: i64) -> Result<Arc<JobWork>> {
        let key = self.job_key(id)?;
        let mut states = WORK
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| AppError::Backend("configuration worker lock poisoned".into()))?;
        Ok(states.entry(key).or_default().clone())
    }

    pub(super) async fn acquire_run_slot(&self, id: i64) -> Result<ActiveJobGuard> {
        let work = self.job_work(id)?;
        let _gate = work.gate.lock().await;
        if self.store.pending_change(id)?.is_some() {
            return Err(AppError::Backend("configuration is being applied".into()));
        }
        ActiveJobGuard::acquire(self.job_key(id)?)
    }

    pub fn has_pending_work(&self, id: i64) -> Result<bool> {
        Ok(self.store.pending_change(id)?.is_some()
            || self.job_work(id)?.working.load(Ordering::Acquire))
    }

    pub fn is_job_running(&self, id: i64) -> Result<bool> {
        Ok(job_is_active(&self.job_key(id)?))
    }

    // A scoped provider seam lets real-shell journeys exercise the production
    // scheduler without opening sockets or accessing the user's keyring.
    #[doc(hidden)]
    pub fn set_provider_for_tests(&self, id: i64, provider: Arc<dyn SyncProvider>) -> Result<()> {
        *self
            .job_work(id)?
            .provider
            .lock()
            .map_err(|_| AppError::Backend("provider lock poisoned".into()))? =
            Some(Arc::downgrade(&provider));
        Ok(())
    }

    pub(super) fn registered_provider(&self, id: i64) -> Result<Option<Arc<dyn SyncProvider>>> {
        let state = self.job_work(id)?;
        let provider = state
            .provider
            .lock()
            .map_err(|_| AppError::Backend("provider lock poisoned".into()))?
            .as_ref()
            .and_then(Weak::upgrade);
        Ok(provider)
    }

    pub(super) async fn provider_for_configuration(
        &self,
        job: &SyncJob,
    ) -> Result<Arc<dyn SyncProvider>> {
        if let Some(provider) = self.registered_provider(job.id)? {
            return Ok(provider);
        }
        let reference = job.credential_ref.clone();
        let password =
            tokio::task::spawn_blocking(move || crate::platform::credentials::load(&reference))
                .await
                .map_err(|e| AppError::Backend(format!("credential task failed: {e}")))??;
        Ok(Arc::new(
            WebDavProvider::new(&job.endpoint, job.username.clone(), password)
                .map_err(provider_error)?,
        ))
    }

    pub fn create_job(&self, request: &super::super::store::NewSyncJob) -> Result<SyncJob> {
        let job = self.store.create_job(request)?;
        *self
            .job_work(job.id)?
            .deletion
            .lock()
            .map_err(|_| AppError::Backend("deletion lock poisoned".into()))? = None;
        self.request_job_sync(job.id)?;
        Ok(job)
    }

    pub async fn set_upload_albums(&self, id: i64, albums: &[String]) -> Result<bool> {
        self.accept_upload_albums(id, albums)
    }

    pub fn accept_upload_albums(&self, id: i64, albums: &[String]) -> Result<bool> {
        let changed = self
            .store
            .queue_edit(id, JobEdit::UploadAlbums(albums.to_vec()))?;
        if changed {
            self.request_job_sync(id)?;
        }
        Ok(changed)
    }

    pub async fn set_remote_root(&self, id: i64, root: &str) -> Result<()> {
        self.accept_remote_root(id, root)
    }

    pub fn accept_remote_root(&self, id: i64, root: &str) -> Result<()> {
        if self
            .store
            .queue_edit(id, JobEdit::RemoteRoot(root.into()))?
        {
            self.request_job_sync(id)?;
        }
        Ok(())
    }

    pub async fn delete_job(&self, id: i64) -> Result<bool> {
        if self.store.get_job(id)?.is_none() {
            return Ok(true);
        }
        if self.store.queue_edit(id, JobEdit::Delete)? {
            *self
                .job_work(id)?
                .deletion
                .lock()
                .map_err(|_| AppError::Backend("deletion lock poisoned".into()))? = None;
        }
        self.request_job_sync(id)?;
        let work = self.job_work(id)?;
        loop {
            if let Some(result) = *work
                .deletion
                .lock()
                .map_err(|_| AppError::Backend("deletion lock poisoned".into()))?
            {
                return Ok(result);
            }
            if !work.working.load(Ordering::Acquire) {
                if let Some(job) = self.store.get_job(id)? {
                    if let Some(error) = job.last_error {
                        return Err(AppError::Backend(error));
                    }
                } else {
                    return Ok(true);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Called only after the global switch has actually changed and persisted.
    pub fn sync_enabled_changed(&self) {
        GLOBAL_RUN_EPOCH.fetch_add(1, Ordering::AcqRel);
        let service = self.clone();
        tokio::spawn(async move {
            match service.store.list_jobs() {
                Ok(jobs) => {
                    for job in jobs {
                        if let Err(error) = service.request_job_sync(job.id) {
                            tracing::warn!(
                                job_id = job.id,
                                "cannot refresh sync configuration: {error}"
                            );
                        }
                    }
                }
                Err(error) => tracing::warn!("cannot enumerate saved sync tasks: {error}"),
            }
        });
    }

    pub fn request_job_sync(&self, id: i64) -> Result<()> {
        let work = self.job_work(id)?;
        work.requested.store(true, Ordering::Release);
        if !work.working.swap(true, Ordering::AcqRel) {
            let service = self.clone();
            tokio::spawn(async move {
                service.configuration_worker(id, work).await;
            });
        }
        Ok(())
    }

    async fn configuration_worker(&self, id: i64, work: Arc<JobWork>) {
        loop {
            work.requested.store(false, Ordering::Release);
            tokio::time::sleep(Duration::from_millis(120)).await;
            let revision = self
                .store
                .pending_change(id)
                .ok()
                .flatten()
                .map(|p| p.revision);
            let result = self.apply_pending_job(id).await;
            match result {
                Err(error) => {
                    // A failure from an older transition cannot fail a newer edit.
                    let latest = self
                        .store
                        .pending_change(id)
                        .ok()
                        .flatten()
                        .map(|p| p.revision);
                    if latest != revision || work.requested.load(Ordering::Acquire) {
                        continue;
                    }
                    if let Some(revision) = revision {
                        let _ = self
                            .store
                            .fail_pending_edit(id, revision, &error.to_string());
                    }
                    tracing::warn!(job_id = id, "could not apply sync edit: {error}");
                }
                Ok(true) => break, // confirmed deletion; no resurrection
                Ok(false) => {
                    work.requested.store(false, Ordering::Release);
                    if self.store.pending_change(id).ok().flatten().is_some() {
                        continue;
                    }
                    if crate::core::prefs::webdav_sync_enabled()
                        && self.store.get_job(id).ok().flatten().is_some()
                    {
                        live_progress_begin_session();
                        let result = self.run_saved_job(id).await;
                        live_progress_end_session();
                        if let Err(error) = result {
                            if self.store.pending_change(id).ok().flatten().is_none()
                                && crate::core::prefs::webdav_sync_enabled()
                            {
                                if let Ok(Some(job)) = self.store.get_job(id) {
                                    let _ = self.store.finish_run(
                                        id,
                                        job.config_generation,
                                        Some(&error.to_string()),
                                    );
                                }
                            }
                            tracing::warn!(
                                job_id = id,
                                "latest-configuration synchronization failed: {error}"
                            );
                        }
                    }
                }
            }
            if work.requested.load(Ordering::Acquire) {
                continue;
            }
            work.working.store(false, Ordering::Release);
            // Close the final accept/exit race without losing an edit.
            if work.requested.load(Ordering::Acquire) && !work.working.swap(true, Ordering::AcqRel)
            {
                continue;
            }
            return;
        }
        work.working.store(false, Ordering::Release);
    }

    pub(super) async fn apply_pending_job(&self, id: i64) -> Result<bool> {
        let work = self.job_work(id)?;
        let _gate = work.gate.lock().await;
        while self.is_job_running(id)? {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let Some(job) = self.store.get_job(id)? else {
            return Ok(true);
        };
        let Some(pending) = self.store.pending_change(id)? else {
            if crate::core::prefs::webdav_sync_enabled() {
                self.store.set_job_paused(id, false)?;
            }
            return Ok(false);
        };
        // This is after durable edit acceptance, not an editing precondition.
        self.store.set_job_paused(id, true)?;
        let root_changed = pending
            .remote_root
            .as_ref()
            .is_some_and(|root| root != &job.remote_root);
        let artifacts = if root_changed || pending.delete {
            self.store.artifact_paths(id)?
        } else {
            Vec::new()
        };
        if (root_changed || pending.delete) && !self.store.unfinished_tasks(id)?.is_empty() {
            if !crate::core::prefs::webdav_sync_enabled() {
                return Err(AppError::Backend("old file recovery must finish before switching or removing this task; synchronization is disabled".into()));
            }
            let provider = self.provider_for_configuration(&job).await?;
            let mut recovery_job = job.clone();
            if pending.upload_albums.is_some() {
                recovery_job.upload_scope = UploadScope::SelectedAlbums;
            }
            for task in self.store.unfinished_tasks(id)? {
                if task.action.starts_with("upload_") {
                    self.recover_upload_task(&recovery_job, provider.as_ref(), &task)
                        .await?;
                } else if task.action.starts_with("download_") {
                    self.recover_download_task(&recovery_job, provider.as_ref(), &task)
                        .await?;
                } else {
                    self.recover_resolution_task(&recovery_job, provider.as_ref(), &task)
                        .await?;
                }
            }
            if !self.store.unfinished_tasks(id)?.is_empty() {
                return Err(AppError::Backend("old file outcome is not proven; its recovery snapshot and accepted edit were retained".into()));
            }
        }
        if pending.conflict.is_some() && !crate::core::prefs::webdav_sync_enabled() {
            return Err(AppError::Backend(
                "accepted conflict choice retained until synchronization is enabled".into(),
            ));
        }
        if let Some((conflict, choice)) = &pending.conflict {
            if self.store.get_open_conflict(conflict.id)?.is_some() {
                let resolution = match choice.as_str() {
                    "use_local" => ConflictResolution::UseLocal,
                    "use_remote" => ConflictResolution::UseRemote,
                    "keep_both" => ConflictResolution::KeepBoth,
                    _ => return Err(AppError::Backend("invalid pending conflict choice".into())),
                };
                let provider = self.provider_for_configuration(&job).await?;
                self.resolve_with_provider(&job, conflict.clone(), resolution, provider)
                    .await?;
            }
        }
        match self.store.apply_change(id, pending.revision)? {
            SyncWriteResult::DeletedJob(reference) => {
                self.cleanup_staging_artifacts(artifacts);
                let clean = if self.registered_provider(id)?.is_some() {
                    true
                } else if let Some(reference) = reference {
                    matches!(
                        tokio::task::spawn_blocking(move || crate::platform::credentials::delete(
                            &reference
                        ))
                        .await,
                        Ok(Ok(()))
                    )
                } else {
                    true
                };
                *work
                    .deletion
                    .lock()
                    .map_err(|_| AppError::Backend("deletion lock poisoned".into()))? = Some(clean);
                Ok(true)
            }
            SyncWriteResult::Changed(applied) => {
                if applied && root_changed {
                    self.cleanup_staging_artifacts(artifacts);
                }
                Ok(false)
            }
            _ => Err(AppError::Backend(
                "unexpected pending configuration result".into(),
            )),
        }
    }

    pub async fn resolve_saved_conflict(
        &self,
        id: i64,
        resolution: ConflictResolution,
    ) -> Result<()> {
        ensure_webdav_sync_enabled()?;
        let conflict = self.store.get_open_conflict(id)?.ok_or_else(|| {
            AppError::Backend("synchronization conflict is no longer open".into())
        })?;
        let job = self
            .store
            .get_job(conflict.job_id)?
            .ok_or_else(|| AppError::Backend("synchronization job no longer exists".into()))?;
        let albums = self
            .store
            .desired_upload_albums(job.id)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let scope = if self
            .store
            .pending_change(job.id)?
            .is_some_and(|p| p.upload_albums.is_some())
        {
            UploadScope::SelectedAlbums
        } else {
            job.upload_scope
        };
        if !upload_allowed(scope, &albums, &conflict.relative_path)
            && resolution != ConflictResolution::UseRemote
        {
            return Err(AppError::Backend(
                "this album is not selected for upload".into(),
            ));
        }
        let current = optional_local_fingerprint(&local::destination(
            &job.local_root,
            &conflict.relative_path,
        )?)?;
        if current.as_ref().map(|fp| fp.blake3.as_str()) != conflict.local_fingerprint.as_deref() {
            return Err(conflict_changed());
        }
        let provider = self.provider_for_configuration(&job).await?;
        let remote = provider
            .stat(&remote_key(&job.remote_root, &conflict.relative_path)?)
            .await
            .map_err(provider_error)?;
        if remote
            .as_ref()
            .and_then(|r| r.revision.as_ref())
            .map(|r| r.value.as_str())
            != conflict.remote_revision.as_deref()
        {
            return Err(conflict_changed());
        }
        self.store.queue_edit(
            job.id,
            JobEdit::Conflict(conflict, resolution.as_str().into()),
        )?;
        self.request_job_sync(job.id)?;
        while self.store.get_open_conflict(id)?.is_some() {
            if !self.job_work(job.id)?.working.load(Ordering::Acquire) {
                return Err(AppError::Backend(
                    self.store
                        .get_job(job.id)?
                        .and_then(|j| j.last_error)
                        .unwrap_or_else(|| "conflict choice could not be applied".into()),
                ));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok(())
    }
}
