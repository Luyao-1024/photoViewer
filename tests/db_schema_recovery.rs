//! Recovery is limited to one incompatible table; other schemas and user data survive.
use photo_viewer::core::db;
use photo_viewer::core::sync::native::protocol::{NativeNamespace, NativeZone};
use photo_viewer::core::sync::native::receive::SUPPORTED_RECORD_TYPES;
use photo_viewer::core::sync::store::SyncStore;
use rusqlite::Connection;
use std::path::Path;

const SCHEMA: &str = include_str!("../src/core/schema.sql");

fn seed(path: &Path, schema: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(schema).unwrap();
    conn.execute_batch(
        "PRAGMA user_version=1;
         INSERT INTO settings VALUES('native_sync_account','saved-account');
         INSERT INTO media_items(id,uri,path,folder_path,mime_type,file_mtime,file_size,blake3_hash,indexed_at,is_favorite)
         VALUES(1,'file:///photos/a.jpg','/photos/a.jpg','/photos','image/jpeg',1,10,'hash',1,1);
         INSERT INTO edits(media_id,edit_type,params,created_at) VALUES(1,'rotate','{}',1);
         INSERT INTO albums VALUES('/photos','Photos','file:///photos/a.jpg',1,1);
         INSERT INTO album_order VALUES('/photos',3);
         INSERT INTO album_covers VALUES('/photos','file:///photos/a.jpg');
         INSERT INTO sync_connections(id,endpoint,credential_ref) VALUES(1,'https://sync.example.invalid/','saved-secret-ref');
         INSERT INTO sync_jobs(id,connection_id,local_root,native_zone_name,native_cycle_id,last_started_at,last_completed_at,last_error)
         VALUES(1,1,'/photos','PrimarySync','old-cycle',10,20,'old-error');
         INSERT INTO sync_jobs(id,connection_id,local_root,native_zone_name,paused)
         VALUES(2,1,'/paused','PausedSync',1);
         INSERT INTO sync_job_upload_albums VALUES(1,'Camera');
         INSERT INTO sync_job_changes VALUES(1,'{}');
         INSERT INTO sync_entries(id,job_id,media_id,relative_path,native_dirty,native_photo_id)
         VALUES(1,1,1,'Camera/a.jpg',4,'old-photo');
         INSERT INTO sync_inventory VALUES(1,'Camera/a.jpg',1);
         INSERT INTO sync_tasks(operation_id,job_id,entry_id,action,state,config_generation,entry_generation)
         VALUES('old-op',1,1,'upload','issued',1,1);
         INSERT INTO sync_conflicts(job_id,entry_id,kind) VALUES(1,1,'content');
         INSERT INTO sync_publications(publication_id,job_id,entry_id) VALUES('old-publish',1,1);
         INSERT INTO sync_native_progress(job_id,zone_name,owner,tenant,container,environment,database,database_generation,scope_digest,snapshot_token,ingest_token,applied_token,target_token,discovery_complete)
         VALUES(1,'PrimarySync','_defaultOwner','t','c','e','d','g','scope','old-snapshot','old-ingest','old-applied','old-target',1);
         INSERT INTO sync_native_inbox(job_id,generation,zone_name,owner,ordinal,record_name,record_type,record_change_tag,kind)
         VALUES(1,'g','PrimarySync','_defaultOwner',1,'old-photo','CPLAsset','rev','created');
         INSERT INTO sync_native_records(job_id,generation,zone_name,owner,record_name,record_type,payload)
         VALUES(1,'g','PrimarySync','_defaultOwner','old-photo','CPLAsset','{}');
         INSERT INTO sync_native_resources(job_id,generation,asset_id,role) VALUES(1,'g','old-asset','original');
         CREATE INDEX retained_progress_index ON sync_native_progress(updated_at);",
    ).unwrap();
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn missing_sync_column_rebuilds_only_entries_and_restarts_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    seed(
        &path,
        &SCHEMA.replace("    native_upload_error    TEXT,\n", ""),
    );
    let photo = dir.path().join("original.jpg");
    std::fs::write(&photo, b"original bytes").unwrap();

    let pool = db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();
    conn.prepare("SELECT native_upload_error FROM sync_entries")
        .unwrap();
    for table in [
        "sync_entries",
        "sync_inventory",
        "sync_tasks",
        "sync_conflicts",
        "sync_publications",
        "sync_native_progress",
        "sync_native_inbox",
        "sync_native_records",
        "sync_native_resources",
    ] {
        assert_eq!(count(&conn, table), 0, "stale sync state in {table}");
    }
    for table in [
        "media_items",
        "edits",
        "albums",
        "album_order",
        "album_covers",
        "settings",
        "sync_connections",
        "sync_job_upload_albums",
        "sync_job_changes",
    ] {
        assert_eq!(count(&conn, table), 1, "lost unrelated data in {table}");
    }
    assert_eq!(count(&conn, "sync_jobs"), 2);
    assert_eq!(
        conn.query_row("SELECT is_favorite FROM media_items", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let job: (String, String, i64, Option<String>, Option<i64>, Option<i64>, Option<String>) = conn.query_row(
        "SELECT local_root,upload_scope,paused,native_cycle_id,last_started_at,last_completed_at,last_error FROM sync_jobs WHERE id=1", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
    ).unwrap();
    assert_eq!(
        job,
        (
            "/photos".into(),
            "selected_albums".into(),
            0,
            None,
            None,
            None,
            None
        )
    );
    assert_eq!(
        conn.query_row("SELECT paused FROM sync_jobs WHERE id=2", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT credential_ref FROM sync_connections", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "saved-secret-ref"
    );
    assert_eq!(
        conn.query_row(
            "SELECT relative_album FROM sync_job_upload_albums",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "Camera"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='retained_progress_index'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "other sync tables must not be rebuilt"
    );
    assert_eq!(
        conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(!conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .exists([])
        .unwrap());
    assert_eq!(std::fs::read(&photo).unwrap(), b"original bytes");
    drop(conn);

    let store = SyncStore::new(pool);
    let progress = store
        .begin_native_receive(
            1,
            NativeNamespace {
                tenant: "t".into(),
                container: "c".into(),
                environment: "e".into(),
                database: "d".into(),
            },
            "g",
            "scope",
            NativeZone {
                zone_name: "PrimarySync".into(),
                owner: "_defaultOwner".into(),
            },
        )
        .unwrap();
    assert!(progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));
    assert!(progress.snapshot_token.is_none());
    assert!(progress.ingest_token.is_none());
    assert!(progress.applied_token.is_none());
    assert!(progress.target_token.is_none());
    assert!(!progress.discovery_complete);
}

#[test]
fn indexed_column_mismatch_is_detected_before_creating_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE sync_entries(id INTEGER PRIMARY KEY,job_id INTEGER,relative_path TEXT)",
    )
    .unwrap();
    drop(conn);
    let pool = db::init_pool(&path).unwrap();
    pool.get()
        .unwrap()
        .prepare("SELECT native_dirty,native_upload_error FROM sync_entries")
        .unwrap();
}

#[test]
fn compatible_schema_keeps_all_data_and_sync_progress() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    seed(&path, SCHEMA);
    let pool = db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(count(&conn, "sync_entries"), 1);
    assert_eq!(count(&conn, "edits"), 1);
    assert_eq!(
        conn.query_row("SELECT applied_token FROM sync_native_progress", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "old-applied"
    );
    assert_eq!(
        conn.query_row(
            "SELECT last_completed_at FROM sync_jobs WHERE id=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        20
    );
}

#[test]
fn changed_type_extra_column_and_changed_default_each_rebuild_one_table() {
    for schema in [
        SCHEMA.replace(
            "native_upload_error    TEXT",
            "native_upload_error    INTEGER",
        ),
        SCHEMA.replace(
            "native_upload_error    TEXT,",
            "native_upload_error    TEXT, obsolete_column TEXT,",
        ),
        SCHEMA.replace(
            "native_dirty           INTEGER NOT NULL DEFAULT 0",
            "native_dirty           INTEGER NOT NULL DEFAULT 1",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photos.db");
        seed(&path, &schema);
        let pool = db::init_pool(&path).unwrap();
        let conn = pool.get().unwrap();
        assert_eq!(count(&conn, "sync_entries"), 0);
        assert_eq!(count(&conn, "edits"), 1);
        assert_eq!(conn.query_row("SELECT type FROM pragma_table_info('sync_entries') WHERE name='native_upload_error'", [], |r| r.get::<_, String>(0)).unwrap(), "TEXT");
    }
}

#[test]
fn multiple_incompatible_tables_are_reported_without_deleting_anything() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    seed(
        &path,
        &SCHEMA
            .replace("    native_upload_error    TEXT,\n", "")
            .replace(
                "    thumbnail_generated_at INTEGER",
                "    old_thumbnail_column INTEGER",
            ),
    );
    let error = match db::init_pool(&path) {
        Ok(_) => panic!("must refuse to rebuild multiple tables"),
        Err(e) => e.to_string(),
    };
    assert!(
        error.contains("sync_entries") && error.contains("media_items"),
        "{error}"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(count(&conn, "sync_entries"), 1);
    assert_eq!(count(&conn, "sync_native_progress"), 1);
    assert_eq!(count(&conn, "media_items"), 1);
}

#[test]
fn rebuilding_a_referenced_library_table_cannot_cascade_into_user_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    seed(
        &path,
        &SCHEMA.replace(
            "    thumbnail_generated_at INTEGER",
            "    old_thumbnail_column INTEGER",
        ),
    );
    assert!(
        db::init_pool(&path).is_err(),
        "must preserve dependent user data"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(count(&conn, "edits"), 1);
    assert_eq!(count(&conn, "media_items"), 1);
    assert_eq!(count(&conn, "sync_entries"), 1);
}

#[test]
fn non_schema_errors_do_not_delete_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    let bytes = b"not a SQLite database";
    std::fs::write(&path, bytes).unwrap();
    assert!(db::init_pool(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn a_failure_after_drop_rolls_back_the_table_and_all_sync_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    seed(
        &path,
        &SCHEMA.replace("    native_upload_error    TEXT,\n", ""),
    );
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_recovery BEFORE UPDATE ON sync_jobs BEGIN SELECT RAISE(ABORT,'injected recovery failure'); END;").unwrap();
    drop(conn);

    let error = match db::init_pool(&path) {
        Ok(_) => panic!("injected failure must abort recovery"),
        Err(e) => e.to_string(),
    };
    assert!(error.contains("injected recovery failure"), "{error}");
    let conn = Connection::open(&path).unwrap();
    assert!(
        conn.prepare("SELECT native_upload_error FROM sync_entries")
            .is_err(),
        "the original table must be restored"
    );
    assert_eq!(count(&conn, "sync_entries"), 1);
    assert_eq!(count(&conn, "sync_tasks"), 1);
    assert_eq!(count(&conn, "sync_native_progress"), 1);
    assert_eq!(count(&conn, "edits"), 1);
}

#[test]
fn one_unreferenced_table_can_be_rebuilt_without_losing_other_library_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    seed(&path, SCHEMA);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("ALTER TABLE album_order ADD COLUMN obsolete TEXT;")
        .unwrap();
    drop(conn);
    let pool = db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(count(&conn, "album_order"), 0);
    for table in [
        "media_items",
        "edits",
        "albums",
        "album_covers",
        "sync_connections",
        "sync_job_upload_albums",
    ] {
        assert_eq!(count(&conn, table), 1, "lost unrelated data in {table}");
    }
    assert_eq!(count(&conn, "sync_jobs"), 2);
    assert_eq!(count(&conn, "sync_native_progress"), 0);
}
