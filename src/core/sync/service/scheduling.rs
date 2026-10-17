//! One bounded poller per application database; UI gestures use the same worker.
use super::*;
use crate::core::sync::SyncClass;

/// Whether an unattended scheduler should start another cycle after a failure.
///
/// The whole policy in one place, so every cause is answered on purpose rather
/// than by whichever branch happened to be written first. Anything the service
/// will refuse again until the user does something is not worth asking on a
/// timer: the answer is already known, and the cycle it consumes is one the rest
/// of the library is not getting.
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

    /// Ask for cycles the way the scheduler does, on a timer rather than on a
    /// gesture.
    ///
    /// A failure the service will refuse again until the user acts is not
    /// retried here. A full account answers every attempt with the same
    /// rejection, so a timer that keeps knocking spends a lease, a round trip
    /// and a cycle every interval to learn nothing — and the cycle it burns is
    /// one the rest of the library is not getting. The job stays exactly as it
    /// was, and it recovers the moment the cause is gone: a run that succeeds
    /// clears the recorded failure, and a user pressing "sync now" goes through
    /// `request_all_sync`, which does not consult this.
    pub fn request_scheduled_sync(&self) -> Result<usize> {
        self.store.normalize_accounts()?;
        if !crate::core::prefs::sync_enabled() || self.store.reset_pending()? {
            return Ok(0);
        }
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

    /// Two things decide when a cycle starts, and they answer different questions.
    ///
    /// The poller below is the floor: it runs on a fixed cadence regardless of
    /// whether anything changed, which is what retries work that did not finish
    /// and re-checks an endpoint that may have come back. It deliberately knows
    /// nothing about what triggered it.
    ///
    /// The debounced task is the responsiveness half: a change to the library
    /// restarts a quiet period and starts a cycle as soon as the changes stop,
    /// instead of waiting for the poller to come around.
    ///
    /// Both funnel into `request_scheduled_sync`, which coalesces them — a quiet
    /// period that expires while the poller already asked still costs one cycle,
    /// not two — and which is also where a failure that only the user can clear
    /// stops the timer from asking again.
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
                if let Err(error) = service.request_scheduled_sync() {
                    tracing::warn!("cannot schedule native synchronization: {error}");
                }
            }
        });
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
