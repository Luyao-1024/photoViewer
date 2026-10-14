//! The provider object a native job runs against.
//!
//! C1.1 wires the connection and capability probe and nothing else. It
//! deliberately does **not** implement a file-operation trait: `list_children`
//! is not an incremental zone feed, and a record is not a file with an ETag, so
//! routing this server through a file-shaped contract would make the engine
//! believe it can reconcile it. C1.2 adds the typed zone, record and delta calls
//! this object will actually use.

use super::protocol::{NativeClient, NativePinnedScope, NativeSyncLimits, NativeZone};

/// A native connection plus the scope it was accepted against.
///
/// Constructing one is cheap and offline: it owns no socket until a call is
/// made, so a job can be built and inspected without touching the network.
#[derive(Clone)]
pub struct NativeProvider {
    client: NativeClient,
    pinned: NativePinnedScope,
    zone: NativeZone,
}

impl NativeProvider {
    pub fn new(client: NativeClient, pinned: NativePinnedScope, zone: NativeZone) -> Self {
        Self {
            client,
            pinned,
            zone,
        }
    }

    /// Re-run health, capabilities and validation against the configured origin.
    /// Named apart from any trait `probe` so the two results cannot be confused
    /// at a call site.
    pub async fn refresh_scope(&self) -> crate::core::sync::SyncResult<NativePinnedScope> {
        self.client.probe().await
    }

    /// The scope this connection was accepted against. Later cycles compare
    /// against it instead of trusting whatever the server announces later.
    pub fn pinned_scope(&self) -> &NativePinnedScope {
        &self.pinned
    }

    /// Page bounds from the server's own configuration, not protocol constants.
    pub fn page_limits(&self) -> NativeSyncLimits {
        self.pinned.sync_limits
    }

    pub fn zone(&self) -> &NativeZone {
        &self.zone
    }

    pub fn client(&self) -> &NativeClient {
        &self.client
    }
}
