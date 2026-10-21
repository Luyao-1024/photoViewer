//! Receive-side shapes and SDK projection mapping.
//!
//! The SDK builds requests and decodes responses; this module keeps the local
//! projection contract the service layer consumes and the error decisions that
//! belong to the client. Server rules this file still encodes:
//!
//! * `zoneID` is exactly `{zoneName, ownerRecordName}` and the owner must be
//!   `_defaultOwner`.
//! * `continuationMarker` is **absent** on the last page, not empty. Treating a
//!   missing key as `""` would loop forever.
//! * A tombstone arrives as `deleted: true` with no fields; the type may be
//!   explicitly null.
//! * `changes/zone` can carry a per-zone `serverErrorCode`, which the SDK
//!   surfaces separately from the records array.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::core::sync::SyncResult;

use crate::core::sync::SyncError;

use super::protocol::{
    map_sdk_error, NativeClient, NativeNamespace, NativeZone, REQUIRED_CAPABILITIES,
};

/// Record types this client understands. Anything else on the wire is retained
/// in the projection as an unknown type rather than being dropped or silently
/// counted as complete supported media.
pub const MASTER_RECORD_TYPE: &str = "CPLMaster";
pub const ASSET_RECORD_TYPE: &str = "CPLAsset";

/// The record types a bootstrap must complete before the inventory can be
/// called complete. One type or one page is never a complete inventory.
pub const SUPPORTED_RECORD_TYPES: [&str; 2] = [MASTER_RECORD_TYPE, ASSET_RECORD_TYPE];

/// The wire form of a zone identity. Sent on every zone-scoped call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneId {
    #[serde(rename = "zoneName")]
    pub zone_name: String,
    #[serde(rename = "ownerRecordName", default = "default_owner")]
    pub owner_record_name: String,
}

fn default_owner() -> String {
    super::DEFAULT_OWNER.to_string()
}

impl From<&NativeZone> for ZoneId {
    fn from(zone: &NativeZone) -> Self {
        Self {
            zone_name: zone.zone_name.clone(),
            owner_record_name: zone.owner.clone(),
        }
    }
}

impl ZoneId {
    pub fn zone(&self) -> NativeZone {
        NativeZone {
            zone_name: self.zone_name.clone(),
            owner: self.owner_record_name.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// Both tokens are minted from one committed head, so they describe the same
/// height: the snapshot token pins record reads, the zone token is where the
/// delta stream resumes. Neither substitutes for the other.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotResponse {
    #[serde(rename = "zoneID")]
    pub zone_id: ZoneId,
    #[serde(rename = "snapshotToken")]
    pub snapshot_token: String,
    #[serde(rename = "syncToken")]
    pub sync_token: String,
}

// ---------------------------------------------------------------------------
// Record query (the shared bootstrap read)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    pub records: Vec<NativeRecord>,
    #[serde(rename = "syncToken")]
    pub sync_token: String,
    /// Present **only** when another page follows.
    #[serde(
        rename = "continuationMarker",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub continuation_marker: Option<String>,
}

impl QueryResponse {
    pub fn more_pages(&self) -> bool {
        self.continuation_marker.is_some()
    }
}

// ---------------------------------------------------------------------------
// Zone deltas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneChangesEntry {
    #[serde(rename = "zoneID", default)]
    pub zone_id: Option<ZoneId>,
    #[serde(default)]
    pub records: Vec<NativeRecord>,
    #[serde(rename = "moreComing", default)]
    pub more_coming: bool,
    #[serde(rename = "syncToken", default)]
    pub sync_token: Option<String>,
    /// An in-band per-zone failure. Present even when the HTTP status was 200.
    #[serde(rename = "serverErrorCode", default)]
    pub server_error_code: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// The record envelope
// ---------------------------------------------------------------------------

/// One record as the server returns it. The two shapes — live and tombstone —
/// are one type because the server sends them in the same array; `deleted`
/// distinguishes them, and a tombstone carries no `fields` key at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRecord {
    #[serde(rename = "recordName")]
    pub record_name: String,
    /// Explicitly `null` on a tombstone, which is how the record's type is
    /// recovered after the type is otherwise gone.
    #[serde(rename = "recordType", default)]
    pub record_type: Option<String>,
    #[serde(rename = "zoneID", default)]
    pub zone_id: Option<ZoneId>,
    #[serde(rename = "recordChangeTag", default)]
    pub record_change_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Map<String, Value>>,
    #[serde(rename = "_syncserver", default)]
    pub events: Option<EditEvents>,
    #[serde(default)]
    pub deleted: bool,
}

impl NativeRecord {
    /// A tombstone's record type is `null`, but a projection keyed by name
    /// needs the previous type to decide what it is removing.
    pub fn is_tombstone(&self) -> bool {
        self.deleted || self.fields.is_none()
    }
}

/// Independent per-category edit stamps. Omitted means unknown — never a
/// substituted "now".
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEvents {
    #[serde(rename = "metadataEvent", default)]
    pub metadata: Option<EditEvent>,
    #[serde(rename = "contentEvent", default)]
    pub content: Option<EditEvent>,
    #[serde(rename = "deletionEvent", default)]
    pub deletion: Option<EditEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEvent {
    #[serde(rename = "editedAtMs")]
    pub edited_at_ms: i64,
    pub source: String,
    #[serde(rename = "eventId")]
    pub event_id: String,
}

// ---------------------------------------------------------------------------
// Client calls
// ---------------------------------------------------------------------------

impl NativeClient {
    /// `POST /v1/snapshots`. Establishes the shared height every bootstrap type
    /// is read at, plus the zone token the delta stream resumes from.
    pub async fn snapshot(&self, zone: &NativeZone) -> SyncResult<SnapshotResponse> {
        let connection = self.connection().await?;
        let snapshot = connection
            .snapshot(&sdk_zone(zone))
            .await
            .map_err(map_sdk_error)?;
        Ok(SnapshotResponse {
            zone_id: ZoneId::from(zone),
            snapshot_token: snapshot.snapshot_token,
            sync_token: snapshot.sync_token,
        })
    }

    /// `POST .../records/query`. One page of one record type at the snapshot's
    /// height. Never a complete inventory on its own.
    pub async fn query_records(
        &self,
        _namespace: &NativeNamespace,
        zone: &NativeZone,
        record_type: &str,
        page_limit: usize,
        snapshot_token: Option<&str>,
        continuation_marker: Option<&str>,
    ) -> SyncResult<QueryResponse> {
        let connection = self.connection().await?;
        let mut request =
            sync_client::QueryRequest::new(sdk_zone(zone), record_type).limit(page_limit);
        if let Some(token) = snapshot_token {
            request = request.snapshot(token);
        }
        if let Some(marker) = continuation_marker {
            request = request.continuation(marker);
        }
        let page = connection
            .query_page(&request)
            .await
            .map_err(map_sdk_error)?;
        if let Some(failure) = page.failures.first() {
            return Err(map_sdk_error(sync_client::Error::InBand(Box::new(
                failure.clone(),
            ))));
        }
        Ok(QueryResponse {
            records: page.records.into_iter().map(native_record).collect(),
            sync_token: page.sync_token,
            continuation_marker: page.continuation_marker,
        })
    }

    /// `POST .../changes/zone`. One page of the fixed-height delta round.
    ///
    /// `more_coming` is what ends the round — never the response length, which
    /// the server is free to shorten for byte reasons.
    pub async fn changes_zone(
        &self,
        _namespace: &NativeNamespace,
        zone: &NativeZone,
        sync_token: Option<&str>,
        page_limit: usize,
    ) -> SyncResult<ZoneChangesEntry> {
        let connection = self.connection().await?;
        let mut request = sync_client::ZoneChangesRequest::new(sdk_zone(zone)).limit(page_limit);
        if let Some(token) = sync_token {
            request = request.token(token);
        }
        let page = match connection.changes_zone(&request).await {
            Ok(page) => page,
            Err(sync_client::Error::InBand(failure)) => {
                return Err(map_zone_error(&failure.code, Some(&failure.reason)))
            }
            Err(error) => return Err(map_sdk_error(error)),
        };
        if let Some(failure) = page.failures.first() {
            return Err(map_zone_error(&failure.code, Some(&failure.reason)));
        }
        if page.sync_token.is_empty() {
            return Err(SyncError::Protocol(
                "zone change response carried no continuation token".into(),
            ));
        }
        Ok(ZoneChangesEntry {
            zone_id: page
                .zone
                .map(native_zone_id)
                .or_else(|| Some(ZoneId::from(zone))),
            records: page.records.into_iter().map(native_record).collect(),
            more_coming: page.more_coming,
            sync_token: Some(page.sync_token),
            server_error_code: None,
            reason: None,
        })
    }

    /// Page size to ask for, bounded by what the server announced.
    pub fn page_limit(&self, pinned: &super::protocol::NativePinnedScope) -> usize {
        pinned.sync_limits.max_page.clamp(1, 100)
    }
}

pub(super) fn sdk_zone(zone: &NativeZone) -> sync_client::ZoneId {
    sync_client::ZoneId::new(zone.zone_name.clone())
}

fn native_zone_id(zone: sync_client::ZoneId) -> ZoneId {
    ZoneId {
        zone_name: zone.zone_name,
        owner_record_name: zone.owner_record_name,
    }
}

fn native_record(record: sync_client::RemoteRecord) -> NativeRecord {
    let fields = if record.deleted {
        None
    } else {
        Some(
            record
                .fields
                .into_iter()
                .map(|(name, field)| {
                    (
                        name,
                        serde_json::json!({"type": field.kind, "value": field.value}),
                    )
                })
                .collect(),
        )
    };
    NativeRecord {
        record_name: record.record_name,
        record_type: record.record_type,
        zone_id: record.zone_id.map(native_zone_id),
        record_change_tag: record.record_change_tag,
        fields,
        events: Some(EditEvents {
            metadata: record.events.metadata_event.map(native_event),
            content: record.events.content_event.map(native_event),
            deletion: record.events.deletion_event.map(native_event),
        }),
        deleted: record.deleted,
    }
}

fn native_event(event: sync_client::EditEvent) -> EditEvent {
    EditEvent {
        edited_at_ms: event.edited_at_ms,
        source: event.source,
        event_id: event.event_id,
    }
}

pub const EXPIRED_SYNC_TOKEN: &str = "EXPIRED_SYNC_TOKEN";
pub const UNKNOWN_ITEM: &str = "UNKNOWN_ITEM";

fn map_zone_error(code: &str, reason: Option<&str>) -> SyncError {
    let detail = match reason {
        Some(reason) => format!("{code}: {reason}"),
        None => code.to_string(),
    };
    match code {
        EXPIRED_SYNC_TOKEN => SyncError::Unsupported(detail),
        UNKNOWN_ITEM => SyncError::NotFound(detail),
        _ => SyncError::Protocol(detail),
    }
}

/// The client speaks one protocol version and requires a documented capability
/// set; a receive call must never be attempted without both already checked.
pub fn receive_is_permitted(bits: &super::protocol::NativeCapabilityBits) -> bool {
    bits.missing_required().is_empty()
        && REQUIRED_CAPABILITIES
            .iter()
            .all(|capability| capability_is_set(bits, *capability))
}

fn capability_is_set(
    bits: &super::protocol::NativeCapabilityBits,
    capability: super::protocol::RequiredCapability,
) -> bool {
    use super::protocol::RequiredCapability as R;
    match capability {
        R::ZoneListing => bits.zone_listing,
        R::RecordCrud => bits.record_crud,
        R::Snapshots => bits.snapshots,
        R::IncrementalChanges => bits.incremental_changes,
        R::UploadLeases => bits.upload_leases,
        R::Fencing => bits.fencing,
        R::TokenCurrentGate => bits.token_current_gate,
        R::UploadPhases => bits.upload_phases,
        R::OperationStatusRead => bits.operation_status_read,
        R::AssetUpload => bits.asset_upload,
        R::AssetDownload => bits.asset_download,
        R::PhotoPublication => bits.photo_publication,
    }
}

#[cfg(test)]
mod tests;
