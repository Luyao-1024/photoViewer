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
        let _slot = self.acquire_run_slot(id).await?;
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| AppError::Backend("job removed".into()))?;
        let client = self.credential_client_for(&job).await?;
        match client.probe().await {
            Ok(scope) => {
                let expected = self
                    .store
                    .native_scope(job.connection_id)?
                    .ok_or_else(|| AppError::Backend("connection has no pinned identity".into()))?;
                validate_identity(&expected, &scope)?;
                client.logout().await.map_err(provider_error)?;
            }
            Err(super::super::SyncError::Authentication) => {} // already revoked/expired
            Err(error) => return Err(provider_error(error)),
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
        let _slot = self.acquire_run_slot(id).await?;
        let reference = job.credential_ref.clone();
        let secret = session.access_token;
        tokio::task::spawn_blocking(move || {
            crate::platform::credentials::store_api_key(&reference, &secret)
        })
        .await
        .map_err(|_| AppError::Backend("credential worker stopped".into()))??;
        self.store.runtime(Command::AcceptAuthentication {
            connection: job.connection_id,
            expected: Box::new(expected),
            updated: Box::new(updated),
        })?;
        self.store.set_job_paused(id, false)?;
        drop(_slot);
        self.request_job_sync(id)?;
        Ok(())
    }
}
