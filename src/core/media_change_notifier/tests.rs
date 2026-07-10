use super::*;
use crate::core::media::MediaItem;
use chrono::Utc;
use std::path::PathBuf;

fn sample_item(uri: &str) -> MediaItem {
    MediaItem {
        id: 1,
        uri: uri.into(),
        path: PathBuf::from(uri.trim_start_matches("file://")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc::now(),
        file_size: 1,
        blake3_hash: "h".into(),
        is_favorite: false,
        trashed_at: None,
    }
}

#[test]
fn notifier_upserted_sends_event_to_receiver() {
    let (notifier, mut rx) = MediaChangeNotifier::new();
    let item = sample_item("file:///tmp/a.jpg");
    notifier.upserted(item.clone());

    match rx.try_recv() {
        Ok(DomainEvent::MediaUpserted { source, items }) => {
            assert_eq!(source, ChangeSource::FilesystemWatcher);
            assert_eq!(items[0].uri, item.uri);
        }
        other => panic!("expected MediaUpserted, got {other:?}"),
    }
}

#[test]
fn notifier_upserted_batch_sends_event_to_receiver() {
    let (notifier, mut rx) = MediaChangeNotifier::new();
    let items = vec![
        sample_item("file:///tmp/a.jpg"),
        sample_item("file:///tmp/b.jpg"),
    ];
    notifier.upserted_batch(ChangeSource::StartupScan, items.clone());

    match rx.try_recv() {
        Ok(DomainEvent::MediaUpserted {
            source,
            items: received,
        }) => {
            assert_eq!(source, ChangeSource::StartupScan);
            assert_eq!(received.len(), 2);
            assert_eq!(received[0].uri, items[0].uri);
            assert_eq!(received[1].uri, items[1].uri);
        }
        other => panic!("expected MediaUpserted batch, got {other:?}"),
    }
}

#[test]
fn notifier_removed_sends_event_to_receiver() {
    let (notifier, mut rx) = MediaChangeNotifier::new();
    notifier.removed("file:///tmp/a.jpg".into());

    match rx.try_recv() {
        Ok(DomainEvent::MediaRemoved { uris, .. }) => {
            assert_eq!(uris, vec!["file:///tmp/a.jpg"])
        }
        other => panic!("expected MediaRemoved, got {other:?}"),
    }
}

#[test]
fn notifier_send_after_receiver_drop_does_not_panic() {
    let (notifier, rx) = MediaChangeNotifier::new();
    drop(rx);
    // Should not panic; only emits a tracing::warn.
    notifier.upserted(sample_item("file:///tmp/a.jpg"));
    notifier.removed("file:///tmp/a.jpg".into());
}
