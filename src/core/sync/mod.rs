//! Provider-neutral, local-first bidirectional file synchronization.

pub mod local;
pub mod model;
pub mod planner;
pub mod provider;
pub mod service;
pub mod store;
pub mod webdav;

pub use model::{
    Baseline, ConflictKind, EntrySnapshot, Fingerprint, Observation, PlanAction, Revision,
    RevisionStrength, SyncDirection,
};
pub use provider::{
    ProviderCapabilities, ProviderError, ProviderResult, RemoteEntry, SyncProvider, WriteCondition,
};
pub use service::{ConflictResolution, RunSummary, SyncCredentials, SyncService};
pub use store::{NewSyncJob, StoredTask, SyncConflict, SyncJob, SyncStore};
