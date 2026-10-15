//! Record mutations and the issued-operation receipts that outlive a request.
//!
//! Two facts drive everything here.
//!
//! **A mutation can commit and still fail to answer.** The write may have
//! landed before the connection dropped, so the client cannot treat a timeout
//! as a failure. Every mutation therefore carries an `Idempotency-Key`, and the
//! intent is recorded locally *before* the request leaves — so a later cycle can
//! ask the server what actually happened instead of guessing.
//!
//! **HTTP 200 is not success.** An atomic batch can return 200 with a failure
//! inline, and a conflict carries the server's current record. A category is
//! cleared only by a result that actually confirms that category.

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::core::sync::{SyncError, SyncResult};

use super::lease::{LeaseProof, UPLOAD_LEASE_LOST};
use super::protocol::{map_status, NativeClient, NativeErrorBody, NativeNamespace, NativeZone};
use super::receive::ZoneId;

/// The categories a record mutation may carry. The service derives its own
/// event for a category the client does not send; sending one that disagrees
/// is a client bug the service rejects. This narrow helper leaves events unset;
/// the coordinator supplies the captured events for actual user edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataPatch {
    pub favorite: Option<bool>,
}

/// One record operation, in the service's exact request shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationOperation {
    pub operation_type: &'static str,
    pub record: MutationRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationRecord {
    #[serde(rename = "recordName")]
    pub record_name: String,
    #[serde(rename = "recordChangeTag")]
    pub record_change_tag: String,
    pub fields: Vec<(&'static str, FieldValue)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldValue {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub value: Value,
}

impl FieldValue {
    fn int64(value: i64) -> Self {
        Self {
            kind: "INT64",
            value: json!(value),
        }
    }
}

impl MutationOperation {
    /// A conditional metadata update. The current `recordChangeTag` is
    /// required: without it the service cannot tell "apply my change" from
    /// "overwrite whatever is there now".
    pub fn update_metadata(
        record_name: &str,
        record_change_tag: String,
        patch: MetadataPatch,
    ) -> Self {
        let mut fields = Vec::new();
        if let Some(favorite) = patch.favorite {
            fields.push(("isFavorite", FieldValue::int64(i64::from(favorite))));
        }
        Self {
            operation_type: "update",
            record: MutationRecord {
                record_name: record_name.to_string(),
                record_change_tag,
                fields,
            },
        }
    }

    fn wire(&self) -> Value {
        let fields: serde_json::Map<String, Value> = self
            .record
            .fields
            .iter()
            .map(|(name, value)| {
                (
                    (*name).to_string(),
                    json!({ "type": value.kind, "value": value.value }),
                )
            })
            .collect();
        json!({
            "operationType": self.operation_type,
            "record": {
                "recordName": self.record.record_name,
                "recordChangeTag": self.record.record_change_tag,
                "fields": fields,
            },
        })
    }
}

/// One entry's mutation, with the version it was captured at.
#[derive(Debug, Clone, PartialEq)]
pub struct MutationBatch {
    pub entry_id: i64,
    pub relative_path: String,
    pub category: super::super::store::dirty::ChangeCategory,
    pub captured_version: i64,
    pub operations: Vec<MutationOperation>,
}

impl MutationBatch {
    /// Compatibility identity for the exact logical metadata request. Different
    /// revisions/values are distinct operations. Production coordination uses
    /// the persisted UUID and frozen body in sync_tasks instead of this helper.
    pub fn operation_id(&self) -> String {
        let body = serde_json::to_vec(
            &self
                .operations
                .iter()
                .map(MutationOperation::wire)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default();
        format!(
            "pv-{}-{}-{}-{:x}",
            self.entry_id,
            category_name(self.category),
            self.captured_version,
            Sha256::digest(body)
        )
    }
}

fn category_name(category: super::super::store::dirty::ChangeCategory) -> &'static str {
    use super::super::store::dirty::ChangeCategory;
    match category {
        ChangeCategory::Metadata => "metadata",
        ChangeCategory::Content => "content",
        ChangeCategory::Deletion => "deletion",
    }
}

/// What the server actually confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfirmedRecord {
    pub record_name: String,
    pub record_change_tag: Option<String>,
    pub photo_id: Option<String>,
    pub master_id: Option<String>,
    pub content_generation: Option<String>,
    pub master_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModifyOutcome {
    /// The record was accepted. Only now may a category be cleared.
    Confirmed(ConfirmedRecord),
    /// This item failed. The obligation stands and the reason is reported.
    ItemFailed(String),
    /// The lease ended before or during this item. Irreversible for the cycle.
    LeaseLost,
}

/// What an issued operation turned out to be, read back through its receipt.
#[derive(Debug, Clone, PartialEq)]
pub enum OperationState {
    Committed(Value),
    /// Still running. Its committed children are valid evidence; the operation
    /// as a whole is not a failure.
    Pending(Vec<Value>),
    /// The server has no receipt under this id. For an operation issued
    /// without an idempotency key that is expected; it means the outcome is
    /// genuinely unknown, never that it failed.
    Unknown,
}

impl NativeClient {
    /// Issue one entry's record mutation under a held lease.
    pub async fn modify_records(
        &self,
        proof: &LeaseProof,
        namespace: &NativeNamespace,
        zone: &NativeZone,
        batch: &MutationBatch,
    ) -> SyncResult<ModifyOutcome> {
        if batch.operations.iter().any(|operation| {
            operation.operation_type == "update" && operation.record.fields.is_empty()
        }) {
            return Err(SyncError::Protocol(
                "empty metadata patches do not confirm synchronization".into(),
            ));
        }
        let url = self
            .cloudkit_url(namespace, &["records", "modify"])
            .map_err(|error| SyncError::Protocol(error.to_string()))?;
        let body = json!({
            "zoneID": ZoneId::from(zone),
            // One entry's operations commit or fail together. A half-applied
            // batch would leave the cloud describing a state this client never
            // meant to create.
            "atomic": true,
            "operations": batch
                .operations
                .iter()
                .map(MutationOperation::wire)
                .collect::<Vec<_>>(),
        });
        let payload = serde_json::to_vec(&body)
            .map_err(|error| SyncError::Protocol(format!("mutation is not encodable: {error}")))?;

        // The key is what lets a later cycle ask what happened to an attempt
        // whose answer never arrived.
        let key = batch.operation_id();
        let (status, bytes) = self
            .send_lease_json(&url, payload, Some(proof), Some(&key))
            .await?;
        let response: Value = if status.is_success() {
            serde_json::from_slice(&bytes).map_err(|error| {
                SyncError::Protocol(format!("mutation is not decodable: {error}"))
            })?
        } else {
            if let Some(error) = NativeErrorBody::parse(&bytes) {
                if error.server_error_code.as_deref() == Some(UPLOAD_LEASE_LOST) {
                    return Ok(ModifyOutcome::LeaseLost);
                }
            }
            return Err(map_status(status.as_u16(), &bytes));
        };
        if let Some(code) = inline_error(&response) {
            if code == UPLOAD_LEASE_LOST {
                return Ok(ModifyOutcome::LeaseLost);
            }
            return Ok(ModifyOutcome::ItemFailed(code));
        }
        let records = response
            .get("records")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                SyncError::Protocol("record mutation returned no records array".into())
            })?;
        if records.len() != batch.operations.len()
            || batch.operations.iter().any(|operation| {
                !records.iter().any(|record| {
                    record["recordName"].as_str() == Some(&operation.record.record_name)
                })
            })
        {
            return Err(SyncError::Protocol(
                "mutation response does not match every requested record".into(),
            ));
        }
        if let Some(failure) = records.iter().find_map(inline_error) {
            return Ok(ModifyOutcome::ItemFailed(failure));
        }
        match records
            .iter()
            .find(|record| record["recordType"] == "CPLAsset")
            .or_else(|| records.first())
        {
            None => Ok(ModifyOutcome::ItemFailed(
                "record mutation confirmed nothing".to_string(),
            )),
            Some(record) => {
                if let Some(code) = inline_error(record) {
                    return Ok(ModifyOutcome::ItemFailed(code));
                }
                Ok(ModifyOutcome::Confirmed(read_confirmed(record)?))
            }
        }
    }

    pub(crate) async fn lookup_current(
        &self,
        namespace: &NativeNamespace,
        zone: &NativeZone,
        names: &[&str],
    ) -> SyncResult<Value> {
        let url = self.cloudkit_url(namespace, &["records", "lookup"])?;
        self.post_json(&url,&json!({"zoneID":ZoneId::from(zone),"records":names.iter().map(|name|json!({"recordName":name})).collect::<Vec<_>>()})).await
    }

    /// Submit an actor-persisted logical request under its globally unique id.
    pub(crate) async fn modify_exact(
        &self,
        proof: &LeaseProof,
        namespace: &NativeNamespace,
        body: &Value,
        operation: &str,
    ) -> SyncResult<Value> {
        let url = self.cloudkit_url(namespace, &["records", "modify"])?;
        let payload = serde_json::to_vec(body)
            .map_err(|_| SyncError::Protocol("invalid mutation body".into()))?;
        let (status, bytes) = self
            .send_lease_json(&url, payload, Some(proof), Some(operation))
            .await?;
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &bytes));
        }
        let response: Value = serde_json::from_slice(&bytes)
            .map_err(|_| SyncError::Protocol("invalid mutation response".into()))?;
        if let Some(code) = inline_error(&response) {
            return Err(SyncError::Protocol(code));
        }
        let records = response["records"]
            .as_array()
            .ok_or_else(|| SyncError::Protocol("missing mutation records".into()))?;
        if records.len() != body["operations"].as_array().map_or(0, Vec::len)
            || records.iter().any(|r| inline_error(r).is_some())
        {
            return Err(SyncError::Protocol(
                "atomic mutation did not confirm every record".into(),
            ));
        }
        Ok(response)
    }

    /// Read one issued operation's receipt. This is a plain authenticated GET:
    /// reconciling an uncertain outcome must work even after the lease that
    /// issued it is gone.
    pub async fn operation_status(&self, operation_id: &str) -> SyncResult<OperationState> {
        let url = self.path(&["v1", "operations", operation_id])?;
        let response = self.authorized_get(&url).await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            // No receipt exists. For an operation issued without a key this is
            // the expected answer and says nothing about whether it committed.
            return Ok(OperationState::Unknown);
        }
        if !status.is_success() {
            return Err(map_status(status.as_u16(), &bytes));
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| SyncError::Protocol(format!("receipt is not decodable: {error}")))?;
        let items: Vec<Value> = value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match value.get("state").and_then(Value::as_str) {
            Some("Committed") => Ok(OperationState::Committed(
                value.get("result").cloned().unwrap_or(Value::Null),
            )),
            // A pending operation may already contain valid committed children;
            // they are returned so a reconciliation can use them.
            Some("Pending") => Ok(OperationState::Pending(
                items
                    .into_iter()
                    .filter_map(|item| item.get("result").cloned())
                    .collect(),
            )),
            other => Err(SyncError::Protocol(format!(
                "operation status reported an unknown state {other:?}"
            ))),
        }
    }
}

fn inline_error(value: &Value) -> Option<String> {
    let code = value.get("serverErrorCode")?.as_str()?;
    let reason = value.get("reason").and_then(Value::as_str);
    Some(match reason {
        Some(reason) => format!("{code}: {reason}"),
        None => code.to_string(),
    })
}

fn read_confirmed(record: &Value) -> SyncResult<ConfirmedRecord> {
    let fields = record.get("fields").and_then(Value::as_object);
    let string = |name: &str| -> Option<String> {
        fields
            .and_then(|fields| fields.get(name))
            .and_then(|field| field.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Ok(ConfirmedRecord {
        record_name: record
            .get("recordName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        record_change_tag: record
            .get("recordChangeTag")
            .and_then(Value::as_str)
            .map(str::to_string),
        photo_id: if record["recordType"] == "CPLAsset" {
            record["recordName"].as_str().map(str::to_string)
        } else {
            string("photoId")
        },
        // A master reference is an envelope whose value is itself a reference
        // object; the photo id is the master's own name.
        master_id: fields
            .and_then(|fields| fields.get("masterRef"))
            .and_then(|field| field.get("value"))
            .and_then(|reference| reference.get("recordName"))
            .and_then(Value::as_str)
            .map(str::to_string),
        content_generation: string("contentGeneration"),
        master_revision: None,
    })
}

#[cfg(test)]
mod tests;
