//! Asset reservation, byte upload, and download.
//!
//! The SDK owns the wire shape and the two encodings of a digest. This module
//! keeps the client's local decisions: which role the caller is moving, the
//! transfer-error vocabulary the coordinator retries on, and the stable
//! descriptor identity it stores (never the signed download URL).
//!
//! There is **no resume**. A failed session is terminal; recovery is a fresh
//! reservation, never a retry of the old session.

use serde::{Deserialize, Serialize};

use crate::core::sync::{SyncError, SyncResult};

use super::lease::LeaseProof;
use super::protocol::{map_sdk_error, NativeClient, NativeNamespace, NativeZone};
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

    fn sdk(self) -> sync_client::AssetRole {
        match self {
            Self::Original => sync_client::AssetRole::Original,
            Self::Thumbnail => sync_client::AssetRole::Thumbnail,
        }
    }

    fn from_sdk(role: sync_client::AssetRole) -> Self {
        match role {
            sync_client::AssetRole::Original => Self::Original,
            sync_client::AssetRole::Thumbnail => Self::Thumbnail,
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

    fn sdk(self) -> sync_client::UploadIntent {
        match self {
            Self::Create => sync_client::UploadIntent::Create,
            Self::Update => sync_client::UploadIntent::Update,
        }
    }
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
    /// Present once the session is published; the only point at which an asset
    /// id may be trusted from a reservation answer.
    pub asset: Option<AssetDescriptor>,
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

    fn from_sdk(state: sync_client::SessionState) -> Self {
        match state {
            sync_client::SessionState::Reserved => Self::Reserved,
            sync_client::SessionState::Receiving => Self::Receiving,
            sync_client::SessionState::PublishPrepared => Self::PublishPrepared,
            sync_client::SessionState::Published => Self::Published,
            sync_client::SessionState::Failed => Self::Failed,
            sync_client::SessionState::Expired => Self::Expired,
        }
    }

    fn sdk(self) -> sync_client::SessionState {
        match self {
            Self::Reserved => sync_client::SessionState::Reserved,
            Self::Receiving => sync_client::SessionState::Receiving,
            Self::PublishPrepared => sync_client::SessionState::PublishPrepared,
            Self::Published => sync_client::SessionState::Published,
            Self::Failed => sync_client::SessionState::Failed,
            Self::Expired => sync_client::SessionState::Expired,
        }
    }
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
        _namespace: &NativeNamespace,
        zone: &NativeZone,
        proof: &LeaseProof,
        master_record: &str,
        intent: UploadIntent,
        requests: &[ReservationRequest],
    ) -> SyncResult<Vec<Reservation>> {
        if requests.iter().any(|request| request.intent != intent) {
            return Err(SyncError::Protocol(
                "one reservation request may carry one upload intent".into(),
            ));
        }
        let sdk_requests = requests
            .iter()
            .map(|request| {
                Ok(sync_client::ReservationRequest::new(
                    request.role.sdk(),
                    sdk_digest(&request.digest)?,
                ))
            })
            .collect::<SyncResult<Vec<_>>>()?;
        let connection = self.connection().await?;
        let granted = connection
            .reserve_assets_with_proof(
                &super::receive::sdk_zone(zone),
                &proof.sdk(),
                master_record,
                intent.sdk(),
                &sdk_requests,
            )
            .await
            .map_err(map_sdk_error)?;
        Ok(granted.into_iter().map(native_reservation).collect())
    }

    /// Upload one resource's bytes to its reserved session.
    pub async fn upload_asset_bytes(
        &self,
        reservation: &Reservation,
        proof: &LeaseProof,
        bytes: &[u8],
    ) -> std::result::Result<AssetDescriptor, TransferError> {
        let digest = ContentDigest::of(bytes);
        let connection = self
            .connection()
            .await
            .map_err(|error| TransferError::Transport(error.to_string()))?;
        let descriptor = connection
            .upload_asset_with_proof(
                &proof.sdk(),
                &sdk_reservation(reservation),
                &sdk_digest(&digest)
                    .map_err(|error| TransferError::Transport(error.to_string()))?,
                bytes,
            )
            .await
            .map_err(transfer_error)?;
        Ok(native_descriptor(descriptor))
    }

    pub(crate) async fn upload_file(
        &self,
        reservation: &Reservation,
        proof: &LeaseProof,
        path: &std::path::Path,
        expected: &ContentDigest,
    ) -> SyncResult<AssetDescriptor> {
        let connection = self.connection().await?;
        let descriptor = connection
            .upload_asset_file_with_proof(
                &proof.sdk(),
                &sdk_reservation(reservation),
                &sdk_digest(expected)?,
                path,
            )
            .await
            .map_err(|error| SyncError::from(transfer_error(error)))?;
        Ok(native_descriptor(descriptor))
    }

    pub(crate) async fn download_to(
        &self,
        descriptor: &DownloadDescriptor,
        path: &std::path::Path,
        expected: &ContentDigest,
    ) -> SyncResult<()> {
        if descriptor.descriptor.size != expected.size
            || descriptor.descriptor.file_checksum != expected.base64
        {
            return Err(SyncError::Protocol(
                "resource description changed identity".into(),
            ));
        }
        let connection = self.connection().await?;
        connection
            .download_to_file(
                &sync_client::DownloadDescriptor {
                    descriptor: sdk_descriptor(&descriptor.descriptor),
                    download_url: descriptor.download_url.clone(),
                    receipt: descriptor.receipt.clone(),
                },
                path,
            )
            .await
            .map_err(map_sdk_error)
    }

    /// What a session turned out to be. Readable without a lease, which is what
    /// makes reconciling an uncertain outcome possible at all.
    pub async fn upload_session(&self, session_id: &str) -> SyncResult<SessionResponse> {
        let connection = self.connection().await?;
        let session = connection
            .upload_session(session_id)
            .await
            .map_err(map_sdk_error)?;
        Ok(SessionResponse {
            session_id: session.session_id,
            state: SessionState::from_sdk(session.state),
            asset: session.asset.map(native_descriptor),
            error_code: session.error_code,
        })
    }

    /// Extend a reserved session. Only possible while nothing has been sent:
    /// once bytes are in flight the outcome is whatever the body said.
    pub async fn renew_upload_session(
        &self,
        session_id: &str,
        proof: &LeaseProof,
    ) -> SyncResult<Reservation> {
        let connection = self.connection().await?;
        let renewed = connection
            .renew_upload_session_with_proof(&proof.sdk(), session_id)
            .await
            .map_err(map_sdk_error)?;
        Ok(native_reservation(renewed))
    }

    /// A fresh capability for an existing asset, plus its authoritative
    /// descriptor. The `role` is part of identity here: asking for a role the
    /// asset never had is a miss, not a lesser truth.
    pub async fn describe_asset(
        &self,
        asset_id: &str,
        role: AssetRole,
    ) -> SyncResult<DownloadDescriptor> {
        let connection = self.connection().await?;
        let descriptor = connection
            .describe_asset(asset_id, role.sdk())
            .await
            .map_err(map_sdk_error)?;
        Ok(DownloadDescriptor {
            descriptor: native_descriptor(descriptor.descriptor),
            download_url: descriptor.download_url,
            receipt: descriptor.receipt,
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
        _role: AssetRole,
        if_range: Option<&str>,
    ) -> SyncResult<AssetBytes> {
        let connection = self.connection().await?;
        let mut options = sync_client::DownloadOptions::new();
        if let Some(etag) = if_range {
            options = options.if_range(etag);
        }
        let bytes = connection
            .download_asset(
                &sync_client::DownloadDescriptor {
                    descriptor: sdk_descriptor(&descriptor.descriptor),
                    download_url: descriptor.download_url.clone(),
                    receipt: descriptor.receipt.clone(),
                },
                &options,
            )
            .await
            .map_err(map_sdk_error)?;
        Ok(AssetBytes {
            bytes: bytes.bytes,
            etag: bytes.etag,
            partial: bytes.status == 206 && if_range.is_some(),
            content_range: bytes.content_range,
        })
    }
}

fn native_reservation(reservation: sync_client::AssetReservation) -> Reservation {
    Reservation {
        role: AssetRole::from_sdk(reservation.role),
        record_name: reservation.record_name,
        field_name: reservation.field_name,
        session_id: reservation.session_id,
        state: SessionState::from_sdk(reservation.state),
        url: reservation.url.unwrap_or_default(),
        asset: reservation.asset.map(native_descriptor),
    }
}

fn sdk_reservation(reservation: &Reservation) -> sync_client::AssetReservation {
    sync_client::AssetReservation {
        role: reservation.role.sdk(),
        record_name: reservation.record_name.clone(),
        field_name: reservation.field_name.clone(),
        session_id: reservation.session_id.clone(),
        state: reservation.state.sdk(),
        url: (!reservation.url.is_empty()).then(|| reservation.url.clone()),
        asset: reservation.asset.as_ref().map(sdk_descriptor),
    }
}

fn sdk_digest(digest: &ContentDigest) -> SyncResult<sync_client::ContentDigest> {
    sync_client::ContentDigest::from_hex(digest.size, &digest.hex).map_err(map_sdk_error)
}

fn sdk_descriptor(descriptor: &AssetDescriptor) -> sync_client::AssetDescriptor {
    sync_client::AssetDescriptor {
        asset_id: descriptor.asset_id.clone(),
        size: descriptor.size,
        file_checksum: descriptor.file_checksum.clone(),
        checksum_algorithm: "SHA256".into(),
        mime_type: descriptor.mime_type.clone(),
        width: descriptor.width,
        height: descriptor.height,
        download_url: None,
        receipt: None,
    }
}

fn native_descriptor(descriptor: sync_client::AssetDescriptor) -> AssetDescriptor {
    AssetDescriptor {
        asset_id: descriptor.asset_id,
        size: descriptor.size,
        file_checksum: descriptor.file_checksum,
        mime_type: descriptor.mime_type,
        width: descriptor.width,
        height: descriptor.height,
    }
}

/// Map a failed transfer onto the recovery it implies.
fn transfer_error(error: sync_client::Error) -> TransferError {
    use sync_client::Error as SdkError;
    match error {
        SdkError::ChecksumMismatch { expected, actual } => {
            TransferError::ChecksumMismatch(format!("expected {expected}, computed {actual}"))
        }
        SdkError::SizeMismatch { expected, actual } => {
            TransferError::SizeMismatch(format!("expected {expected} bytes, received {actual}"))
        }
        SdkError::UploadRejected { code, reason } => match code.as_str() {
            "CHECKSUM_MISMATCH" => TransferError::ChecksumMismatch(reason),
            "UPLOAD_SIZE_MISMATCH" => TransferError::SizeMismatch(reason),
            "INVALID_THUMBNAIL" => TransferError::InvalidThumbnail(reason),
            "UPLOAD_LEASE_LOST" => TransferError::LeaseLost,
            "UPLOAD_TIMEOUT"
            | "UPLOAD_INTERRUPTED"
            | "UPLOAD_EXPIRED"
            | "UPLOAD_STATE_CONFLICT" => TransferError::SessionFailed(reason),
            _ => TransferError::Transport(format!("{code}: {reason}")),
        },
        SdkError::UploadFailed(detail) => TransferError::SessionFailed(detail),
        SdkError::LeaseLost => TransferError::LeaseLost,
        SdkError::Transport(detail) => TransferError::Transport(detail),
        other => TransferError::Transport(map_sdk_error(other).to_string()),
    }
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

#[cfg(test)]
mod tests;
