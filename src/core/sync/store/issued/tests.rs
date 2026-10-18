use super::*;
fn operation() -> Issued {
    let asset = json!({"assetId":"asset","fileChecksum":"digest","size":8,"mimeType":"image/jpeg","width":4});
    Issued {
        id: "pv-relational-operation".into(),
        job: 1,
        entry: 1,
        config: 3,
        generation: "generation".into(),
        phase: "create".into(),
        versions: [4, 2, 3, 0],
        dirty: 1,
        photo: "photo".into(),
        master: "master".into(),
        content_generation: "content".into(),
        body: json!({"zoneID":{"zoneName":"PrimarySync","ownerRecordName":"_defaultOwner"},"atomic":true,"operations":[{"operationType":"create","record":{"recordName":"master","recordType":"CPLMaster","fields":{"resOriginalRes":{"type":"ASSETID","value":asset}}}},{"operationType":"create","record":{"recordName":"photo","recordType":"CPLAsset","fields":{"relativePath":{"type":"STRING","value":"相册/图 %.jpg"},"assetDate":null},"_syncserver":{}}}]}),
        preparation: json!({"sendMetadata":true,"metadata":{"relativePath":{"type":"STRING","value":"相册/图 %.jpg"},"assetDate":null},"events":{"metadataEvent":null,"contentEvent":null,"deletionEvent":null},"assetRevision":null,"masterRevision":null,"resources":{"original":{"path":"/snapshots/original","hex":"original-hash","base64":"original-digest","size":8,"session":"session","asset":asset},"thumbnail":{"path":"/snapshots/thumbnail","hex":"thumb-hash","base64":"thumb-digest","size":3,"asset":null}}}),
    }
}
fn legacy_database(path: &std::path::Path, issued: &Issued) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_sync_legacy.sql"
    )))
    .unwrap();
    conn.execute_batch("INSERT INTO sync_connections(id,endpoint,credential_ref) VALUES(1,'https://sync.example.invalid/','ref'); INSERT INTO sync_jobs(id,connection_id,local_root,native_zone_name) VALUES(1,1,'/photos','PrimarySync'); INSERT INTO sync_entries(id,job_id,relative_path) VALUES(1,1,'photo.jpg');").unwrap();
    conn.execute("INSERT INTO sync_tasks(operation_id,job_id,entry_id,action,state,config_generation,entry_generation,native_capture) VALUES(?1,1,1,'create','issued',3,4,?2)",params![issued.id,serde_json::to_string(issued).unwrap()]).unwrap();
}
#[test]
fn legacy_issued_request_keeps_exact_body_preparation_and_unknown_result_evidence() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("photos.db");
    let issued = operation();
    legacy_database(&path, &issued);
    let pool = crate::core::db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(read(&conn, &issued.id).unwrap(), Some(issued.clone()));
    assert_eq!(
        conn.query_row("SELECT state FROM sync_tasks", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "issued"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_task_resources", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(!conn
        .prepare("PRAGMA table_info(sync_tasks)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|row| row.unwrap())
        .any(|c| c == "native_capture"));
}
#[test]
fn unsupported_legacy_capture_rolls_back_the_entire_schema_conversion() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("photos.db");
    let mut issued = operation();
    issued.preparation["future-unrepresented-field"] = json!(true);
    legacy_database(&path, &issued);
    assert!(crate::core::db::init_pool(&path).is_err());
    let conn = rusqlite::Connection::open(&path).unwrap();
    let raw: String = conn
        .query_row("SELECT native_capture FROM sync_tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(serde_json::from_str::<Issued>(&raw).unwrap(), issued);
    assert!(!conn
        .prepare("PRAGMA table_info(sync_tasks)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|row| row.unwrap())
        .any(|c| c == "capture_present"));
}
