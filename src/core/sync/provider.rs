use async_trait::async_trait;
use std::path::Path;

use super::model::Revision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProviderCapabilities {
    pub read: bool,
    pub write_new: bool,
    pub conditional_replace: bool,
    pub conditional_delete: bool,
    pub move_object: bool,
    pub range_read: bool,
    pub incremental_listing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub key: String,
    pub is_collection: bool,
    pub size: u64,
    pub modified_unix: Option<i64>,
    pub revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteCondition {
    CreateOnly,
    ReplaceIf(Revision),
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("authentication failed")]
    Authentication,
    #[error("permission denied")]
    Permission,
    #[error("remote object not found: {0}")]
    NotFound(String),
    #[error("remote version changed: {0}")]
    PreconditionFailed(String),
    #[error("remote storage is full")]
    StorageFull,
    #[error("remote service is rate limiting requests")]
    RateLimited,
    #[error("network is unavailable: {0}")]
    Offline(String),
    #[error("provider capability is unsupported: {0}")]
    Unsupported(String),
    #[error("invalid provider response: {0}")]
    Protocol(String),
    #[error("local I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

#[async_trait]
pub trait SyncProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;

    async fn probe(&self) -> ProviderResult<ProviderCapabilities>;

    async fn list_children(&self, collection: &str) -> ProviderResult<Vec<RemoteEntry>>;

    async fn stat(&self, key: &str) -> ProviderResult<Option<RemoteEntry>>;

    async fn ensure_collection(&self, collection: &str) -> ProviderResult<()>;

    async fn download(
        &self,
        key: &str,
        expected: Option<&Revision>,
        destination: &Path,
    ) -> ProviderResult<RemoteEntry>;

    async fn upload(
        &self,
        key: &str,
        source: &Path,
        condition: WriteCondition,
    ) -> ProviderResult<RemoteEntry>;

    async fn delete(&self, key: &str, expected: &Revision) -> ProviderResult<()>;
}
