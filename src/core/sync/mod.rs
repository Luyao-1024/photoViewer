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
    RevisionStrength, SyncDirection, UploadScope,
};
pub use provider::{
    ProviderCapabilities, ProviderError, ProviderResult, RemoteEntry, SyncProvider, WriteCondition,
};
pub use service::{
    live_progress, ConflictResolution, RunSummary, SyncCredentials, SyncLivePhase,
    SyncLiveProgress, SyncService,
};
pub use store::{
    CloudState, NewSyncJob, StoredTask, SyncConflict, SyncJob, SyncOverview, SyncOverviewStatus,
    SyncStore,
};
