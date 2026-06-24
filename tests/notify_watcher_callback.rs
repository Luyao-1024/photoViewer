//! Verify `start_watching` emits `MediaChangeEvent`s on the notifier.
//!
//! This test depends on `notify`'s inotify/fsevent behavior, so it's
//! `#[ignore]` by default. Run locally with
//! `cargo test --test notify_watcher_callback -- --ignored`.
mod common;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::media_change_notifier::{MediaChangeEvent, MediaChangeNotifier};
use photo_viewer::core::notify_watcher;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_upserted_after_successful_upsert() {
    let dir = tempdir().unwrap();
    let shots = dir.path().join("截图");
    std::fs::create_dir(&shots).unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let (notifier, mut rx) = MediaChangeNotifier::new();
    let _h = notify_watcher::start_watching(pool.clone(), vec![dir.path().to_path_buf()], notifier);

    write_plain_png(&shots, "new.png");

    // Poll the receiver (50ms inotify event + 50ms write-sleep + DB write).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received = false;
    while Instant::now() < deadline {
        if let Ok(MediaChangeEvent::Upserted(item)) = rx.try_recv() {
            assert!(item.path.ends_with("new.png"));
            received = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(received, "watcher should have emitted an Upserted event");
}
