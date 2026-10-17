//! Transport failures shared by every synchronization provider.
//!
//! The variants are deliberately coarse. A caller decides what to *do* about a
//! failure — retry a bounded read, skip this cycle's uplink, abort a cycle
//! irreversibly — and that decision belongs to the caller, not to a shared
//! enumeration that would have to grow a variant per endpoint.

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The server rejected our credential. For native this means the Bearer key
    /// or an expired asset capability, which are different problems with
    /// different fixes; the caller separates them from the detail string.
    #[error("authentication failed")]
    Authentication,
    #[error("remote object not found: {0}")]
    NotFound(String),
    #[error("remote version changed: {0}")]
    PreconditionFailed(String),
    #[error("remote service is rate limiting requests")]
    RateLimited,
    #[error("network is unavailable: {0}")]
    Offline(String),
    /// The endpoint answered, but not with something this client can act on:
    /// an unsupported protocol version, a missing capability bit, an unparsable
    /// body, or an over-limit payload.
    #[error("provider capability is unsupported: {0}")]
    Unsupported(String),
    #[error("invalid provider response: {0}")]
    Protocol(String),
    #[error("local I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub type SyncResult<T> = Result<T, SyncError>;

/// Why synchronization stopped, in the terms a person can act on.
///
/// The service speaks a published vocabulary of `serverErrorCode` values. This
/// is the translation layer from that vocabulary — plus the transport failures
/// around it — to the handful of things worth telling a user. A new service
/// code lands in an existing bucket instead of growing this enum, so the
/// user-facing surface stays small as the service adds codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncCause {
    /// The account has no room left. Retrying cannot help until space is freed.
    Quota,
    /// The endpoint could not be reached, or the connection dropped.
    Network,
    /// The server no longer accepts our credential.
    Credential,
    /// The service asked us to come back later.
    ServerBusy,
    /// The remote copy moved underneath us and has to be reconciled.
    Conflict,
    /// The two versions cannot talk to each other at all.
    Incompatible,
    /// Bytes arrived but did not match what was declared.
    Checksum,
    /// No bucket fits; the raw detail is the best we have.
    Unknown,
}

/// What a user would have to do for another attempt to stand a chance.
///
/// Retrying is a scheduling decision, so the distinction has to exist before
/// anything can decide *whether* to retry rather than only how often.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncClass {
    /// Another attempt may well succeed on its own.
    Retryable,
    /// Pointless until the user frees space, signs in, or updates something.
    NeedsUser,
    /// Permanent. Repeating the request cannot change the answer.
    Unsupported,
}

impl SyncCause {
    pub fn class(self) -> SyncClass {
        match self {
            SyncCause::Quota | SyncCause::Credential => SyncClass::NeedsUser,
            SyncCause::Incompatible => SyncClass::Unsupported,
            SyncCause::Network
            | SyncCause::ServerBusy
            | SyncCause::Conflict
            | SyncCause::Checksum
            | SyncCause::Unknown => SyncClass::Retryable,
        }
    }

    /// i18n key for the one-line headline shown in the overview row.
    pub fn headline_key(self) -> &'static str {
        match self {
            SyncCause::Quota => "photos.overview.sync.quota",
            SyncCause::Network => "photos.overview.sync.network",
            SyncCause::Credential => "photos.overview.sync.credential",
            SyncCause::ServerBusy => "photos.overview.sync.busy",
            SyncCause::Conflict => "photos.overview.sync.conflict",
            SyncCause::Incompatible => "photos.overview.sync.incompatible",
            SyncCause::Checksum => "photos.overview.sync.checksum",
            SyncCause::Unknown => "photos.overview.sync.failed",
        }
    }

    /// i18n key for the dimmer second line that says what to do about it.
    pub fn detail_key(self) -> &'static str {
        match self {
            SyncCause::Quota => "photos.overview.sync.quota_detail",
            SyncCause::Network => "photos.overview.sync.network_detail",
            SyncCause::Credential => "photos.overview.sync.credential_detail",
            SyncCause::ServerBusy => "photos.overview.sync.busy_detail",
            SyncCause::Conflict => "photos.overview.sync.conflict_detail",
            SyncCause::Incompatible => "photos.overview.sync.incompatible_detail",
            SyncCause::Checksum => "photos.overview.sync.checksum_detail",
            SyncCause::Unknown => "photos.overview.sync.failed_detail",
        }
    }
}

/// Service `serverErrorCode` values and the cause each one means.
///
/// Matched as substrings because that is how they travel: `describe` in the
/// native protocol renders them as `HTTP {status} {code}: {reason}`, and that
/// rendered detail is what a cycle stores as its job error.
const SERVICE_CODES: &[(&str, SyncCause)] = &[
    ("STORAGE_QUOTA_EXCEEDED", SyncCause::Quota),
    ("AUTHENTICATION_REQUIRED", SyncCause::Credential),
    ("SERVER_BUSY", SyncCause::ServerBusy),
    ("UPLOAD_TIMEOUT", SyncCause::ServerBusy),
    ("PATH_CONFLICT", SyncCause::Conflict),
    ("IDEMPOTENCY_CONFLICT", SyncCause::Conflict),
    ("UPLOAD_STATE_CONFLICT", SyncCause::Conflict),
    ("UPLOAD_PHASE_MISMATCH", SyncCause::Conflict),
    ("UPLOAD_LEASE_LOST", SyncCause::Conflict),
    ("UPLOAD_LOCK_BUSY", SyncCause::Conflict),
    ("UPLOAD_BUSY", SyncCause::Conflict),
    ("EXPIRED_SYNC_TOKEN", SyncCause::Conflict),
    ("UNSUPPORTED_CAPABILITY", SyncCause::Incompatible),
    ("UNSUPPORTED_MEDIA_TYPE", SyncCause::Incompatible),
    ("CHECKSUM_MISMATCH", SyncCause::Checksum),
    ("UPLOAD_SIZE_MISMATCH", SyncCause::Checksum),
    ("UPLOAD_INTERRUPTED", SyncCause::Checksum),
    ("INVALID_THUMBNAIL", SyncCause::Checksum),
];

/// Recover the cause from a stored or rendered failure detail.
///
/// Two shapes reach here: a `SyncError`'s own message, and the per-item reason
/// a cycle writes to `sync_jobs.last_error`. Both are produced by this
/// codebase, so their wording is a contract we can rely on — the first group
/// matches service codes, the second matches the variant messages of
/// [`SyncError`] that carry no detail of their own.
pub fn classify_detail(detail: &str) -> SyncCause {
    for (code, cause) in SERVICE_CODES {
        if detail.contains(code) {
            return *cause;
        }
    }
    if detail.contains("network is unavailable") {
        return SyncCause::Network;
    }
    if detail.contains("authentication failed")
        || detail.contains("HTTP 401")
        || detail.contains("HTTP 403")
    {
        return SyncCause::Credential;
    }
    if detail.contains("rate limiting")
        || detail.contains("HTTP 429")
        || detail.contains("HTTP 503")
    {
        return SyncCause::ServerBusy;
    }
    if detail.contains("capability is unsupported") || detail.contains("HTTP 413") {
        return SyncCause::Incompatible;
    }
    if detail.contains("HTTP 404") {
        return SyncCause::Conflict;
    }
    SyncCause::Unknown
}

impl SyncError {
    pub fn cause(&self) -> SyncCause {
        classify_detail(&self.to_string())
    }

    pub fn class(&self) -> SyncClass {
        self.cause().class()
    }
}

#[cfg(test)]
mod tests;
