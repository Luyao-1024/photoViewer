//! Server-owned authentication contract adaptation; no password persistence.
use super::protocol::{map_status, NativeClient, NativeEndpoint};
use crate::core::sync::{SyncError, SyncResult};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeUser {
    pub id: String,
    pub username: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAuthentication {
    pub mode: String,
    pub user: Option<NativeUser>,
    #[serde(default)]
    pub resource_bearer_required: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSession {
    pub access_token: String,
    pub token_type: String,
    pub expires_at: i64,
    pub user: NativeUser,
}
impl NativeClient {
    pub async fn login(
        endpoint: &NativeEndpoint,
        username: &str,
        password: &str,
    ) -> SyncResult<NativeSession> {
        // Reuse the exact transport policy; this temporary key never goes on
        // the login request, which is the only unauthenticated operation here.
        let transport = Self::new(endpoint, "login-transport-placeholder-00000000")?;
        let url = endpoint
            .base()
            .join("v1/auth/login")
            .map_err(|_| SyncError::Protocol("invalid login endpoint".into()))?;
        if username.len() > 64 || password.len() > 1024 {
            return Err(SyncError::Authentication);
        }
        let payload =
            serde_json::to_vec(&serde_json::json!({"username":username,"password":password}))
                .map_err(|_| SyncError::Protocol("invalid login form".into()))?;
        let mut response = transport
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload)
            .send()
            .await?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 8192 {
                return Err(SyncError::Protocol(
                    "login response exceeds its limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &bytes));
        }
        let session: NativeSession = serde_json::from_slice(&bytes)
            .map_err(|_| SyncError::Protocol("invalid login response".into()))?;
        super::protocol::validate_api_key(&session.access_token)?;
        if session.token_type != "Bearer"
            || uuid::Uuid::parse_str(&session.user.id).is_err()
            || !session.user.username.eq_ignore_ascii_case(username)
            || session.user.username.is_empty()
            || session.user.username.len() > 64
            || session.expires_at <= chrono::Utc::now().timestamp()
        {
            return Err(SyncError::Protocol(
                "invalid session identity or expiry".into(),
            ));
        }
        Ok(session)
    }
}
