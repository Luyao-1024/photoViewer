mod common;
use chrono::Utc;
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn schema_creates_all_tables() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();

    // 验证所有表存在
    let conn = pool.get().unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    assert!(tables.contains(&"media_items".to_string()));
    assert!(tables.contains(&"albums".to_string()));
    assert!(tables.contains(&"edits".to_string()));
    assert!(tables.contains(&"settings".to_string()));
    assert!(tables.contains(&"sync_connections".to_string()));
    assert!(tables.contains(&"sync_jobs".to_string()));
    assert!(tables.contains(&"sync_job_upload_albums".to_string()));
    assert!(tables.contains(&"sync_entries".to_string()));
    assert!(tables.contains(&"sync_inventory".to_string()));
    assert!(tables.contains(&"sync_tasks".to_string()));
    assert!(tables.contains(&"sync_conflicts".to_string()));
    assert!(tables.contains(&"sync_job_changes".to_string()));
    assert!(tables.contains(&"sync_native_progress".to_string()));
    assert!(tables.contains(&"sync_native_inbox".to_string()));
    assert!(tables.contains(&"sync_native_records".to_string()));
    assert!(tables.contains(&"sync_native_resources".to_string()));
    assert!(tables.contains(&"sync_publications".to_string()));
}

#[test]
fn media_items_has_media_kind_column_and_upsert_populates_it() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();

    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(media_items)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(
        columns.iter().any(|c| c == "media_kind"),
        "media_items should persist image/video type separately from MIME, got {columns:?}"
    );

    drop(conn);
    common::db::insert_media_item(
        &pool,
        &NewMediaItem {
            uri: "file:///tmp/clip.mp4".into(),
            path: "/tmp/clip.mp4".into(),
            folder_path: "/tmp".into(),
            mime_type: "video/mp4".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: Some(12.5),
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 10,
            blake3_hash: "hash-video".into(),
        },
    )
    .unwrap();

    let conn = pool.get().unwrap();
    let media_kind: String = conn
        .query_row(
            "SELECT media_kind FROM media_items WHERE uri = 'file:///tmp/clip.mp4'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(media_kind, "video");
}

#[test]
fn media_items_has_subkind_and_attributes_columns() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();

    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(media_items)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    assert!(
        columns.iter().any(|c| c == "media_subkind"),
        "media_items should persist secondary media classification"
    );
    assert!(
        columns.iter().any(|c| c == "media_attributes"),
        "media_items should persist extensible media attributes"
    );
}

#[test]
fn media_items_has_video_duration_column() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();

    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(media_items)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    assert!(
        columns.iter().any(|c| c == "video_duration_secs"),
        "media_items should persist video duration; got {columns:?}"
    );
}

#[test]
fn live_media_page_query_uses_sort_index_without_temp_btree() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();

    let plan: Vec<String> = conn
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at
             FROM media_items
             WHERE trashed_at IS NULL
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT 500 OFFSET 50000",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    let plan_text = plan.join("\n");

    assert!(
        plan_text.contains("idx_media_live_sort"),
        "live media paging should use the expression sort index; plan:\n{plan_text}"
    );
    assert!(
        !plan_text.contains("USE TEMP B-TREE"),
        "live media paging must not sort large libraries into a temp B-tree; plan:\n{plan_text}"
    );
}

#[test]
fn album_media_queries_use_filtered_sort_indexes_without_temp_btree() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();

    let cases = [
        (
            "folder album",
            "idx_media_folder_sort",
            "EXPLAIN QUERY PLAN
             SELECT id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at
             FROM media_items
             WHERE trashed_at IS NULL AND folder_path = '/tmp/Camera'
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT 500 OFFSET 1000",
        ),
        (
            "favorites album",
            "idx_media_favorite_sort",
            "EXPLAIN QUERY PLAN
             SELECT id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at
             FROM media_items
             WHERE trashed_at IS NULL AND is_favorite = 1
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT 500 OFFSET 1000",
        ),
        (
            "media kind album",
            "idx_media_kind_sort",
            "EXPLAIN QUERY PLAN
             SELECT id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at
             FROM media_items
             WHERE trashed_at IS NULL AND media_kind = 'image'
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT 500 OFFSET 1000",
        ),
        (
            "media subkind album",
            "idx_media_subkind_sort",
            "EXPLAIN QUERY PLAN
             SELECT id, uri, path, folder_path, mime_type, media_subkind,
                    media_attributes, width, height, video_duration_secs, taken_at,
                    file_mtime, file_size, blake3_hash, is_favorite, trashed_at
             FROM media_items
             WHERE trashed_at IS NULL AND media_subkind = 'motion_photo'
             ORDER BY COALESCE(taken_at, file_mtime) DESC, id DESC
             LIMIT 500 OFFSET 1000",
        ),
    ];

    for (name, expected_index, sql) in cases {
        let plan: Vec<String> = conn
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let plan_text = plan.join("\n");

        assert!(
            plan_text.contains(expected_index),
            "{name} query should use {expected_index}; plan:\n{plan_text}"
        );
        assert!(
            !plan_text.contains("USE TEMP B-TREE"),
            "{name} query must not sort large album results into a temp B-tree; plan:\n{plan_text}"
        );
    }
}

#[test]
fn indexes_created() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();

    let indexes: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    assert!(indexes.iter().any(|n| n.contains("taken_at")));
    assert!(indexes.iter().any(|n| n.contains("folder")));
    assert!(indexes.iter().any(|n| n.contains("trashed")));
}

#[test]
fn connections_wait_for_transient_write_locks() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let conn = pool.get().unwrap();
    let timeout_ms: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();

    assert!(
        timeout_ms >= 5_000,
        "database connections should wait for transient SQLite writer contention"
    );
}

#[test]
fn unversioned_library_migrates_without_losing_user_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE media_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uri TEXT UNIQUE NOT NULL,
                path TEXT NOT NULL,
                folder_path TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                width INTEGER,
                height INTEGER,
                taken_at INTEGER,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                blake3_hash TEXT NOT NULL,
                is_favorite INTEGER NOT NULL DEFAULT 0,
                trashed_at INTEGER,
                indexed_at INTEGER NOT NULL
             );
             CREATE TABLE albums (
                folder_path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                cover_uri TEXT,
                photo_count INTEGER NOT NULL DEFAULT 0,
                last_modified INTEGER NOT NULL
             );
             CREATE TABLE album_order (folder_path TEXT PRIMARY KEY, sort_order INTEGER NOT NULL);
             CREATE TABLE album_covers (folder_path TEXT PRIMARY KEY, cover_uri TEXT NOT NULL);
             INSERT INTO media_items
                (uri, path, folder_path, mime_type, width, height, taken_at,
                 file_mtime, file_size, blake3_hash, is_favorite, trashed_at, indexed_at)
             VALUES
                ('file:///library/a.gif', '/library/a.gif', '/library', 'image/gif',
                 10, 20, NULL, 100, 42, 'hash', 1, NULL, 101);
             INSERT INTO album_order VALUES ('/library', 7);
             INSERT INTO album_covers VALUES ('/library', 'file:///library/a.gif');",
        )
        .unwrap();
    drop(connection);

    let pool = db::init_pool(&path).unwrap();
    let item = db::list_all_media(&pool).unwrap().pop().unwrap();
    assert_eq!(item.uri, "file:///library/a.gif");
    assert!(item.is_favorite);
    assert!(item.is_animated());

    let connection = pool.get().unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 10);
    assert_eq!(
        connection
            .query_row(
                "SELECT sort_order FROM album_order WHERE folder_path = '/library'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        7
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT cover_uri FROM album_covers WHERE folder_path = '/library'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "file:///library/a.gif"
    );
}

#[test]
fn newer_schema_is_rejected_instead_of_being_modified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.db");
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    drop(connection);

    let error = db::init_pool(&path).unwrap_err().to_string();
    assert!(error.contains("requires a newer Photo Viewer"));
}

#[test]
fn version_one_migration_normalizes_legacy_raw_file_uris() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v1.db");
    let media_path = dir.path().join("照片 #100%?.jpg");
    let pool = db::init_pool(&path).unwrap();
    let id = common::db::insert_media_item(
        &pool,
        &NewMediaItem {
            uri: photo_viewer::core::file_uri::from_path(&media_path),
            path: media_path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: String::new(),
        },
    )
    .unwrap();
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE media_items SET uri = ?1 WHERE id = ?2",
            rusqlite::params![format!("file://{}", media_path.display()), id],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    drop(pool);

    let migrated = db::init_pool(&path).unwrap();
    let item = db::get_media_item(&migrated, id).unwrap();
    assert_eq!(
        item.uri,
        photo_viewer::core::file_uri::from_path(&media_path)
    );
    assert_eq!(
        photo_viewer::core::file_uri::to_path(&item.uri).unwrap(),
        media_path
    );
}

/// The sync layer is native-only from schema 9: every WebDAV-era column is
/// gone, and the native columns exist with neutral defaults so a fresh library
/// never mistakes an unprobed entry for a dirtied one.
#[test]
fn schema_nine_creates_native_sync_columns_with_neutral_defaults() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("native-shape.db");
    let pool = db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();

    let entry_columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('sync_entries')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    for column in [
        "native_dirty",
        "native_local_version",
        "native_metadata_version",
        "native_content_version",
        "native_deletion_version",
        "native_acked_metadata_version",
        "native_acked_content_version",
        "native_acked_deletion_version",
        "native_metadata_event",
        "native_content_event",
        "native_deletion_event",
        "native_photo_id",
        "native_master_id",
        "native_asset_revision",
        "native_master_revision",
        "native_confirmed_generation",
        "native_content_generation",
        "native_desired_metadata",
        "native_baseline_metadata",
        "native_resource_baseline",
    ] {
        assert!(
            entry_columns.contains(&column.to_string()),
            "sync_entries must carry {column}"
        );
    }
    // The WebDAV fingerprint pipeline is deleted, not merely unused.
    for gone in [
        "local_fingerprint",
        "remote_fingerprint",
        "baseline_fingerprint",
        "suppressed_fingerprint",
        "remote_revision",
        "state",
        "etag_is_sha256",
        "remote_root",
    ] {
        let job_columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('sync_jobs')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            !entry_columns.contains(&gone.to_string()) && !job_columns.contains(&gone.to_string()),
            "{gone} belonged to the deleted WebDAV engine"
        );
    }

    // A zone is required at the database level, not defaulted.
    conn.execute(
        "INSERT INTO sync_connections(endpoint,credential_ref) VALUES('https://sync.example.invalid/','shape')",
        [],
    )
    .unwrap();
    assert!(conn
        .execute(
            "INSERT INTO sync_jobs(connection_id,local_root) VALUES(1,'/photos')",
            [],
        )
        .is_err());
    conn.execute(
        "INSERT INTO sync_jobs(connection_id,local_root,native_zone_name) VALUES(1,'/photos','PrimarySync')",
        [],
    )
    .unwrap();
    let dirty: i64 = conn
        .query_row("SELECT native_dirty FROM sync_entries LIMIT 0", [], |row| {
            row.get(0)
        })
        .unwrap_or(0);
    assert_eq!(dirty, 0);
}

/// Upgrading a schema-8 library drops the WebDAV sync tables and recreates them
/// empty, while media, albums, favorites and edits survive untouched. Sync
/// history described a transport this build no longer speaks, so carrying it
/// forward would only be dead rows; the user's photographs are the opposite.
#[test]
fn schema_nine_upgrade_discards_webdav_history_and_keeps_the_library() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("upgrade.db");
    {
        let pool = db::init_pool(&path).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO media_items(uri,path,folder_path,mime_type,media_kind,file_mtime,file_size,blake3_hash,indexed_at)
                 VALUES('file:///photos/a.jpg','/photos/a.jpg','/photos','image/jpeg','image',1700000000,1024,'hash',1700000000);
             UPDATE media_items SET is_favorite=1 WHERE path='/photos/a.jpg';
             INSERT INTO albums(folder_path,name,last_modified) VALUES('/photos','Photos',1700000000);",
        )
        .unwrap();
        // Replace the sync tables with their schema-8 shapes, seed real WebDAV
        // rows, and stamp the old version: this is what an upgrading library
        // actually looks like.
        conn.execute_batch(
            r#"
            DROP TABLE sync_publications;
            DROP TABLE sync_native_resources;
            DROP TABLE sync_native_records;
            DROP TABLE sync_native_inbox;
            DROP TABLE sync_native_progress;
            DROP TABLE sync_conflicts;
            DROP TABLE sync_tasks;
            DROP TABLE sync_inventory;
            DROP TABLE sync_job_upload_albums;
            DROP TABLE sync_job_changes;
            DROP TABLE sync_entries;
            DROP TABLE sync_jobs;
            DROP TABLE sync_connections;
            CREATE TABLE sync_connections(id INTEGER PRIMARY KEY AUTOINCREMENT, provider_kind TEXT NOT NULL,
                endpoint TEXT NOT NULL, username TEXT NOT NULL, credential_ref TEXT NOT NULL UNIQUE,
                enabled INTEGER NOT NULL DEFAULT 1);
            CREATE TABLE sync_jobs(id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL,
                local_root TEXT NOT NULL, remote_root TEXT NOT NULL, direction TEXT NOT NULL DEFAULT 'bidirectional',
                etag_is_sha256 INTEGER NOT NULL DEFAULT 0, upload_scope TEXT NOT NULL DEFAULT 'selected_albums',
                propagate_deletes INTEGER NOT NULL DEFAULT 0, paused INTEGER NOT NULL DEFAULT 0,
                config_generation INTEGER NOT NULL DEFAULT 1);
            CREATE TABLE sync_entries(id INTEGER PRIMARY KEY AUTOINCREMENT, job_id INTEGER NOT NULL,
                relative_path TEXT NOT NULL, local_fingerprint TEXT, local_size INTEGER, local_mtime_ns INTEGER,
                remote_fingerprint TEXT, remote_size INTEGER, remote_revision TEXT,
                baseline_fingerprint TEXT, baseline_size INTEGER, state TEXT NOT NULL DEFAULT 'pending');
            INSERT INTO sync_connections(id,provider_kind,endpoint,username,credential_ref)
                VALUES(1,'webdav','https://dav.example.test/','alice','legacy');
            INSERT INTO sync_jobs(id,connection_id,local_root,remote_root)
                VALUES(1,1,'/photos','Photos');
            INSERT INTO sync_entries(job_id,relative_path,baseline_fingerprint,state)
                VALUES(1,'a.jpg','legacy-hash','synced');
            PRAGMA user_version=8;"#,
        )
        .unwrap();
    }
    // Migration must be idempotent: an interrupted upgrade re-runs cleanly.
    for _ in 0..2 {
        let pool = db::init_pool(&path).unwrap();
        let conn = pool.get().unwrap();

        // The library itself is intact.
        assert_eq!(
            conn.query_row(
                "SELECT is_favorite FROM media_items WHERE path='/photos/a.jpg'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        assert!(
            conn.query_row("SELECT COUNT(*) FROM albums", [], |row| row
                .get::<_, i64>(0))
                .unwrap()
                > 0
        );

        // The WebDAV connection, its job and its baseline are gone, and the
        // tables came back in the native shape.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sync_connections", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sync_entries", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        let has_native: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('sync_entries') WHERE name='native_dirty')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_native);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
    }
}

/// Deriving the uplink view filters dirty entries per job; that query must use
/// the covering index rather than sorting the table.
#[test]
fn native_dirty_uplift_query_uses_its_index_without_temp_btree() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("explain.db");
    let pool = db::init_pool(&path).unwrap();
    let conn = pool.get().unwrap();

    let plan: String = conn
        .prepare("EXPLAIN QUERY PLAN SELECT id, native_dirty FROM sync_entries WHERE job_id = 1 AND native_dirty > 0")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .filter_map(Result::ok)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("idx_sync_entries_native_dirty"),
        "the dirty-entry scan must use its index: {plan}"
    );
    assert!(
        !plan.contains("TEMP B-TREE"),
        "a covering index must not need an intermediate sort: {plan}"
    );
}

#[test]
fn schema_ten_preserves_native_schema_nine_dirty_rows() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("native-nine.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    let schema = include_str!("../src/core/schema.sql")
        .lines()
        .filter(|line| !line.contains("native_cycle_id"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    connection.execute_batch(&schema).unwrap();
    connection.execute_batch("INSERT INTO sync_connections(id,endpoint,credential_ref) VALUES(1,'https://sync.example.invalid/','native-reference'); INSERT INTO sync_jobs(id,connection_id,local_root,native_zone_name) VALUES(1,1,'/owned/library','PrimarySync'); INSERT INTO sync_entries(job_id,relative_path,native_dirty,native_local_version,native_metadata_version,native_content_version) VALUES(1,'photo.jpg',7,4,2,3); PRAGMA user_version=9;").unwrap();
    drop(connection);
    let pool = photo_viewer::core::db::init_pool(&path).unwrap();
    let connection = pool.get().unwrap();
    let row:(i64,i64,i64,i64)=connection.query_row("SELECT native_dirty,native_local_version,native_metadata_version,native_content_version FROM sync_entries WHERE job_id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(row, (7, 4, 2, 3));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sync_jobs') WHERE name='native_cycle_id'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
}
