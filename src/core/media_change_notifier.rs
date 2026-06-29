//! Media change notification channel
//!
//! Decouples the filesystem watcher (producer) from the GTK main thread
//! consumer that mutates the shared `gio::ListStore`. The watcher holds
//! a `MediaChangeNotifier` clone; a `glib::MainContext::spawn_local` task
//! owns the receiver and applies splice/append/remove diffs.

use crate::core::events::{ChangeSource, DomainEvent};
use crate::core::media::MediaItem;
use tokio::sync::mpsc;

/// Producer side of the media-change channel.
///
/// Cheap to clone (wraps an `UnboundedSender`). Watcher keeps one clone
/// in its `spawn_blocking` thread.
#[derive(Clone)]
pub struct MediaChangeNotifier {
    tx: mpsc::UnboundedSender<DomainEvent>,
}

impl MediaChangeNotifier {
    /// Create a paired notifier + receiver. The receiver is typically
    /// moved into a `glib::MainContext::spawn_local` task.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<DomainEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Notify that `item` was inserted or updated. The current GTK-thread
    /// consumer will splice it into the shared list.
    pub fn upserted(&self, item: MediaItem) {
        if let Err(e) = self.tx.send(DomainEvent::MediaUpserted {
            source: ChangeSource::FilesystemWatcher,
            items: vec![item],
        }) {
            tracing::warn!("MediaChangeNotifier::upserted send failed: {e}");
        }
    }

    /// Notify that multiple items were inserted or updated.
    pub fn upserted_batch(&self, source: ChangeSource, items: Vec<MediaItem>) {
        if items.is_empty() {
            return;
        }
        if let Err(e) = self.tx.send(DomainEvent::MediaUpserted { source, items }) {
            tracing::warn!("MediaChangeNotifier::upserted_batch send failed: {e}");
        }
    }

    /// Notify that the item with the given `uri` was removed.
    pub fn removed(&self, uri: String) {
        if let Err(e) = self.tx.send(DomainEvent::MediaRemoved {
            source: ChangeSource::FilesystemWatcher,
            ids: Vec::new(),
            uris: vec![uri],
        }) {
            tracing::warn!("MediaChangeNotifier::removed send failed: {e}");
        }
    }

    /// Notify that the system trash changed and the DB has been re-reconciled.
    /// Consumers refresh any visible Trash view.
    pub fn trash_changed(&self) {
        if let Err(e) = self.tx.send(DomainEvent::TrashChanged {
            source: ChangeSource::TrashReconcile,
        }) {
            tracing::warn!("MediaChangeNotifier::trash_changed send failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
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
}
