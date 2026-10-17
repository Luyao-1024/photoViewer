//! The typed remote graph: a photo as the cloud describes it.
//!
//! The wire carries a record envelope; this module turns one envelope into a
//! validated, typed view. It exists so the projection can be *rejected* when the
//! graph is inconsistent, rather than storing rows that no later stage would
//! know how to satisfy.
//!
//! Two invariants are enforced here and nowhere else, because they are what
//! makes a photo a photo:
//!
//! * `CPLMaster.photoId` is exactly the paired `CPLAsset.recordName`, and
//!   `CPLAsset.masterRef` names a master in the *same* zone.
//! * Both sides carry the same `contentGeneration`, which is what makes the
//!   original/thumbnail pair and the field baselines describe one version.
//!
//! Anything that violates them is an explicit rejection, never a partial accept:
//! half a pair must not become a visible photo.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::receive::{
    EditEvent, EditEvents, NativeRecord, ZoneId, ASSET_RECORD_TYPE, MASTER_RECORD_TYPE,
};

/// Why a projected graph cannot be promoted. Each variant names a concrete,
/// user-visible condition rather than "invalid data".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("record {name:?} has no change tag")]
    MissingRevision { name: String },
    #[error("photo {name:?} references master {master:?}, which this zone does not contain")]
    DanglingMaster { name: String, master: String },
    #[error("master {master:?} is not referenced by any photo")]
    UnreferencedMaster { master: String },
    #[error("master {master:?} names photo {claimed:?} but is referenced as {actual:?}")]
    MasterPhotoIdMismatch {
        master: String,
        claimed: String,
        actual: String,
    },
    #[error("photo {name:?} and master {master:?} disagree on content generation")]
    GenerationMismatch { name: String, master: String },
    #[error("photo {name:?} is missing its {field} asset reference")]
    MissingAsset { name: String, field: &'static str },
    #[error("photo {name:?} has an invalid relative path")]
    InvalidPath { name: String },
    #[error("photo {name:?} shares relative path {path:?} with another photo in this zone")]
    DuplicatePath { name: String, path: String },
    #[error("photo {name:?} declares a thumbnail digest that is not valid SHA-256 hex")]
    InvalidThumbnailDigest { name: String },
}

/// A resource descriptor as stored. The signed `downloadURL` and `receipt` are
/// deliberately **not** part of this: they expire, and a fresh description is
/// obtained per transfer (C1.4). Identity here is asset id + size + digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetDescriptor {
    pub asset_id: String,
    pub size: u64,
    /// Canonical padded Base64 SHA-256 as the service reports it — not the
    /// lowercase hex the photo field uses.
    pub file_checksum: String,
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// A validated photo: one master record plus its paired asset record at one
/// content generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePhoto {
    /// Stable cloud identity of the photo. Never a path or a local media id.
    pub photo_id: String,
    pub master_id: String,
    pub content_generation: String,
    pub relative_path: String,
    pub is_favorite: bool,
    /// Milliseconds since the epoch, exactly as the wire states it.
    pub asset_date_ms: Option<i64>,
    /// The service supports it; this client has no local model for it, so it is
    /// preserved and never turned into a trash or gallery admission decision.
    pub is_hidden: bool,
    pub original: AssetDescriptor,
    pub thumbnail: AssetDescriptor,
    /// Lowercase hex SHA-256 of the original, as declared by the photo record.
    pub thumbnail_source_sha256: String,
    pub events: EditEvents,
}

impl RemotePhoto {
    /// Resource work this photo owes, in the order the storage contract expects
    /// (original before thumbnail — the thumbnail is regenerated from it).
    pub fn resources(&self) -> Vec<RemoteResource> {
        vec![
            RemoteResource::new(&self.original, "original", &self.photo_id),
            RemoteResource::new(&self.thumbnail, "thumbnail", &self.photo_id),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteResource {
    pub asset_id: String,
    pub role: String,
    pub record_name: String,
    pub expected_size: u64,
    pub expected_sha256: String,
}

impl RemoteResource {
    fn new(descriptor: &AssetDescriptor, role: &str, record_name: &str) -> Self {
        Self {
            asset_id: descriptor.asset_id.clone(),
            role: role.to_string(),
            record_name: record_name.to_string(),
            expected_size: descriptor.size,
            expected_sha256: crate::core::sync::ContentDigest::hex_from_descriptor_base64(
                &descriptor.file_checksum,
            )
            .unwrap_or_default(),
        }
    }
}

/// The `record_type` recorded for a projected row whose payload is a validated
/// photo. Retained unknown records keep their real type, so this marker is
/// unambiguous when the projection is read back.
pub const PHOTO_PAYLOAD_MARKER: &str = "__photo__";

/// One projected remote record, keyed by name. Unknown record types are kept
/// rather than dropped, but never counted as complete supported media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedRecord {
    pub record_name: String,
    pub record_type: String,
    pub record_change_tag: String,
    pub payload: Value,
}

/// The whole zone at one height.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteGraph {
    /// Live photos, validated. Empty when a round produced an incomplete graph.
    pub photos: Vec<RemotePhoto>,
    /// Everything else the server holds, including record types this client does
    /// not model, so a later milestone or a re-probe still sees them.
    pub records: Vec<ProjectedRecord>,
    pub tombstones: Vec<String>,
}

impl RemoteGraph {
    pub fn photo_count(&self) -> usize {
        self.photos.len()
    }

    /// Record types present that this client does not model. Retained, never
    /// silently counted as complete supported media.
    pub fn unknown_record_types(&self) -> BTreeMap<String, usize> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for record in &self.records {
            if record.record_type != MASTER_RECORD_TYPE && record.record_type != ASSET_RECORD_TYPE {
                *counts.entry(record.record_type.clone()).or_default() += 1;
            }
        }
        counts
    }
}

/// The live half of the projection, before graph validation. Built by replaying
/// ordered events; each `recordName` keeps the revision from the newest event
/// that mentioned it, and a tombstone removes it.
#[derive(Debug, Default)]
pub struct RecordSet {
    live: HashMap<String, LiveRecord>,
}

#[derive(Debug, Clone)]
struct LiveRecord {
    record: NativeRecord,
}

impl std::ops::Deref for LiveRecord {
    type Target = NativeRecord;
    fn deref(&self) -> &Self::Target {
        &self.record
    }
}

impl RecordSet {
    /// Apply one received record. A later event for a name replaces the earlier
    /// one regardless of the revision's textual value: revisions are opaque and
    /// only the server's ordering is authoritative.
    pub fn apply(&mut self, record: NativeRecord) {
        if record.is_tombstone() {
            self.live.remove(&record.record_name);
            return;
        }
        self.live
            .insert(record.record_name.clone(), LiveRecord { record });
    }

    /// Rebuild the live set from the durable inbox, in ordinal order.
    ///
    /// Order is the server's, not the revision's: `recordChangeTag` is opaque
    /// and comparing it would be meaningless. A tombstone removes the name, so a
    /// later re-creation with a fresh revision starts from nothing.
    pub fn from_events(
        events: &[super::super::store::receive::InboxEvent],
    ) -> Result<Self, serde_json::Error> {
        let mut set = RecordSet::default();
        for event in events {
            if event.kind == super::super::store::receive::InboxKind::Deleted {
                set.live.remove(&event.record_name);
                continue;
            }
            let record: NativeRecord = serde_json::from_value(event.payload.clone())?;
            set.live
                .insert(record.record_name.clone(), LiveRecord { record });
        }
        Ok(set)
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Validate the photo graph over everything currently live.
    pub fn to_graph(&self, zone: &ZoneId) -> Result<RemoteGraph, GraphError> {
        let mut masters: HashMap<&str, &NativeRecord> = HashMap::new();
        let mut assets: HashMap<&str, &NativeRecord> = HashMap::new();
        let mut unknown = Vec::new();

        for record in self.live.values() {
            match record.record.record_type.as_deref() {
                Some(MASTER_RECORD_TYPE) => {
                    masters.insert(record.record_name.as_str(), &record.record);
                }
                Some(ASSET_RECORD_TYPE) => {
                    assets.insert(record.record_name.as_str(), &record.record);
                }
                // Retained verbatim: an unknown type is evidence, not noise.
                _ => unknown.push(ProjectedRecord {
                    record_name: record.record_name.clone(),
                    record_type: record.record_type.clone().unwrap_or_default(),
                    record_change_tag: revision_of(&record.record)?,
                    payload: serde_json::to_value(&record.record).unwrap_or(Value::Null),
                }),
            }
        }

        let mut photos = Vec::with_capacity(assets.len());
        let mut referenced_masters = HashSet::new();
        let mut paths = HashSet::new();

        for (photo_id, asset) in assets.iter() {
            let fields = asset.fields.as_ref();
            let master_ref = reference_target(fields, "masterRef").ok_or_else(|| {
                GraphError::DanglingMaster {
                    name: (*photo_id).to_string(),
                    master: String::from("<missing masterRef>"),
                }
            })?;
            let master =
                masters
                    .get(master_ref.as_str())
                    .ok_or_else(|| GraphError::DanglingMaster {
                        name: (*photo_id).to_string(),
                        master: master_ref.clone(),
                    })?;
            // A cross-zone reference is not a dangling name but a different
            // failure: the name may exist elsewhere and still be unusable here.
            if let Some(record_zone) = &master.zone_id {
                if record_zone != zone {
                    return Err(GraphError::DanglingMaster {
                        name: (*photo_id).to_string(),
                        master: master_ref.clone(),
                    });
                }
            }

            let master_fields = master.fields.as_ref();
            let claimed = string_field(master_fields, "photoId").ok_or_else(|| {
                GraphError::MasterPhotoIdMismatch {
                    master: master_ref.clone(),
                    claimed: String::from("<missing photoId>"),
                    actual: (*photo_id).to_string(),
                }
            })?;
            if claimed != *photo_id {
                return Err(GraphError::MasterPhotoIdMismatch {
                    master: master_ref.clone(),
                    claimed,
                    actual: (*photo_id).to_string(),
                });
            }

            let generation = string_field(master_fields, "contentGeneration");
            let asset_generation = string_field(fields, "contentGeneration");
            match (&generation, &asset_generation) {
                (Some(master_generation), Some(asset_generation))
                    if master_generation == asset_generation =>
                {
                    photos.push(build_photo(
                        photo_id,
                        master,
                        asset,
                        master_generation.clone(),
                    )?)
                }
                _ => {
                    return Err(GraphError::GenerationMismatch {
                        name: (*photo_id).to_string(),
                        master: master_ref.clone(),
                    })
                }
            }

            referenced_masters.insert(master_ref.clone());
            let photo = photos.last().expect("just pushed");
            if !paths.insert(photo.relative_path.clone()) {
                return Err(GraphError::DuplicatePath {
                    name: (*photo_id).to_string(),
                    path: photo.relative_path.clone(),
                });
            }
        }

        for master in masters.keys() {
            if !referenced_masters.contains(*master) {
                return Err(GraphError::UnreferencedMaster {
                    master: (*master).to_string(),
                });
            }
        }

        let mut records = unknown;
        records.sort_by(|left, right| left.record_name.cmp(&right.record_name));
        Ok(RemoteGraph {
            photos,
            records,
            tombstones: Vec::new(),
        })
    }
}

fn build_photo(
    photo_id: &str,
    master: &NativeRecord,
    asset: &NativeRecord,
    content_generation: String,
) -> Result<RemotePhoto, GraphError> {
    let master_fields = master.fields.as_ref();
    let asset_fields = asset.fields.as_ref();
    let original = asset_descriptor(master_fields, "resOriginalRes", photo_id)?;
    let thumbnail = asset_descriptor(master_fields, "resJPEGThumbRes", photo_id)?;

    let relative_path =
        string_field(asset_fields, "relativePath").ok_or_else(|| GraphError::MissingAsset {
            name: photo_id.to_string(),
            field: "relativePath",
        })?;
    if !is_safe_relative_path(&relative_path) {
        return Err(GraphError::InvalidPath {
            name: photo_id.to_string(),
        });
    }

    let thumbnail_source_sha256 = string_field(master_fields, "thumbnailSourceOriginalSHA256")
        .ok_or_else(|| GraphError::MissingAsset {
            name: photo_id.to_string(),
            field: "thumbnailSourceOriginalSHA256",
        })?;
    if !is_lower_hex(&thumbnail_source_sha256)
        || crate::core::sync::ContentDigest::hex_from_descriptor_base64(&original.file_checksum)
            .as_deref()
            != Some(&thumbnail_source_sha256)
    {
        return Err(GraphError::InvalidThumbnailDigest {
            name: photo_id.to_string(),
        });
    }

    Ok(RemotePhoto {
        photo_id: photo_id.to_string(),
        master_id: master.record_name.clone(),
        content_generation,
        relative_path,
        is_favorite: int_field(asset_fields, "isFavorite").unwrap_or(0) != 0,
        asset_date_ms: int_field(asset_fields, "assetDate"),
        // Preserved, never acted on: this client has no local hidden model.
        is_hidden: int_field(asset_fields, "isHidden").unwrap_or(0) != 0,
        original,
        thumbnail,
        thumbnail_source_sha256,
        events: asset.events.clone().unwrap_or_default(),
    })
}

fn revision_of(record: &NativeRecord) -> Result<String, GraphError> {
    record
        .record_change_tag
        .clone()
        .ok_or_else(|| GraphError::MissingRevision {
            name: record.record_name.clone(),
        })
}

/// The `value` inside a typed field envelope. Every field on the wire is
/// `{"type": …, "value": …}`, so unwrapping happens here exactly once rather
/// than at every call site.
fn field_value<'a>(
    fields: Option<&'a serde_json::Map<String, Value>>,
    name: &str,
) -> Option<&'a Value> {
    fields?.get(name)?.as_object()?.get("value")
}

/// The target name of a `REFERENCE` field. Only `action=NONE` references carry
/// no write intent, which is the only kind the service produces.
fn reference_target(fields: Option<&serde_json::Map<String, Value>>, name: &str) -> Option<String> {
    field_value(fields, name)?
        .get("recordName")?
        .as_str()
        .map(str::to_string)
}

fn string_field(fields: Option<&serde_json::Map<String, Value>>, name: &str) -> Option<String> {
    field_value(fields, name)?.as_str().map(str::to_string)
}

fn int_field(fields: Option<&serde_json::Map<String, Value>>, name: &str) -> Option<i64> {
    field_value(fields, name)?.as_i64()
}

fn asset_descriptor(
    fields: Option<&serde_json::Map<String, Value>>,
    name: &'static str,
    photo_id: &str,
) -> Result<AssetDescriptor, GraphError> {
    let value = field_value(fields, name)
        .and_then(|value| value.as_object())
        .ok_or_else(|| GraphError::MissingAsset {
            name: photo_id.to_string(),
            field: name,
        })?;
    let descriptor: AssetDescriptor = serde_json::from_value(Value::Object(value.clone()))
        .map_err(|_| GraphError::MissingAsset {
            name: photo_id.to_string(),
            field: name,
        })?;
    if crate::core::sync::ContentDigest::hex_from_descriptor_base64(&descriptor.file_checksum)
        .is_none_or(|digest| !is_lower_hex(&digest))
    {
        return Err(GraphError::MissingAsset {
            name: photo_id.to_string(),
            field: name,
        });
    }
    Ok(descriptor)
}

/// A job-root-relative placement with no traversal, no empty segment and no
/// symlink-indicating component. This is checked here rather than at admission
/// so an unsafe remote path can never reach the filesystem layer.
pub(crate) fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    path.split('/').all(|segment| {
        !segment.is_empty() && segment != "." && segment != ".." && segment.len() <= 255
    })
}

fn is_lower_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The latest of two category events, by the documented rule: real edit time
/// first, then a stable source/event-id tie. A missing event never wins.
pub fn latest_event(left: Option<&EditEvent>, right: Option<&EditEvent>) -> Option<EditEvent> {
    match (left, right) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (Some(left), Some(right)) => {
            let key = |event: &EditEvent| {
                (
                    event.edited_at_ms,
                    event.source.clone(),
                    event.event_id.clone(),
                )
            };
            if key(left) >= key(right) {
                Some(left.clone())
            } else {
                Some(right.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests;
