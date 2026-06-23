use photo_viewer::core::db;
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
        conn.execute(
            "ALTER TABLE media_items ADD COLUMN legacy_extra TEXT",
            [],
        )
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
