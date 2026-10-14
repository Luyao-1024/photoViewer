//! Native SyncServer client boundary.
//!
//! C1.1 delivers the connection and capability probe only. Later milestones add
//! receive, lease and asset calls to [`protocol`] and media orchestration to
//! [`provider`].

pub mod protocol;
pub mod provider;

pub use protocol::{
    validate_api_key, validate_capabilities, validate_zone, ForbiddenCapability,
    NativeCapabilitiesResponse, NativeCapabilityBits, NativeClient, NativeEndpoint,
    NativeErrorBody, NativeLimits, NativeNamespace, NativePinnedScope, NativeProbeFailure,
    NativeRecordLimits, NativeSyncLimits, NativeZone, RequiredCapability, CHECKSUM_ALGORITHM,
    FORBIDDEN_CAPABILITIES, REQUIRED_CAPABILITIES, SERVICE_NAME, SUPPORTED_PROTOCOL_VERSION,
};
pub use provider::NativeProvider;

#[cfg(test)]
pub(crate) use protocol::demo_pinned_scope;

/// The owner a C1 zone is configured under. Explicit rather than assumed,
/// because record names are unique per zone *and* owner.
pub const DEFAULT_OWNER: &str = "_defaultOwner";
