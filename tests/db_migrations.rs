use chrono::Utc;
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn migrations_create_all_tables() {
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
    db::insert_media_item(
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
fn migrations_are_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    // 再次运行迁移不应失败
    db::run_migrations(&pool).unwrap();
    db::run_migrations(&pool).unwrap();
}

#[test]
fn init_pool_regenerates_when_migration_fails() {
    // 模拟旧版本遗留的不兼容数据库：媒体表缺少 `taken_at`，
    // 使新 schema 里 `CREATE INDEX ... ON media_items(taken_at)` 直接报错
    // —— 这就是用户口中的“升级失败”。
    // 应用尚未对外发布，允许通过删库换取自愈。
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE media_items (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                uri         TEXT NOT NULL,
                file_mtime  INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items (uri, file_mtime) VALUES ('legacy', 0)",
            [],
        )
        .unwrap();
    }

    // 即便迁移在第一次会失败，init_pool 应当通过删库重建恢复。
    let pool = db::init_pool(&db_path).expect("init_pool should recover by recreating DB");
    let conn = pool.get().unwrap();

    // 重建后的 media_items 必须包含新增列（这里以 is_favorite 为代表）。
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(media_items)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(
        columns.iter().any(|c| c == "is_favorite"),
        "regenerated media_items missing is_favorite; got: {columns:?}"
    );

    // 重建意味着旧数据被丢弃。
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "legacy row should be wiped after regeneration");
}

#[test]
fn init_pool_regenerates_when_required_column_missing() {
    // 旧库可能有 media_items，但只少了 `video_duration_secs` 这种新版本新增列。
    // 迁移 SQL 自身不报错，若不做列校验就会“晚到”在查询阶段炸掉。
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE media_items (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                uri                 TEXT    UNIQUE NOT NULL,
                path                TEXT    NOT NULL,
                folder_path         TEXT    NOT NULL,
                mime_type           TEXT    NOT NULL,
                media_kind          TEXT    NOT NULL DEFAULT 'image',
                media_subkind        TEXT    NOT NULL DEFAULT 'standard',
                media_attributes     TEXT    NOT NULL DEFAULT '{}',
                width               INTEGER,
                height              INTEGER,
                taken_at            INTEGER,
                file_mtime          INTEGER NOT NULL,
                file_size           INTEGER NOT NULL,
                blake3_hash         TEXT    NOT NULL,
                is_favorite         INTEGER NOT NULL DEFAULT 0,
                trashed_at          INTEGER,
                indexed_at          INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items (uri, path, folder_path, mime_type, file_mtime, file_size, blake3_hash, indexed_at)
             VALUES ('legacy', '/tmp/legacy.jpg', '/', 'image/jpeg', 0, 0, 'h', 0)",
            [],
        )
        .unwrap();
    }

    let pool =
        db::init_pool(&db_path).expect("init_pool should regenerate for missing required columns");
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "legacy rows should be removed when schema columns are missing"
    );
}

#[test]
fn init_pool_does_not_wipe_when_columns_only_differ_in_extras() {
    // 反向保险：仅仅多/少一些**不影响迁移成功**的字段（即 SQLite 自己没报错）
    // 时，不应触发删库。这是“去掉 verify_schema”的初衷。
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    // 先正常建库
    {
        let pool = db::init_pool(&db_path).unwrap();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO media_items
                (uri, path, folder_path, mime_type, file_mtime, file_size,
                 blake3_hash, indexed_at)
             VALUES ('keep', '/p', '/', 'image/jpeg', 0, 0, 'h', 0)",
            [],
        )
        .unwrap();
        // 加一列旧版本里没有的扩展字段；新 schema 没引用它，迁移不会失败。
        conn.execute("ALTER TABLE media_items ADD COLUMN legacy_extra TEXT", [])
            .unwrap();
    }

    let pool = db::init_pool(&db_path).expect("init_pool should succeed without wiping");
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 1,
        "data must be preserved when migration itself succeeds"
    );
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
