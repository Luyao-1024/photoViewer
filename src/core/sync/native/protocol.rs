//! Typed boundary between the local sync engine and the native SyncServer SDK.
//!
//! Everything here is connection policy and DTOs: no SQL, no GTK. The wire
//! transport, request encoding and response decoding belong to the `sync-client`
//! SDK; this module owns endpoint/capability validation, the SDK error
//! translation and the pinned-scope hand-off. The capability field names are
//! transcribed from the service's own `GET /v1/capabilities` payload
//! (`syncserver@2d9f9a7`, stage M4, protocolVersion 1) and are deliberately
//! tolerant of unknown future members so a newer server cannot fail to parse.
//!
//! See `docs/modules/native-sync-protocol.md` for the contract these types
//! implement.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::core::sync::error::{SyncError, SyncResult};

/// The only wire version this client speaks. A textual `stage` alone never
/// enables a feature, so the version number is the gate instead.
pub const SUPPORTED_PROTOCOL_VERSION: i64 = 1;

/// The service must identify itself; a same-path reverse proxy or another
/// service would otherwise answer `/v1/capabilities` with unrelated JSON.
pub const SERVICE_NAME: &str = "syncserver";

/// The checksum the service signs asset bodies with. Anything else means the
/// digests this client stores would not be verifiable.
pub const CHECKSUM_ALGORITHM: &str = "SHA256";

/// Capability bits C1 requires before a native configuration may be accepted,
/// in the order the wire contract lists them. Optional optimizations
/// (`queryBatch`, `changeAliases`) are deliberately absent: they change batch
/// shape, never correctness.
pub const REQUIRED_CAPABILITIES: [RequiredCapability; 12] = [
    RequiredCapability::ZoneListing,
    RequiredCapability::RecordCrud,
    RequiredCapability::Snapshots,
    RequiredCapability::IncrementalChanges,
    RequiredCapability::UploadLeases,
    RequiredCapability::Fencing,
    RequiredCapability::TokenCurrentGate,
    RequiredCapability::UploadPhases,
    RequiredCapability::OperationStatusRead,
    RequiredCapability::AssetUpload,
    RequiredCapability::AssetDownload,
    RequiredCapability::PhotoPublication,
];

/// Bits that stay false on any C1 server. They are recorded, never asserted:
/// upload resume in particular must not be confused with a retryable upload.
pub const FORBIDDEN_CAPABILITIES: [ForbiddenCapability; 3] = [
    ForbiddenCapability::UploadResume,
    ForbiddenCapability::Sharing,
    ForbiddenCapability::AppleAuthentication,
];

/// Bounds mirrored from the service's API key file rules so a malformed key is
/// reported deterministically instead of as an opaque 401.
pub const API_KEY_MIN_LEN: usize = 32;
pub const API_KEY_MAX_LEN: usize = 512;

/// One normalized native endpoint. It is always an origin root: no path
/// prefix, query, fragment or userinfo, and HTTPS unless the host is a literal
/// loopback address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEndpoint {
    base: Url,
}

impl NativeEndpoint {
    pub fn parse(raw: &str) -> SyncResult<Self> {
        let url = Url::parse(raw.trim()).map_err(|error| NativeProbeFailure::Endpoint {
            reason: format!("endpoint is not a valid URL: {error}"),
        })?;
        Self::from_url(url)
    }

    fn from_url(url: Url) -> SyncResult<Self> {
        if !url.username().is_empty() || url.password().is_some() {
            return Err(NativeProbeFailure::Endpoint {
                reason: "endpoint must not carry userinfo".into(),
            }
            .into());
        }
        if url.query().is_some() {
            return Err(NativeProbeFailure::Endpoint {
                reason: "endpoint must not carry a query".into(),
            }
            .into());
        }
        if url.fragment().is_some() {
            return Err(NativeProbeFailure::Endpoint {
                reason: "endpoint must not carry a fragment".into(),
            }
            .into());
        }
        // A base path would silently prefix every control and CloudKit call.
        if url.path() != "/" {
            return Err(NativeProbeFailure::Endpoint {
                reason: "endpoint must be an origin root without a path prefix".into(),
            }
            .into());
        }
        if url.port() == Some(0) {
            return Err(NativeProbeFailure::Endpoint {
                reason: "endpoint port must not be zero".into(),
            }
            .into());
        }
        let host = url
            .host_str()
            .ok_or(NativeProbeFailure::Endpoint {
                reason: "endpoint must contain a host".into(),
            })?
            .to_string();
        // A literal IP is required for cleartext, never a DNS name: a name can
        // resolve elsewhere, and `localhost` aliases are ambiguous.
        if url.scheme() == "http" && !is_loopback_literal(&host) {
            return Err(NativeProbeFailure::Endpoint {
                reason: "endpoint must use HTTPS (cleartext HTTP is allowed only for a literal \
                         loopback address)"
                    .into(),
            }
            .into());
        }
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(NativeProbeFailure::Endpoint {
                reason: format!("endpoint scheme {} is not supported", url.scheme()),
            }
            .into());
        }
        Ok(Self { base: url })
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    /// `(scheme, host, port)`. Two URLs share an origin only when all three
    /// agree, which is what keeps a Bearer key off an unrelated host.
    pub fn origin(&self) -> (String, String, Option<u16>) {
        (
            self.base.scheme().to_string(),
            self.base.host_str().unwrap_or_default().to_string(),
            self.base.port(),
        )
    }
}

/// `Host::is_loopback` was stabilized after the `url` version this crate locks
/// to, and `url` accepts hosts this check must reject, so the literal address
/// test is explicit. Only a literal IP qualifies: `localhost` is a DNS name.
fn is_loopback_literal(host: &str) -> bool {
    // `Url::host_str` keeps the brackets around an IPv6 literal.
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    let Ok(address) = host.parse::<IpAddr>() else {
        return false;
    };
    match address {
        IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST || v4.octets()[0] == 127,
        IpAddr::V6(v6) => v6 == Ipv6Addr::LOCALHOST,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredCapability {
    ZoneListing,
    RecordCrud,
    Snapshots,
    IncrementalChanges,
    UploadLeases,
    Fencing,
    TokenCurrentGate,
    UploadPhases,
    OperationStatusRead,
    AssetUpload,
    AssetDownload,
    PhotoPublication,
}

impl RequiredCapability {
    /// The exact wire key. The unit test pins these against a captured server
    /// payload so a rename cannot silently weaken the gate.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::ZoneListing => "zoneListing",
            Self::RecordCrud => "recordCrud",
            Self::Snapshots => "snapshots",
            Self::IncrementalChanges => "incrementalChanges",
            Self::UploadLeases => "uploadLeases",
            Self::Fencing => "fencing",
            Self::TokenCurrentGate => "tokenCurrentGate",
            Self::UploadPhases => "uploadPhases",
            Self::OperationStatusRead => "operationStatusRead",
            Self::AssetUpload => "assetUpload",
            Self::AssetDownload => "assetDownload",
            Self::PhotoPublication => "photoPublication",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForbiddenCapability {
    UploadResume,
    Sharing,
    AppleAuthentication,
}

impl ForbiddenCapability {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::UploadResume => "uploadResume",
            Self::Sharing => "sharing",
            Self::AppleAuthentication => "appleAuthentication",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCapabilityBits {
    pub zone_listing: bool,
    pub record_crud: bool,
    pub snapshots: bool,
    pub incremental_changes: bool,
    pub upload_leases: bool,
    pub fencing: bool,
    pub token_current_gate: bool,
    pub upload_phases: bool,
    pub operation_status_read: bool,
    pub asset_upload: bool,
    pub asset_download: bool,
    pub photo_publication: bool,
    /// Optional batch optimizations: recorded, never required.
    #[serde(default)]
    pub query_batch: bool,
    #[serde(default)]
    pub change_aliases: bool,
    /// Client features C1 never claims, whatever the server announces.
    #[serde(default)]
    pub apple_authentication: bool,
    #[serde(default)]
    pub sharing: bool,
    #[serde(default)]
    pub upload_resume: bool,
}

impl NativeCapabilityBits {
    fn required(&self, capability: RequiredCapability) -> bool {
        match capability {
            RequiredCapability::ZoneListing => self.zone_listing,
            RequiredCapability::RecordCrud => self.record_crud,
            RequiredCapability::Snapshots => self.snapshots,
            RequiredCapability::IncrementalChanges => self.incremental_changes,
            RequiredCapability::UploadLeases => self.upload_leases,
            RequiredCapability::Fencing => self.fencing,
            RequiredCapability::TokenCurrentGate => self.token_current_gate,
            RequiredCapability::UploadPhases => self.upload_phases,
            RequiredCapability::OperationStatusRead => self.operation_status_read,
            RequiredCapability::AssetUpload => self.asset_upload,
            RequiredCapability::AssetDownload => self.asset_download,
            RequiredCapability::PhotoPublication => self.photo_publication,
        }
    }

    fn forbidden(&self, capability: ForbiddenCapability) -> bool {
        match capability {
            ForbiddenCapability::UploadResume => self.upload_resume,
            ForbiddenCapability::Sharing => self.sharing,
            ForbiddenCapability::AppleAuthentication => self.apple_authentication,
        }
    }

    /// Names of the required bits that are not enabled. A server that announces
    /// `"stage":"M4"` while leaving `photoPublication` false is not usable, and
    /// the missing bit is what the user needs to see.
    pub fn missing_required(&self) -> Vec<&'static str> {
        REQUIRED_CAPABILITIES
            .into_iter()
            .filter(|capability| !self.required(*capability))
            .map(RequiredCapability::wire_name)
            .collect()
    }

    /// Bits C1 refuses to act on. They are reported, not rejected: a future
    /// server may add them, and their presence alone changes nothing here.
    pub fn claimed_optional_features(&self) -> Vec<&'static str> {
        FORBIDDEN_CAPABILITIES
            .into_iter()
            .filter(|capability| self.forbidden(*capability))
            .map(ForbiddenCapability::wire_name)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeNamespace {
    pub tenant: String,
    pub container: String,
    pub environment: String,
    pub database: String,
}

/// The server's effective configured limits, not protocol constants. C1 stores
/// them so later batch sizing follows this deployment instead of a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeLimits {
    pub max_asset_bytes: u64,
    pub thumbnail_max_bytes: u64,
    pub upload_concurrency: usize,
    pub download_concurrency: usize,
    pub upload_idle_timeout_secs: u64,
    pub upload_total_timeout_secs: u64,
    pub db_queue_capacity: usize,
    pub json_body_limit: usize,
    pub lease_ttl_secs: u64,
    pub lease_heartbeat_secs: u64,
    pub control_queue_capacity: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSyncLimits {
    pub max_page: usize,
    pub max_zones: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRecordLimits {
    pub max_operations: usize,
    pub max_record_bytes: u64,
    pub max_result_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCapabilitiesResponse {
    #[serde(default)]
    pub authentication: Option<super::auth::NativeAuthentication>,
    pub service: String,
    pub version: String,
    pub stage: String,
    pub namespace: NativeNamespace,
    /// Bare zone names, not `zoneID` objects.
    pub zones: Vec<String>,
    /// Always carries a trailing slash and never a path prefix. The spelling
    /// is the service's `publicBaseURL`, not camelCase's `publicBaseUrl`.
    #[serde(rename = "publicBaseURL")]
    pub public_base_url: String,
    /// UUID identifying the server's database file; a restored backup changes
    /// it, which invalidates applied progress.
    pub database_generation: String,
    pub schema_version: i64,
    pub capabilities: NativeCapabilityBits,
    pub limits: NativeLimits,
    pub protocol_version: i64,
    pub checksum_algorithm: String,
    pub history_retention: String,
    pub record_field_types: Vec<String>,
    pub sync_limits: NativeSyncLimits,
    pub record_limits: NativeRecordLimits,
}

/// What a successful probe pins onto the connection. Stored so later cycles
/// compare against the same namespace/generation instead of re-deriving it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePinnedScope {
    #[serde(default)]
    pub authentication: Option<super::auth::NativeAuthentication>,
    #[serde(rename = "publicBaseURL")]
    pub public_base_url: String,
    pub namespace: NativeNamespace,
    pub database_generation: String,
    pub protocol_version: i64,
    pub service_version: String,
    pub stage: String,
    pub checksum_algorithm: String,
    pub zones: Vec<String>,
    pub capabilities: NativeCapabilityBits,
    pub limits: NativeLimits,
    pub sync_limits: NativeSyncLimits,
    pub record_limits: NativeRecordLimits,
}

impl NativePinnedScope {
    pub fn has_zone(&self, zone_name: &str) -> bool {
        self.zones.iter().any(|zone| zone == zone_name)
    }
}

/// A native zone identity. C1 maps one job to one zone owned by one owner; the
/// owner is explicit because record names are unique per zone *and* owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeZone {
    pub zone_name: String,
    pub owner: String,
}

impl NativeZone {
    pub fn new(zone_name: impl Into<String>) -> Self {
        Self {
            zone_name: zone_name.into(),
            owner: super::DEFAULT_OWNER.to_string(),
        }
    }
}

/// Deterministic reasons a native connection cannot be accepted. These carry no
/// transport state, so the validation unit tests need no server.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NativeProbeFailure {
    #[error("native endpoint is invalid: {reason}")]
    Endpoint { reason: String },
    #[error("native API key is invalid: {reason}")]
    ApiKey { reason: String },
    #[error("expected the {SERVICE_NAME} service but the endpoint answered {found:?}")]
    UnexpectedService { found: String },
    #[error(
        "native protocol version {found} is not supported (this client speaks {SUPPORTED_PROTOCOL_VERSION})"
    )]
    ProtocolVersion { found: i64 },
    #[error("native server does not support required capabilities: {}", .missing.join(", "))]
    MissingCapability { missing: Vec<&'static str> },
    #[error("native server announces {CHECKSUM_ALGORITHM}-less digests: {found:?}")]
    ChecksumAlgorithm { found: String },
    #[error("native database generation {found:?} is not a valid identifier; rebuild is required")]
    DatabaseGeneration { found: String },
    #[error(
        "native server announces public base URL {announced}, which is not the configured origin \
         {configured}; alternate asset origins are not supported"
    )]
    PublicBaseUrlMismatch {
        announced: String,
        configured: String,
    },
    #[error("native zone {zone:?} is not configured on this server")]
    ZoneUnknown { zone: String },
}

impl From<NativeProbeFailure> for SyncError {
    fn from(failure: NativeProbeFailure) -> Self {
        SyncError::Unsupported(failure.to_string())
    }
}

impl From<reqwest::Error> for SyncError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() || error.is_connect() || error.is_request() {
            SyncError::Offline(error.without_url().to_string())
        } else {
            SyncError::Protocol(error.without_url().to_string())
        }
    }
}

/// Validate an API key before it leaves the process. The service rejects
/// anything outside 32..=512 printable non-whitespace ASCII; reporting that
/// here keeps a typo from surfacing as an indistinguishable 401.
pub fn validate_api_key(api_key: &str) -> Result<(), NativeProbeFailure> {
    if api_key.is_empty() {
        return Err(NativeProbeFailure::ApiKey {
            reason: "key must not be empty".into(),
        });
    }
    if api_key.len() < API_KEY_MIN_LEN || api_key.len() > API_KEY_MAX_LEN {
        return Err(NativeProbeFailure::ApiKey {
            reason: format!(
                "key length {} is outside {API_KEY_MIN_LEN}..={API_KEY_MAX_LEN} bytes",
                api_key.len()
            ),
        });
    }
    if !api_key
        .chars()
        .all(|character| character.is_ascii_graphic())
    {
        return Err(NativeProbeFailure::ApiKey {
            reason: "key must contain only printable non-whitespace ASCII".into(),
        });
    }
    Ok(())
}

/// Translate the SDK's error model into the decisions this client makes. The
/// variants are coarse on purpose; the rendered detail still carries the
/// service's `serverErrorCode` where a caller classifies it later.
pub(super) fn map_sdk_error(error: sync_client::Error) -> SyncError {
    use sync_client::Error as SdkError;
    match error {
        SdkError::Endpoint(detail) => {
            SyncError::Protocol(format!("native endpoint is invalid: {detail}"))
        }
        SdkError::Transport(detail) => SyncError::Offline(detail),
        SdkError::Protocol(detail) => SyncError::Protocol(detail),
        SdkError::Authentication(_) => SyncError::Authentication,
        SdkError::Api { status, failure } => map_failure(status, &failure),
        SdkError::InBand(failure) => SyncError::Protocol(failure.detail()),
        SdkError::LeaseLost => SyncError::Unsupported("UPLOAD_LEASE_LOST".into()),
        SdkError::LeaseBusy => SyncError::RateLimited,
        SdkError::PhaseMismatch(detail) => {
            SyncError::PreconditionFailed(format!("UPLOAD_PHASE_MISMATCH: {detail}"))
        }
        SdkError::NotCurrent(detail) => {
            SyncError::PreconditionFailed(format!("SYNC_NOT_CURRENT: {detail}"))
        }
        SdkError::ExpiredSyncToken => SyncError::Unsupported("EXPIRED_SYNC_TOKEN".into()),
        SdkError::NotFound(detail) => SyncError::NotFound(detail),
        SdkError::IdempotencyConflict => {
            SyncError::PreconditionFailed("IDEMPOTENCY_CONFLICT".into())
        }
        SdkError::QuotaExceeded => SyncError::Protocol("STORAGE_QUOTA_EXCEEDED".into()),
        SdkError::ChecksumMismatch { expected, actual } => SyncError::Protocol(format!(
            "CHECKSUM_MISMATCH: expected {expected}, computed {actual}"
        )),
        SdkError::SizeMismatch { expected, actual } => SyncError::Protocol(format!(
            "UPLOAD_SIZE_MISMATCH: expected {expected} bytes, received {actual}"
        )),
        SdkError::UploadRejected { code, reason } => {
            SyncError::Protocol(format!("{code}: {reason}"))
        }
        SdkError::UploadFailed(detail) => SyncError::Protocol(detail),
        SdkError::Io(error) => SyncError::Io(error),
        SdkError::Graph(error) => SyncError::Protocol(error.to_string()),
    }
}

fn map_failure(status: u16, failure: &sync_client::ServerFailure) -> SyncError {
    let code = failure.code.as_str();
    let reason = if failure.reason.is_empty() {
        code
    } else {
        failure.reason.as_str()
    };
    let detail = failure.detail();
    match code {
        // Every loss flavour ends the cycle irreversibly.
        "UPLOAD_LEASE_LOST" => SyncError::Unsupported("UPLOAD_LEASE_LOST".into()),
        "UPLOAD_LOCK_BUSY" => SyncError::RateLimited,
        "SYNC_NOT_CURRENT" => SyncError::PreconditionFailed(format!("SYNC_NOT_CURRENT: {detail}")),
        "UPLOAD_PHASE_MISMATCH" => {
            SyncError::PreconditionFailed(format!("UPLOAD_PHASE_MISMATCH: {detail}"))
        }
        "EXPIRED_SYNC_TOKEN" => SyncError::Unsupported(detail),
        "UNKNOWN_ITEM" => SyncError::NotFound(detail),
        _ => match status {
            401 | 403 => SyncError::Authentication,
            404 => SyncError::NotFound(detail),
            409 | 412 => SyncError::PreconditionFailed(format!("HTTP {status} {code}: {reason}")),
            413 => SyncError::Unsupported(format!("HTTP {status} {code}: {reason}")),
            429 | 503 => SyncError::RateLimited,
            _ => SyncError::Protocol(format!("HTTP {status} {code}: {reason}")),
        },
    }
}

#[derive(Clone)]
pub struct NativeClient {
    client: sync_client::Client,
    base: NativeEndpoint,
    /// The scope a successful probe pinned. Methods that need a namespace use
    /// it; a client that has not probed yet probes lazily on first use.
    pinned: Arc<RwLock<Option<NativePinnedScope>>>,
}

impl NativeClient {
    pub fn new(endpoint: &NativeEndpoint, api_key: &str) -> SyncResult<Self> {
        validate_api_key(api_key)?;
        let client = sync_client::Client::builder(endpoint.base.as_str())
            .map_err(map_sdk_error)?
            .access_token(api_key)
            .user_agent(concat!("PhotoViewer/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(map_sdk_error)?;
        Ok(Self {
            client,
            base: endpoint.clone(),
            pinned: Arc::new(RwLock::new(None)),
        })
    }

    pub fn endpoint(&self) -> &NativeEndpoint {
        &self.base
    }

    /// Exchange credentials for a session. The SDK owns the request; this
    /// client keeps its identity/expiry checks so a malformed answer never
    /// becomes a stored credential.
    pub async fn login(
        endpoint: &NativeEndpoint,
        username: &str,
        password: &str,
    ) -> SyncResult<super::auth::NativeSession> {
        if username.len() > 64 || password.len() > 1024 {
            return Err(SyncError::Authentication);
        }
        let client = sync_client::Client::builder(endpoint.base.as_str())
            .map_err(map_sdk_error)?
            .user_agent(concat!("PhotoViewer/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(map_sdk_error)?;
        let session = client
            .login(username, password)
            .await
            .map_err(map_sdk_error)?;
        validate_api_key(&session.access_token)?;
        if session.token_type != "Bearer"
            || uuid::Uuid::parse_str(&session.user.id).is_err()
            || !session.user.username.eq_ignore_ascii_case(username)
            || session.user.username.is_empty()
            || session.user.username.len() > 64
            || session.expires_at <= chrono::Utc::now().timestamp()
        {
            return Err(SyncError::Protocol(
                "invalid session identity or expiry".into(),
            ));
        }
        Ok(super::auth::NativeSession {
            access_token: session.access_token,
            token_type: session.token_type,
            expires_at: session.expires_at,
            user: super::auth::NativeUser {
                id: session.user.id,
                username: session.user.username,
            },
        })
    }

    pub async fn logout(&self) -> SyncResult<()> {
        self.client.logout().await.map_err(map_sdk_error)
    }

    fn store_scope(&self, scope: &NativePinnedScope) {
        if let Ok(mut slot) = self.pinned.write() {
            *slot = Some(scope.clone());
        }
    }

    fn cached_scope(&self) -> Option<NativePinnedScope> {
        self.pinned.read().ok().and_then(|scope| scope.clone())
    }

    /// A connection pinned to the last successful probe. A client used before
    /// an explicit probe performs one here, so every call has a namespace.
    pub(super) async fn connection(&self) -> SyncResult<sync_client::Connection> {
        let scope = match self.cached_scope() {
            Some(scope) => scope,
            None => self.probe().await?,
        };
        self.client
            .connect_with_scope(scope.sdk_scope())
            .await
            .map_err(map_sdk_error)
    }

    /// `GET /readyz`, unauthenticated. Confirms the process is serving before
    /// an authenticated call is blamed for the failure.
    pub async fn ready(&self) -> SyncResult<()> {
        self.client.ready().await.map_err(map_sdk_error)
    }

    pub async fn capabilities(&self) -> SyncResult<NativeCapabilitiesResponse> {
        let capabilities = self.client.capabilities().await.map_err(map_sdk_error)?;
        Ok(native_capabilities(capabilities))
    }

    /// Fetch the account's storage usage unless a recent reading is still good,
    /// and remember the result for the overview row.
    ///
    /// Call this only once a cycle has actually been refused for quota — which
    /// is what keeps it to one request per reading rather than one per attempt.
    /// It is deliberately not part of a cycle's outcome: failing to read a
    /// figure we chose to display must not turn a quota failure into some other
    /// failure.
    pub async fn ensure_storage_usage(&self) -> SyncResult<super::usage::StorageUsage> {
        if let Some(usage) = super::usage::cached() {
            return Ok(usage);
        }
        let connection = self.connection().await?;
        let usage = connection.storage_usage().await.map_err(map_sdk_error)?;
        let reading = super::usage::StorageUsage {
            quota_bytes: usage.quota_bytes,
            used_bytes: usage.used_bytes,
        };
        super::usage::record(reading);
        Ok(reading)
    }

    /// Health, then capabilities, then validation. Nothing is persisted here, so
    /// an invalid draft leaves no trace.
    pub async fn probe(&self) -> SyncResult<NativePinnedScope> {
        self.ready().await?;
        let capabilities = self.capabilities().await?;
        let scope = validate_capabilities(&self.base, &capabilities)?;
        self.store_scope(&scope);
        Ok(scope)
    }
}

impl NativePinnedScope {
    /// The SDK's view of the same pinned contract. The SDK re-checks the
    /// origin, protocol version, capabilities and namespace in
    /// `connect_with_scope`, so a scope read back from the database is never
    /// trusted merely because it parsed.
    pub(super) fn sdk_scope(&self) -> sync_client::PinnedScope {
        sync_client::PinnedScope {
            authentication: self.authentication.as_ref().map(|authentication| {
                sync_client::Authentication {
                    mode: authentication.mode.clone(),
                    user: authentication.user.as_ref().map(|user| {
                        sync_client::SessionUser {
                            id: user.id.clone(),
                            username: user.username.clone(),
                            // This client keeps no role model; the field is only
                            // carried for display and is not part of identity.
                            role: String::new(),
                        }
                    }),
                    resource_bearer_required: authentication.resource_bearer_required,
                    session_ttl_seconds: 0,
                }
            }),
            public_base_url: self.public_base_url.clone(),
            namespace: sync_client::Namespace {
                tenant: self.namespace.tenant.clone(),
                container: self.namespace.container.clone(),
                environment: self.namespace.environment.clone(),
                database: self.namespace.database.clone(),
            },
            database_generation: self.database_generation.clone(),
            protocol_version: self.protocol_version,
            service_version: self.service_version.clone(),
            stage: self.stage.clone(),
            checksum_algorithm: self.checksum_algorithm.clone(),
            zones: self.zones.clone(),
            capabilities: sync_client::CapabilityBits {
                zone_listing: self.capabilities.zone_listing,
                record_crud: self.capabilities.record_crud,
                snapshots: self.capabilities.snapshots,
                incremental_changes: self.capabilities.incremental_changes,
                upload_leases: self.capabilities.upload_leases,
                fencing: self.capabilities.fencing,
                token_current_gate: self.capabilities.token_current_gate,
                upload_phases: self.capabilities.upload_phases,
                operation_status_read: self.capabilities.operation_status_read,
                asset_upload: self.capabilities.asset_upload,
                asset_download: self.capabilities.asset_download,
                photo_publication: self.capabilities.photo_publication,
                storage_quota: false,
                storage_usage_by_media_type: false,
                query_batch: self.capabilities.query_batch,
                change_aliases: self.capabilities.change_aliases,
                upload_resume: self.capabilities.upload_resume,
                sharing: self.capabilities.sharing,
                apple_authentication: self.capabilities.apple_authentication,
            },
            limits: sync_client::Limits {
                max_asset_bytes: self.limits.max_asset_bytes,
                user_storage_quota_bytes: 0,
                thumbnail_max_bytes: self.limits.thumbnail_max_bytes,
                upload_concurrency: self.limits.upload_concurrency,
                download_concurrency: self.limits.download_concurrency,
                upload_idle_timeout_secs: self.limits.upload_idle_timeout_secs,
                upload_total_timeout_secs: self.limits.upload_total_timeout_secs,
                db_queue_capacity: self.limits.db_queue_capacity,
                json_body_limit: self.limits.json_body_limit,
                lease_ttl_secs: self.limits.lease_ttl_secs,
                lease_heartbeat_secs: self.limits.lease_heartbeat_secs,
                control_queue_capacity: self.limits.control_queue_capacity,
            },
            sync_limits: sync_client::SyncLimits {
                max_page: self.sync_limits.max_page,
                max_zones: self.sync_limits.max_zones,
            },
            record_limits: sync_client::RecordLimits {
                max_operations: self.record_limits.max_operations,
                max_record_bytes: self.record_limits.max_record_bytes,
                max_result_bytes: self.record_limits.max_result_bytes,
            },
        }
    }
}

/// Convert the SDK's decoded capability document into the client-owned shape
/// that is validated and persisted. This is a field-for-field map, not a second
/// protocol: the SDK parsed the wire; the local rules still decide acceptance.
fn native_capabilities(capabilities: sync_client::Capabilities) -> NativeCapabilitiesResponse {
    NativeCapabilitiesResponse {
        authentication: capabilities.authentication.map(|authentication| {
            super::auth::NativeAuthentication {
                mode: authentication.mode,
                user: authentication.user.map(|user| super::auth::NativeUser {
                    id: user.id,
                    username: user.username,
                }),
                resource_bearer_required: authentication.resource_bearer_required,
            }
        }),
        service: capabilities.service,
        version: capabilities.version,
        stage: capabilities.stage,
        namespace: NativeNamespace {
            tenant: capabilities.namespace.tenant,
            container: capabilities.namespace.container,
            environment: capabilities.namespace.environment,
            database: capabilities.namespace.database,
        },
        zones: capabilities.zones,
        public_base_url: capabilities.public_base_url,
        database_generation: capabilities.database_generation,
        schema_version: capabilities.schema_version,
        capabilities: NativeCapabilityBits {
            zone_listing: capabilities.capabilities.zone_listing,
            record_crud: capabilities.capabilities.record_crud,
            snapshots: capabilities.capabilities.snapshots,
            incremental_changes: capabilities.capabilities.incremental_changes,
            upload_leases: capabilities.capabilities.upload_leases,
            fencing: capabilities.capabilities.fencing,
            token_current_gate: capabilities.capabilities.token_current_gate,
            upload_phases: capabilities.capabilities.upload_phases,
            operation_status_read: capabilities.capabilities.operation_status_read,
            asset_upload: capabilities.capabilities.asset_upload,
            asset_download: capabilities.capabilities.asset_download,
            photo_publication: capabilities.capabilities.photo_publication,
            query_batch: capabilities.capabilities.query_batch,
            change_aliases: capabilities.capabilities.change_aliases,
            apple_authentication: capabilities.capabilities.apple_authentication,
            sharing: capabilities.capabilities.sharing,
            upload_resume: capabilities.capabilities.upload_resume,
        },
        limits: NativeLimits {
            max_asset_bytes: capabilities.limits.max_asset_bytes,
            thumbnail_max_bytes: capabilities.limits.thumbnail_max_bytes,
            upload_concurrency: capabilities.limits.upload_concurrency,
            download_concurrency: capabilities.limits.download_concurrency,
            upload_idle_timeout_secs: capabilities.limits.upload_idle_timeout_secs,
            upload_total_timeout_secs: capabilities.limits.upload_total_timeout_secs,
            db_queue_capacity: capabilities.limits.db_queue_capacity,
            json_body_limit: capabilities.limits.json_body_limit,
            lease_ttl_secs: capabilities.limits.lease_ttl_secs,
            lease_heartbeat_secs: capabilities.limits.lease_heartbeat_secs,
            control_queue_capacity: capabilities.limits.control_queue_capacity,
        },
        protocol_version: capabilities.protocol_version,
        checksum_algorithm: capabilities.checksum_algorithm,
        history_retention: capabilities.history_retention,
        record_field_types: capabilities.record_field_types,
        sync_limits: NativeSyncLimits {
            max_page: capabilities.sync_limits.max_page,
            max_zones: capabilities.sync_limits.max_zones,
        },
        record_limits: NativeRecordLimits {
            max_operations: capabilities.record_limits.max_operations,
            max_record_bytes: capabilities.record_limits.max_record_bytes,
            max_result_bytes: capabilities.record_limits.max_result_bytes,
        },
    }
}

/// Turn an announced capability document into the scope this connection pins.
/// Every rejection is deterministic: an unusable server must fail before it
/// becomes an effective configuration.
pub fn validate_capabilities(
    configured: &NativeEndpoint,
    response: &NativeCapabilitiesResponse,
) -> Result<NativePinnedScope, NativeProbeFailure> {
    if response.service != SERVICE_NAME {
        return Err(NativeProbeFailure::UnexpectedService {
            found: response.service.clone(),
        });
    }
    if response.protocol_version != SUPPORTED_PROTOCOL_VERSION {
        return Err(NativeProbeFailure::ProtocolVersion {
            found: response.protocol_version,
        });
    }
    let missing = response.capabilities.missing_required();
    if !missing.is_empty() {
        return Err(NativeProbeFailure::MissingCapability { missing });
    }
    if response.checksum_algorithm != CHECKSUM_ALGORITHM {
        return Err(NativeProbeFailure::ChecksumAlgorithm {
            found: response.checksum_algorithm.clone(),
        });
    }
    if !is_uuid(&response.database_generation) {
        return Err(NativeProbeFailure::DatabaseGeneration {
            found: response.database_generation.clone(),
        });
    }
    // C1 serves assets from the same origin as control calls, so the announced
    // base must be the configured one. This is what stops a signed asset URL
    // from becoming a credential delivered to an unrelated host.
    let announced = Url::parse(&response.public_base_url).map_err(|error| {
        NativeProbeFailure::PublicBaseUrlMismatch {
            announced: response.public_base_url.clone(),
            configured: format!(
                "{} (invalid announced URL: {error})",
                describe_origin(configured)
            ),
        }
    })?;
    let announced_origin = (
        announced.scheme().to_string(),
        announced.host_str().unwrap_or_default().to_string(),
        announced.port(),
    );
    let configured_origin = configured.origin();
    if announced_origin != configured_origin || announced.path() != "/" {
        return Err(NativeProbeFailure::PublicBaseUrlMismatch {
            announced: response.public_base_url.clone(),
            configured: describe_origin(configured),
        });
    }

    Ok(NativePinnedScope {
        authentication: response.authentication.clone(),
        public_base_url: response.public_base_url.clone(),
        namespace: response.namespace.clone(),
        database_generation: response.database_generation.clone(),
        protocol_version: response.protocol_version,
        service_version: response.version.clone(),
        stage: response.stage.clone(),
        checksum_algorithm: response.checksum_algorithm.clone(),
        zones: response.zones.clone(),
        capabilities: response.capabilities,
        limits: response.limits,
        sync_limits: response.sync_limits,
        record_limits: response.record_limits,
    })
}

/// Reject a zone the server does not serve before a job configuration is
/// accepted, so a job can never be created against a zone that cannot exist.
pub fn validate_zone(
    pinned: &NativePinnedScope,
    zone_name: &str,
) -> Result<NativeZone, NativeProbeFailure> {
    let zone_name = zone_name.trim();
    if zone_name.is_empty() {
        return Err(NativeProbeFailure::ZoneUnknown {
            zone: zone_name.to_string(),
        });
    }
    if !pinned.has_zone(zone_name) {
        return Err(NativeProbeFailure::ZoneUnknown {
            zone: zone_name.to_string(),
        });
    }
    Ok(NativeZone::new(zone_name))
}

fn describe_origin(endpoint: &NativeEndpoint) -> String {
    let (scheme, host, port) = endpoint.origin();
    match port {
        Some(port) => format!("{scheme}://{host}:{port}/"),
        None => format!("{scheme}://{host}/"),
    }
}

/// Hyphenated lowercase UUID, checked without pulling `uuid` parsing into the
/// transport module's failure path.
fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => *byte == b'-',
        _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
    })
}

/// A scope shaped like a real M4 server's, for store and service tests.
///
/// `#[cfg(test)]` keeps it out of the shipped binary: a production caller must
/// fill every field from an actual probe, because "all required capabilities
/// present" is exactly the claim a fixture must never be allowed to make on its
/// own.
#[cfg(test)]
pub(crate) fn demo_pinned_scope(public_base_url: &str, zones: &[&str]) -> NativePinnedScope {
    NativePinnedScope {
        authentication: None,
        public_base_url: public_base_url.into(),
        namespace: NativeNamespace {
            tenant: "local".into(),
            container: "com.apple.photos.cloud".into(),
            environment: "production".into(),
            database: "private".into(),
        },
        database_generation: "501855bb-18d3-4008-abc3-37e4948bf6f0".into(),
        protocol_version: SUPPORTED_PROTOCOL_VERSION,
        service_version: "0.1.0".into(),
        stage: "M4".into(),
        checksum_algorithm: CHECKSUM_ALGORITHM.into(),
        zones: zones.iter().map(|zone| (*zone).to_string()).collect(),
        capabilities: NativeCapabilityBits {
            zone_listing: true,
            record_crud: true,
            snapshots: true,
            incremental_changes: true,
            upload_leases: true,
            fencing: true,
            token_current_gate: true,
            upload_phases: true,
            operation_status_read: true,
            asset_upload: true,
            asset_download: true,
            photo_publication: true,
            query_batch: false,
            change_aliases: false,
            apple_authentication: false,
            sharing: false,
            upload_resume: false,
        },
        limits: NativeLimits {
            max_asset_bytes: 536_870_912,
            thumbnail_max_bytes: 2_097_152,
            upload_concurrency: 2,
            download_concurrency: 8,
            upload_idle_timeout_secs: 30,
            upload_total_timeout_secs: 3600,
            db_queue_capacity: 128,
            json_body_limit: 1_048_576,
            lease_ttl_secs: 90,
            lease_heartbeat_secs: 30,
            control_queue_capacity: 16,
        },
        sync_limits: NativeSyncLimits {
            max_page: 200,
            max_zones: 20,
        },
        record_limits: NativeRecordLimits {
            max_operations: 400,
            max_record_bytes: 1_048_576,
            max_result_bytes: 8_388_608,
        },
    }
}

#[cfg(test)]
mod tests;
