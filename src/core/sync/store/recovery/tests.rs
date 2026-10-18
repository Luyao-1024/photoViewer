use super::*;
#[test]
fn publication_capture_and_file_stamps_survive_retiring_the_media_entry() {
    let root = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&root.path().join("photos.db")).unwrap();
    let conn = pool.get().unwrap();
    conn.execute_batch("INSERT INTO sync_connections(id,endpoint,credential_ref) VALUES(1,'https://sync.example.invalid/','ref'); INSERT INTO sync_jobs(id,connection_id,local_root,native_zone_name) VALUES(1,1,'/photos','PrimarySync'); INSERT INTO sync_entries(id,job_id,relative_path) VALUES(1,1,'a.jpg'); INSERT INTO sync_publications(publication_id,job_id,entry_id,photo_id,state) VALUES('publication',1,1,'photo','complete');").unwrap();
    let digest = crate::core::sync::ContentDigest::of(b"");
    let asset = crate::core::sync::AssetDescriptor {
        asset_id: "asset".into(),
        size: 0,
        file_checksum: digest.base64,
        mime_type: "image/jpeg".into(),
        width: None,
        height: None,
    };
    let photo = RemotePhoto {
        photo_id: "photo".into(),
        master_id: "master".into(),
        content_generation: "content".into(),
        relative_path: "a.jpg".into(),
        is_favorite: false,
        asset_date_ms: None,
        is_hidden: false,
        original: asset.clone(),
        thumbnail: asset,
        thumbnail_source_sha256: digest.hex.clone(),
        events: Default::default(),
    };
    set_capture(&conn,"publication",&json!({"job":1,"entry":1,"local":0,"config":1,"dirty":0,"projection":serde_json::to_string(&photo).unwrap()})).unwrap();
    set_details(&conn,"publication",Some(&json!({"target":"/photos/a.jpg","new":"/photos/.new","backup":"/photos/.backup","root":"/photos","hadOld":true,"oldStamp":[8,123456789],"oldLocalStamp":[8,123456789],"sha256":digest.hex,"size":0}))).unwrap();
    conn.execute("DELETE FROM sync_entries WHERE id=1", [])
        .unwrap();
    let restored = capture(&conn, "publication").unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<RemotePhoto>(restored["projection"].as_str().unwrap()).unwrap(),
        photo
    );
    let restored = details(&conn, "publication").unwrap().unwrap();
    assert_eq!(restored["oldStamp"], json!([8, 123456789]));
    assert_eq!(restored["oldLocalStamp"], json!([8, 123456789]));
    set_details(&conn, "publication", None).unwrap();
    assert!(details(&conn, "publication").unwrap().is_none());
    conn.execute("DELETE FROM sync_publications", []).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_records", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
