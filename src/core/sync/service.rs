use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

mod configuration;
pub mod coordinator;
pub mod receive;
mod reset;
pub mod transfer;
use configuration::JobKey;

use crate::core::error::{AppError, Result};

use super::model::{SyncDirection, UploadScope};
use super::native::protocol::{validate_zone, NativeClient, NativeEndpoint, NativePinnedScope};
use super::store::{NewSyncJob, SyncJob, SyncStore};

static ACTIVE_JOBS: OnceLock<Mutex<HashSet<JobKey>>> = OnceLock::new();

fn job_is_active(job_id: &JobKey) -> bool {
    ACTIVE_JOBS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|active| active.contains(job_id))
        .unwrap_or(false)
}

/// Re-entrancy guard for one job. A native cycle in C1.2 acquires this before
/// claiming any work; until then it only records that nothing is running.
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

#[derive(Clone)]
pub struct SyncService {
    store: SyncStore,
    pool: crate::core::db::DbPool,
    staging_root: PathBuf,
    credential_reader: Arc<dyn Fn(&str) -> Result<String> + Send + Sync>,
}

impl SyncService {
    pub fn new(pool: crate::core::db::DbPool) -> Self {
        Self::with_optional_actor(pool, None)
    }

    pub fn with_actor(
        pool: crate::core::db::DbPool,
        actor: crate::core::db_actor::DbActorHandle,
    ) -> Self {
        Self::with_optional_actor(pool, Some(actor))
    }

    fn with_optional_actor(
        pool: crate::core::db::DbPool,
        actor: Option<crate::core::db_actor::DbActorHandle>,
    ) -> Self {
        Self::with_staging_root_and_optional_actor(pool, actor, None)
    }

    /// `staging_override` exists for tests: the default staging root is the
    /// user's real data directory, which a test must never write into — and
    /// which two concurrently running tests would fight over. Production always
    /// passes `None`.
    fn with_staging_root_and_optional_actor(
        pool: crate::core::db::DbPool,
        actor: Option<crate::core::db_actor::DbActorHandle>,
        staging_override: Option<PathBuf>,
    ) -> Self {
        let staging_root =
            staging_override.unwrap_or_else(|| crate::config::data_dir().join("sync-staging"));
        Self {
            store: SyncStore::with_optional_actor(pool.clone(), actor),
            pool,
            staging_root,
            credential_reader: Arc::new(crate::platform::credentials::load_api_key),
        }
    }

    /// A service whose staging lives in a caller-owned directory.
    #[doc(hidden)]
    pub fn with_staging_root_for_tests(
        pool: crate::core::db::DbPool,
        staging_root: PathBuf,
    ) -> Self {
        Self::with_staging_root_and_optional_actor(pool, None, Some(staging_root))
    }

    /// Isolate the platform secret boundary in integration fixtures. Production
    /// constructors always read the platform Secret Service and store no key in SQLite.
    #[doc(hidden)]
    pub fn with_credential_reader(
        mut self,
        reader: impl Fn(&str) -> Result<String> + Send + Sync + 'static,
    ) -> Self {
        self.credential_reader = Arc::new(reader);
        self
    }

    pub fn store(&self) -> &SyncStore {
        &self.store
    }

    pub fn pool(&self) -> &crate::core::db::DbPool {
        &self.pool
    }

    /// Owned staging directory. Native publication writes its in-progress pair
    /// here, so it is never inside a configured media root.
    pub(super) fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    /// Close runs that a previous process left open.
    ///
    /// A run cannot outlive the process that started it, so any job whose
    /// `last_started_at` is still ahead of `last_completed_at` at launch is
    /// describing a run this process never began.
    pub fn reconcile_interrupted_runs(&self) -> Result<usize> {
        for job in self.store.list_jobs()? {
            self.recover_publications(job.id)?;
        }
        self.store.reconcile_interrupted_runs()
    }

    /// Check a native endpoint and return the capability scope a connection
    /// would be pinned to.
    ///
    /// This is a pure read: nothing is written, so an invalid draft leaves no
    /// connection row, no credential and no configuration generation behind.
    pub async fn probe_native_connection(
        &self,
        endpoint: &str,
        api_key: &str,
    ) -> Result<NativePinnedScope> {
        let endpoint = NativeEndpoint::parse(endpoint).map_err(provider_error)?;
        let client = NativeClient::new(&endpoint, api_key).map_err(provider_error)?;
        client.probe().await.map_err(provider_error)
    }

    /// Create a native task against an already-probed scope.
    ///
    /// The probe runs first, validation runs before the write, and the write is
    /// one transaction — so a wrong endpoint, an unknown zone or an overlapping
    /// local root leaves no partial job and no stored credential behind.
    pub async fn create_native_job(
        &self,
        endpoint: &str,
        credential_ref: &str,
        api_key: &str,
        local_root: &Path,
        zone_name: &str,
        direction: SyncDirection,
        upload_albums: Vec<String>,
    ) -> Result<SyncJob> {
        // C1.1 enables native Sync/Bidirectional only. Native Backup needs its
        // own immutable-history and restore acceptance, so offering it here
        // would promise behavior nothing has been built for.
        if direction == SyncDirection::UploadOnly {
            return Err(AppError::Backend(
                "native Backup is not available yet; choose Sync".into(),
            ));
        }
        let scope = self.probe_native_connection(endpoint, api_key).await?;
        let zone = validate_zone(&scope, zone_name)
            .map_err(|error| AppError::Backend(error.to_string()))?;
        crate::platform::credentials::store_api_key(credential_ref, api_key)?;
        let created = self.store.create_job(&NewSyncJob {
            provider_kind: super::model::ProviderKind::NativeSyncServer,
            // The pinned base URL, not the typed draft: a value that survived
            // same-origin validation is the one whose zone list was read.
            endpoint: scope.public_base_url.clone(),
            local_root: local_root.to_path_buf(),
            direction,
            upload_scope: if upload_albums.is_empty() {
                UploadScope::All
            } else {
                UploadScope::SelectedAlbums
            },
            upload_albums,
            native_zone: Some(zone),
            native_pinned: Some(scope),
            credential_ref: credential_ref.to_string(),
        });
        if created.is_err() {
            // The credential is only useful together with its job; leaving it
            // behind would strand a secret the user cannot see or remove.
            let _ = crate::platform::credentials::delete_api_key(credential_ref);
        }
        created
    }

    /// Every job that still has an accepted configuration edit waiting to be
    /// applied. Used by startup to prove no accepted edit was lost in memory.
    pub fn pending_configuration_edits(&self) -> Result<usize> {
        Ok(self
            .store
            .list_jobs()?
            .into_iter()
            .filter(|job| self.store.pending_change(job.id).ok().flatten().is_some())
            .count())
    }
}

fn provider_error(error: super::error::SyncError) -> AppError {
    AppError::Backend(error.to_string())
}

#[cfg(test)]
mod tests;
