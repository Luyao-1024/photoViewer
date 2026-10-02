use async_trait::async_trait;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::model::Revision;

/// Byte counters for one in-flight transfer. A transport adapter records
/// progress through this sink; the service attaches it to the live progress
/// session so the UI can show sub-file movement while a large file streams.
#[derive(Debug, Default)]
pub struct TransferProgress {
    bytes: AtomicU64,
    total: AtomicU64,
}

impl TransferProgress {
    /// Restart the counters for a new transfer with the expected size
    /// (`0` when the size is unknown).
    pub fn start(&self, total: u64) {
        self.bytes.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn record(&self, additional: u64) {
        self.bytes.fetch_add(additional, Ordering::Relaxed);
    }

    /// `(transferred, expected total)` for the current transfer.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.bytes.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

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

/// A remote store the sync engine can read and write.
///
/// `#[allow(clippy::double_must_use)]`: every `async fn` here is desugared by
/// `async_trait` into a method carrying a bare `#[must_use]`, and each one returns
/// `ProviderResult`, which is already must-use. Clippy 0.1.99's `double_must_use`
/// flags that pairing, and the attribute it objects to is macro-generated rather
/// than written here, so it cannot be given the explicit reason the lint suggests.
#[allow(clippy::double_must_use)]
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
