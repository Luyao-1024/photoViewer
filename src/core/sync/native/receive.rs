//! Receive-side wire types: snapshots, record queries and zone deltas.
//!
//! These live apart from [`super::protocol`] because that module is the
//! *connection* contract — what may be configured — while this one is the
//! *receive* contract: what the cloud says and how it is paged. Keeping them
//! separate means a field rename in one cannot silently satisfy the other.
//!
//! Server rules this file encodes, all verified against `syncserver@2d9f9a7`:
//!
//! * Every request DTO is `deny_unknown_fields`, so an extra key is a hard 400.
//!   Optional members are therefore omitted rather than sent as `null`.
//! * `zoneID` is exactly `{zoneName, ownerRecordName}` and the owner must be
//!   `_defaultOwner`.
//! * `resultsLimit` is 1..=200; the pinned `syncLimits.maxPage` is the ceiling
//!   this client asks for, and the server may still return a shorter page.
//! * `continuationMarker` is **absent** on the last page, not empty. Treating a
//!   missing key as `""` would loop forever.
//! * A tombstone arrives in the same `records` array as a live record, with
//!   `recordType: null` (explicitly null, not omitted), `deleted: true` and no
//!   `fields` key at all.
//! * `changes/zone` can carry a per-zone `serverErrorCode` inside an HTTP 200,
//!   so each entry is checked before its `records` are read.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::core::sync::SyncResult;

use crate::core::sync::SyncError;

use super::protocol::{
    map_status, NativeClient, NativeErrorBody, NativeNamespace, NativeZone, REQUIRED_CAPABILITIES,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotRequest<'a> {
    #[serde(rename = "zoneID")]
    zone: &'a ZoneId,
}

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryEnvelope<'a> {
    #[serde(rename = "zoneID")]
    zone: &'a ZoneId,
    query: QueryBody<'a>,
    #[serde(rename = "resultsLimit")]
    results_limit: usize,
    #[serde(rename = "snapshotToken", skip_serializing_if = "Option::is_none")]
    snapshot_token: Option<&'a str>,
    #[serde(rename = "continuationMarker", skip_serializing_if = "Option::is_none")]
    continuation_marker: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct QueryBody<'a> {
    #[serde(rename = "recordType")]
    record_type: &'a str,
}

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ZoneChangesRequest<'a> {
    zones: Vec<ZoneChangesBody<'a>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ZoneChangesBody<'a> {
    #[serde(rename = "zoneID")]
    zone: &'a ZoneId,
    /// Absent on the first page of a round.
    #[serde(rename = "syncToken", skip_serializing_if = "Option::is_none")]
    sync_token: Option<&'a str>,
    #[serde(rename = "resultsLimit")]
    results_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneChangesResponse {
    pub zones: Vec<ZoneChangesEntry>,
}

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
// Database-level zone discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseChangesRequest<'a> {
    #[serde(rename = "syncToken", skip_serializing_if = "Option::is_none")]
    sync_token: Option<&'a str>,
    #[serde(rename = "resultsLimit")]
    results_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseChangesResponse {
    pub zones: Vec<DatabaseZoneChange>,
    #[serde(rename = "syncToken", default)]
    pub sync_token: Option<String>,
    #[serde(rename = "moreComing", default)]
    pub more_coming: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseZoneChange {
    #[serde(rename = "zoneID", default)]
    pub zone_id: Option<ZoneId>,
    #[serde(default)]
    pub deleted: bool,
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

/// Cap on any single JSON body this client will accept. The wire limit is 8 MiB
/// (`recordLimits.maxResultBytes`); this is the same number, named once.
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

impl NativeClient {
    /// `POST /v1/snapshots`. Establishes the shared height every bootstrap type
    /// is read at, plus the zone token the delta stream resumes from.
    pub async fn snapshot(&self, zone: &NativeZone) -> SyncResult<SnapshotResponse> {
        let url = self.path(&["v1", "snapshots"])?;
        let zone_id = ZoneId::from(zone);
        let body = SnapshotRequest { zone: &zone_id };
        self.post_json::<_, SnapshotResponse>(&url, &body).await
    }

    /// `POST .../records/query`. One page of one record type at the snapshot's
    /// height. Never a complete inventory on its own.
    pub async fn query_records(
        &self,
        namespace: &NativeNamespace,
        zone: &NativeZone,
        record_type: &str,
        page_limit: usize,
        snapshot_token: Option<&str>,
        continuation_marker: Option<&str>,
    ) -> SyncResult<QueryResponse> {
        let url = self.cloudkit_url(namespace, &["records", "query"])?;
        let zone_id = ZoneId::from(zone);
        let body = QueryEnvelope {
            zone: &zone_id,
            query: QueryBody { record_type },
            results_limit: page_limit,
            snapshot_token,
            continuation_marker,
        };
        self.post_json::<_, QueryResponse>(&url, &body).await
    }

    /// `POST .../changes/zone`. One page of the fixed-height delta round.
    ///
    /// `more_coming` is what ends the round — never the response length, which
    /// the server is free to shorten for byte reasons.
    pub async fn changes_zone(
        &self,
        namespace: &NativeNamespace,
        zone: &NativeZone,
        sync_token: Option<&str>,
        page_limit: usize,
    ) -> SyncResult<ZoneChangesEntry> {
        let url = self.cloudkit_url(namespace, &["changes", "zone"])?;
        let zone_id = ZoneId::from(zone);
        let body = ZoneChangesRequest {
            zones: vec![ZoneChangesBody {
                zone: &zone_id,
                sync_token,
                results_limit: page_limit,
            }],
        };
        let response: ZoneChangesResponse = self
            .post_json::<_, ZoneChangesResponse>(&url, &body)
            .await?;
        let mut entries = response.zones;
        if entries.len() != 1 {
            return Err(SyncError::Protocol(format!(
                "zone change response carried {} entries for one requested zone",
                entries.len()
            )));
        }
        let entry = entries.remove(0);
        if let Some(code) = &entry.server_error_code {
            return Err(map_zone_error(code, entry.reason.as_deref()));
        }
        if entry.sync_token.is_none() {
            return Err(SyncError::Protocol(
                "zone change response carried no continuation token".into(),
            ));
        }
        Ok(entry)
    }

    /// `POST .../changes/database`. Zone discovery, which has its own progress
    /// token and may repeat a zone across pages.
    pub async fn changes_database(
        &self,
        namespace: &NativeNamespace,
        sync_token: Option<&str>,
        page_limit: usize,
    ) -> SyncResult<DatabaseChangesResponse> {
        let url = self.cloudkit_url(namespace, &["changes", "database"])?;
        let body = DatabaseChangesRequest {
            sync_token,
            results_limit: page_limit,
        };
        self.post_json::<_, DatabaseChangesResponse>(&url, &body)
            .await
    }

    /// `POST .../zones/list`. The request body must be exactly `{}` — the
    /// service rejects anything else, so it is serialized literally rather than
    /// from a value that would encode as `null`.
    pub async fn list_zones(&self, namespace: &NativeNamespace) -> SyncResult<Vec<ZoneId>> {
        let url = self.cloudkit_url(namespace, &["zones", "list"])?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ZonesResponse {
            zones: Vec<ZoneIdEntry>,
        }
        #[derive(Deserialize)]
        struct ZoneIdEntry {
            #[serde(rename = "zoneID")]
            zone_id: ZoneId,
        }
        let response: ZonesResponse = self.post_json_raw::<ZonesResponse>(&url, b"{}").await?;
        Ok(response
            .zones
            .into_iter()
            .map(|entry| entry.zone_id)
            .collect())
    }

    /// Page size to ask for, bounded by what the server announced.
    pub fn page_limit(&self, pinned: &super::protocol::NativePinnedScope) -> usize {
        pinned.sync_limits.max_page.clamp(1, 200)
    }

    fn cloudkit_url(&self, namespace: &NativeNamespace, segments: &[&str]) -> SyncResult<Url> {
        let base = self.cloudkit_base(namespace)?;
        let mut url = base;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| SyncError::Protocol("pinned namespace is not a usable base".into()))?;
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }
}

/// The one JSON POST path, shared by every control and metadata call.
pub(super) async fn post_json<B: Serialize, R: serde::de::DeserializeOwned>(
    client: &NativeClient,
    url: &Url,
    body: &B,
) -> SyncResult<R> {
    let payload = serde_json::to_vec(body)
        .map_err(|error| SyncError::Protocol(format!("request is not encodable: {error}")))?;
    post_json_raw(client, url, &payload).await
}

/// The same path with a literal body, for the one endpoint that takes an empty
/// object rather than a DTO.
pub(super) async fn post_json_raw<R: serde::de::DeserializeOwned>(
    client: &NativeClient,
    url: &Url,
    payload: &[u8],
) -> SyncResult<R> {
    let response = client.send_json(url, payload.to_vec()).await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(SyncError::Protocol(
            "response exceeds the wire result limit".into(),
        ));
    }
    if !status.is_success() {
        // The token's own expiry has its own recovery, so it must stay
        // distinguishable from a generic bad request.
        if let Some(error) = NativeErrorBody::parse(&bytes) {
            if error.server_error_code.as_deref() == Some(EXPIRED_SYNC_TOKEN) {
                return Err(SyncError::Unsupported(EXPIRED_SYNC_TOKEN.into()));
            }
        }
        return Err(map_status(status.as_u16(), &bytes));
    }
    // An HTTP 200 can still carry an in-band failure at the top level.
    let in_band = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .as_ref()
        .and_then(NativeErrorBody::from_value)
        .and_then(|error| error.server_error_code);
    if let Some(code) = in_band {
        return Err(map_zone_error(&code, None));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| SyncError::Protocol(format!("response is not decodable: {error}")))
}

impl NativeErrorBody {
    /// Read the envelope out of an already-parsed JSON value. Returns `None`
    /// when the value is not an error object, which is the common case.
    pub fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if !object.contains_key("serverErrorCode") {
            return None;
        }
        serde_json::from_value(value.clone()).ok()
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
