//! The exclusive upload lease.
//!
//! A zone has one effective uplink device. That is a property of the *server*,
//! not of this client: a local mutex would only stop this process. The lease is
//! what makes two installations agree, and the fencing token is what proves the
//! holder is still the holder at the moment a write is committed.
//!
//! ## The deadline is conservative, and deliberately so
//!
//! Authority is bounded by *a successful renewal* plus the TTL that renewal
//! granted, measured from **when the request started**, on a monotonic clock.
//! It is never derived from the server's `expiresAtMs`, which is wall-clock:
//! a clock that jumps forward would otherwise end the lease early, and one
//! that jumps backward would extend authority this client no longer has.
//!
//! A renewal that arrives after the current deadline does not revive the lease.
//! A slow response cannot grant time that has already run out.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::core::sync::{SyncError, SyncResult};

use super::protocol::{map_sdk_error, NativeClient, NativeZone};
use super::receive::ZoneId;

/// The error code the service returns for every flavour of lease loss:
/// expired, released, fenced by another device, or lost across a restart.
pub const UPLOAD_LEASE_LOST: &str = "UPLOAD_LEASE_LOST";
pub const UPLOAD_LOCK_BUSY: &str = "UPLOAD_LOCK_BUSY";
pub const UPLOAD_PHASE_MISMATCH: &str = "UPLOAD_PHASE_MISMATCH";
pub const SYNC_NOT_CURRENT: &str = "SYNC_NOT_CURRENT";

/// How much of a granted TTL is held back.
///
/// The renewal round trip itself, plus scheduling jitter, is the uncertainty.
/// Claiming the full TTL would mean a request issued just before expiry is
/// still assumed live after it. A tenth of the TTL is far larger than either
/// and far smaller than the safety it buys.
const DEADLINE_SAFETY: f64 = 0.1;

/// The proof carried on every mutating call. Exactly one header of each kind,
/// and the path's lease id must match the header's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseProof {
    pub lease_id: String,
    pub lease_token: String,
    /// An opaque decimal string. Kept as text because the service compares it
    /// as a canonical number; reformatting it would change its meaning.
    pub fencing_token: String,
}

impl LeaseProof {
    fn headers(&self) -> [(&'static str, &str); 3] {
        [
            ("X-Sync-Lease-ID", self.lease_id.as_str()),
            ("X-Sync-Lease-Token", self.lease_token.as_str()),
            ("X-Sync-Lease-Fence", self.fencing_token.as_str()),
        ]
    }

    pub(super) fn sdk(&self) -> sync_client::LeaseProof {
        sync_client::LeaseProof {
            lease_id: self.lease_id.clone(),
            lease_token: self.lease_token.clone(),
            fencing_token: self.fencing_token.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseGrant {
    #[serde(rename = "leaseId")]
    pub lease_id: String,
    #[serde(rename = "leaseToken")]
    pub lease_token: String,
    #[serde(rename = "fencingToken")]
    pub fencing_token: String,
    #[serde(rename = "ttlSeconds")]
    pub ttl_seconds: u64,
    #[serde(rename = "heartbeatSeconds")]
    pub heartbeat_seconds: u64,
    pub phase: String,
}

impl LeaseGrant {
    pub fn proof(&self) -> LeaseProof {
        LeaseProof {
            lease_id: self.lease_id.clone(),
            lease_token: self.lease_token.clone(),
            fencing_token: self.fencing_token.clone(),
        }
    }
}

/// The three phases a cycle walks, in order. It never goes backwards, and an
/// empty phase is still advanced: settling issued operations before the next
/// phase is not optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadPhase {
    Delete,
    Modify,
    Create,
}

impl UploadPhase {
    pub const ORDER: [UploadPhase; 3] = [Self::Delete, Self::Modify, Self::Create];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Modify => "modify",
            Self::Create => "create",
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::Delete => Some(Self::Modify),
            Self::Modify => Some(Self::Create),
            Self::Create => None,
        }
    }

    pub(super) fn sdk(self) -> sync_client::UploadPhase {
        match self {
            Self::Delete => sync_client::UploadPhase::Delete,
            Self::Modify => sync_client::UploadPhase::Modify,
            Self::Create => sync_client::UploadPhase::Create,
        }
    }

    pub(super) fn from_sdk(phase: sync_client::UploadPhase) -> Self {
        match phase {
            sync_client::UploadPhase::Delete => Self::Delete,
            sync_client::UploadPhase::Modify => Self::Modify,
            sync_client::UploadPhase::Create => Self::Create,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseCheck {
    #[serde(rename = "isCurrent")]
    pub is_current: bool,
    #[serde(rename = "mayUpload")]
    pub may_upload: bool,
    pub phase: String,
    #[serde(rename = "zoneTokens")]
    pub zone_tokens: Vec<ZoneTokenValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneTokenValue {
    #[serde(rename = "zoneID")]
    pub zone: ZoneId,
    #[serde(rename = "syncToken")]
    pub sync_token: String,
}

/// Why a cycle cannot keep going. Each variant is a decision, not a symptom.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    /// Another device holds the zone. This cycle skips its uplink once and
    /// keeps every dirty obligation; it does not retry acquisition.
    #[error("another device holds the upload lease for this zone")]
    Busy,
    /// The lease is gone: expired, released, or fenced. Irreversible for this
    /// cycle — nothing that follows may be issued.
    #[error("the upload lease was lost")]
    Lost,
    /// The client's own position is behind the cloud's. The lease is kept and
    /// catch-up repeats downstream.
    #[error("this client is not current with the zone: {0}")]
    NotCurrent(String),
    #[error("upload phase mismatch: {0}")]
    PhaseMismatch(String),
    #[error("transport failed: {0}")]
    Transport(String),
}

impl From<LeaseError> for SyncError {
    fn from(error: LeaseError) -> Self {
        SyncError::Protocol(error.to_string())
    }
}

/// A held lease with its conservative authority window.
///
/// Shared because the heartbeat runs on its own task while the cycle does file
/// and record work; the deadline must be one value, not one per reader.
#[derive(Debug, Clone)]
pub struct UploadLease {
    proof: LeaseProof,
    ttl: Duration,
    heartbeat: Duration,
    /// Set once on loss or explicit release. A cycle that sees this can never
    /// issue another mutation, however many of its own steps remain.
    lost: Arc<Mutex<LeaseState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseState {
    Held { deadline: Instant },
    Lost,
}

impl UploadLease {
    pub fn from_grant(grant: &LeaseGrant, started: Instant) -> Self {
        Self {
            proof: grant.proof(),
            ttl: Duration::from_secs(grant.ttl_seconds),
            heartbeat: Duration::from_secs(grant.heartbeat_seconds).max(Duration::from_millis(10)),
            lost: Arc::new(Mutex::new(LeaseState::Held {
                deadline: started
                    .checked_add(conservative(grant.ttl_seconds))
                    .unwrap_or(started),
            })),
        }
    }

    pub(crate) fn valid_now(&self) -> bool {
        self.lost.lock().is_ok_and(
            |state| matches!(*state, LeaseState::Held {deadline} if Instant::now()<deadline),
        )
    }

    pub fn proof(&self) -> &LeaseProof {
        &self.proof
    }

    /// The granted TTL, before the safety margin. Reported rather than used as
    /// authority, so a caller can reason about how much of it is actually held.
    pub fn granted_ttl(&self) -> Duration {
        self.ttl
    }

    /// How often the heartbeat renews, taken from the grant.
    pub fn heartbeat_interval(&self) -> Duration {
        self.heartbeat
    }

    /// The instant after which this client stops assuming it holds the lease.
    pub async fn deadline(&self) -> Option<Instant> {
        match *self.lost.lock().unwrap_or_else(|error| error.into_inner()) {
            LeaseState::Held { deadline } => Some(deadline),
            LeaseState::Lost => None,
        }
    }

    /// Whether authority still stands. Checked immediately before every
    /// mutation, so a cycle cannot keep issuing after losing the lease.
    pub async fn is_held(&self) -> bool {
        match *self.lost.lock().unwrap_or_else(|error| error.into_inner()) {
            LeaseState::Lost => false,
            LeaseState::Held { deadline } => Instant::now() < deadline,
        }
    }

    pub async fn mark_lost(&self) {
        *self.lost.lock().unwrap_or_else(|error| error.into_inner()) = LeaseState::Lost;
    }

    /// Record a successful renewal. `started` is when the request left, so a
    /// slow response grants only what was actually earned.
    pub async fn renew(&self, ttl_seconds: u64, started: Instant) {
        let mut state = self.lost.lock().unwrap_or_else(|error| error.into_inner());
        *state = renewed(*state, Instant::now(), started, ttl_seconds);
    }
}

/// The state a renewal leaves behind, given the current state, the moment the
/// answer arrived, the moment the request left, and what it granted.
///
/// Pulled out as a pure function so the rule can be tested with synthetic
/// instants instead of by sleeping through a real lease: authority that had
/// already ended is gone, whatever the answer says.
fn renewed(current: LeaseState, now: Instant, started: Instant, ttl_seconds: u64) -> LeaseState {
    match current {
        LeaseState::Lost => LeaseState::Lost,
        LeaseState::Held { deadline } => {
            if now >= deadline {
                // Authority ended while this renewal was in flight. A late
                // answer cannot hand back time the server has already given to
                // another device.
                LeaseState::Lost
            } else {
                let offered = started
                    .checked_add(conservative(ttl_seconds))
                    .unwrap_or(started);
                if offered > deadline {
                    LeaseState::Held { deadline: offered }
                } else {
                    LeaseState::Held { deadline }
                }
            }
        }
    }
}

/// The usable window of a granted TTL.
fn conservative(ttl_seconds: u64) -> Duration {
    let seconds = ttl_seconds as f64 * (1.0 - DEADLINE_SAFETY);
    Duration::from_secs_f64(seconds.max(0.0))
}

/// Lease control for one client.
impl NativeClient {
    /// Acquire the zone's uplink lease. `Err(Busy)` means another device holds
    /// it, which the caller answers by skipping this cycle's uplink once.
    pub async fn acquire_lease(
        &self,
        device_id: &str,
        zone: &NativeZone,
    ) -> SyncResult<(LeaseGrant, Instant)> {
        let started = Instant::now();
        let connection = self.connection().await?;
        let grant = connection
            .acquire_upload_lease(device_id, &super::receive::sdk_zone(zone))
            .await
            .map_err(map_sdk_error)?;
        Ok((
            LeaseGrant {
                lease_id: grant.lease_id,
                lease_token: grant.lease_token,
                fencing_token: grant.fencing_token,
                ttl_seconds: grant.ttl_seconds,
                heartbeat_seconds: grant.heartbeat_seconds,
                phase: grant.phase,
            },
            started,
        ))
    }

    /// Prove this client's position and move the lease out of `checking`.
    pub async fn check_lease(
        &self,
        proof: &LeaseProof,
        zone: &NativeZone,
        applied_token: &str,
        local_ready: bool,
    ) -> SyncResult<LeaseCheck> {
        let connection = self.connection().await?;
        let check = connection
            .check_upload_lease(
                &proof.sdk(),
                &super::receive::sdk_zone(zone),
                applied_token,
                local_ready,
            )
            .await
            .map_err(map_sdk_error)?;
        Ok(LeaseCheck {
            is_current: check.is_current,
            may_upload: check.may_upload,
            phase: check.phase,
            zone_tokens: check
                .zone_tokens
                .into_iter()
                .map(|token| ZoneTokenValue {
                    zone: ZoneId {
                        zone_name: token.zone.zone_name,
                        owner_record_name: token.zone.owner_record_name,
                    },
                    sync_token: token.sync_token,
                })
                .collect(),
        })
    }

    pub async fn set_phase(
        &self,
        proof: &LeaseProof,
        phase: UploadPhase,
    ) -> SyncResult<UploadPhase> {
        let connection = self.connection().await?;
        let returned = connection
            .set_upload_phase(&proof.sdk(), phase.sdk())
            .await
            .map_err(map_sdk_error)?;
        if returned != phase.sdk() {
            return Err(SyncError::Protocol(format!(
                "the service acknowledged phase {:?} for a {:?} request",
                UploadPhase::from_sdk(returned),
                phase
            )));
        }
        Ok(phase)
    }

    pub async fn renew_lease(&self, proof: &LeaseProof) -> SyncResult<(u64, Instant)> {
        let connection = self.connection().await?;
        let renewal = connection
            .renew_upload_lease(&proof.sdk())
            .await
            .map_err(map_sdk_error)?;
        Ok((renewal.ttl_seconds, renewal.started))
    }

    pub async fn release_lease(&self, proof: &LeaseProof) -> SyncResult<bool> {
        let connection = self.connection().await?;
        connection
            .release_upload_lease(&proof.sdk())
            .await
            .map_err(map_sdk_error)
    }
}

/// Build the three lease headers for a mutating request.
pub fn lease_headers(proof: &LeaseProof) -> Vec<(&'static str, &str)> {
    proof.headers().to_vec()
}

#[cfg(test)]
mod tests;
