use super::*;
use crate::core::sync::native::{NativeSession, NativeUser};
use crate::core::sync::store::runtime::Command;
pub(super) fn user(scope: &NativePinnedScope) -> Option<&NativeUser> {
    scope.authentication.as_ref().and_then(|a| a.user.as_ref())
}
pub(super) fn validate_identity(
    expected: &NativePinnedScope,
    current: &NativePinnedScope,
) -> Result<()> {
    if expected.public_base_url != current.public_base_url
        || expected.namespace != current.namespace
        || expected.database_generation != current.database_generation
        || user(expected).is_some_and(|u| Some(&u.id) != user(current).map(|u| &u.id))
    {
        return Err(AppError::Backend("server account or database identity changed; the existing job cannot use this credential".into()));
    }
    Ok(())
}
impl SyncService {
    /// Password lives only in this request; only the resulting session may be stored.
    pub async fn login_native_connection(
        &self,
        endpoint: &str,
        username: &str,
        password: &str,
    ) -> Result<(NativeSession, NativePinnedScope)> {
        let endpoint = NativeEndpoint::parse(endpoint).map_err(provider_error)?;
        let session = NativeClient::login(&endpoint, username, password)
            .await
            .map_err(provider_error)?;
        let client = NativeClient::new(&endpoint, &session.access_token).map_err(provider_error)?;
        let scope = client.probe().await.map_err(provider_error)?;
        if user(&scope) != Some(&session.user)
            || scope
                .authentication
                .as_ref()
                .is_none_or(|a| !a.resource_bearer_required)
        {
            return Err(AppError::Backend(
                "login and capability identities disagree".into(),
            ));
        }
        Ok((session, scope))
    }
    pub async fn logout_native_job(&self, id: i64) -> Result<()> {
        let jobs = self.account_jobs(id)?;
        for job in &jobs {
            self.store.set_job_paused(job.id, true)?;
        }
        for job in jobs {
            while self.is_job_running(job.id)? {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            self.logout_native_connection(job.id).await?;
        }
        Ok(())
    }

    /// Forget all folders of a saved account without requiring a valid session.
    /// Folder removal owns draining, publication recovery and credential cleanup.
    pub async fn remove_native_account(&self, id: i64) -> Result<bool> {
        let jobs = self.account_jobs(id)?;
        for job in &jobs {
            self.store.set_job_paused(job.id, true)?;
        }
        let mut cleaned = true;
        for job in jobs {
            cleaned &= self.delete_job(job.id).await?;
        }
        Ok(cleaned)
    }

    fn account_jobs(&self, id: i64) -> Result<Vec<SyncJob>> {
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| AppError::Backend("job removed".into()))?;
        let expected = self
            .store
            .native_scope(job.connection_id)?
            .ok_or_else(|| AppError::Backend("connection has no pinned identity".into()))?;
        let mut jobs = Vec::new();
        for candidate in self.store.saved_jobs()? {
            if self
                .store
                .native_scope(candidate.connection_id)?
                .as_ref()
                .is_some_and(|scope| super::super::store::accounts::same_account(&expected, scope))
            {
                jobs.push(candidate);
            }
        }
        Ok(jobs)
    }

    async fn logout_native_connection(&self, id: i64) -> Result<()> {
        let _slot = self.acquire_authentication_slot(id).await?;
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| AppError::Backend("job removed".into()))?;
        let revoke = async {
            let client = self.credential_client_for(&job).await?;
            match client.probe().await {
                Ok(scope) => {
                    let expected =
                        self.store.native_scope(job.connection_id)?.ok_or_else(|| {
                            AppError::Backend("connection has no pinned identity".into())
                        })?;
                    validate_identity(&expected, &scope)?;
                    client.logout().await.map_err(provider_error)?;
                }
                Err(super::super::SyncError::Authentication) => {} // already revoked/expired
                Err(error) => return Err(provider_error(error)),
            }
            Ok::<_, AppError>(())
        };
        if !matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), revoke).await,
            Ok(Ok(()))
        ) {
            tracing::warn!(
                job_id = id,
                "remote session revocation could not be confirmed; signing out locally"
            );
        }
        let reference = job.credential_ref.clone();
        tokio::task::spawn_blocking(move || {
            crate::platform::credentials::delete_api_key(&reference)
        })
        .await
        .map_err(|_| AppError::Backend("credential worker stopped".into()))??;
        self.store.set_job_paused(id, true)?;
        Ok(())
    }
    pub async fn reauthenticate_native_job(
        &self,
        id: i64,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| AppError::Backend("job removed".into()))?;
        let expected = self
            .store
            .native_scope(job.connection_id)?
            .ok_or_else(|| AppError::Backend("connection has no pinned identity".into()))?;
        let (session, updated) = self
            .login_native_connection(&job.endpoint, username, password)
            .await?;
        validate_identity(&expected, &updated)?;
        let jobs = self.account_jobs(id)?;
        // Validate the whole account before an accepted login stops old work.
        let mut scopes = Vec::new();
        for candidate in &jobs {
            let expected = self
                .store
                .native_scope(candidate.connection_id)?
                .ok_or_else(|| AppError::Backend("connection has no pinned identity".into()))?;
            validate_identity(&expected, &updated)?;
            scopes.push(expected);
        }
        for candidate in &jobs {
            self.store.set_job_paused(candidate.id, true)?;
        }
        for (candidate, expected) in jobs.iter().zip(scopes) {
            while self.is_job_running(candidate.id)? {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            let _slot = self.acquire_authentication_slot(candidate.id).await?;
            let reference = candidate.credential_ref.clone();
            let secret = session.access_token.clone();
            tokio::task::spawn_blocking(move || {
                crate::platform::credentials::store_api_key(&reference, &secret)
            })
            .await
            .map_err(|_| AppError::Backend("credential worker stopped".into()))??;
            self.store.runtime(Command::AcceptAuthentication {
                connection: candidate.connection_id,
                expected: Box::new(expected),
                updated: Box::new(updated.clone()),
            })?;
        }
        for candidate in jobs {
            self.store.set_job_paused(candidate.id, false)?;
        }
        self.activate_native_account(id).await?;
        self.request_job_sync(id)?;
        Ok(())
    }
}

impl SyncService {
    /// Only a verified saved identity can replace the current account.
    pub async fn switch_native_account(&self, id: i64) -> Result<()> {
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| AppError::Backend("account task was removed".into()))?;
        let expected = self
            .store
            .native_scope(job.connection_id)?
            .ok_or_else(|| AppError::Backend("connection has no pinned identity".into()))?;
        let client = self.credential_client_for(&job).await?;
        let current = client.probe().await.map_err(provider_error)?;
        validate_identity(&expected, &current)?;
        self.activate_native_account(id).await
    }

    pub(super) async fn activate_native_account(&self, id: i64) -> Result<()> {
        let reset = self.reset_work()?;
        let gate = reset.gate.lock().await;
        if self.store.reset_pending()? {
            return Err(AppError::Backend(
                "synchronization history is being cleared".into(),
            ));
        }
        let previous_account = self.store.list_jobs()?.first().map(|job| job.id);
        let previous = self.store.list_jobs()?;
        self.store
            .runtime(Command::SelectAccount { job: Some(id) })?;
        // Disable old epochs first. Keep admission closed until every old
        // account run (including receive/publication) has settled.
        loop {
            let mut draining = false;
            for old in &previous {
                if !self.store.job_is_selected(old.id)? && self.is_job_running(old.id)? {
                    draining = true;
                    break;
                }
            }
            if !draining {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let retired = previous
            .into_iter()
            .filter_map(|job| match self.store.job_is_selected(job.id) {
                Ok(false) => Some(Ok(job.id)),
                Ok(true) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?;
        let service = self.clone();
        let settled = tokio::task::spawn_blocking(move || {
            for id in retired {
                service.recover_publications(id)?;
                service
                    .store
                    .runtime(Command::DiscardPendingDeletions { job: id })?;
            }
            Ok::<_, AppError>(())
        })
        .await
        .map_err(|_| AppError::Backend("account publication recovery worker stopped".into()))
        .and_then(|result| result);
        if let Err(error) = settled {
            // A failed settlement must never admit the new account over an old
            // publication. Restore the previous selection before reopening admission.
            match previous_account {
                Some(id) => self
                    .store
                    .runtime(Command::SelectAccount { job: Some(id) })?,
                None => self.store.runtime(Command::ClearAccount)?,
            }
            return Err(error);
        }
        drop(gate);
        self.request_all_sync()
    }

    pub(super) fn require_selected_account(&self, id: i64) -> Result<()> {
        if !self.store.job_is_selected(id)? {
            return Err(AppError::Backend(
                "this account is inactive; switch accounts before synchronizing".into(),
            ));
        }
        Ok(())
    }
}
