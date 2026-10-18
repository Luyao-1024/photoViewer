use super::*;
fn connection() -> (tempfile::TempDir, crate::core::db::DbPool) {
    let root = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&root.path().join("photos.db")).unwrap();
    pool.get().unwrap().execute_batch("INSERT INTO sync_connections(id,endpoint,credential_ref) VALUES(1,'https://sync.example.invalid/','ref'); INSERT INTO sync_jobs(id,connection_id,local_root,native_zone_name) VALUES(1,1,'/photos','PrimarySync'); INSERT INTO sync_entries(id,job_id,relative_path) VALUES(1,1,'Camera/a.jpg');").unwrap();
    (root, pool)
}
#[test]
fn relational_record_round_trips_fields_and_events_and_indexes_identity() {
    let (_root, pool) = connection();
    let conn = pool.get().unwrap();
    let record:NativeRecord=serde_json::from_value(json!({"recordName":"photo","recordType":"CPLAsset","recordChangeTag":"revision","fields":{
        "relativePath":{"type":"STRING","value":"Camera/中文 % #.jpg"},
        "isFavorite":{"type":"INT64","value":1},
        "masterRef":{"type":"REFERENCE","value":{"recordName":"master","action":"NONE"}},
        "assetDate":{"type":"TIMESTAMP","value":null},
        "resOriginalRes":{"type":"ASSETID","value":{"assetId":"original","size":8,"fileChecksum":"digest","mimeType":"image/jpeg","width":4,"height":2}}
    },"_syncserver":{"metadataEvent":{"editedAtMs":123,"source":"device","eventId":"event"}}})).unwrap();
    save(&conn, "pending", "1", 1, Some(1), 1, None, &record).unwrap();
    assert_eq!(load(&conn, "pending", "1").unwrap(), vec![record]);
    let mut s=conn.prepare("EXPLAIN QUERY PLAN SELECT record_revision FROM sync_records WHERE job_id=?1 AND owner_kind='pending' AND record_name=?2").unwrap();
    let plan = s
        .query_map(params![1, "photo"], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join(" ");
    assert!(plan.contains("idx_sync_records_identity"), "{plan}");
    let definition: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='sync_record_fields'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!definition.contains("json_"));
    clear(&conn, "pending", "1").unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_record_fields", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn cloud_state_tables_have_only_typed_columns_and_index_both_record_ids() {
    let (_root, pool) = connection();
    let conn = pool.get().unwrap();
    let definitions = conn
        .prepare("SELECT name,sql FROM sqlite_schema WHERE type='table' AND name LIKE 'sync_%'")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for (name, sql) in definitions {
        assert!(
            !sql.contains("json_valid"),
            "{name} still stores a JSON document"
        );
        let columns = conn
            .prepare(&format!("PRAGMA table_info({name})"))
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for (column, kind) in columns {
            assert_ne!(kind, "BLOB", "opaque {name}.{column}");
            assert!(
                ![
                    "payload",
                    "native_capture",
                    "native_detail",
                    "captured_versions",
                    "role_state",
                    "old_snapshot",
                    "native_pinned_scope",
                    "native_pending_master",
                    "native_pending_asset",
                    "native_resource_baseline"
                ]
                .contains(&column.as_str()),
                "document column {name}.{column}"
            );
        }
    }
    for (column, index) in [
        ("native_photo_id", "idx_sync_entries_native_photo"),
        ("native_master_id", "idx_sync_entries_native_master"),
    ] {
        let plan = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT id FROM sync_entries WHERE job_id=?1 AND {column}=?2"
            ))
            .unwrap()
            .query_map(params![1, "photo"], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" ");
        assert!(plan.contains(index), "{plan}");
    }
}

#[test]
fn first_asset_exposes_its_pending_master_without_a_confirmed_baseline() {
    let (_root, pool) = connection();
    let conn = pool.get().unwrap();
    let master=serde_json::from_value(json!({"recordName":"master","recordType":"CPLMaster","recordChangeTag":"m1","fields":{"photoId":{"type":"STRING","value":"photo"}}})).unwrap();
    let asset=serde_json::from_value(json!({"recordName":"photo","recordType":"CPLAsset","recordChangeTag":"a1","fields":{"masterRef":{"type":"REFERENCE","value":{"recordName":"master","action":"NONE"}}}})).unwrap();
    set_pending(&conn, 1, true, Some(&master)).unwrap();
    set_pending(&conn, 1, false, Some(&asset)).unwrap();
    drop(conn);
    let store = super::super::SyncStore::new(pool);
    let records = store.pending_records(1).unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|r| r.record_name == "master"));
    assert!(records.iter().any(|r| r.record_name == "photo"));
}

#[test]
fn global_off_clears_relational_receive_and_recovery_state_but_keeps_jobs() {
    let (_root, pool) = connection();
    let conn = pool.get().unwrap();
    conn.execute_batch("INSERT INTO sync_native_progress(job_id,zone_name,owner,tenant,container,environment,database,database_generation,scope_digest) VALUES(1,'PrimarySync','_defaultOwner','t','c','e','d','g','s'); INSERT INTO sync_bootstrap_types(job_id,record_type,complete) VALUES(1,'CPLMaster',1); INSERT INTO sync_native_resources(job_id,generation,asset_id,role) VALUES(1,'g','asset','original'); INSERT INTO sync_publications(publication_id,job_id,entry_id,state) VALUES('publication',1,1,'complete');").unwrap();
    let record=serde_json::from_value(json!({"recordName":"photo","recordType":"CPLAsset","recordChangeTag":"revision","fields":{}})).unwrap();
    set_pending(&conn, 1, false, Some(&record)).unwrap();
    drop(conn);
    let store = super::super::SyncStore::new(pool.clone());
    store.begin_reset().unwrap();
    let intent = store.reset_intent().unwrap().unwrap();
    store
        .fail_reset(&intent.token, "retryable cleanup")
        .unwrap();
    assert_eq!(
        store.reset_error().unwrap().as_deref(),
        Some("retryable cleanup")
    );
    assert!(store.finish_reset(&intent.token).unwrap());
    assert!(!store.reset_pending().unwrap());
    let conn = pool.get().unwrap();
    for table in [
        "sync_entries",
        "sync_records",
        "sync_record_fields",
        "sync_native_progress",
        "sync_bootstrap_types",
        "sync_native_resources",
        "sync_publications",
        "sync_reset_state",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_jobs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT credential_ref FROM sync_connections", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "ref"
    );
}
