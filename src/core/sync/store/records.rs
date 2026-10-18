//! Relational native record storage. JSON exists only at the wire/DTO boundary.
use crate::core::error::{AppError, Result};
use crate::core::sync::native::receive::{EditEvent, EditEvents, NativeRecord, ZoneId};
use crate::core::sync::native::records::{RecordSet, RemotePhoto};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

fn invalid(message: &str) -> AppError {
    AppError::Backend(message.into())
}
fn string(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_owned)
}
fn event(record: &NativeRecord, category: usize) -> Option<&EditEvent> {
    record.events.as_ref().and_then(|e| match category {
        0 => e.metadata.as_ref(),
        1 => e.content.as_ref(),
        _ => e.deletion.as_ref(),
    })
}

pub(super) fn save(
    tx: &Connection,
    kind: &str,
    owner: &str,
    job: i64,
    entry: Option<i64>,
    position: i64,
    operation: Option<&str>,
    record: &NativeRecord,
) -> Result<i64> {
    let fields = record.fields.as_ref();
    let field = |name: &str| {
        fields
            .and_then(|f| f.get(name))
            .and_then(|v| v.get("value"))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let asset = record.record_type.as_deref() == Some("CPLAsset");
    let photo_id = if asset {
        Some(record.record_name.clone())
    } else {
        field("photoId").as_str().map(str::to_owned)
    };
    let master_id = if asset {
        string(&field("masterRef"), "recordName")
    } else {
        Some(record.record_name.clone())
    };
    let zone = record.zone_id.as_ref();
    let mut events = Vec::new();
    for category in 0..3 {
        let e = event(record, category);
        events.push((
            e.map(|e| e.edited_at_ms),
            e.map(|e| e.source.as_str()),
            e.map(|e| e.event_id.as_str()),
        ));
    }
    tx.execute("INSERT INTO sync_records(owner_kind,owner_key,job_id,entry_id,position,operation_type,record_name,record_type,record_revision,photo_id,master_id,relative_path,content_generation,zone_name,zone_owner,deleted,fields_present,events_present,metadata_edited_ms,metadata_source,metadata_event_id,content_edited_ms,content_source,content_event_id,deletion_edited_ms,deletion_source,deletion_event_id)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)
        ON CONFLICT(owner_kind,owner_key,position) DO UPDATE SET operation_type=excluded.operation_type,record_name=excluded.record_name,record_type=excluded.record_type,record_revision=excluded.record_revision,photo_id=excluded.photo_id,master_id=excluded.master_id,relative_path=excluded.relative_path,content_generation=excluded.content_generation,zone_name=excluded.zone_name,zone_owner=excluded.zone_owner,deleted=excluded.deleted,fields_present=excluded.fields_present,events_present=excluded.events_present,metadata_edited_ms=excluded.metadata_edited_ms,metadata_source=excluded.metadata_source,metadata_event_id=excluded.metadata_event_id,content_edited_ms=excluded.content_edited_ms,content_source=excluded.content_source,content_event_id=excluded.content_event_id,deletion_edited_ms=excluded.deletion_edited_ms,deletion_source=excluded.deletion_source,deletion_event_id=excluded.deletion_event_id",
        params![kind,owner,job,entry,position,operation,record.record_name,record.record_type,record.record_change_tag,photo_id,master_id,field("relativePath").as_str(),field("contentGeneration").as_str(),zone.map(|z|z.zone_name.as_str()),zone.map(|z|z.owner_record_name.as_str()),record.deleted,fields.is_some(),record.events.is_some(),events[0].0,events[0].1,events[0].2,events[1].0,events[1].1,events[1].2,events[2].0,events[2].1,events[2].2])?;
    let id: i64 = tx.query_row(
        "SELECT id FROM sync_records WHERE owner_kind=?1 AND owner_key=?2 AND position=?3",
        params![kind, owner, position],
        |r| r.get(0),
    )?;
    tx.execute("DELETE FROM sync_record_fields WHERE record_id=?1", [id])?;
    if let Some(fields) = fields {
        for (name, field) in fields {
            save_field(tx, id, name, field)?;
        }
    }
    Ok(id)
}

fn save_field(conn: &Connection, record: i64, name: &str, field: &Value) -> Result<()> {
    let ty = field.get("type").and_then(Value::as_str);
    let value = field.get("value").unwrap_or(&Value::Null);
    let mut text = None;
    let mut integer = None;
    let mut real = None;
    let mut reference = None;
    let mut action = None;
    let mut asset_id = None;
    let mut checksum = None;
    let mut size = None;
    let mut mime = None;
    let mut width = None;
    let mut height = None;
    let mut algorithm = None;
    let value_kind = match value {
        Value::Null => "null",
        Value::String(v) => {
            text = Some(v.as_str());
            "text"
        }
        Value::Bool(v) => {
            integer = Some(i64::from(*v));
            "boolean"
        }
        Value::Number(v) => {
            if let Some(v) = v.as_i64() {
                integer = Some(v);
                "integer"
            } else {
                real = v.as_f64();
                "real"
            }
        }
        Value::Object(v) if v.contains_key("recordName") => {
            reference = v.get("recordName").and_then(Value::as_str);
            action = v.get("action").and_then(Value::as_str);
            "reference"
        }
        Value::Object(v) if v.contains_key("assetId") => {
            asset_id = v.get("assetId").and_then(Value::as_str);
            checksum = v.get("fileChecksum").and_then(Value::as_str);
            size = v.get("size").and_then(Value::as_i64);
            mime = v.get("mimeType").and_then(Value::as_str);
            width = v.get("width").and_then(Value::as_i64);
            height = v.get("height").and_then(Value::as_i64);
            algorithm = v.get("checksumAlgorithm").and_then(Value::as_str);
            "asset"
        }
        _ => return Err(invalid("unsupported structured native record field")),
    };
    conn.execute("INSERT INTO sync_record_fields(record_id,field_name,field_type,field_present,value_present,value_kind,text_value,integer_value,real_value,reference_name,reference_action,asset_id,asset_checksum,asset_size,asset_mime,asset_width,asset_height,checksum_algorithm) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",params![record,name,ty,!field.is_null(),field.get("value").is_some(),value_kind,text,integer,real,reference,action,asset_id,checksum,size,mime,width,height,algorithm])?;
    Ok(())
}

pub(super) fn load(conn: &Connection, kind: &str, owner: &str) -> Result<Vec<NativeRecord>> {
    let ids = {
        let mut s = conn.prepare(
            "SELECT id FROM sync_records WHERE owner_kind=?1 AND owner_key=?2 ORDER BY position",
        )?;
        let rows = s
            .query_map(params![kind, owner], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    ids.into_iter().map(|id| load_id(conn, id)).collect()
}
pub(super) fn load_id(conn: &Connection, id: i64) -> Result<NativeRecord> {
    let (mut record,has_fields):(NativeRecord,bool)=conn.query_row("SELECT record_name,record_type,record_revision,zone_name,zone_owner,deleted,fields_present,events_present,metadata_edited_ms,metadata_source,metadata_event_id,content_edited_ms,content_source,content_event_id,deletion_edited_ms,deletion_source,deletion_event_id FROM sync_records WHERE id=?1",[id],|r|{
        let stamp=|index|->rusqlite::Result<Option<EditEvent>>{r.get::<_,Option<i64>>(index)?.map(|edited_at_ms|Ok::<_,rusqlite::Error>(EditEvent{edited_at_ms,source:r.get(index+1)?,event_id:r.get(index+2)?})).transpose()};
        let zone=r.get::<_,Option<String>>(3)?.map(|zone_name|Ok::<_,rusqlite::Error>(ZoneId{zone_name,owner_record_name:r.get(4)?})).transpose()?;
        Ok((NativeRecord{record_name:r.get(0)?,record_type:r.get(1)?,record_change_tag:r.get(2)?,zone_id:zone,deleted:r.get(5)?,fields:None,events:if r.get::<_,bool>(7)?{Some(EditEvents{metadata:stamp(8)?,content:stamp(11)?,deletion:stamp(14)?})}else{None}},r.get(6)?))
    })?;
    if has_fields {
        let mut s=conn.prepare("SELECT field_name,field_type,field_present,value_present,value_kind,text_value,integer_value,real_value,reference_name,reference_action,asset_id,asset_checksum,asset_size,asset_mime,asset_width,asset_height,checksum_algorithm FROM sync_record_fields WHERE record_id=?1 ORDER BY field_name")?;
        let fields=s.query_map([id],|r|{
            let name:String=r.get(0)?;if !r.get::<_,bool>(2)?{return Ok((name,Value::Null));}
            let kind:String=r.get(4)?;
            let value=match kind.as_str(){
                "text"=>json!(r.get::<_,Option<String>>(5)?),"integer"=>json!(r.get::<_,Option<i64>>(6)?),"real"=>json!(r.get::<_,Option<f64>>(7)?),"boolean"=>json!(r.get::<_,Option<i64>>(6)?.is_some_and(|v|v!=0)),
                "reference"=>{let mut v=json!({"recordName":r.get::<_,Option<String>>(8)?});if let Some(action)=r.get::<_,Option<String>>(9)?{v["action"]=json!(action);}v},
                "asset"=>{let mut v=json!({"assetId":r.get::<_,Option<String>>(10)?,"fileChecksum":r.get::<_,Option<String>>(11)?,"size":r.get::<_,Option<i64>>(12)?,"mimeType":r.get::<_,Option<String>>(13)?});for (name,col) in [("width",14),("height",15)]{if let Some(n)=r.get::<_,Option<i64>>(col)?{v[name]=json!(n);}}if let Some(a)=r.get::<_,Option<String>>(16)?{v["checksumAlgorithm"]=json!(a);}v},_=>Value::Null,
            };
            let mut field=Map::new();if let Some(ty)=r.get::<_,Option<String>>(1)?{field.insert("type".into(),json!(ty));}
            if r.get::<_,bool>(3)?{field.insert("value".into(),value);}Ok((name,Value::Object(field)))
        })?.collect::<rusqlite::Result<Map<String,Value>>>()?;
        record.fields = Some(fields);
    }
    Ok(record)
}

pub(super) fn clear(conn: &Connection, kind: &str, owner: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM sync_records WHERE owner_kind=?1 AND owner_key=?2",
        params![kind, owner],
    )?;
    Ok(())
}

pub(super) fn save_photo(
    tx: &Connection,
    kind: &str,
    owner: &str,
    job: i64,
    entry: i64,
    photo: &RemotePhoto,
) -> Result<()> {
    let revisions: Option<(Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT native_asset_revision,native_master_revision FROM sync_entries WHERE id=?1",
            [entry],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let record_entry = revisions.as_ref().map(|_| entry);
    let (asset_revision, master_revision) = revisions.unwrap_or_default();
    let master:NativeRecord=serde_json::from_value(json!({"recordName":photo.master_id,"recordType":"CPLMaster","recordChangeTag":master_revision,"fields":{
        "photoId":{"type":"STRING","value":photo.photo_id},"contentGeneration":{"type":"STRING","value":photo.content_generation},"thumbnailSourceOriginalSHA256":{"type":"STRING","value":photo.thumbnail_source_sha256},"resOriginalRes":{"type":"ASSETID","value":photo.original},"resJPEGThumbRes":{"type":"ASSETID","value":photo.thumbnail}
    }})).map_err(|_|invalid("invalid structured master"))?;
    let asset:NativeRecord=serde_json::from_value(json!({"recordName":photo.photo_id,"recordType":"CPLAsset","recordChangeTag":asset_revision,"fields":{
        "masterRef":{"type":"REFERENCE","value":{"recordName":photo.master_id,"action":"NONE"}},"contentGeneration":{"type":"STRING","value":photo.content_generation},"relativePath":{"type":"STRING","value":photo.relative_path},"isFavorite":{"type":"INT64","value":photo.is_favorite as i64},"assetDate":{"type":"TIMESTAMP","value":photo.asset_date_ms},"isHidden":{"type":"INT64","value":photo.is_hidden as i64}
    },"_syncserver":photo.events})).map_err(|_|invalid("invalid structured photo"))?;
    save(tx, kind, owner, job, record_entry, 0, None, &master)?;
    save(tx, kind, owner, job, record_entry, 1, None, &asset)?;
    Ok(())
}
pub(super) fn load_photo(
    conn: &Connection,
    kind: &str,
    owner: &str,
) -> Result<Option<RemotePhoto>> {
    let records = load(conn, kind, owner)?;
    if records.is_empty() {
        return Ok(None);
    }
    let mut set = RecordSet::default();
    for mut r in records {
        r.record_change_tag
            .get_or_insert_with(|| "local-baseline".into());
        set.apply(r);
    }
    let graph = set
        .to_graph(&ZoneId::from(&crate::core::sync::native::NativeZone::new(
            "PrimarySync",
        )))
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(graph.photos.into_iter().next())
}

#[cfg(test)]
mod tests;

pub(super) fn baseline_document(conn: &Connection, entry: i64) -> Result<Option<String>> {
    load_photo(conn, "baseline", &entry.to_string())?
        .map(|p| serde_json::to_string(&p).map_err(|e| invalid(&e.to_string())))
        .transpose()
}
pub(super) fn set_baseline_document(
    conn: &Connection,
    entry: i64,
    document: Option<&str>,
) -> Result<()> {
    if let Some(document) = document {
        let photo: RemotePhoto =
            serde_json::from_str(document).map_err(|e| invalid(&e.to_string()))?;
        let job: i64 = conn.query_row(
            "SELECT job_id FROM sync_entries WHERE id=?1",
            [entry],
            |r| r.get(0),
        )?;
        save_photo(conn, "baseline", &entry.to_string(), job, entry, &photo)?;
    } else {
        clear(conn, "baseline", &entry.to_string())?;
    }
    conn.execute(
        "UPDATE sync_entries SET native_has_baseline=?2 WHERE id=?1",
        params![entry, document.is_some()],
    )?;
    Ok(())
}
pub(super) fn pending_document(
    conn: &Connection,
    entry: i64,
    master: bool,
) -> Result<Option<String>> {
    let position = if master { 0 } else { 1 };
    let id:Option<i64>=conn.query_row("SELECT id FROM sync_records WHERE owner_kind='pending' AND owner_key=?1 AND position=?2",params![entry.to_string(),position],|r|r.get(0)).optional()?;
    id.map(|id| {
        load_id(conn, id)
            .and_then(|r| serde_json::to_string(&r).map_err(|e| invalid(&e.to_string())))
    })
    .transpose()
}
pub(crate) fn set_pending(
    conn: &Connection,
    entry: i64,
    master: bool,
    record: Option<&NativeRecord>,
) -> Result<()> {
    let position = if master { 0 } else { 1 };
    if let Some(record) = record {
        let job = conn.query_row(
            "SELECT job_id FROM sync_entries WHERE id=?1",
            [entry],
            |r| r.get::<_, i64>(0),
        )?;
        save(
            conn,
            "pending",
            &entry.to_string(),
            job,
            Some(entry),
            position,
            None,
            record,
        )?;
    } else {
        conn.execute(
            "DELETE FROM sync_records WHERE owner_kind='pending' AND owner_key=?1 AND position=?2",
            params![entry.to_string(), position],
        )?;
    }
    if record.is_some() {
        conn.execute("UPDATE sync_records SET photo_id=CASE WHEN position=1 THEN record_name ELSE COALESCE(photo_id,(SELECT native_photo_id FROM sync_entries WHERE id=?1)) END WHERE owner_kind='pending' AND owner_key=?2 AND position=?3",params![entry,entry.to_string(),position])?;
    }
    let field = if master {
        "native_has_pending_master"
    } else {
        "native_has_pending_asset"
    };
    conn.execute(
        &format!("UPDATE sync_entries SET {field}=?2 WHERE id=?1"),
        params![entry, record.is_some()],
    )?;
    Ok(())
}
pub(super) fn clear_pending(conn: &Connection, entry: i64) -> Result<()> {
    clear(conn, "pending", &entry.to_string())?;
    conn.execute("UPDATE sync_entries SET native_has_pending_master=0,native_has_pending_asset=0 WHERE id=?1",[entry])?;
    Ok(())
}
pub(super) fn clear_job_pending(conn: &Connection, job: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM sync_records WHERE job_id=?1 AND owner_kind='pending'",
        [job],
    )?;
    conn.execute("UPDATE sync_entries SET native_has_pending_master=0,native_has_pending_asset=0 WHERE job_id=?1",[job])?;
    Ok(())
}

pub(crate) const LEGACY_ENTRY_COLUMNS: &[&str] = &[
    "native_metadata_event",
    "native_content_event",
    "native_deletion_event",
    "native_desired_metadata",
    "native_baseline_metadata",
    "native_resource_baseline",
    "native_pending_master",
    "native_pending_asset",
    "native_local_digest",
];

pub(crate) fn migrate_legacy(conn: &Connection) -> Result<()> {
    let legacy = conn
        .prepare("PRAGMA table_info(sync_entries)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !legacy.iter().any(|name| name == "native_resource_baseline") {
        return Ok(());
    }

    let mut s=conn.prepare("SELECT id,native_pending_master,native_pending_asset,native_resource_baseline FROM sync_entries WHERE native_pending_master IS NOT NULL OR native_pending_asset IS NOT NULL OR (native_resource_baseline IS NOT NULL AND native_has_baseline=0)")?;
    let rows = s
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(s);
    for (entry, master, asset, baseline) in rows {
        for (master, raw) in [(true, master), (false, asset)] {
            if let Some(raw) = raw {
                let record = serde_json::from_str(&raw)
                    .map_err(|e| invalid(&format!("cannot convert pending record: {e}")))?;
                set_pending(conn, entry, master, Some(&record))?;
            }
        }
        if let Some(raw) = baseline {
            set_baseline_document(conn, entry, Some(&raw))?;
        }
        conn.execute("UPDATE sync_entries SET native_pending_master=NULL,native_pending_asset=NULL WHERE id=?1",[entry])?;
    }
    let rows = {
        let mut stmt=conn.prepare("SELECT id,native_metadata_event,native_content_event,native_deletion_event,native_local_digest FROM sync_entries WHERE native_metadata_event IS NOT NULL OR native_content_event IS NOT NULL OR native_deletion_event IS NOT NULL OR native_local_digest IS NOT NULL")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (entry, metadata, content, deletion, digest) in rows {
        for (category, doc) in [
            ("metadata", metadata),
            ("content", content),
            ("deletion", deletion),
        ] {
            if let Some(doc) = doc {
                set_event_document(conn, entry, category, Some(&doc))?;
            }
        }
        if let Some(raw) = digest {
            let captured = serde_json::from_str(&raw)
                .map_err(|e| invalid(&format!("cannot convert cached digest: {e}")))?;
            set_digest(conn, entry, Some(&captured))?;
        }
    }
    for (column, kind) in [
        ("native_desired_metadata", "desired_metadata"),
        ("native_baseline_metadata", "baseline_metadata"),
    ] {
        let rows = {
            let mut stmt = conn.prepare(&format!(
                "SELECT id,job_id,{column} FROM sync_entries WHERE {column} IS NOT NULL"
            ))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (entry, job, raw) in rows {
            let value: Value = serde_json::from_str(&raw).map_err(|e| invalid(&e.to_string()))?;
            if let Some(map) = value.as_object() {
                let mut fields = Map::new();
                for (name, value) in map {
                    let field = if value.is_object() || value.is_null() {
                        value.clone()
                    } else {
                        json!({"type":if name=="assetDate"{"TIMESTAMP"}else if value.is_string(){"STRING"}else{"INT64"},"value":if let Some(v)=value.as_bool(){json!(v as i64)}else{value.clone()}})
                    };
                    fields.insert(name.clone(), field);
                }
                let record = NativeRecord {
                    record_name: format!("local-state-{entry}"),
                    record_type: None,
                    record_change_tag: None,
                    zone_id: None,
                    deleted: false,
                    fields: Some(fields),
                    events: None,
                };
                save(
                    conn,
                    kind,
                    &entry.to_string(),
                    job,
                    Some(entry),
                    0,
                    None,
                    &record,
                )?;
            }
        }
    }
    for column in LEGACY_ENTRY_COLUMNS {
        conn.execute(
            &format!("ALTER TABLE sync_entries DROP COLUMN {column}"),
            [],
        )?;
    }

    Ok(())
}

pub(super) fn read_events(conn: &Connection, entry: i64) -> Result<EditEvents> {
    conn.query_row("SELECT native_metadata_edited_ms,native_metadata_source,native_metadata_event_id,native_content_edited_ms,native_content_source,native_content_event_id,native_deletion_edited_ms,native_deletion_source,native_deletion_event_id FROM sync_entries WHERE id=?1",[entry],|r|{
        let read=|i|->rusqlite::Result<Option<EditEvent>>{r.get::<_,Option<i64>>(i)?.map(|edited_at_ms|Ok::<_,rusqlite::Error>(EditEvent{edited_at_ms,source:r.get(i+1)?,event_id:r.get(i+2)?})).transpose()};
        Ok(EditEvents{metadata:read(0)?,content:read(3)?,deletion:read(6)?})
    }).map_err(Into::into)
}
pub(super) fn set_event(
    conn: &Connection,
    entry: i64,
    category: &str,
    event: Option<&EditEvent>,
) -> Result<()> {
    if !matches!(category, "metadata" | "content" | "deletion") {
        return Err(invalid("invalid native event category"));
    }
    conn.execute(&format!("UPDATE sync_entries SET native_{category}_edited_ms=?2,native_{category}_source=?3,native_{category}_event_id=?4 WHERE id=?1"),params![entry,event.map(|e|e.edited_at_ms),event.map(|e|e.source.as_str()),event.map(|e|e.event_id.as_str())])?;
    Ok(())
}
pub(super) fn set_event_document(
    conn: &Connection,
    entry: i64,
    category: &str,
    document: Option<&str>,
) -> Result<()> {
    let event: Option<EditEvent> = document
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| invalid(&e.to_string()))?;
    set_event(conn, entry, category, event.as_ref())
}
pub(super) fn digest_parts(hex: String, size: i64) -> Option<crate::core::sync::ContentDigest> {
    if hex.len() != 64 || !hex.is_ascii() || size < 0 {
        return None;
    }
    let bytes = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    Some(crate::core::sync::ContentDigest {
        hex,
        base64: crate::core::sync::content::to_base64(&bytes),
        size: size as u64,
    })
}

pub(super) fn set_digest(
    conn: &Connection,
    entry: i64,
    digest: Option<&super::receive::LocalDigest>,
) -> Result<()> {
    conn.execute("UPDATE sync_entries SET native_digest_entry_id=?2,native_digest_version=?3,native_digest_sha256=?4,native_digest_size=?5 WHERE id=?1",params![entry,digest.map(|d|d.entry_id),digest.map(|d|d.local_version),digest.map(|d|d.digest.hex.as_str()),digest.map(|d|d.digest.size as i64)])?;
    Ok(())
}

/// Preserve wire key presence for idempotent issued requests. Receiving DTOs
/// have defaults; issued requests must retain absent versus explicit null.
pub(crate) fn save_value(
    conn: &Connection,
    kind: &str,
    owner: &str,
    job: i64,
    entry: Option<i64>,
    position: i64,
    operation: Option<&str>,
    value: &Value,
) -> Result<()> {
    let record: NativeRecord =
        serde_json::from_value(value.clone()).map_err(|e| invalid(&e.to_string()))?;
    let id = save(conn, kind, owner, job, entry, position, operation, &record)?;
    let shape = [
        "recordName",
        "recordType",
        "recordChangeTag",
        "zoneID",
        "fields",
        "_syncserver",
        "deleted",
    ]
    .iter()
    .enumerate()
    .fold(0i64, |bits, (i, key)| {
        bits | if value.get(*key).is_some() { 1 << i } else { 0 }
    });
    let events = ["metadataEvent", "contentEvent", "deletionEvent"]
        .iter()
        .enumerate()
        .fold(0i64, |bits, (i, key)| {
            bits | if value["_syncserver"].get(*key).is_some() {
                1 << i
            } else {
                0
            }
        });
    conn.execute(
        "UPDATE sync_records SET record_shape=?2,event_shape=?3 WHERE id=?1",
        params![id, shape, events],
    )?;
    Ok(())
}
pub(crate) fn load_value(conn: &Connection, id: i64) -> Result<Value> {
    let record = load_id(conn, id)?;
    let mut value = serde_json::to_value(&record).map_err(|e| invalid(&e.to_string()))?;
    let (shape, events): (i64, i64) = conn.query_row(
        "SELECT record_shape,event_shape FROM sync_records WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    for (i, key) in [
        "recordName",
        "recordType",
        "recordChangeTag",
        "zoneID",
        "fields",
        "_syncserver",
        "deleted",
    ]
    .into_iter()
    .enumerate()
    {
        if shape & (1 << i) == 0 {
            value.as_object_mut().unwrap().remove(key);
        } else if value.get(key).is_none() {
            value[key] = Value::Null;
        }
    }
    if let Some(map) = value.get_mut("_syncserver").and_then(Value::as_object_mut) {
        for (i, key) in ["metadataEvent", "contentEvent", "deletionEvent"]
            .into_iter()
            .enumerate()
        {
            if events & (1 << i) == 0 {
                map.remove(key);
            }
        }
    }
    Ok(value)
}
