//! One local-change listener per application database; UI gestures use the same worker.
use super::*;
use crate::core::sync::SyncClass;

/// Whether an unattended scheduler should start another cycle after a failure.
///
/// User-action failures suppress automatic local-change requests and immediate
/// continuation batches. Explicit synchronization can still retry them.
pub(super) fn unattended_retry_is_worthwhile(cause: Option<crate::core::sync::SyncCause>) -> bool {
    !cause.is_some_and(|cause| cause.class() == SyncClass::NeedsUser)
}

impl SyncService {
    pub(super) async fn index_upload_sources(&self, job: &SyncJob) -> Result<()> {
        let roots = if job.upload_scope == UploadScope::All {
            vec![job.local_root.clone()]
        } else {
            self.store
                .upload_albums(job.id)?
                .iter()
                .map(|a| job.local_root.join(a))
                .collect()
        };
        let service = self.clone();
        let job = job.clone();
        tokio::task::spawn_blocking(move || {
            let backend = crate::core::backend::local::LocalBackend::new(service.pool().clone());
            let excluded = crate::core::prefs::excluded_scan_roots();
            let canonical = std::fs::canonicalize(&job.local_root)?;
            for root in roots {
                if !root.is_dir() {
                    continue;
                }
                if !std::fs::canonicalize(&root)?.starts_with(&canonical) {
                    return Err(AppError::Backend(
                        "album escaped the configured root".into(),
                    ));
                }
                backend.scan_and_submit_dir_notify_with_exclusions(
                    &root,
                    &excluded,
                    |items| {
                        if !service.store.job_is_selected(job.id)?
                            || service.store.pending_change(job.id)?.is_some()
                            || service.store.reset_pending()?
                        {
                            return Err(AppError::Backend(
                                "configuration changed while scanning".into(),
                            ));
                        }
                        service.store.commit_local_scan(items)
                    },
                    |_| {},
                )?;
            }
            Ok::<_, AppError>(())
        })
        .await
        .map_err(|_| AppError::Backend("album scan worker stopped".into()))?
    }

    /// Request cycles after local changes have settled.
    ///
    /// Failures requiring user action stay suppressed until an explicit request
    /// succeeds and clears them. There is no periodic retry.
    pub fn request_scheduled_sync(&self) -> Result<usize> {
        self.store.normalize_accounts()?;
        if !crate::core::prefs::sync_enabled() || self.store.reset_pending()? {
            return Ok(0);
        }
        self.store.resume_local_changes()?;
        let mut asked = 0;
        for job in self.store.list_jobs()? {
            if job.paused {
                continue;
            }
            if !unattended_retry_is_worthwhile(self.store.failure_cause(job.id)?) {
                continue;
            }
            if !self.has_pending_work(job.id)? && !self.is_job_running(job.id)? {
                self.request_job_sync(job.id)?;
                asked += 1;
            }
        }
        Ok(asked)
    }

    /// Finish durable work a completed run left behind, once per launch.
    ///
    /// This is not a poll: it asks at most once per job, and only when an entry
    /// is still dirty or a receive-side resource is unresolved. Without it, a
    /// process that ended between chained batches would leave admissible work
    /// stranded until an unrelated local change.
    pub fn resume_unfinished_work(&self) -> Result<usize> {
        self.store.normalize_accounts()?;
        if !crate::core::prefs::sync_enabled() || self.store.reset_pending()? {
            return Ok(0);
        }
        self.store.resume_local_changes()?;
        let mut asked = 0;
        for job in self.store.list_jobs()? {
            if job.paused
                || !unattended_retry_is_worthwhile(self.store.failure_cause(job.id)?)
                || !self.store.has_unfinished_work(job.id)?
            {
                continue;
            }
            if !self.has_pending_work(job.id)? && !self.is_job_running(job.id)? {
                self.request_job_sync(job.id)?;
                asked += 1;
            }
        }
        if asked > 0 {
            tracing::info!(
                target: crate::core::log_targets::SYNC,
                jobs = asked,
                "启动时恢复未完成的云同步工作"
            );
        }
        Ok(asked)
    }

    pub fn request_all_sync(&self) -> Result<()> {
        self.store.normalize_accounts()?;
        if !crate::core::prefs::sync_enabled() || self.store.reset_pending()? {
            return Ok(());
        }
        self.store.resume_local_changes()?;
        for job in self.store.list_jobs()? {
            if !job.paused && !self.has_pending_work(job.id)? && !self.is_job_running(job.id)? {
                self.request_job_sync(job.id)?;
            }
        }
        Ok(())
    }

    /// Listen for local changes and request a cycle after a 10-second quiet period.
    /// This listener never fires on startup or idle time; `resume_unfinished_work`
    /// owns the one-shot launch resume. Failures wait for another change or an
    /// explicit trigger rather than arming a periodic retry.
    pub fn start_scheduler(&self) {
        static DATABASES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
        let Ok((database, _)) = self.job_key(0) else {
            return;
        };
        let Ok(mut databases) = DATABASES.get_or_init(|| Mutex::new(HashSet::new())).lock() else {
            return;
        };
        if !databases.insert(database) {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                crate::core::sync::debounce::settled(crate::core::sync::debounce::QUIET_PERIOD)
                    .await;
                if let Err(error) = service.request_scheduled_sync() {
                    tracing::warn!("cannot schedule native synchronization: {error}");
                }
            }
        });
    }
}
