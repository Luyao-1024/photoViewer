//! Edit acceptance is durable; the worker, never the editor, owns quiescence.
use super::super::store::{JobEdit, SyncWriteResult};
use super::*;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub(super) type JobKey = (PathBuf, i64);

/// Per-job configuration state.
///
/// `requested`/`working` coalesce rapid edits so a burst of accepted changes
/// applies the latest one instead of restarting per notification; `deletion`
/// carries the confirmed result of a task removal back to the caller.
#[derive(Default)]
struct JobWork {
    gate: tokio::sync::Mutex<()>,
    requested: AtomicBool,
    working: AtomicBool,
    deletion: Mutex<Option<bool>>,
}
static WORK: OnceLock<Mutex<HashMap<JobKey, Arc<JobWork>>>> = OnceLock::new();
pub(super) static GLOBAL_RUN_EPOCH: AtomicU64 = AtomicU64::new(0);

/// How many cycles one worker may chain back-to-back before handing the job
/// back to the next change or explicit trigger.
///
/// Chaining drains a large library without requiring a new change per batch.
/// Left open it is a tight loop with no delay at all, which is the wrong shape
/// for anything that can fail repeatedly: the worker holds its slot, and the
/// rest of the app starves behind it.
const MAX_CHAINED_CYCLES: usize = 8;

/// The fairness yield between bounded chains.
///
/// Reaching the cap is not a reason to strand work the service already agreed
/// to finish: the worker releases its slot, and one short delay later it asks
/// for another. A cycle that reported failures never arms this, so the delay
/// never becomes a timed retry.
const CONTINUATION_YIELD: Duration = Duration::from_secs(1);

impl SyncService {
    pub(super) fn job_key(&self, id: i64) -> Result<JobKey> {
        let conn = self.pool().get()?;
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

    /// Claim the job for one cycle. Ordering matters: the reset barrier is taken
    /// first so a global Off is never overtaken by a job that starts behind it,
    /// then the per-job gate, then the re-entrancy guard.
    pub(super) async fn acquire_run_slot(&self, id: i64) -> Result<ActiveJobGuard> {
        self.acquire_slot(id, true).await
    }

    pub(super) async fn acquire_authentication_slot(&self, id: i64) -> Result<ActiveJobGuard> {
        self.acquire_slot(id, false).await
    }

    async fn acquire_slot(&self, id: i64, require_selected: bool) -> Result<ActiveJobGuard> {
        let reset = self.reset_work()?;
        let _database_gate = reset.gate.lock().await;
        if self.store.reset_pending()? {
            return Err(AppError::Backend(
                "synchronization history is being cleared".into(),
            ));
        }
        self.store.normalize_accounts()?;
        if require_selected {
            self.require_selected_account(id)?;
        }
        let work = self.job_work(id)?;
        let _gate = work.gate.lock().await;
        if self.store.pending_change(id)?.is_some() {
            return Err(AppError::Backend("configuration is being applied".into()));
        }
        ActiveJobGuard::acquire(self.job_key(id)?)
    }

    /// Whether another cycle may start without waiting for a trigger.
    ///
    /// A cycle that stopped for a reason the user has to clear gets no
    /// follow-up, however much work is left: the service will refuse exactly
    /// the same way, and each attempt spends a lease and a round trip to learn
    /// nothing. The remaining work is not lost — it stays dirty, and the job
    /// resumes as soon as the cause is gone.
    fn may_chain_another_cycle(&self, id: i64, chained: usize) -> Result<bool> {
        if chained >= MAX_CHAINED_CYCLES {
            return Ok(false);
        }
        Ok(super::scheduling::unattended_retry_is_worthwhile(
            self.store.failure_cause(id)?,
        ))
    }

    /// Resume a chain that stopped only to yield the worker slot.
    ///
    /// The remaining work was already admitted by a cycle that made progress
    /// without failures, so this is a continuation, not a retry: the job keeps
    /// draining instead of waiting for an unrelated local change.
    fn schedule_continuation(&self, id: i64, outstanding: usize) {
        tracing::info!(
            target: crate::core::log_targets::SYNC,
            job_id = id,
            outstanding,
            delay_ms = CONTINUATION_YIELD.as_millis() as u64,
            "云同步批次达到连续上限，让出调度后继续"
        );
        let service = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(CONTINUATION_YIELD).await;
            if !crate::core::prefs::sync_enabled() || service.store.reset_pending().unwrap_or(true)
            {
                return;
            }
            let resumable = service
                .store
                .get_job(id)
                .ok()
                .flatten()
                .is_some_and(|job| !job.paused);
            if resumable {
                if let Err(error) = service.request_job_sync(id) {
                    tracing::warn!(
                        target: crate::core::log_targets::SYNC,
                        job_id = id, error = %error,
                        "不能继续云同步批次"
                    );
                }
            }
        });
    }

    pub fn has_pending_work(&self, id: i64) -> Result<bool> {
        Ok(self.reset_is_working()?
            || self.store.reset_pending()?
            || self.store.pending_change(id)?.is_some()
            || self.job_work(id)?.working.load(Ordering::Acquire))
    }

    pub fn is_job_running(&self, id: i64) -> Result<bool> {
        Ok(job_is_active(&self.job_key(id)?))
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

    pub fn accept_direction(&self, id: i64, direction: SyncDirection) -> Result<bool> {
        let changed = self.store.queue_edit(id, JobEdit::Direction(direction))?;
        if changed {
            self.request_job_sync(id)?;
        }
        Ok(changed)
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

    /// Removing a task is itself an edit: it is queued durably and applied by the
    /// same worker, so a delete can never race an in-flight operation into a
    /// half-removed job.
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
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// Called after the global switch has persisted. Off starts a durable history
    /// reset; enabling during cleanup waits for the same reset barrier.
    pub fn sync_enabled_changed(&self) -> Result<()> {
        GLOBAL_RUN_EPOCH.fetch_add(1, Ordering::AcqRel);
        if !crate::core::prefs::sync_enabled() {
            self.store.begin_reset()?;
            return self.start_reset_worker();
        }
        if self.store.reset_pending()? {
            return self.start_reset_worker();
        }
        self.store.resume_local_changes()?;
        for job in self.store.list_jobs()? {
            self.request_job_sync(job.id)?;
        }
        Ok(())
    }

    /// Drop every queued request for this database. Called once a global reset
    /// has finished, so a stale trigger cannot resurrect discarded work.
    pub(super) fn discard_queued_work(&self) -> Result<()> {
        let database = self.job_key(0)?.0;
        if let Some(work) = WORK.get() {
            let work = work
                .lock()
                .map_err(|_| AppError::Backend("configuration worker lock poisoned".into()))?;
            for (_, state) in work.iter().filter(|(key, _)| key.0 == database) {
                state.requested.store(false, Ordering::Release);
                *state
                    .deletion
                    .lock()
                    .map_err(|_| AppError::Backend("deletion lock poisoned".into()))? = None;
            }
        }
        Ok(())
    }

    /// Ask the configuration worker to apply this job's latest accepted edit.
    ///
    /// Viewing and drafting never call this: only an accepted effective change
    /// does, which is what keeps an open form from pausing a running job.
    pub fn request_job_sync(&self, id: i64) -> Result<()> {
        if self.store.reset_pending()? {
            return self.start_reset_worker();
        }
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

    /// Coalesce a burst of accepted edits, then apply the latest one.
    ///
    /// The drain loop re-checks `requested` after every applied edit so an edit
    /// accepted *during* the transition is not lost; the final close accepts an
    /// edit that landed between the last check and the `working` release.
    async fn configuration_worker(&self, id: i64, work: Arc<JobWork>) {
        loop {
            let coalesce_pending_edit = self
                .store
                .pending_change(id)
                .map(|pending| pending.is_some())
                .unwrap_or(true);
            if coalesce_pending_edit {
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            } else {
                tokio::task::yield_now().await;
            }
            if !work.requested.swap(false, Ordering::AcqRel) {
                break;
            }
            if let Err(error) = self.apply_pending_job(id).await {
                tracing::warn!("cannot apply an accepted synchronization edit: {error}");
                // Attribute the failure to the edit that is still queued, so the
                // user's choice is retained rather than dropped on the floor.
                if let Ok(Some(pending)) = self.store.pending_change(id) {
                    let _ = self
                        .store
                        .fail_pending_edit(id, pending.revision, &error.to_string());
                }
                continue;
            }
            if crate::core::prefs::sync_enabled() && !self.store.reset_pending().unwrap_or(true) {
                if let Ok(Some(job)) = self.store.get_job(id) {
                    if !job.paused {
                        match self.run_cycle(id).await {
                            Ok(report) => {
                                if report.caught_up_behind_lock {
                                    // A store read that fails stops the chain:
                                    // not knowing why the cycle stopped is no
                                    // reason to keep starting them.
                                    if self.may_chain_another_cycle(id, 0).unwrap_or(false) {
                                        work.requested.store(true, Ordering::Release);
                                    }
                                } else {
                                    let mut outstanding = report.outstanding;
                                    let mut chained = 0usize;
                                    let mut failure_free = report.failures.is_empty();
                                    let mut chain = (report.mutations_sent > 0
                                        || report.mutations_confirmed > 0)
                                        && outstanding > 0;
                                    while chain {
                                        if chained >= MAX_CHAINED_CYCLES {
                                            // `chain` is still true here: the
                                            // last cycle made progress and work
                                            // remains. Failures keep the work
                                            // waiting for the next trigger.
                                            let retry_worthwhile = self
                                                .store
                                                .failure_cause(id)
                                                .map(|cause| {
                                                    super::scheduling::unattended_retry_is_worthwhile(
                                                        cause,
                                                    )
                                                })
                                                .unwrap_or(false);
                                            if failure_free && retry_worthwhile {
                                                self.schedule_continuation(id, outstanding);
                                            }
                                            break;
                                        }
                                        if !self
                                            .may_chain_another_cycle(id, chained)
                                            .unwrap_or(false)
                                        {
                                            break;
                                        }
                                        chained += 1;
                                        match self.run_uplink_cycle(id).await {
                                            Ok(report) => {
                                                if report.caught_up_behind_lock {
                                                    if self
                                                        .may_chain_another_cycle(id, chained)
                                                        .unwrap_or(false)
                                                    {
                                                        work.requested
                                                            .store(true, Ordering::Release);
                                                    }
                                                    break;
                                                }
                                                outstanding = report.outstanding;
                                                failure_free = report.failures.is_empty();
                                                chain = (report.mutations_sent > 0
                                                    || report.mutations_confirmed > 0)
                                                    && outstanding > 0;
                                            }
                                            Err(error) => {
                                                tracing::warn!(
                                                    "synchronization uplink cycle failed: {error}"
                                                );
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::warn!("synchronization cycle failed: {error}");
                            }
                        }
                    }
                }
            }
        }
        work.working.store(false, Ordering::Release);
        // An edit accepted between the last drain check and the release above
        // would otherwise wait for an unrelated later trigger.
        if work.requested.load(Ordering::Acquire) {
            self.request_job_sync(id).ok();
        }
    }

    /// Apply this job's queued edit, if any, holding the configuration
    /// transition. Returns whether a task removal completed.
    ///
    /// This runs after an edit has already been accepted and recorded durably,
    /// never as a precondition for editing. It pauses the job, waits out any
    /// in-flight cycle, then installs the latest desired values and lets the
    /// scheduler run them.
    pub(super) async fn apply_pending_job(&self, id: i64) -> Result<bool> {
        let Some(_transition) = self.acquire_configuration_transition().await? else {
            return Ok(false);
        };
        let work = self.job_work(id)?;
        let _gate = work.gate.lock().await;
        while self.is_job_running(id)? {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        if self.store.reset_pending()? {
            return Ok(false);
        }
        let Some(_job) = self.store.get_job(id)? else {
            return Ok(true);
        };
        let Some(pending) = self.store.pending_change(id)? else {
            return Ok(false);
        };
        // An accepted edit stops the old run here; pausing first is what makes
        // the transition safe without ever requiring a manual resume.
        self.store.set_job_paused(id, true)?;
        match self.store.apply_change(id, pending.revision)? {
            SyncWriteResult::DeletedJob(reference) => {
                // A removed task must take its credential with it, otherwise a
                // secret outlives every row that referenced it.
                let clean = match reference {
                    Some(reference) => matches!(
                        tokio::task::spawn_blocking(move || {
                            crate::platform::credentials::delete_api_key(&reference)
                        })
                        .await,
                        Ok(Ok(()))
                    ),
                    None => true,
                };
                *work
                    .deletion
                    .lock()
                    .map_err(|_| AppError::Backend("deletion lock poisoned".into()))? = Some(clean);
                Ok(true)
            }
            SyncWriteResult::Changed(_) => Ok(false),
            _ => Err(AppError::Backend(
                "unexpected pending configuration result".into(),
            )),
        }
    }
}
