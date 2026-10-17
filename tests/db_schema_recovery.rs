//! Recovery is limited to one incompatible table; other schemas and user data survive.
use photo_viewer::core::db;
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
         INSERT INTO sync_tasks(operation_id,job_id,entry_id,action,state,config_generation,entry_generation)
         VALUES('old-op',1,1,'upload','issued',1,1);
         INSERT INTO sync_conflicts(job_id,entry_id,kind) VALUES(1,1,'content');
         INSERT INTO sync_publications(publication_id,job_id,entry_id) VALUES('old-publish',1,1);
         INSERT INTO sync_native_progress(job_id,zone_name,owner,tenant,container,environment,database,database_generation,scope_digest,snapshot_token,ingest_token,applied_token,target_token,discovery_complete)
         VALUES(1,'PrimarySync','_defaultOwner','t','c','e','d','g','scope','old-snapshot','old-ingest','old-applied','old-target',1);
         INSERT INTO sync_native_resources(job_id,generation,asset_id,role) VALUES(1,'g','old-asset','original');
         CREATE INDEX retained_progress_index ON sync_native_progress(updated_at);",
    ).unwrap();
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn missing_nullable_field_preserves_library_deletions_and_receive_progress() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    seed(
        &path,
        &SCHEMA.replace("    native_upload_error    TEXT,\n", ""),
    );
    let pool = db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();
    conn.prepare("SELECT native_upload_error FROM sync_entries")
        .unwrap();
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
        "sync_entries",
        "sync_tasks",
        "sync_conflicts",
        "sync_publications",
        "sync_native_progress",
        "sync_native_resources",
    ] {
        assert_eq!(count(&conn, table), 1, "lost data in {table}");
    }
    assert_eq!(count(&conn, "sync_jobs"), 2);
    let (dirty, photo): (i64, String) = conn
        .query_row(
            "SELECT native_dirty,native_photo_id FROM sync_entries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((dirty, photo), (4, "old-photo".into()));
    assert_eq!(
        conn.query_row("SELECT applied_token FROM sync_native_progress", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "old-applied"
    );
    assert_eq!(
        conn.query_row("SELECT last_error FROM sync_jobs WHERE id=1", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "old-error"
    );
    assert!(!conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .exists([])
        .unwrap());
}

#[test]
fn presence_fields_and_obsolete_mirror_cleanup_keep_pending_deletion_and_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photos.db");
    let old=SCHEMA.replace("    storage_location INTEGER NOT NULL DEFAULT 1 CHECK (storage_location IN (1,2,3)),\n","")
        .replace("    thumbnail_ready INTEGER NOT NULL DEFAULT 0 CHECK (thumbnail_ready IN (0,1))\n","")
        .replace("    thumbnail_generated_at INTEGER,\n","    thumbnail_generated_at INTEGER\n")
        .replace("    native_pending_master TEXT CHECK (native_pending_master IS NULL OR json_valid(native_pending_master)),\n","")
        .replace("    native_pending_asset TEXT CHECK (native_pending_asset IS NULL OR json_valid(native_pending_asset)),\n","");
    seed(&path, &old);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE sync_native_inbox(value TEXT); INSERT INTO sync_native_inbox VALUES('obsolete'); CREATE TABLE sync_native_records(value TEXT); CREATE TABLE sync_inventory(value TEXT);").unwrap();
    drop(conn);
    let pool = db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();
    let state:(i64,String,i64,bool)=conn.query_row("SELECT e.native_dirty,e.native_photo_id,m.storage_location,m.thumbnail_ready FROM sync_entries e JOIN media_items m ON m.id=e.media_id",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(state, (4, "old-photo".into(), 1, false));
    for table in ["sync_entries", "edits", "sync_tasks", "sync_publications"] {
        assert_eq!(count(&conn, table), 1);
    }
    assert!(conn
        .query_row("SELECT applied_token FROM sync_native_progress", [], |r| {
            r.get::<_, Option<String>>(0)
        })
        .unwrap()
        .is_none());
    assert!(!conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name IN ('sync_native_inbox','sync_native_records','sync_inventory'))",[],|r|r.get::<_,bool>(0)).unwrap());
    drop(conn);
    drop(pool);
    let reopened = db::init_pool(&path).unwrap();
    assert_eq!(
        reopened
            .get()
            .unwrap()
            .query_row("SELECT native_dirty FROM sync_entries", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
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
            .replace(
                "    native_upload_error    TEXT,",
                "    native_upload_error    INTEGER,",
            )
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
        &SCHEMA.replace(
            "native_upload_error    TEXT",
            "native_upload_error    INTEGER",
        ),
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
        conn.query_row(
            "SELECT type FROM pragma_table_info('sync_entries') WHERE name='native_upload_error'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap()
            == "INTEGER",
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
