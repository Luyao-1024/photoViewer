//! End-to-end event emission tests for `notify_watcher` + `MediaChangeNotifier`.
//!
//! All tests in this file depend on `notify`'s inotify/fsevent behavior, so
//! they are `#[ignore]` by default. Run locally with
//! `cargo test --test notify_watcher_notifier -- --ignored`.
mod common;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::media_change_notifier::{MediaChangeEvent, MediaChangeNotifier};
use photo_viewer::core::notify_watcher;
use std::time::{Duration, Instant};
use tempfile::tempdir;

/// Spin up a watcher in `root` and return the receiver.
fn spawn_watcher(
    root: std::path::PathBuf,
) -> (
    photo_viewer::core::db::DbPool,
    tokio::sync::mpsc::UnboundedReceiver<MediaChangeEvent>,
    tokio::task::JoinHandle<()>,
) {
    let pool = db::init_pool(&root.join("test.db")).unwrap();
    let (notifier, rx) = MediaChangeNotifier::new();
    let h = notify_watcher::start_watching(pool.clone(), vec![root], notifier);
    // Give the watcher a moment to call `watcher.watch(...)`.
    std::thread::sleep(Duration::from_millis(300));
    (pool, rx, h)
}

/// Drain `rx` until we see an event whose uri matches `uri`, or the deadline
/// passes. Returns the event on success.
fn wait_for_uri(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<MediaChangeEvent>,
    uri: &str,
    timeout: Duration,
) -> Option<MediaChangeEvent> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(event) => {
                let matches = match &event {
                    MediaChangeEvent::Upserted(item) => item.uri == uri,
                    MediaChangeEvent::Removed { uri: u } => u == uri,
                };
                if matches {
                    return Some(event);
                }
                // Skip non-matching events.
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    None
}

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_upserted_for_new_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _h) = spawn_watcher(root.clone());

    let path = write_plain_jpeg(&root, "watched.jpg");
    let uri = format!("file://{}", path.display());

    let event = wait_for_uri(&mut rx, &uri, Duration::from_secs(5));
    assert!(
        matches!(event, Some(MediaChangeEvent::Upserted(_))),
        "expected Upserted for {uri}, got {event:?}"
    );
}

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_removed_for_deleted_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _h) = spawn_watcher(root.clone());

    let path = write_plain_jpeg(&root, "doomed.jpg");
    let uri = format!("file://{}", path.display());
    // Let the upsert settle before removing.
    assert!(
        wait_for_uri(&mut rx, &uri, Duration::from_secs(5)).is_some(),
        "expected upsert before delete"
    );

    std::fs::remove_file(&path).unwrap();
    let event = wait_for_uri(&mut rx, &uri, Duration::from_secs(5));
    assert!(
        matches!(event, Some(MediaChangeEvent::Removed { .. })),
        "expected Removed for {uri}, got {event:?}"
    );
}

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_upserted_for_modified_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _h) = spawn_watcher(root.clone());

    let path = write_plain_jpeg(&root, "modified.jpg");
    let uri = format!("file://{}", path.display());
    // First upsert.
    assert!(
        wait_for_uri(&mut rx, &uri, Duration::from_secs(5)).is_some(),
        "expected initial upsert"
    );

    // Re-write with different (still valid) image content so the file bytes
    // change — triggering a Modify(Data) event — while remaining a decodable
    // JPEG for `upsert_from_path`.
    write_distinct_jpeg(&path, 48, 48, [255, 0, 0]);

    // We expect either another Upserted for the same uri, OR a Removed
    // + Upserted pair (depends on backend). Count Upserted events for
    // this uri within the window.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut upsert_count = 0;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(MediaChangeEvent::Upserted(item)) if item.uri == uri => upsert_count += 1,
            Ok(MediaChangeEvent::Removed { uri: u }) if u == uri => {
                // Some backends emit Removed+Upserted on modify (delete-then-reinsert);
                // we only assert on the Upserted count below, so swallow the Removed
                // pair member here.
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    // After the initial upsert (counted in setup) + the post-modify
    // upsert, we expect at least 1 more Upserted event in the window.
    assert!(
        upsert_count >= 1,
        "expected at least one more Upserted after modify, got {upsert_count}"
    );
}
