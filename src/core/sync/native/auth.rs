//! Server-owned authentication contract adaptation; no password persistence.
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
