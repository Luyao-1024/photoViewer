//! End-to-end event emission tests for `notify_watcher` + `MediaChangeNotifier`.
//!
//! All tests in this file depend on `notify`'s inotify/fsevent behavior and
//! run in the default test suite to cover watcher event delivery.
mod common;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::events::DomainEvent;
use photo_viewer::core::media_change_notifier::MediaChangeNotifier;
use photo_viewer::core::notify_watcher;
use std::time::{Duration, Instant};
use tempfile::tempdir;

struct WatcherHarness {
    rt: Option<tokio::runtime::Runtime>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for WatcherHarness {
    fn drop(&mut self) {
        self.handle.abort();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_millis(100));
        }
    }
}

/// Spin up a watcher in `root` and return the receiver.
fn spawn_watcher(
    root: std::path::PathBuf,
) -> (
    photo_viewer::core::db::DbPool,
    tokio::sync::mpsc::UnboundedReceiver<DomainEvent>,
    WatcherHarness,
) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pool = db::init_pool(&root.join("test.db")).unwrap();
    let (notifier, rx) = MediaChangeNotifier::new();
    let h = {
        let _guard = rt.enter();
        notify_watcher::start_watching(pool.clone(), vec![root.clone()], vec![], root, notifier)
    };
    // Give the watcher a moment to call `watcher.watch(...)`.
    std::thread::sleep(Duration::from_millis(300));
    (
        pool,
        rx,
        WatcherHarness {
            rt: Some(rt),
            handle: h,
        },
    )
}

/// Drain `rx` until we see an event whose uri matches `uri`, or the deadline
/// passes. Returns the event on success.
fn wait_for_uri(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<DomainEvent>,
    uri: &str,
    timeout: Duration,
) -> Option<DomainEvent> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(event) => {
                let matches = match &event {
                    DomainEvent::MediaUpserted { items, .. }
                    | DomainEvent::MediaUpdated { items, .. } => {
                        items.iter().any(|item| item.uri == uri)
                    }
                    DomainEvent::MediaRemoved { uris, .. } => uris.iter().any(|u| u == uri),
                    DomainEvent::TrashChanged { .. }
                    | DomainEvent::AlbumsDirty { .. }
                    | DomainEvent::ThumbnailStatsDirty
                    | DomainEvent::LiveCountDirty => false,
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
fn watcher_emits_upserted_for_new_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _watcher) = spawn_watcher(root.clone());

    let path = write_plain_jpeg(&root, "watched.jpg");
    let uri = format!("file://{}", path.display());

    let event = wait_for_uri(&mut rx, &uri, Duration::from_secs(5));
    assert!(
        matches!(event, Some(DomainEvent::MediaUpserted { .. })),
        "expected Upserted for {uri}, got {event:?}"
    );
}

#[test]
fn watcher_emits_removed_for_deleted_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _watcher) = spawn_watcher(root.clone());

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
        matches!(event, Some(DomainEvent::MediaRemoved { .. })),
        "expected Removed for {uri}, got {event:?}"
    );
}

#[test]
fn watcher_emits_upserted_for_modified_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _watcher) = spawn_watcher(root.clone());

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
            Ok(DomainEvent::MediaUpserted { items, .. })
                if items.iter().any(|item| item.uri == uri) =>
            {
                upsert_count += 1
            }
            Ok(DomainEvent::MediaRemoved { uris, .. }) if uris.iter().any(|u| u == &uri) => {
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
