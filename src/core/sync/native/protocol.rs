//! Typed wire boundary for the native SyncServer service.
//!
//! Everything here is transport and DTOs: no SQL, no GTK. The field names are
//! transcribed from the service's own `GET /v1/capabilities` payload
//! (`syncserver@2d9f9a7`, stage M4, protocolVersion 1) and are deliberately
//! tolerant of unknown future members so a newer server cannot fail to parse.
//!
//! See `docs/modules/native-sync-protocol.md` for the contract these types
//! implement. Later milestones add delta/lease/asset calls here.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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

    fn join(&self, segments: &[&str]) -> SyncResult<Url> {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| NativeProbeFailure::Endpoint {
                    reason: "endpoint cannot be extended with path segments".into(),
                })?;
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
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
            SyncError::Offline(error.to_string())
        } else {
            SyncError::Protocol(error.to_string())
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

/// The error envelope the service uses at both transport and batch level: a
/// non-2xx body and an in-band `serverErrorCode` inside an HTTP 200 carry the
/// same two keys, so one parser covers both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeErrorBody {
    #[serde(default)]
    pub server_error_code: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

impl NativeErrorBody {
    pub fn parse(body: &[u8]) -> Option<Self> {
        serde_json::from_slice(body).ok()
    }
}

fn describe(status: u16, body: &[u8]) -> String {
    match NativeErrorBody::parse(body) {
        Some(error) => match (error.server_error_code, error.reason) {
            (Some(code), Some(reason)) => format!("HTTP {status} {code}: {reason}"),
            (Some(code), None) => format!("HTTP {status} {code}"),
            (None, Some(reason)) => format!("HTTP {status}: {reason}"),
            (None, None) => format!("HTTP {status}"),
        },
        None => format!("HTTP {status}"),
    }
}

pub(super) fn map_status(status: u16, body: &[u8]) -> SyncError {
    let detail = describe(status, body);
    match status {
        401 | 403 => SyncError::Authentication,
        404 => SyncError::NotFound(detail),
        409 | 412 => SyncError::PreconditionFailed(detail),
        413 => SyncError::Unsupported(detail),
        429 | 503 => SyncError::RateLimited,
        _ => SyncError::Protocol(detail),
    }
}

/// Maximum accepted `/v1/capabilities` body. The payload is a few kilobytes;
/// the cap stops a hostile endpoint from streaming an unbounded document.
const MAX_CAPABILITIES_BYTES: u64 = 256 * 1024;

#[derive(Clone)]
pub struct NativeClient {
    client: reqwest::Client,
    base: NativeEndpoint,
    api_key: String,
}

impl NativeClient {
    pub fn new(endpoint: &NativeEndpoint, api_key: &str) -> SyncResult<Self> {
        validate_api_key(api_key)?;
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(60))
            // A cross-origin redirect would carry the Bearer key to a host the
            // user never configured, so redirects are never followed.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("PhotoViewer/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            base: endpoint.clone(),
            api_key: api_key.to_string(),
        })
    }

    pub fn endpoint(&self) -> &NativeEndpoint {
        &self.base
    }

    /// Absolute URL for a path under the configured origin.
    pub(super) fn path(&self, segments: &[&str]) -> SyncResult<Url> {
        self.base.join(segments)
    }

    /// The single JSON POST path. Auth, the shared error envelope, the response
    /// size ceiling and the in-band-error check all live here so every control
    /// and metadata call is treated identically.
    pub(super) async fn post_json<B: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        url: &Url,
        body: &B,
    ) -> SyncResult<R> {
        super::receive::post_json(self, url, body).await
    }

    /// POST a body that must be exactly the given bytes, for the one endpoint
    /// whose request shape is an empty object rather than a DTO.
    pub(super) async fn post_json_raw<R: serde::de::DeserializeOwned>(
        &self,
        url: &Url,
        body: &[u8],
    ) -> SyncResult<R> {
        super::receive::post_json_raw(self, url, body).await
    }

    fn request(&self, url: Url) -> reqwest::RequestBuilder {
        self.client.get(url).bearer_auth(&self.api_key)
    }

    /// Authenticated JSON POST carrying a lease proof.
    ///
    /// A lease-bearing call and an ordinary one share everything except the
    /// three proof headers, so both go through the same request builder rather
    /// than two that could drift.
    pub(super) async fn send_lease_json(
        &self,
        url: &Url,
        payload: Vec<u8>,
        proof: Option<&super::lease::LeaseProof>,
        idempotency_key: Option<&str>,
    ) -> SyncResult<(reqwest::StatusCode, Vec<u8>)> {
        let mut request = self
            .client
            .post(url.clone())
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload);
        if let Some(proof) = proof {
            for (name, value) in super::lease::lease_headers(proof) {
                request = request.header(name, value);
            }
        }
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        let response = request.send().await?;
        let status = response.status();
        Ok((status, response.bytes().await?.to_vec()))
    }

    /// Authenticated JSON POST carrying a lease proof and an optional
    /// idempotency key, returning the raw status and body.
    ///
    /// The caller decides what a status means: a lease call maps its failures
    /// onto arbitration decisions, while a record mutation has to read an
    /// in-band error inside a 200. Both go through the same request so the
    /// proof headers cannot drift apart.
    pub async fn send_with_proof(
        &self,
        url: &Url,
        payload: Vec<u8>,
        proof: &super::lease::LeaseProof,
        idempotency_key: Option<&str>,
    ) -> SyncResult<(reqwest::StatusCode, Vec<u8>)> {
        self.send_lease_json(url, payload, Some(proof), idempotency_key)
            .await
    }

    /// Authenticated GET. Reconciling an uncertain outcome has to work after
    /// the lease that issued it is gone, so this carries no lease proof.
    pub(super) async fn authorized_get(&self, url: &Url) -> SyncResult<reqwest::Response> {
        Ok(self.request(url.clone()).send().await?)
    }

    /// Authenticated JSON POST. This is the only path that attaches the Bearer
    /// key to a body-bearing request, so there is one place to audit.
    pub(super) async fn send_json(
        &self,
        url: &Url,
        payload: Vec<u8>,
    ) -> Result<reqwest::Response, crate::core::sync::SyncError> {
        Ok(self
            .client
            .post(url.clone())
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload)
            .send()
            .await?)
    }

    /// `GET /readyz`, unauthenticated. Confirms the process is serving before
    /// an authenticated call is blamed for the failure.
    pub async fn ready(&self) -> SyncResult<()> {
        let url = self.base.join(&["readyz"])?;
        let response = self.client.get(url).send().await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &body));
        }
        let ready: serde_json::Value = serde_json::from_slice(&body).map_err(|error| {
            SyncError::Protocol(format!("readiness response is not JSON: {error}"))
        })?;
        if ready.get("status").and_then(|value| value.as_str()) != Some("ready") {
            return Err(SyncError::Offline(
                "endpoint is reachable but not ready to serve".into(),
            ));
        }
        Ok(())
    }

    pub async fn capabilities(&self) -> SyncResult<NativeCapabilitiesResponse> {
        let url = self.base.join(&["v1", "capabilities"])?;
        let response = self.request(url).send().await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &body));
        }
        if body.len() as u64 > MAX_CAPABILITIES_BYTES {
            return Err(SyncError::Protocol(
                "capabilities response exceeds the accepted size".into(),
            ));
        }
        serde_json::from_slice(&body).map_err(|error| {
            SyncError::Protocol(format!("capabilities are not decodable: {error}"))
        })
    }

    /// Health, then capabilities, then validation. Nothing is persisted here, so
    /// an invalid draft leaves no trace.
    pub async fn probe(&self) -> SyncResult<NativePinnedScope> {
        self.ready().await?;
        let capabilities = self.capabilities().await?;
        Ok(validate_capabilities(&self.base, &capabilities)?)
    }

    /// The CloudKit-shaped base every metadata call hangs off.
    ///
    /// The container is taken from the pinned namespace rather than hardcoded,
    /// and `path_segments_mut` percent-encodes each segment — so a container
    /// containing `/` or `..` stays inside its own segment instead of walking
    /// the URL.
    pub fn cloudkit_base(&self, namespace: &NativeNamespace) -> SyncResult<Url> {
        if namespace.container.is_empty()
            || namespace.environment.is_empty()
            || namespace.database.is_empty()
        {
            return Err(SyncError::Protocol("pinned namespace is incomplete".into()));
        }
        self.base.join(&[
            "database",
            "1",
            &namespace.container,
            &namespace.environment,
            &namespace.database,
        ])
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
