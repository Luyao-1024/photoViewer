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
    assert_eq!(version, 1);
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
