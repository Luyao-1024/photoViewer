//! Native SyncServer client boundary.
//!
//! Typed server adaptation for connection probes, fixed-height receive,
//! fenced upload coordination and paired resource transfer. Client orchestration
//! and local publication live in `sync::service`; protocol ownership stays on the server.

pub mod assets;
pub mod lease;
pub mod modify;
pub mod protocol;
pub mod provider;
pub mod receive;
pub mod records;

pub use lease::{LeaseError, LeaseGrant, LeaseProof, UploadLease, UploadPhase};
pub use modify::{ModifyOutcome, MutationBatch, OperationState};
pub use protocol::{
    validate_api_key, validate_capabilities, validate_zone, ForbiddenCapability,
    NativeCapabilitiesResponse, NativeCapabilityBits, NativeClient, NativeEndpoint,
    NativeErrorBody, NativeLimits, NativeNamespace, NativePinnedScope, NativeProbeFailure,
    NativeRecordLimits, NativeSyncLimits, NativeZone, RequiredCapability, CHECKSUM_ALGORITHM,
    FORBIDDEN_CAPABILITIES, REQUIRED_CAPABILITIES, SERVICE_NAME, SUPPORTED_PROTOCOL_VERSION,
};
pub use provider::NativeProvider;
pub use receive::{NativeRecord, ZoneId};
pub use records::{GraphError, RemoteGraph, RemotePhoto, RemoteResource};

#[cfg(test)]
pub(crate) use protocol::demo_pinned_scope;

/// The owner a C1 zone is configured under. Explicit rather than assumed,
/// because record names are unique per zone *and* owner.
pub const DEFAULT_OWNER: &str = "_defaultOwner";
