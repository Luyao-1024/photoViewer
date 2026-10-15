//! Provider-neutral, local-first bidirectional file synchronization.

pub mod device;
mod error;
pub mod model;
pub mod native;
mod remote_path;
pub mod service;
pub mod store;

pub use error::{SyncError, SyncResult};
pub use model::{ProviderKind, SyncDirection, UploadScope};
pub use native::lease::{UploadLease, UploadPhase};
pub use native::{
    LeaseProof, ModifyOutcome, MutationBatch, NativeCapabilitiesResponse, NativeCapabilityBits,
    NativeClient, NativeEndpoint, NativeLimits, NativeNamespace, NativePinnedScope,
    NativeProbeFailure, NativeProvider, NativeSyncLimits, NativeZone, OperationState, ZoneId,
};
pub use service::SyncService;
pub use store::{
    ChangeCategory, CloudState, NewSyncJob, SyncJob, SyncOverview, SyncOverviewStatus, SyncStore,
};
