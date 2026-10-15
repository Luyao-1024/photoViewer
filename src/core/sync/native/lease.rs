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

use super::protocol::{map_status, NativeClient, NativeErrorBody, NativeZone};
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcquireRequest<'a> {
    device_id: &'a str,
    #[serde(rename = "zoneIDs")]
    zones: Vec<ZoneId>,
}

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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckRequest<'a> {
    #[serde(rename = "zoneTokens")]
    zone_tokens: Vec<ZoneToken<'a>>,
    #[serde(rename = "localReady")]
    local_ready: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ZoneToken<'a> {
    #[serde(rename = "zoneID")]
    zone: &'a ZoneId,
    #[serde(rename = "syncToken")]
    sync_token: &'a str,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenewResponse {
    #[serde(rename = "ttlSeconds")]
    ttl_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhaseResponse {
    phase: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseResponse {
    released: bool,
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
        let body = AcquireRequest {
            device_id,
            zones: vec![ZoneId::from(zone)],
        };
        let url = self.path(&["v1", "upload-leases"])?;
        let payload = serde_json::to_vec(&body).map_err(|error| {
            SyncError::Protocol(format!("lease request is not encodable: {error}"))
        })?;
        let (status, bytes) = self.lease_round_trip(&url, payload, None).await?;
        if !status.is_success() {
            return Err(lease_error(status.as_u16(), &bytes));
        }
        let grant: LeaseGrant = serde_json::from_slice(&bytes).map_err(|error| {
            SyncError::Protocol(format!("lease grant is not decodable: {error}"))
        })?;
        Ok((grant, started))
    }

    /// Prove this client's position and move the lease out of `checking`.
    pub async fn check_lease(
        &self,
        proof: &LeaseProof,
        zone: &NativeZone,
        applied_token: &str,
        local_ready: bool,
    ) -> SyncResult<LeaseCheck> {
        let zone_id = ZoneId::from(zone);
        let body = CheckRequest {
            zone_tokens: vec![ZoneToken {
                zone: &zone_id,
                sync_token: applied_token,
            }],
            local_ready,
        };
        let (status, bytes) = self
            .lease_round_trip(
                &self.lease_url(proof, "check")?,
                serde_json::to_vec(&body).map_err(|error| {
                    SyncError::Protocol(format!("lease check is not encodable: {error}"))
                })?,
                Some(proof),
            )
            .await?;
        if !status.is_success() {
            return Err(lease_error(status.as_u16(), &bytes));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| SyncError::Protocol(format!("lease check is not decodable: {error}")))
    }

    pub async fn set_phase(
        &self,
        proof: &LeaseProof,
        phase: UploadPhase,
    ) -> SyncResult<UploadPhase> {
        let body = serde_json::json!({ "phase": phase.as_str() });
        let (status, bytes) = self
            .lease_round_trip(
                &self.lease_url(proof, "phase")?,
                serde_json::to_vec(&body).map_err(|error| {
                    SyncError::Protocol(format!("lease phase is not encodable: {error}"))
                })?,
                Some(proof),
            )
            .await?;
        if !status.is_success() {
            return Err(lease_error(status.as_u16(), &bytes));
        }
        let response: PhaseResponse = serde_json::from_slice(&bytes).map_err(|error| {
            SyncError::Protocol(format!("lease phase is not decodable: {error}"))
        })?;
        Ok(match response.phase.as_str() {
            "delete" => UploadPhase::Delete,
            "modify" => UploadPhase::Modify,
            "create" => UploadPhase::Create,
            other => {
                return Err(SyncError::Protocol(format!(
                    "the service reported an unknown upload phase {other:?}"
                )))
            }
        })
    }

    pub async fn renew_lease(&self, proof: &LeaseProof) -> SyncResult<(u64, Instant)> {
        let started = Instant::now();
        let (status, bytes) = self
            .lease_round_trip(
                &self.lease_url(proof, "renew")?,
                b"{}".to_vec(),
                Some(proof),
            )
            .await?;
        if !status.is_success() {
            return Err(lease_error(status.as_u16(), &bytes));
        }
        let response: RenewResponse = serde_json::from_slice(&bytes)
            .map_err(|error| SyncError::Protocol(format!("renewal is not decodable: {error}")))?;
        Ok((response.ttl_seconds, started))
    }

    pub async fn release_lease(&self, proof: &LeaseProof) -> SyncResult<bool> {
        let (status, bytes) = self
            .lease_round_trip(
                &self.lease_url(proof, "release")?,
                b"{}".to_vec(),
                Some(proof),
            )
            .await?;
        if !status.is_success() {
            return Err(lease_error(status.as_u16(), &bytes));
        }
        let response: ReleaseResponse = serde_json::from_slice(&bytes).map_err(|error| {
            SyncError::Protocol(format!("release response is not decodable: {error}"))
        })?;
        Ok(response.released)
    }

    fn lease_url(&self, proof: &LeaseProof, operation: &str) -> SyncResult<url::Url> {
        // Appended segment-wise, never with `Url::join`: joining onto
        // `…/upload-leases/{id}` replaces the last segment and silently drops
        // the lease id, so every call would address the wrong path.
        let base = self.path(&["v1", "upload-leases"])?;
        let mut url = base;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| SyncError::Protocol("lease URL cannot be extended".into()))?;
            segments.pop_if_empty();
            segments.push(&proof.lease_id);
            segments.push(operation);
        }
        Ok(url)
    }

    async fn lease_round_trip(
        &self,
        url: &url::Url,
        payload: Vec<u8>,
        proof: Option<&LeaseProof>,
    ) -> SyncResult<(reqwest::StatusCode, Vec<u8>)> {
        self.send_lease_json(url, payload, proof, None).await
    }
}

/// Map a failed lease call onto the decision it implies.
fn lease_error(status: u16, body: &[u8]) -> SyncError {
    let code = NativeErrorBody::parse(body)
        .and_then(|error| error.server_error_code)
        .unwrap_or_default();
    match code.as_str() {
        UPLOAD_LOCK_BUSY => SyncError::RateLimited,
        // Every flavour of loss — expired, released, fenced, wrong generation —
        // arrives as one code, and all of them end the cycle irreversibly.
        UPLOAD_LEASE_LOST => SyncError::Unsupported(UPLOAD_LEASE_LOST.to_string()),
        SYNC_NOT_CURRENT => SyncError::PreconditionFailed(format!("{SYNC_NOT_CURRENT}: {code}")),
        UPLOAD_PHASE_MISMATCH => {
            SyncError::PreconditionFailed(format!("{UPLOAD_PHASE_MISMATCH}: {code}"))
        }
        _ => map_status(status, body),
    }
}

/// Build the three lease headers for a mutating request.
pub fn lease_headers(proof: &LeaseProof) -> Vec<(&'static str, &str)> {
    proof.headers().to_vec()
}

#[cfg(test)]
mod tests;
