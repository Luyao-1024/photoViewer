//! Scalar connection/configuration/progress state. No serialized SQLite fields.
use super::receive::{BootstrapState, TypeMarker, TypeStatus};
use crate::core::error::{AppError, Result};
use crate::core::sync::native::protocol::NativePinnedScope;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde_json::{json, Value};

const SCOPE_FIELDS: &[(&str, &str, bool)] = &[
    ("native_public_base_url", "publicBaseURL", false),
    ("native_protocol_version", "protocolVersion", false),
    ("native_service_version", "serviceVersion", false),
    ("native_stage", "stage", false),
    ("native_checksum_algorithm", "checksumAlgorithm", false),
    ("native_database_generation", "databaseGeneration", false),
    ("native_tenant", "namespace.tenant", false),
    ("native_container", "namespace.container", false),
    ("native_environment", "namespace.environment", false),
    ("native_database", "namespace.database", false),
    ("native_auth_mode", "authentication.mode", false),
    ("native_user_id", "authentication.user.id", false),
    ("native_username", "authentication.user.username", false),
    (
        "native_resource_bearer_required",
        "authentication.resourceBearerRequired",
        true,
    ),
    ("native_cap_zone_listing", "capabilities.zoneListing", true),
    ("native_cap_record_crud", "capabilities.recordCrud", true),
    ("native_cap_snapshots", "capabilities.snapshots", true),
    (
        "native_cap_incremental_changes",
        "capabilities.incrementalChanges",
        true,
    ),
    (
        "native_cap_upload_leases",
        "capabilities.uploadLeases",
        true,
    ),
    ("native_cap_fencing", "capabilities.fencing", true),
    (
        "native_cap_token_current_gate",
        "capabilities.tokenCurrentGate",
        true,
    ),
    (
        "native_cap_upload_phases",
        "capabilities.uploadPhases",
        true,
    ),
    (
        "native_cap_operation_status_read",
        "capabilities.operationStatusRead",
        true,
    ),
    ("native_cap_asset_upload", "capabilities.assetUpload", true),
    (
        "native_cap_asset_download",
        "capabilities.assetDownload",
        true,
    ),
    (
        "native_cap_photo_publication",
        "capabilities.photoPublication",
        true,
    ),
    ("native_cap_query_batch", "capabilities.queryBatch", true),
    (
        "native_cap_change_aliases",
        "capabilities.changeAliases",
        true,
    ),
    (
        "native_cap_apple_authentication",
        "capabilities.appleAuthentication",
        true,
    ),
    ("native_cap_sharing", "capabilities.sharing", true),
    (
        "native_cap_upload_resume",
        "capabilities.uploadResume",
        true,
    ),
    (
        "native_limit_max_asset_bytes",
        "limits.maxAssetBytes",
        false,
    ),
    (
        "native_limit_thumbnail_max_bytes",
        "limits.thumbnailMaxBytes",
        false,
    ),
    (
        "native_limit_upload_concurrency",
        "limits.uploadConcurrency",
        false,
    ),
    (
        "native_limit_download_concurrency",
        "limits.downloadConcurrency",
        false,
    ),
    (
        "native_limit_upload_idle_timeout_secs",
        "limits.uploadIdleTimeoutSecs",
        false,
    ),
    (
        "native_limit_upload_total_timeout_secs",
        "limits.uploadTotalTimeoutSecs",
        false,
    ),
    (
        "native_limit_db_queue_capacity",
        "limits.dbQueueCapacity",
        false,
    ),
    (
        "native_limit_json_body_limit",
        "limits.jsonBodyLimit",
        false,
    ),
    ("native_limit_lease_ttl_secs", "limits.leaseTtlSecs", false),
    (
        "native_limit_lease_heartbeat_secs",
        "limits.leaseHeartbeatSecs",
        false,
    ),
    (
        "native_limit_control_queue_capacity",
        "limits.controlQueueCapacity",
        false,
    ),
    ("native_sync_limit_max_page", "syncLimits.maxPage", false),
    ("native_sync_limit_max_zones", "syncLimits.maxZones", false),
    (
        "native_record_limit_max_operations",
        "recordLimits.maxOperations",
        false,
    ),
    (
        "native_record_limit_max_record_bytes",
        "recordLimits.maxRecordBytes",
        false,
    ),
    (
        "native_record_limit_max_result_bytes",
        "recordLimits.maxResultBytes",
        false,
    ),
];

fn invalid(message: impl std::fmt::Display) -> AppError {
    AppError::Backend(format!(
        "invalid relational synchronization state: {message}"
    ))
}
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |value, key| &value[key])
}
fn put(value: &mut Value, path: &str, item: Value) {
    let keys = path.split('.').collect::<Vec<_>>();
    let mut node = value;
    for key in &keys[..keys.len() - 1] {
        if !node[*key].is_object() {
            node[*key] = json!({});
        }
        node = &mut node[*key];
    }
    node[keys[keys.len() - 1]] = item;
}
pub(crate) fn write_scope(conn: &Connection, id: i64, scope: &NativePinnedScope) -> Result<()> {
    let previous: Option<i64> = conn.query_row(
        "SELECT native_account_id FROM sync_connections WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let user = scope
        .authentication
        .as_ref()
        .and_then(|a| a.user.as_ref())
        .map(|u| u.id.as_str());
    conn.execute(
        "INSERT OR IGNORE INTO sync_accounts(public_base_url,tenant,user_id) VALUES(?1,?2,?3)",
        params![scope.public_base_url, scope.namespace.tenant, user],
    )?;
    let account: i64 = conn.query_row(
        "SELECT id FROM sync_accounts WHERE public_base_url=?1 AND tenant=?2 AND user_id IS ?3",
        params![scope.public_base_url, scope.namespace.tenant, user],
        |r| r.get(0),
    )?;
    conn.execute(
        "UPDATE sync_connections SET native_account_id=?2 WHERE id=?1",
        params![id, account],
    )?;
    if let Some(previous) = previous.filter(|old| *old != account) {
        conn.execute(
            "UPDATE settings SET value=?1 WHERE key='native_sync_account' AND value=?2",
            params![account.to_string(), previous.to_string()],
        )?;
        conn.execute("DELETE FROM sync_accounts WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM sync_connections WHERE native_account_id=?1)",[previous])?;
    }
    let value = serde_json::to_value(scope).map_err(invalid)?;
    let mut values = vec![SqlValue::Integer(id)];
    for (_, path, _) in SCOPE_FIELDS {
        let v = at(&value, path);
        values.push(match v {
            Value::Null => SqlValue::Null,
            Value::Bool(v) => SqlValue::Integer(i64::from(*v)),
            Value::Number(v) => SqlValue::Integer(
                v.as_i64()
                    .ok_or_else(|| invalid("capability integer out of range"))?,
            ),
            Value::String(v) => SqlValue::Text(v.clone()),
            _ => return Err(invalid("non-scalar capability field")),
        });
    }
    let set = SCOPE_FIELDS
        .iter()
        .enumerate()
        .map(|(i, (name, _, _))| format!("{name}=?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    conn.execute(&format!("UPDATE sync_connections SET native_pinned=1,{set},native_probed_at=unixepoch(),native_probe_error=NULL,updated_at=unixepoch() WHERE id=?1"),params_from_iter(values))?;
    conn.execute(
        "DELETE FROM sync_connection_zones WHERE connection_id=?1",
        [id],
    )?;
    for (position, name) in scope.zones.iter().enumerate() {
        conn.execute(
            "INSERT INTO sync_connection_zones(connection_id,position,zone_name) VALUES(?1,?2,?3)",
            params![id, position as i64, name],
        )?;
    }
    Ok(())
}
pub(crate) fn read_scope(conn: &Connection, id: i64) -> Result<Option<NativePinnedScope>> {
    let columns = SCOPE_FIELDS
        .iter()
        .map(|(c, _, _)| *c)
        .collect::<Vec<_>>()
        .join(",");
    let row=conn.query_row(&format!("SELECT native_pinned,{columns} FROM sync_connections WHERE id=?1 AND provider_kind='native_syncserver'"),[id],|r|{let pinned=r.get::<_,bool>(0)?;let values=(0..SCOPE_FIELDS.len()).map(|i|r.get::<_,SqlValue>(i+1)).collect::<rusqlite::Result<Vec<_>>>()?;Ok((pinned,values))}).optional()?;
    let Some((true, fields)) = row else {
        return Ok(None);
    };
    let mut value = json!({});
    for ((_, path, boolean), field) in SCOPE_FIELDS.iter().zip(fields) {
        let field = match field {
            SqlValue::Null => Value::Null,
            SqlValue::Integer(n) if *boolean => json!(n != 0),
            SqlValue::Integer(n) => json!(n),
            SqlValue::Text(s) => json!(s),
            _ => return Err(invalid("invalid scalar capability field")),
        };
        put(&mut value, path, field);
    }
    if value["authentication"]["mode"].is_null() {
        value["authentication"] = Value::Null;
    } else if value["authentication"]["user"]["id"].is_null() {
        value["authentication"]["user"] = Value::Null;
    }
    let zones = {
        let mut s = conn.prepare(
            "SELECT zone_name FROM sync_connection_zones WHERE connection_id=?1 ORDER BY position",
        )?;
        let rows = s
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    value["zones"] = json!(zones);
    serde_json::from_value(value).map(Some).map_err(invalid)
}
pub(crate) fn write_bootstrap(conn: &Connection, job: i64, state: &BootstrapState) -> Result<()> {
    conn.execute(
        "UPDATE sync_native_progress SET bootstrap_started=?2 WHERE job_id=?1",
        params![job, state.started],
    )?;
    conn.execute("DELETE FROM sync_bootstrap_types WHERE job_id=?1", [job])?;
    for (kind, marker) in &state.types {
        conn.execute("INSERT INTO sync_bootstrap_types(job_id,record_type,complete,continuation_marker,record_count) VALUES(?1,?2,?3,?4,?5)",params![job,kind,marker.status==TypeStatus::Complete,marker.continuation_marker,marker.record_count])?;
    }
    Ok(())
}
pub(crate) fn read_bootstrap(conn: &Connection, job: i64, started: bool) -> Result<BootstrapState> {
    let mut s=conn.prepare("SELECT record_type,complete,continuation_marker,record_count FROM sync_bootstrap_types WHERE job_id=?1 ORDER BY record_type")?;
    let types = s
        .query_map([job], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TypeMarker {
                    status: if r.get::<_, bool>(1)? {
                        TypeStatus::Complete
                    } else {
                        TypeStatus::Pending
                    },
                    continuation_marker: r.get(2)?,
                    record_count: r.get(3)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(BootstrapState { started, types })
}
pub(crate) fn clear_bootstrap(conn: &Connection, job: i64) -> Result<()> {
    conn.execute("DELETE FROM sync_bootstrap_types WHERE job_id=?1", [job])?;
    Ok(())
}
pub(crate) fn read_change(conn: &Connection, job: i64) -> Result<Option<super::PendingChange>> {
    let row=conn.query_row("SELECT revision,desired_direction,albums_changed,delete_requested FROM sync_job_changes WHERE job_id=?1",[job],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,bool>(2)?,r.get::<_,bool>(3)?))).optional()?;
    let Some((revision, direction, albums_changed, delete)) = row else {
        return Ok(None);
    };
    let upload_albums = if albums_changed {
        let mut s = conn.prepare(
            "SELECT relative_album FROM sync_job_change_albums WHERE job_id=?1 ORDER BY position",
        )?;
        let rows = s
            .query_map([job], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Some(rows)
    } else {
        None
    };
    let direction = direction
        .map(|d| {
            super::super::SyncDirection::parse(&d)
                .ok_or_else(|| invalid("invalid desired direction"))
        })
        .transpose()?;
    Ok(Some(super::PendingChange {
        revision,
        direction,
        upload_albums,
        delete,
    }))
}
pub(crate) fn write_change(
    conn: &Connection,
    job: i64,
    pending: &super::PendingChange,
) -> Result<()> {
    let changed=conn.execute("UPDATE sync_job_changes SET revision=?2,desired_direction=?3,albums_changed=?4,delete_requested=?5 WHERE job_id=?1",params![job,pending.revision,pending.direction.map(|d|d.as_str()),pending.upload_albums.is_some(),pending.delete])?;
    if changed == 0 {
        conn.execute("INSERT INTO sync_job_changes(job_id,revision,desired_direction,albums_changed,delete_requested) VALUES(?1,?2,?3,?4,?5)",params![job,pending.revision,pending.direction.map(|d|d.as_str()),pending.upload_albums.is_some(),pending.delete])?;
    }
    conn.execute("DELETE FROM sync_job_change_albums WHERE job_id=?1", [job])?;
    if let Some(albums) = &pending.upload_albums {
        for (position, album) in albums.iter().enumerate() {
            conn.execute("INSERT INTO sync_job_change_albums(job_id,position,relative_album) VALUES(?1,?2,?3)",params![job,position as i64,album])?;
        }
    }
    Ok(())
}

pub(crate) fn legacy_columns(table: &str) -> &'static [&'static str] {
    match table {
        "sync_entries" => super::records::LEGACY_ENTRY_COLUMNS,
        "sync_connections" => &["native_pinned_scope", "native_zones"],
        "sync_native_progress" => &["bootstrap_state", "round_state"],
        "sync_job_changes" => &["payload"],
        "sync_publications" => &["captured_versions", "role_state", "old_snapshot"],
        "sync_conflicts" => &["native_detail"],
        "sync_tasks" => &["native_capture"],
        _ => &[],
    }
}
pub(crate) fn migrate_legacy_table(conn: &Connection, table: &str) -> Result<()> {
    let columns = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let legacy = legacy_columns(table)
        .iter()
        .filter(|name| columns.iter().any(|c| c == **name))
        .copied()
        .collect::<Vec<_>>();
    if legacy.is_empty() {
        return Ok(());
    }
    match table {
        "sync_entries" => return super::records::migrate_legacy(conn),
        "sync_connections" => {
            let rows = {
                let mut stmt = conn
                    .prepare("SELECT id,native_pinned_scope,native_zones FROM sync_connections")?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (id, pinned, zones) in rows {
                if let Some(raw) = pinned {
                    let scope = serde_json::from_str(&raw).map_err(invalid)?;
                    write_scope(conn, id, &scope)?;
                } else if let Some(raw) = zones {
                    let zones: Vec<String> = serde_json::from_str(&raw).map_err(invalid)?;
                    for (position, name) in zones.iter().enumerate() {
                        conn.execute("INSERT OR REPLACE INTO sync_connection_zones(connection_id,position,zone_name) VALUES(?1,?2,?3)",params![id,position as i64,name])?;
                    }
                }
            }
        }
        "sync_native_progress" => {
            let rows = {
                let mut stmt = conn.prepare(
                    "SELECT job_id,bootstrap_state,round_state FROM sync_native_progress",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (job, raw, round) in rows {
                if let Some(raw) = raw {
                    let state = serde_json::from_str(&raw).map_err(invalid)?;
                    write_bootstrap(conn, job, &state)?;
                }
                if round.as_deref().is_some_and(|s| s != "null" && s != "{}") {
                    return Err(invalid(
                        "unrecognized legacy receive round; conversion rolled back",
                    ));
                }
            }
        }
        "sync_job_changes" => {
            let rows = {
                let mut stmt = conn.prepare("SELECT job_id,payload FROM sync_job_changes")?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (job, raw) in rows {
                if let Some(raw) = raw {
                    let value: Value = serde_json::from_str(&raw).map_err(invalid)?;
                    let pending = super::PendingChange {
                        revision: value["revision"].as_i64().unwrap_or(0),
                        direction: value["direction"]
                            .as_str()
                            .map(|d| {
                                super::super::SyncDirection::parse(d)
                                    .ok_or_else(|| invalid("legacy direction"))
                            })
                            .transpose()?,
                        delete: value["delete"].as_bool().unwrap_or(false),
                        upload_albums: value
                            .get("upload_albums")
                            .filter(|v| !v.is_null())
                            .map(|v| serde_json::from_value(v.clone()).map_err(invalid))
                            .transpose()?,
                    };
                    write_change(conn, job, &pending)?;
                }
            }
        }
        "sync_publications" | "sync_conflicts" => super::recovery::migrate_table(conn, table)?,
        "sync_tasks" => super::issued::migrate(conn)?,
        _ => {}
    }
    for column in legacy {
        conn.execute(&format!("ALTER TABLE {table} DROP COLUMN {column}"), [])?;
    }
    Ok(())
}

pub(crate) fn migrate_account_selection(conn: &Connection) -> Result<()> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key='native_sync_account'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = value else { return Ok(()) };
    if raw.parse::<i64>().is_ok() {
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Ok(());
    };
    let tuple = value.get("Ok").unwrap_or(&value).as_array();
    let legacy_id = value.get("Err").and_then(Value::as_i64);
    let ids = {
        let mut stmt=conn.prepare("SELECT c.id FROM sync_connections c WHERE EXISTS(SELECT 1 FROM sync_jobs j WHERE j.connection_id=c.id) ORDER BY c.id")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut selected = None;
    for id in ids {
        let scope = read_scope(conn, id)?;
        let matches = if let Some(tuple) = tuple {
            scope.as_ref().is_some_and(|s| {
                tuple.len() == 3
                    && tuple[0].as_str() == Some(s.public_base_url.as_str())
                    && tuple[1].as_str() == Some(s.namespace.tenant.as_str())
                    && tuple[2].as_str()
                        == s.authentication
                            .as_ref()
                            .and_then(|a| a.user.as_ref())
                            .map(|u| u.id.as_str())
            })
        } else {
            legacy_id == Some(id)
        };
        if matches {
            let account: Option<i64> = conn.query_row(
                "SELECT native_account_id FROM sync_connections WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            selected = Some(account.unwrap_or(-id));
            break;
        }
    }
    let selected = selected.unwrap_or(0);
    conn.execute(
        "UPDATE settings SET value=?1 WHERE key='native_sync_account'",
        [selected.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;

pub(crate) fn migrate_reset_intent(conn: &Connection) -> Result<()> {
    let old: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key='sync_reset_pending'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = old {
        let intent: super::reset::ResetIntent = serde_json::from_str(&raw).map_err(invalid)?;
        conn.execute("INSERT INTO sync_reset_state(id,token,last_error) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET token=excluded.token,last_error=excluded.last_error",params![intent.token,intent.error])?;
        conn.execute("DELETE FROM settings WHERE key='sync_reset_pending'", [])?;
    }
    Ok(())
}
