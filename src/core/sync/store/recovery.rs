//! Relational conflict and file-publication recovery evidence.
use crate::core::error::{AppError, Result};
use crate::core::sync::native::records::RemotePhoto;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde_json::{json, Value};
fn invalid(message: impl std::fmt::Display) -> AppError {
    AppError::Backend(format!("invalid native recovery state: {message}"))
}
const CAPTURE: &[(&str, &str, bool)] = &[
    ("job", "capture_job", false),
    ("entry", "capture_entry", false),
    ("local", "capture_local_version", false),
    ("config", "capture_config", false),
    ("dirty", "capture_dirty", false),
    ("photo", "capture_photo", false),
    ("contentGeneration", "capture_content_generation", false),
    ("preserveMetadata", "capture_preserve_metadata", true),
    ("localRelativePath", "capture_local_path", false),
    ("favorite", "capture_favorite", false),
    ("takenAt", "capture_taken_at", false),
];
const DETAILS: &[(&str, &str, bool)] = &[
    ("collision", "recovery_collision", true),
    ("source", "recovery_source", false),
    ("preserved", "recovery_preserved", false),
    ("root", "recovery_root", false),
    ("target", "recovery_target", false),
    ("new", "recovery_new", false),
    ("backup", "recovery_backup", false),
    ("oldPath", "recovery_old_path", false),
    ("sha256", "recovery_sha256", false),
    ("size", "recovery_size", false),
    ("hadOld", "recovery_had_old", true),
    ("stamp.0", "recovery_stamp_0", false),
    ("stamp.1", "recovery_stamp_1", false),
    ("stamp.2", "recovery_stamp_2", false),
    ("stamp.3", "recovery_stamp_3", false),
    ("stamp.4", "recovery_stamp_4", false),
    ("oldStamp.0", "recovery_old_stamp_0", false),
    ("oldStamp.1", "recovery_old_stamp_1", false),
    ("oldLocalStamp.0", "recovery_old_local_stamp_0", false),
    ("oldLocalStamp.1", "recovery_old_local_stamp_1", false),
];
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |v, key| {
        if let Ok(i) = key.parse::<usize>() {
            &v[i]
        } else {
            &v[key]
        }
    })
}
fn write_scalars(
    conn: &Connection,
    id: &str,
    fields: &[(&str, &str, bool)],
    value: Option<&Value>,
) -> Result<()> {
    let mut values = vec![SqlValue::Text(id.into())];
    for (key, _, _) in fields {
        let v = value.map(|v| at(v, key)).unwrap_or(&Value::Null);
        values.push(match v {
            Value::Null => SqlValue::Null,
            Value::String(s) => SqlValue::Text(s.clone()),
            Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
            Value::Number(n) => SqlValue::Integer(
                n.as_i64()
                    .ok_or_else(|| invalid("recovery integer out of range"))?,
            ),
            _ => return Err(invalid("recovery field is not scalar")),
        });
    }
    let set = fields
        .iter()
        .enumerate()
        .map(|(i, (_, column, _))| format!("{column}=?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    conn.execute(
        &format!("UPDATE sync_publications SET {set} WHERE publication_id=?1"),
        params_from_iter(values),
    )?;
    Ok(())
}
fn read_scalars(conn: &Connection, id: &str, fields: &[(&str, &str, bool)]) -> Result<Value> {
    let columns = fields
        .iter()
        .map(|(_, column, _)| *column)
        .collect::<Vec<_>>()
        .join(",");
    let values = conn.query_row(
        &format!("SELECT {columns} FROM sync_publications WHERE publication_id=?1"),
        [id],
        |r| {
            (0..fields.len())
                .map(|i| r.get::<_, SqlValue>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        },
    )?;
    let mut output = json!({});
    for ((key, _, boolean), value) in fields.iter().zip(values) {
        let v = match value {
            SqlValue::Null => Value::Null,
            SqlValue::Integer(n) if *boolean => json!(n != 0),
            SqlValue::Integer(n) => json!(n),
            SqlValue::Text(s) => json!(s),
            _ => return Err(invalid("invalid recovery scalar")),
        };
        if let Some((name, index)) = key.split_once('.') {
            if !v.is_null() {
                if !output[name].is_array() {
                    output[name] = json!([]);
                }
                let array = output[name].as_array_mut().unwrap();
                let i: usize = index.parse().map_err(invalid)?;
                while array.len() <= i {
                    array.push(Value::Null);
                }
                array[i] = v;
            }
        } else {
            output[*key] = v;
        }
    }
    Ok(output)
}
pub(crate) fn capture(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let present:Option<(bool,bool)>=conn.query_row("SELECT capture_present,capture_projection_present FROM sync_publications WHERE publication_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((true, projection)) = present else {
        return Ok(None);
    };
    let mut value = read_scalars(conn, id, CAPTURE)?;
    if projection {
        value["projection"] = match super::records::load_photo(conn, "publication", id)? {
            Some(photo) => json!(serde_json::to_string(&photo).map_err(invalid)?),
            None => Value::Null,
        };
    }
    Ok(Some(value))
}
pub(crate) fn set_capture(conn: &Connection, id: &str, value: &Value) -> Result<()> {
    let (job,entry,state,present):(i64,Option<i64>,String,bool)=conn.query_row("SELECT job_id,entry_id,state,capture_present FROM sync_publications WHERE publication_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if present && state != "pending" && state != "superseded" {
        return Ok(());
    }
    write_scalars(conn, id, CAPTURE, Some(value))?;
    let projection = value.get("projection");
    conn.execute("UPDATE sync_publications SET capture_present=1,capture_projection_present=?2 WHERE publication_id=?1",params![id,projection.is_some()])?;
    if let Some(raw) = projection.and_then(Value::as_str) {
        let photo = serde_json::from_str(raw).map_err(invalid)?;
        super::records::save_photo(
            conn,
            "publication",
            id,
            job,
            entry.or_else(|| value["entry"].as_i64()).unwrap_or(0),
            &photo,
        )?;
    } else {
        super::records::clear(conn, "publication", id)?;
    }
    Ok(())
}
pub(crate) fn details(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let present: bool = conn.query_row(
        "SELECT recovery_present FROM sync_publications WHERE publication_id=?1",
        [id],
        |r| r.get(0),
    )?;
    if present {
        read_scalars(conn, id, DETAILS).map(Some)
    } else {
        Ok(None)
    }
}
pub(crate) fn set_details(conn: &Connection, id: &str, value: Option<&Value>) -> Result<()> {
    write_scalars(conn, id, DETAILS, value)?;
    conn.execute("UPDATE sync_publications SET recovery_present=?2,recovery_old_snapshot_path=?3 WHERE publication_id=?1",params![id,value.is_some(),value.filter(|v|v["hadOld"]==true).and_then(|v|v["backup"].as_str())])?;
    Ok(())
}
pub(crate) fn conflict_document(conn: &Connection, id: i64) -> Result<Option<String>> {
    let row=conn.query_row("SELECT native_detail_present,native_reason,native_recovery_path,native_remote_present FROM sync_conflicts WHERE id=?1",[id],|r|Ok((r.get::<_,bool>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,bool>(3)?))).optional()?;
    let Some((true, reason, recovery, remote)) = row else {
        return Ok(None);
    };
    let mut value = json!({"reason":reason});
    if remote {
        value["remote"] = json!(super::records::load_photo(
            conn,
            "conflict",
            &id.to_string()
        )?);
    }
    if let Some(path) = recovery {
        value["recovery"] = json!(path);
    }
    serde_json::to_string(&value).map(Some).map_err(invalid)
}
pub(crate) fn write_conflict(
    conn: &Connection,
    job: i64,
    entry: i64,
    reason: &str,
    photo: Option<&RemotePhoto>,
) -> Result<()> {
    conn.execute("INSERT INTO sync_conflicts(job_id,entry_id,kind,native_detail_present,native_reason,native_remote_present) VALUES(?1,?2,'native',1,?3,1) ON CONFLICT(entry_id) WHERE state='open' DO UPDATE SET native_detail_present=1,native_reason=excluded.native_reason,native_remote_present=1,native_recovery_path=NULL",params![job,entry,reason])?;
    let id: i64 = conn.query_row(
        "SELECT id FROM sync_conflicts WHERE entry_id=?1 AND state='open'",
        [entry],
        |r| r.get(0),
    )?;
    if let Some(photo) = photo {
        super::records::save_photo(conn, "conflict", &id.to_string(), job, entry, photo)?;
    } else {
        super::records::clear(conn, "conflict", &id.to_string())?;
    }
    Ok(())
}
pub(crate) fn migrate_table(conn: &Connection, table: &str) -> Result<()> {
    match table {
        "sync_publications" => {
            let rows = {
                let mut s=conn.prepare("SELECT publication_id,captured_versions,role_state,old_snapshot FROM sync_publications")?;
                let rows = s
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, Option<String>>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (id, capture, details, old) in rows {
                if let Some(raw) = capture {
                    let value = serde_json::from_str(&raw).map_err(invalid)?;
                    set_capture(conn, &id, &value)?;
                }
                if let Some(raw) = details {
                    let value = serde_json::from_str(&raw).map_err(invalid)?;
                    set_details(conn, &id, Some(&value))?;
                }
                if let Some(raw) = old {
                    let value: Value = serde_json::from_str(&raw).map_err(invalid)?;
                    conn.execute("UPDATE sync_publications SET recovery_old_snapshot_path=?2 WHERE publication_id=?1",params![id,value["path"].as_str()])?;
                }
            }
        }
        "sync_conflicts" => {
            let rows = {
                let mut s=conn.prepare("SELECT id,job_id,entry_id,native_detail FROM sync_conflicts WHERE native_detail IS NOT NULL")?;
                let rows = s
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (id, job, entry, raw) in rows {
                let value: Value = serde_json::from_str(&raw).map_err(invalid)?;
                conn.execute("UPDATE sync_conflicts SET native_detail_present=1,native_reason=?2,native_recovery_path=?3,native_remote_present=?4 WHERE id=?1",params![id,value["reason"].as_str(),value["recovery"].as_str(),value.get("remote").is_some()])?;
                if let Some(value) = value.get("remote").filter(|v| !v.is_null()) {
                    let photo = serde_json::from_value(value.clone()).map_err(invalid)?;
                    super::records::save_photo(
                        conn,
                        "conflict",
                        &id.to_string(),
                        job,
                        entry,
                        &photo,
                    )?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
