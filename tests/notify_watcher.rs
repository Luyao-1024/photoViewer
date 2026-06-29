//! M5-T5: 文件监听集成测试
//!
//! 这些测试依赖 `notify` 的 inotify/fsevent 行为，必须在默认测试集中运行，
//! 以覆盖文件监听端到端行为。
//!
//! 测试聚焦三件事：
//!   1. `upsert_from_path` 在已有/新文件上行为正确；
//!   2. supported media extension filtering;
//!   3. `start_watching` 在临时目录里能创建 watcher 而不 panic（端到端可观测）。
mod common;
use common::*;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn upsert_from_path_inserts_new_file() {
    let dir = tmp_dir();
    write_plain_jpeg(dir.path(), "new.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    backend
        .upsert_from_path(&dir.path().join("new.jpg"))
        .expect("upsert_from_path 应当成功");

    let items = db::list_all_media(&pool).unwrap();
    assert_eq!(items.len(), 1, "新文件应被 upsert 到 DB");
    assert!(items[0].path.ends_with("new.jpg"));
}

#[test]
fn upsert_from_path_updates_existing_file() {
    let dir = tmp_dir();
    let path = write_plain_jpeg(dir.path(), "dup.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    backend.upsert_from_path(&path).unwrap();
    let first_id = db::list_all_media(&pool).unwrap()[0].id;

    // 第二次 upsert 应当命中同一条记录，ID 不变。
    backend.upsert_from_path(&path).unwrap();
    let items = db::list_all_media(&pool).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, first_id, "upsert 应当复用同一 id");
}

#[test]
fn upsert_from_path_ignores_non_file() {
    let dir = tmp_dir();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    // 目录路径应当静默返回 Ok(())，DB 中不增加任何行。
    backend
        .upsert_from_path(&dir.path().join("subdir"))
        .expect("目录路径应被忽略，不应报错");
    let items = db::list_all_media(&pool).unwrap();
    assert!(items.is_empty());
}

#[test]
fn upsert_from_path_skips_unsupported_extension() {
    let dir = tmp_dir();
    let txt = dir.path().join("notes.txt");
    std::fs::write(&txt, b"hello").unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    // .txt 不是图片，metadata::extract 会返回 Decode 错误。
    let result = backend.upsert_from_path(&txt);
    assert!(result.is_err(), "非图片扩展名应返回错误");
}

#[test]
fn watcher_picks_up_new_file() {
    // 端到端：启动 watcher，丢一个文件进去，等待 upsert 出现在 DB 中。
    use photo_viewer::core::media_change_notifier::MediaChangeNotifier;
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let pool = db::init_pool(&dir.path().join("watch.db")).unwrap();
    let (notifier, _rx) = MediaChangeNotifier::new();
    let watcher = {
        let _guard = rt.enter();
        photo_viewer::core::notify_watcher::start_watching(
            pool.clone(),
            vec![root.clone()],
            vec![],
            root.clone(),
            notifier,
        )
    };

    // 给 watcher 一点时间完成 setup
    std::thread::sleep(Duration::from_millis(300));

    write_plain_jpeg(&root, "watched.jpg");

    // 轮询 DB，最多 5 秒。
    let mut found = false;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        let items = db::list_all_media(&pool).unwrap();
        if items.iter().any(|m| m.path.ends_with("watched.jpg")) {
            found = true;
            break;
        }
    }
    assert!(found, "watcher 应当在 5s 内拾取新文件");
    watcher.abort();
    rt.shutdown_timeout(Duration::from_millis(100));
}
