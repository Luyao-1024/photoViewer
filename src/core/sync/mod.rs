//! Provider-neutral, local-first bidirectional file synchronization.

mod error;
pub mod model;
pub mod native;
mod remote_path;
pub mod service;
pub mod store;

pub use error::{SyncError, SyncResult};
pub use model::{ProviderKind, SyncDirection, UploadScope};
pub use native::{
    NativeCapabilitiesResponse, NativeCapabilityBits, NativeClient, NativeEndpoint, NativeLimits,
    NativeNamespace, NativePinnedScope, NativeProbeFailure, NativeProvider, NativeSyncLimits,
    NativeZone,
};
pub use service::SyncService;
pub use store::{CloudState, NewSyncJob, SyncJob, SyncOverview, SyncOverviewStatus, SyncStore};
