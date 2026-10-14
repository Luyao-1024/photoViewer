//! Global off is a durable reset barrier, not a reversible scheduling pause.
use super::*;
use crate::core::sync::remote_path::is_internal_sync_name;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Default)]
pub(super) struct ResetWork {
    pub gate: AsyncMutex<()>,
    working: AtomicBool,
    transitions: AtomicUsize,
}

pub(super) struct ConfigurationTransition(Arc<ResetWork>);
impl Drop for ConfigurationTransition {
    fn drop(&mut self) {
        self.0.transitions.fetch_sub(1, Ordering::AcqRel);
    }
}
static RESETS: OnceLock<Mutex<HashMap<PathBuf, Arc<ResetWork>>>> = OnceLock::new();

impl SyncService {
    pub(super) fn reset_work(&self) -> Result<Arc<ResetWork>> {
        let path = self.job_key(0)?.0;
        let mut work = RESETS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| AppError::Backend("sync reset lock poisoned".into()))?;
        Ok(work.entry(path).or_default().clone())
    }

    pub(super) async fn acquire_configuration_transition(
        &self,
    ) -> Result<Option<ConfigurationTransition>> {
        let work = self.reset_work()?;
        let _gate = work.gate.lock().await;
        if self.store.reset_pending()? {
            return Ok(None);
        }
        work.transitions.fetch_add(1, Ordering::AcqRel);
        Ok(Some(ConfigurationTransition(work.clone())))
    }

    pub(super) fn reset_is_working(&self) -> Result<bool> {
        Ok(self.reset_work()?.working.load(Ordering::Acquire))
    }

    pub(super) fn start_reset_worker(&self) -> Result<()> {
        let work = self.reset_work()?;
        if !work.working.swap(true, Ordering::AcqRel) {
            let service = self.clone();
            tokio::spawn(async move {
                let result = service.perform_sync_reset().await;
                match result {
                    // A newer opt-in may have landed during cleanup. It stays
                    // queued in `sync_job_changes`, so the next trigger picks it
                    // up without losing the user's accepted edit.
                    Ok(()) => {}
                    Err(error) => {
                        tracing::warn!("cannot clear synchronization state: {error}")
                    }
                }
                // Stay pending until a newer opt-in has actually been queued.
                work.working.store(false, Ordering::Release);
                // A newer close can arrive while the previous worker exits. A
                // recorded failure waits for retry instead of spinning forever.
                if service
                    .store
                    .reset_intent()
                    .ok()
                    .flatten()
                    .is_some_and(|intent| intent.error.is_none())
                {
                    if let Err(error) = service.start_reset_worker() {
                        tracing::warn!("cannot continue a newer synchronization reset: {error}");
                    }
                }
            });
        }
        Ok(())
    }

    pub(super) async fn perform_sync_reset(&self) -> Result<()> {
        let work = self.reset_work()?;
        let _gate = work.gate.lock().await;
        let database = self.job_key(0)?.0;
        while let Some(intent) = self.store.reset_intent()? {
            loop {
                let active = ACTIVE_JOBS
                    .get_or_init(|| Mutex::new(HashSet::new()))
                    .lock()
                    .map_err(|_| AppError::Backend("active sync lock poisoned".into()))?
                    .iter()
                    .any(|key| key.0 == database);
                if !active && work.transitions.load(Ordering::Acquire) == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let staging = self.staging_root().to_path_buf();
            let roots = self.store.reset_local_roots()?;
            let cleanup = match tokio::task::spawn_blocking(move || {
                let owned_staging =
                    std::fs::canonicalize(&staging).unwrap_or_else(|_| staging.clone());
                if roots.iter().any(|root| root.starts_with(&owned_staging)) {
                    return Err(AppError::Backend(
                        "sync staging overlaps a configured media source".into(),
                    ));
                }
                clear_local_temporaries(&roots)?;
                clear_staging(&staging)
            })
            .await
            {
                Ok(result) => result,
                Err(error) => Err(AppError::Backend(format!(
                    "sync staging cleanup failed: {error}"
                ))),
            };
            if let Err(error) = cleanup {
                if self
                    .store
                    .reset_intent()?
                    .is_some_and(|latest| latest.token != intent.token)
                {
                    continue;
                }
                self.store.fail_reset(&intent.token, &error.to_string())?;
                return Err(error);
            }
            match self.store.finish_reset(&intent.token) {
                Ok(true) => {
                    self.discard_queued_work()?;
                }
                Ok(false) => {}
                Err(error) => {
                    self.store.fail_reset(&intent.token, &error.to_string())?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

fn clear_staging(root: &Path) -> Result<()> {
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(root).map_err(AppError::from),
        Ok(_) => Err(AppError::Backend(
            "sync staging root is not an owned directory".into(),
        )),
    }
}

/// Local publication also owns sibling temporary files in the reserved
/// .photoviewer-sync-* namespace. Ordinary media and directory symlink targets
/// never participate in this explicit cleanup.
fn clear_local_temporaries(roots: &BTreeSet<PathBuf>) -> Result<()> {
    for root in roots {
        match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(AppError::Backend(
                    "sync media root is no longer an owned directory".into(),
                ))
            }
        }
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = entry.map_err(|error| {
                AppError::Backend(format!("cannot inspect local sync temporaries: {error}"))
            })?;
            if !entry.file_type().is_dir() && is_internal_sync_name(entry.path()) {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}
