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
