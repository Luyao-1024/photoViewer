//! One bounded poller per application database; UI gestures use the same worker.
use super::*;

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

    pub fn request_all_sync(&self) -> Result<()> {
        self.store.normalize_accounts()?;
        if !crate::core::prefs::sync_enabled() || self.store.reset_pending()? {
            return Ok(());
        }
        for job in self.store.list_jobs()? {
            if !job.paused && !self.has_pending_work(job.id)? && !self.is_job_running(job.id)? {
                self.request_job_sync(job.id)?;
            }
        }
        Ok(())
    }

    /// Startup + bounded automatic retries also observe external/local edits.
    /// Coalescing prevents overlapping cycles and unbounded request backlogs.
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
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(error) = service.request_all_sync() {
                    tracing::warn!("cannot schedule native synchronization: {error}");
                }
            }
        });
    }
}
