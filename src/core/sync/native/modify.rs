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
//! inline. The coordinator persists the exact logical envelope it issued; this
//! module hands that envelope to the SDK unchanged and maps the receipt back.

use serde_json::{json, Value};

use crate::core::sync::{SyncError, SyncResult};

use super::lease::LeaseProof;
use super::protocol::{map_sdk_error, NativeClient, NativeNamespace, NativeZone};

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
    /// Read the named records at the current head as a wire-shaped lookup
    /// response. The coordinator compares its frozen request fields against it.
    pub(crate) async fn lookup_current(
        &self,
        _namespace: &NativeNamespace,
        zone: &NativeZone,
        names: &[&str],
    ) -> SyncResult<Value> {
        let connection = self.connection().await?;
        let lookup = connection
            .lookup_current(&super::receive::sdk_zone(zone), names)
            .await
            .map_err(map_sdk_error)?;
        let records = lookup
            .records
            .into_iter()
            .map(|result| match result {
                sync_client::RecordResult::Confirmed(record) => serde_json::to_value(record)
                    .map_err(|error| {
                        SyncError::Protocol(format!("lookup record is not encodable: {error}"))
                    }),
                sync_client::RecordResult::Failed(failure) => Ok(json!({
                    "serverErrorCode": failure.code,
                    "reason": failure.reason,
                })),
            })
            .collect::<SyncResult<Vec<_>>>()?;
        Ok(json!({"records": records, "syncToken": lookup.sync_token}))
    }

    /// Submit an actor-persisted logical request under its globally unique id.
    /// The body is handed to the SDK unchanged so a retry remains the same
    /// logical request even when the local projection has moved since.
    pub(crate) async fn modify_exact(
        &self,
        proof: &LeaseProof,
        _namespace: &NativeNamespace,
        body: &Value,
        operation: &str,
    ) -> SyncResult<Value> {
        let connection = self.connection().await?;
        connection
            .modify_exact(&proof.sdk(), body, operation)
            .await
            .map_err(map_sdk_error)
    }

    /// Read one issued operation's receipt. This has no lease dependency, so it
    /// works even after the lease that issued the operation is gone.
    pub async fn operation_status(&self, operation_id: &str) -> SyncResult<OperationState> {
        let connection = self.connection().await?;
        match connection
            .operation_status(operation_id)
            .await
            .map_err(map_sdk_error)?
        {
            sync_client::OperationState::Committed(value) => Ok(OperationState::Committed(value)),
            sync_client::OperationState::Pending(values) => Ok(OperationState::Pending(values)),
            sync_client::OperationState::Unknown => Ok(OperationState::Unknown),
        }
    }
}
