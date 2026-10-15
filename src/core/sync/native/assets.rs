//! Asset reservation, byte upload, and download.
//!
//! Three things about this protocol are easy to get wrong, so they are spelled
//! out here rather than left to the call sites:
//!
//! * **A digest is stated twice, in two encodings.** A reservation declares
//!   `expectedSha256` in lowercase hex; a descriptor returns `fileChecksum` in
//!   canonical Base64; and the photo's `thumbnailSourceOriginalSHA256` is hex
//!   again. `ContentDigest::hex_from_descriptor_base64` is the join — never a
//!   second hash of the bytes.
//! * **The upload URL carries its own authority.** It is a capability URL, so
//!   the request that uses it must **not** also carry `Authorization`: the
//!   service treats the two as contradictory. Only the lease proof travels.
//! * **There is no resume.** A failed session is `Failed` and its bytes are
//!   gone; the recovery is a fresh reservation, never a retry of the old
//!   session. Treating "already uploaded" as "must retry" produces a duplicate
//!   asset every cycle.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::sync::{SyncError, SyncResult};

use super::lease::LeaseProof;
use super::protocol::{map_status, NativeClient, NativeErrorBody, NativeNamespace, NativeZone};
use super::receive::ZoneId;
use super::records::AssetDescriptor;
use crate::core::sync::content::ContentDigest;

/// The two roles a photo's resources have. They are bound to one master record
/// and are never interchangeable: a thumbnail in the original's field, or the
/// reverse, is a different photo as far as the service is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetRole {
    Original,
    Thumbnail,
}

impl AssetRole {
    /// The field on `CPLMaster` this role is bound to. The service refuses any
    /// other pairing.
    pub fn field_name(self) -> &'static str {
        match self {
            Self::Original => "resOriginalRes",
            Self::Thumbnail => "resJPEGThumbRes",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Thumbnail => "thumbnail",
        }
    }
}

/// Whether a reservation creates the binding or replaces it. This must equal
/// the lease's current phase, so a client cannot reserve `create` while it is
/// still walking `modify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadIntent {
    Create,
    Update,
}

impl UploadIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReserveRequest<'a> {
    #[serde(rename = "zoneID")]
    zone: &'a ZoneId,
    tokens: Vec<ReserveToken<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReserveToken<'a> {
    #[serde(rename = "recordName")]
    record_name: &'a str,
    #[serde(rename = "recordType")]
    record_type: &'static str,
    #[serde(rename = "fieldName")]
    field_name: &'static str,
    #[serde(rename = "_syncserver")]
    options: ReserveOptions<'a>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReserveOptions<'a> {
    role: AssetRole,
    intent: UploadIntent,
    #[serde(rename = "expectedSize")]
    expected_size: u64,
    #[serde(rename = "expectedSha256")]
    expected_sha256: &'a str,
}

/// One resource to reserve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRequest {
    pub role: AssetRole,
    pub intent: UploadIntent,
    pub digest: ContentDigest,
}

/// A reservation the service granted.
#[derive(Debug, Clone, PartialEq)]
pub struct Reservation {
    pub role: AssetRole,
    pub record_name: String,
    pub field_name: String,
    pub session_id: String,
    pub state: SessionState,
    /// The capability URL the bytes go to. Never stored: it expires.
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SessionState {
    Reserved,
    Receiving,
    PublishPrepared,
    Published,
    Failed,
    Expired,
}

impl SessionState {
    pub fn is_terminal_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Expired)
    }

    pub fn is_published(self) -> bool {
        matches!(self, Self::Published)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveResponse {
    tokens: Vec<ReserveTokenResponse>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveTokenResponse {
    #[serde(rename = "recordName")]
    record_name: String,
    #[serde(rename = "fieldName")]
    field_name: String,
    /// Absent when the session is already Published and the answer is the
    /// descriptor itself.
    #[serde(default)]
    url: Option<String>,
    #[serde(rename = "_syncserver", default)]
    options: Option<ReserveTokenOptions>,
    #[serde(default)]
    asset: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveTokenOptions {
    #[serde(rename = "sessionId")]
    session_id: String,
    state: SessionState,
}

/// What a session turned out to be.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub state: SessionState,
    /// The descriptor, once the bytes are durably published. `None` for every
    /// other state, which is exactly why an asset id may only be trusted here.
    #[serde(default)]
    pub asset: Option<AssetDescriptor>,
    /// Why it failed, when it did.
    #[serde(rename = "serverErrorCode", default)]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadResponse {
    #[serde(rename = "singleFile")]
    single_file: AssetDescriptor,
}

/// A resource the service accepted.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadDescriptor {
    #[serde(flatten)]
    pub descriptor: AssetDescriptor,
    /// Freshly signed on every read. Never stored as identity and never
    /// persisted: it expires, and a stored one would silently fail later.
    #[serde(rename = "downloadURL")]
    pub download_url: String,
    #[serde(rename = "receipt")]
    pub receipt: String,
}

impl DownloadDescriptor {
    pub fn asset_id(&self) -> &str {
        &self.descriptor.asset_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferError {
    /// The bytes did not match the digest declared at reservation.
    ChecksumMismatch(String),
    /// The declared size and the sent size disagreed.
    SizeMismatch(String),
    /// The service refused this thumbnail's format or dimensions.
    InvalidThumbnail(String),
    /// The session failed and has no bytes left. Recovery is a new reservation.
    SessionFailed(String),
    /// The lease ended during the transfer.
    LeaseLost,
    Transport(String),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChecksumMismatch(reason) => write!(formatter, "checksum mismatch: {reason}"),
            Self::SizeMismatch(reason) => write!(formatter, "size mismatch: {reason}"),
            Self::InvalidThumbnail(reason) => write!(formatter, "invalid thumbnail: {reason}"),
            Self::SessionFailed(reason) => write!(formatter, "upload session failed: {reason}"),
            Self::LeaseLost => write!(formatter, "the upload lease was lost"),
            Self::Transport(reason) => write!(formatter, "transfer failed: {reason}"),
        }
    }
}

impl From<TransferError> for SyncError {
    fn from(error: TransferError) -> Self {
        match &error {
            TransferError::LeaseLost => {
                SyncError::Unsupported(super::lease::UPLOAD_LEASE_LOST.to_string())
            }
            TransferError::ChecksumMismatch(_)
            | TransferError::SizeMismatch(_)
            | TransferError::InvalidThumbnail(_)
            | TransferError::SessionFailed(_) => SyncError::Protocol(error.to_string()),
            TransferError::Transport(_) => SyncError::Offline(error.to_string()),
        }
    }
}

impl NativeClient {
    /// Reserve one master's resources, in a single request.
    ///
    /// Both roles go together because the service binds a pair atomically:
    /// reserving one and failing the other would leave a session with no
    /// counterpart, and sessions cannot be resumed — the recovery would be a
    /// second reservation, not a continuation.
    pub async fn reserve_assets(
        &self,
        namespace: &NativeNamespace,
        zone: &NativeZone,
        proof: &LeaseProof,
        master_record: &str,
        intent: UploadIntent,
        requests: &[ReservationRequest],
    ) -> SyncResult<Vec<Reservation>> {
        let url = self
            .cloudkit_url(namespace, &["assets", "upload"])
            .map_err(|error| SyncError::Protocol(error.to_string()))?;
        let zone_id = ZoneId::from(zone);
        let tokens: Vec<ReserveToken<'_>> = requests
            .iter()
            .map(|request| ReserveToken {
                record_name: master_record,
                record_type: super::receive::MASTER_RECORD_TYPE,
                field_name: request.role.field_name(),
                options: ReserveOptions {
                    role: request.role,
                    intent,
                    expected_size: request.digest.size,
                    // Hex, not Base64: the reservation validates this against
                    // `[0-9a-f]{64}`.
                    expected_sha256: &request.digest.hex,
                },
            })
            .collect();
        let body = ReserveRequest {
            zone: &zone_id,
            tokens,
        };
        let payload = serde_json::to_vec(&body).map_err(|error| {
            SyncError::Protocol(format!("reservation is not encodable: {error}"))
        })?;
        // A master has many content generations and each role can be reserved
        // independently. Bind retries to this exact request and lease, rather
        // than reusing one permanent key for every upload of the master.
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(&payload);
        hash.update(proof.lease_id.as_bytes());
        hash.update(proof.fencing_token.as_bytes());
        let key = format!("reserve-{:x}", hash.finalize());
        let (status, bytes) = self
            .send_lease_json(&url, payload, Some(proof), Some(&key))
            .await?;
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &bytes));
        }
        let response: ReserveResponse = serde_json::from_slice(&bytes).map_err(|error| {
            SyncError::Protocol(format!("reservation is not decodable: {error}"))
        })?;
        let mut granted = Vec::with_capacity(response.tokens.len());
        for token in response.tokens {
            let options = token.options.ok_or_else(|| {
                SyncError::Protocol(format!(
                    "reservation for {} carried no session",
                    token.field_name
                ))
            })?;
            granted.push(Reservation {
                role: role_from_field(&token.field_name),
                record_name: token.record_name,
                field_name: token.field_name,
                session_id: options.session_id,
                state: options.state,
                url: token.url.unwrap_or_default(),
            });
        }
        // A reservation that quietly granted fewer resources than asked for
        // would leave the pair half-bound, so it is refused rather than used.
        if granted.len() != requests.len() {
            return Err(SyncError::Protocol(format!(
                "the service granted {} of {} requested resources",
                granted.len(),
                requests.len()
            )));
        }
        Ok(granted)
    }

    /// Upload one resource's bytes to its reserved session.
    pub async fn upload_asset_bytes(
        &self,
        reservation: &Reservation,
        proof: &LeaseProof,
        bytes: &[u8],
    ) -> std::result::Result<AssetDescriptor, TransferError> {
        let (status, body) = self
            .send_upload_bytes(&reservation.url, proof, bytes)
            .await?;
        if !status.is_success() {
            return Err(transfer_error(status.as_u16(), &body));
        }
        let response: UploadResponse = serde_json::from_slice(&body).map_err(|error| {
            TransferError::Transport(format!("upload response is not decodable: {error}"))
        })?;
        Ok(response.single_file)
    }

    pub(crate) async fn upload_file(
        &self,
        reservation: &Reservation,
        proof: &LeaseProof,
        path: &std::path::Path,
        expected: &ContentDigest,
    ) -> SyncResult<AssetDescriptor> {
        let url = url::Url::parse(&reservation.url)
            .map_err(|_| SyncError::Protocol("invalid upload capability".into()))?;
        self.check_origin(&url)?;
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|_| SyncError::Protocol("upload snapshot is missing".into()))?;
        if file
            .metadata()
            .await
            .map_err(|_| SyncError::Protocol("cannot inspect upload snapshot".into()))?
            .len()
            != expected.size
        {
            return Err(SyncError::Protocol("upload snapshot size changed".into()));
        }
        let stream = tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024);
        // The normal request builder is private to the protocol adapter.
        let response = self
            .upload_stream(
                &url,
                proof,
                reqwest::Body::wrap_stream(stream),
                expected.size,
            )
            .await?;
        let descriptor: AssetDescriptor = serde_json::from_value(response["singleFile"].clone())
            .map_err(|_| SyncError::Protocol("invalid upload descriptor".into()))?;
        if descriptor.size != expected.size || descriptor.file_checksum != expected.base64 {
            return Err(SyncError::Protocol(
                "uploaded resource does not match its snapshot".into(),
            ));
        }
        Ok(descriptor)
    }

    pub(crate) async fn download_to(
        &self,
        descriptor: &DownloadDescriptor,
        path: &std::path::Path,
        expected: &ContentDigest,
    ) -> SyncResult<()> {
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncWriteExt;
        let url = url::Url::parse(&descriptor.download_url)
            .map_err(|_| SyncError::Protocol("invalid download capability".into()))?;
        if descriptor.descriptor.size != expected.size
            || descriptor.descriptor.file_checksum != expected.base64
        {
            return Err(SyncError::Protocol(
                "resource description changed identity".into(),
            ));
        }
        let mut response = self.capability_get(&url).await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(SyncError::Protocol(format!(
                "resource download failed with {}",
                response.status()
            )));
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
            .map_err(|_| SyncError::Protocol("cannot create owned download staging".into()))?;
        let mut size = 0u64;
        let mut hash = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            size = size
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| SyncError::Protocol("download size overflow".into()))?;
            if size > expected.size {
                return Err(SyncError::Protocol("download exceeds declared size".into()));
            }
            hash.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| SyncError::Protocol("download staging write failed".into()))?;
        }
        if size != expected.size
            || crate::core::sync::content::to_hex(&hash.finalize()) != expected.hex
        {
            return Err(SyncError::Protocol(
                "download checksum/size mismatch".into(),
            ));
        }
        file.sync_all()
            .await
            .map_err(|_| SyncError::Protocol("download staging flush failed".into()))?;
        Ok(())
    }

    /// What a session turned out to be. Readable without a lease, which is what
    /// makes reconciling an uncertain outcome possible at all.
    pub async fn upload_session(&self, session_id: &str) -> SyncResult<SessionResponse> {
        let url = self.path(&["v1", "uploads", session_id])?;
        let response = self.authorized_get(&url).await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &bytes));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            SyncError::Protocol(format!("session status is not decodable: {error}"))
        })
    }

    /// Extend a reserved session. Only possible while nothing has been sent:
    /// once bytes are in flight the outcome is whatever the body said.
    pub async fn renew_upload_session(
        &self,
        session_id: &str,
        proof: &LeaseProof,
    ) -> SyncResult<Reservation> {
        let url = self.path(&["v1", "uploads", session_id, "renew"])?;
        let (status, bytes) = self
            .send_lease_json(&url, b"{}".to_vec(), Some(proof), None)
            .await?;
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &bytes));
        }
        let renewed: ReserveTokenResponse = serde_json::from_slice(&bytes).map_err(|error| {
            SyncError::Protocol(format!("renewed session is not decodable: {error}"))
        })?;
        Ok(Reservation {
            role: role_from_field(&renewed.field_name),
            record_name: renewed.record_name.clone(),
            field_name: renewed.field_name,
            session_id: renewed
                .options
                .as_ref()
                .map(|options| options.session_id.clone())
                .unwrap_or_else(|| session_id.to_string()),
            state: renewed
                .options
                .map(|options| options.state)
                .unwrap_or(SessionState::Reserved),
            url: renewed.url.unwrap_or_default(),
        })
    }

    /// A fresh capability for an existing asset, plus its authoritative
    /// descriptor. The `role` is part of identity here: asking for a role the
    /// asset never had is a miss, not a lesser truth.
    pub async fn describe_asset(
        &self,
        asset_id: &str,
        role: AssetRole,
    ) -> SyncResult<DownloadDescriptor> {
        let url = with_query(
            &self.path(&["v1", "assets", asset_id])?,
            &[("role", role.as_str())],
        )?;
        let response = self.authorized_get(&url).await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &bytes));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            SyncError::Protocol(format!("asset description is not decodable: {error}"))
        })
    }

    /// The asset's bytes.
    ///
    /// `if_range` carries the ETag a resumed transfer started from. A server
    /// whose asset changed answers with a full 200, which is `continues() ==
    /// false`: the caller must restart rather than append, because the two
    /// halves would not be one version.
    pub async fn download_asset(
        &self,
        descriptor: &DownloadDescriptor,
        role: AssetRole,
        if_range: Option<&str>,
    ) -> SyncResult<AssetBytes> {
        // The descriptor's own URL is used verbatim. Rebuilding it from the
        // asset id and a role would drop the `expires` and `token` capability
        // parameters, and the request would fail authentication — the URL *is*
        // the authority, not a hint about where to build one.
        let url = url::Url::parse(&descriptor.download_url).map_err(|error| {
            SyncError::Protocol(format!("the download URL is unusable: {error}"))
        })?;
        // That capability authorizes the request on its own, so it carries **no**
        // `Authorization` header: the service treats a capability URL and a
        // Bearer key on the same request as contradictory. Describing an asset
        // still uses the ordinary authenticated path.
        let _ = role;
        let response = self.capability_get(&url).await?;
        let status = response.status();
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_range = response_range(&response);
        let bytes = response.bytes().await?;
        match status.as_u16() {
            200 | 206 => Ok(AssetBytes {
                bytes: bytes.to_vec(),
                etag,
                // Only a 206 continues a resume. A 200 in answer to a request
                // that expected a range means the asset changed underneath us.
                partial: status.as_u16() == 206 && if_range.is_some(),
                content_range,
            }),
            // A 304 is usable only against a copy this client already verified;
            // it cannot be interpreted without that context.
            304 => Ok(AssetBytes {
                bytes: Vec::new(),
                etag,
                partial: false,
                content_range,
            }),
            status => Err(map_status(status, &bytes)),
        }
    }

    async fn send_upload_bytes(
        &self,
        url: &str,
        proof: &LeaseProof,
        bytes: &[u8],
    ) -> std::result::Result<(reqwest::StatusCode, Vec<u8>), TransferError> {
        self.upload_bytes_request(url, proof, bytes)
            .await
            .map_err(|error| TransferError::Transport(error.to_string()))
    }
}

fn role_from_field(field_name: &str) -> AssetRole {
    match field_name {
        "resOriginalRes" => AssetRole::Original,
        _ => AssetRole::Thumbnail,
    }
}

fn response_range(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Downloaded bytes plus the headers that say what they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetBytes {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
    /// True for a 206. A full 200 in answer to a ranged request means the
    /// asset changed: the caller restarts rather than appending.
    pub partial: bool,
    pub content_range: Option<String>,
}

impl AssetBytes {
    /// Whether these bytes continue a resumed transfer rather than replacing
    /// what was already written.
    pub fn continues(&self) -> bool {
        self.partial
    }
}

fn with_query(base: &url::Url, pairs: &[(&str, &str)]) -> SyncResult<url::Url> {
    let mut url = base.clone();
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
    }
    Ok(url)
}

/// Map a failed transfer onto the recovery it implies.
fn transfer_error(status: u16, body: &[u8]) -> TransferError {
    let code = NativeErrorBody::parse(body)
        .and_then(|error| error.server_error_code)
        .unwrap_or_default();
    let reason = NativeErrorBody::parse(body)
        .and_then(|error| error.reason)
        .unwrap_or_else(|| format!("HTTP {status}"));
    match code.as_str() {
        "CHECKSUM_MISMATCH" => TransferError::ChecksumMismatch(reason),
        "UPLOAD_SIZE_MISMATCH" => TransferError::SizeMismatch(reason),
        "INVALID_THUMBNAIL" => TransferError::InvalidThumbnail(reason),
        "UPLOAD_LEASE_LOST" => TransferError::LeaseLost,
        "UPLOAD_TIMEOUT" | "UPLOAD_INTERRUPTED" => TransferError::SessionFailed(reason),
        _ => TransferError::Transport(format!("{code}: {reason}")),
    }
}

#[cfg(test)]
mod tests;
