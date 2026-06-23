//! 验证 `start_watching` 的 `on_change` 回调：upsert 成功后调用方收到信号。
//!
//! 该测试依赖 `notify` 的 inotify/fsevent 行为，在 CI/沙箱中可能不可靠，
//! 因此默认 `#[ignore]`。本地运行：`cargo test --test notify_watcher_callback -- --ignored`。
mod common;
use common::*;
use photo_viewer::core::albums;
use photo_viewer::core::db;
use photo_viewer::core::notify_watcher;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn on_change_callback_fires_after_successful_upsert() {
    let dir = tempdir().unwrap();
    let shots = dir.path().join("截图");
    std::fs::create_dir(&shots).unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let _h =
        notify_watcher::start_watching(pool.clone(), vec![dir.path().to_path_buf()], move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

    write_plain_png(&shots, "new.png");

    // 轮询 inotify 事件 + 50ms 写入延迟 + upsert DB
    let deadline = Instant::now() + Duration::from_secs(5);
    while counter.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(counter.load(Ordering::SeqCst) >= 1, "callback should fire");

    // 显式 refresh 一次确保 albums 物化（callback 只触发"有变化"信号，聚合在调用方做）。
    albums::refresh(&pool).unwrap();
    assert!(albums::list(&pool).unwrap().iter().any(|a| {
        a.folder_path
            .file_name()
            .map(|s| s == "截图")
            .unwrap_or(false)
    }));
}
