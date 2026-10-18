//! Issued-operation evidence in typed columns and relational request records.
use super::runtime::Issued;
use crate::core::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
fn invalid(message: impl std::fmt::Display) -> AppError {
    AppError::Backend(format!("invalid issued operation state: {message}"))
}
fn member_kind(value: Option<&Value>) -> i64 {
    match value {
        None => 0,
        Some(Value::Null) => 1,
        Some(_) => 2,
    }
}
fn put_member(target: &mut Value, name: &str, kind: i64, value: Value) {
    if kind != 0 {
        target[name] = if kind == 1 { Value::Null } else { value };
    }
}

pub(crate) fn write(conn: &Connection, issued: &Issued) -> Result<()> {
    if !issued.body.is_null() && !issued.body.is_object() {
        return Err(invalid("request body must be an object"));
    }
    if !issued.preparation.is_null() && !issued.preparation.is_object() {
        return Err(invalid("preparation must be an object"));
    }
    for name in ["metadata", "events", "resources"] {
        if issued
            .preparation
            .get(name)
            .is_some_and(|v| !v.is_null() && !v.is_object())
        {
            return Err(invalid("preparation member must be an object"));
        }
    }
    if issued
        .body
        .get("operations")
        .is_some_and(|v| !v.is_null() && !v.is_array())
    {
        return Err(invalid("operations must be an array"));
    }

    let body = if issued.body.is_null() {
        None
    } else {
        Some(&issued.body)
    };
    let prep = if issued.preparation.is_null() {
        None
    } else {
        Some(&issued.preparation)
    };
    let record_entry = if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_entries WHERE id=?1)",
        [issued.entry],
        |r| r.get::<_, bool>(0),
    )? {
        Some(issued.entry)
    } else {
        None
    };
    let metadata = prep.and_then(|v| v.get("metadata"));
    let events = prep.and_then(|v| v.get("events"));
    let resources = prep.and_then(|v| v.get("resources"));
    conn.execute("UPDATE sync_tasks SET capture_present=1,capture_job=?2,capture_entry=?3,capture_config=?4,capture_generation=?5,capture_phase=?6,capture_local_version=?7,capture_metadata_version=?8,capture_content_version=?9,capture_deletion_version=?10,capture_dirty=?11,capture_photo=?12,capture_master=?13,capture_content_generation=?14,body_present=?15,body_zone_kind=?16,body_zone_name=?17,body_zone_owner=?18,body_atomic_present=?19,body_atomic=?20,body_operations_kind=?21,prep_present=?22,prep_metadata_kind=?23,prep_events_kind=?24,prep_resources_kind=?25,prep_send_present=?26,prep_send_metadata=?27,prep_asset_revision_present=?28,prep_asset_revision=?29,prep_master_revision_present=?30,prep_master_revision=?31 WHERE operation_id=?1",
    params![issued.id,issued.job,issued.entry,issued.config,issued.generation,issued.phase,issued.versions[0],issued.versions[1],issued.versions[2],issued.versions[3],issued.dirty,issued.photo,issued.master,issued.content_generation,body.is_some(),member_kind(body.and_then(|v|v.get("zoneID"))),body.and_then(|v|v["zoneID"]["zoneName"].as_str()),body.and_then(|v|v["zoneID"]["ownerRecordName"].as_str()),body.is_some_and(|v|v.get("atomic").is_some()),body.and_then(|v|v["atomic"].as_bool()),member_kind(body.and_then(|v|v.get("operations"))),prep.is_some(),member_kind(metadata),member_kind(events),member_kind(resources),prep.is_some_and(|v|v.get("sendMetadata").is_some()),prep.and_then(|v|v["sendMetadata"].as_bool()),prep.is_some_and(|v|v.get("assetRevision").is_some()),prep.and_then(|v|v["assetRevision"].as_str()),prep.is_some_and(|v|v.get("masterRevision").is_some()),prep.and_then(|v|v["masterRevision"].as_str())])?;
    super::records::clear(conn, "issued", &issued.id)?;
    super::records::clear(conn, "preparation", &issued.id)?;
    for role in ["original", "thumbnail"] {
        super::records::clear(conn, "prepared_asset", &format!("{}:{role}", issued.id))?;
    }

    if let Some(operations) = body.and_then(|b| b["operations"].as_array()) {
        for (position, operation) in operations.iter().enumerate() {
            super::records::save_value(
                conn,
                "issued",
                &issued.id,
                issued.job,
                record_entry,
                position as i64,
                operation["operationType"].as_str(),
                &operation["record"],
            )?;
        }
    }
    let mut prepared = json!({"recordName":issued.photo});
    if let Some(metadata) = metadata {
        prepared["fields"] = metadata.clone();
    }
    if let Some(events) = events {
        prepared["_syncserver"] = events.clone();
    }
    super::records::save_value(
        conn,
        "preparation",
        &issued.id,
        issued.job,
        record_entry,
        0,
        None,
        &prepared,
    )?;
    let task: i64 = conn.query_row(
        "SELECT id FROM sync_tasks WHERE operation_id=?1",
        [&issued.id],
        |r| r.get(0),
    )?;
    conn.execute("DELETE FROM sync_task_resources WHERE task_id=?1", [task])?;
    if let Some(resources) = resources.and_then(Value::as_object) {
        for (role, resource) in resources {
            if !matches!(role.as_str(), "original" | "thumbnail") {
                return Err(invalid("unknown prepared resource role"));
            }
            let mask = ["path", "hex", "base64", "size", "session", "asset"]
                .iter()
                .enumerate()
                .fold(0i64, |bits, (i, key)| {
                    bits | if resource.get(*key).is_some() {
                        1 << i
                    } else {
                        0
                    }
                });
            conn.execute("INSERT INTO sync_task_resources(task_id,role,document_present,member_mask,source_path,digest_sha256,digest_base64,digest_size,session_id,asset_kind) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![task,role,!resource.is_null(),mask,resource["path"].as_str(),resource["hex"].as_str(),resource["base64"].as_str(),resource["size"].as_i64(),resource["session"].as_str(),member_kind(resource.get("asset"))])?;
            if let Some(asset) = resource.get("asset").filter(|v| !v.is_null()) {
                let record = json!({"recordName":issued.photo,"fields":{"asset":{"type":"ASSETID","value":asset}}});
                super::records::save_value(
                    conn,
                    "prepared_asset",
                    &format!("{}:{role}", issued.id),
                    issued.job,
                    record_entry,
                    0,
                    None,
                    &record,
                )?;
            }
        }
    }
    if read(conn, &issued.id)?.as_ref() != Some(issued) {
        return Err(invalid(
            "operation cannot be represented losslessly in typed fields",
        ));
    }

    Ok(())
}
pub(crate) fn read(conn: &Connection, operation: &str) -> Result<Option<Issued>> {
    let exists: Option<(bool, i64)> = conn
        .query_row(
            "SELECT capture_present,id FROM sync_tasks WHERE operation_id=?1",
            [operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((true, task)) = exists else {
        return Ok(None);
    };
    let(mut issued,body_present,zone_kind,zone_name,zone_owner,atomic_present,atomic,operations_kind,prep_present,metadata_kind,events_kind,resources_kind,send_present,send_metadata,asset_present,asset_revision,master_present,master_revision)=conn.query_row("SELECT capture_job,capture_entry,capture_config,capture_generation,capture_phase,capture_local_version,capture_metadata_version,capture_content_version,capture_deletion_version,capture_dirty,capture_photo,capture_master,capture_content_generation,body_present,body_zone_kind,body_zone_name,body_zone_owner,body_atomic_present,body_atomic,body_operations_kind,prep_present,prep_metadata_kind,prep_events_kind,prep_resources_kind,prep_send_present,prep_send_metadata,prep_asset_revision_present,prep_asset_revision,prep_master_revision_present,prep_master_revision FROM sync_tasks WHERE id=?1",[task],|r|Ok((Issued{id:operation.into(),job:r.get(0)?,entry:r.get(1)?,config:r.get(2)?,generation:r.get(3)?,phase:r.get(4)?,versions:[r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?],dirty:r.get(9)?,photo:r.get(10)?,master:r.get(11)?,content_generation:r.get(12)?,body:Value::Null,preparation:Value::Null},r.get::<_,bool>(13)?,r.get::<_,i64>(14)?,r.get::<_,Option<String>>(15)?,r.get::<_,Option<String>>(16)?,r.get::<_,bool>(17)?,r.get::<_,Option<bool>>(18)?,r.get::<_,i64>(19)?,r.get::<_,bool>(20)?,r.get::<_,i64>(21)?,r.get::<_,i64>(22)?,r.get::<_,i64>(23)?,r.get::<_,bool>(24)?,r.get::<_,Option<bool>>(25)?,r.get::<_,bool>(26)?,r.get::<_,Option<String>>(27)?,r.get::<_,bool>(28)?,r.get::<_,Option<String>>(29)?)))?;
    if body_present {
        let mut body = json!({});
        put_member(
            &mut body,
            "zoneID",
            zone_kind,
            json!({"zoneName":zone_name,"ownerRecordName":zone_owner}),
        );
        if atomic_present {
            body["atomic"] = json!(atomic);
        }
        if operations_kind != 0 {
            let ids = {
                let mut s=conn.prepare("SELECT id,operation_type FROM sync_records WHERE owner_kind='issued' AND owner_key=?1 ORDER BY position")?;
                let rows = s
                    .query_map([operation], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let operations = ids
                .into_iter()
                .map(|(id, kind)| {
                    let record = super::records::load_value(conn, id)?;
                    let mut value = json!({"record":record});
                    if let Some(kind) = kind {
                        value["operationType"] = json!(kind);
                    }
                    Ok(value)
                })
                .collect::<Result<Vec<_>>>()?;
            put_member(&mut body, "operations", operations_kind, json!(operations));
        }
        issued.body = body;
    }
    if prep_present {
        let prepared_id: i64 = conn.query_row(
            "SELECT id FROM sync_records WHERE owner_kind='preparation' AND owner_key=?1 LIMIT 1",
            [operation],
            |r| r.get(0),
        )?;
        let prepared = super::records::load_value(conn, prepared_id)?;
        let mut prep = json!({});
        put_member(
            &mut prep,
            "metadata",
            metadata_kind,
            prepared["fields"].clone(),
        );
        put_member(
            &mut prep,
            "events",
            events_kind,
            prepared["_syncserver"].clone(),
        );
        if send_present {
            prep["sendMetadata"] = json!(send_metadata);
        }
        if asset_present {
            prep["assetRevision"] = json!(asset_revision);
        }
        if master_present {
            prep["masterRevision"] = json!(master_revision);
        }
        if resources_kind != 0 {
            let mut resources = json!({});
            let rows = {
                let mut s=conn.prepare("SELECT role,document_present,member_mask,source_path,digest_sha256,digest_base64,digest_size,session_id,asset_kind FROM sync_task_resources WHERE task_id=?1 ORDER BY role")?;
                let rows = s
                    .query_map([task], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, bool>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, Option<i64>>(6)?,
                            r.get::<_, Option<String>>(7)?,
                            r.get::<_, i64>(8)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (role, present, mask, path, hex, base64, size, session, asset_kind) in rows {
                if !present {
                    resources[&role] = Value::Null;
                    continue;
                }
                let mut resource = json!({});
                for (i, key, value) in [
                    (0, "path", json!(path)),
                    (1, "hex", json!(hex)),
                    (2, "base64", json!(base64)),
                    (3, "size", json!(size)),
                    (4, "session", json!(session)),
                ] {
                    if mask & (1 << i) != 0 {
                        resource[key] = value;
                    }
                }
                if asset_kind != 0 {
                    let id:Option<i64>=conn.query_row("SELECT id FROM sync_records WHERE owner_kind='prepared_asset' AND owner_key=?1 LIMIT 1",[format!("{operation}:{role}")],|r|r.get(0)).optional()?;
                    let asset = if let Some(id) = id {
                        super::records::load_value(conn, id)?["fields"]["asset"]["value"].clone()
                    } else {
                        Value::Null
                    };
                    put_member(&mut resource, "asset", asset_kind, asset);
                }
                resources[role] = resource;
            }
            put_member(&mut prep, "resources", resources_kind, resources);
        }
        issued.preparation = prep;
    }
    Ok(Some(issued))
}
pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    let rows = {
        let mut s =
            conn.prepare("SELECT native_capture FROM sync_tasks WHERE native_capture IS NOT NULL")?;
        let rows = s
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for raw in rows {
        let issued = serde_json::from_str(&raw).map_err(invalid)?;
        write(conn, &issued)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests;
