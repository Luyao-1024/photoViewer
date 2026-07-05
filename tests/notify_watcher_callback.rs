//! Verify `start_watching` emits `DomainEvent`s on the notifier.
//!
//! This test depends on `notify`'s inotify/fsevent behavior and runs in the
//! default test suite to cover watcher-to-domain-event delivery.
mod common;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::events::DomainEvent;
use photo_viewer::core::notify_watcher;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn watcher_emits_upserted_after_successful_upsert() {
    let dir = tempdir().unwrap();
    let shots = dir.path().join("截图");
    std::fs::create_dir(&shots).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let (event_sender, mut rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(pool.clone(), event_sender);
    let watcher = {
        let _guard = rt.enter();
        notify_watcher::start_watching(
            db_actor,
            vec![dir.path().to_path_buf()],
            vec![],
            vec![],
            dir.path().to_path_buf(),
        )
    };

    write_plain_png(&shots, "new.png");

    // Poll the receiver (50ms inotify event + 50ms write-sleep + DB write).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received = false;
    while Instant::now() < deadline {
        if let Ok(DomainEvent::MediaUpserted { items, .. }) = rx.try_recv() {
            assert!(items.iter().any(|item| item.path.ends_with("new.png")));
            received = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(received, "watcher should have emitted an Upserted event");
    watcher.abort();
    rt.shutdown_timeout(Duration::from_millis(100));
}
