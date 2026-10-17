//! Provider-neutral, local-first bidirectional file synchronization.

pub mod content;
pub mod debounce;
pub mod device;
mod error;
pub mod model;
pub mod native;
pub mod publication_io;
mod remote_path;
pub mod service;
pub mod store;

pub use content::ContentDigest;
pub use error::{classify_detail, SyncCause, SyncClass, SyncError, SyncResult};
pub use model::{ProviderKind, SyncDirection, UploadScope};
pub use native::lease::{UploadLease, UploadPhase};
pub use native::records::{AssetDescriptor, RemoteGraph, RemotePhoto, RemoteResource};
pub use native::{
    LeaseProof, ModifyOutcome, MutationBatch, NativeCapabilitiesResponse, NativeCapabilityBits,
    NativeClient, NativeEndpoint, NativeLimits, NativeNamespace, NativePinnedScope,
    NativeProbeFailure, NativeProvider, NativeSyncLimits, NativeZone, OperationState, ZoneId,
};
pub use service::SyncService;
pub use store::{
    ChangeCategory, CloudState, NewSyncJob, PublicationIntent, ResourceState, ResourceWork,
    SyncJob, SyncOverview, SyncOverviewStatus, SyncStore,
};
