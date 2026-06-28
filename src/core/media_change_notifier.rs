//! Media change notification channel
//!
//! Decouples the filesystem watcher (producer) from the GTK main thread
//! consumer that mutates the shared `gio::ListStore`. The watcher holds
//! a `MediaChangeNotifier` clone; a `glib::MainContext::spawn_local` task
//! owns the receiver and applies splice/append/remove diffs.

use crate::core::media::MediaItem;
use tokio::sync::mpsc;

/// Origin of a media change batch. Consumers use this to choose refresh
/// urgency: user-visible edits should update immediately, while startup scan
/// batches must be throttled to keep the UI responsive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaChangeSource {
    StartupScan,
    UserInteractive,
    Watcher,
}

/// Media change event emitted by the watcher, consumed by the UI.
#[derive(Debug, Clone)]
pub enum MediaChangeEvent {
    /// A new item was inserted, or an existing item was updated.
    /// Consumers match by `uri`: existing → splice-replace; absent → append.
    Upserted(MediaItem),
    /// Multiple inserts/updates from a bulk source such as the first startup
    /// filesystem scan. Consumers should apply this as one list mutation so
    /// expensive UI model observers rebuild once per batch instead of once per
    /// file.
    UpsertedBatch {
        source: MediaChangeSource,
        items: Vec<MediaItem>,
    },
    /// An item was removed. Consumer matches by `uri`.
    Removed { uri: String },
    /// The system trash changed (external restore / empty / delete-from-trash, or
    /// the app's own trash op). The watcher has already re-run `reconcile_trash`
    /// against the DB; consumers should refresh any visible Trash view so it
    /// matches without a page switch.
    TrashChanged,
}

/// Producer side of the media-change channel.
///
/// Cheap to clone (wraps an `UnboundedSender`). Watcher keeps one clone
/// in its `spawn_blocking` thread.
#[derive(Clone)]
pub struct MediaChangeNotifier {
    tx: mpsc::UnboundedSender<MediaChangeEvent>,
}

impl MediaChangeNotifier {
    /// Create a paired notifier + receiver. The receiver is typically
    /// moved into a `glib::MainContext::spawn_local` task.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<MediaChangeEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Notify that `item` was inserted or updated. The current GTK-thread
    /// consumer will splice it into the shared list.
    pub fn upserted(&self, item: MediaItem) {
        if let Err(e) = self.tx.send(MediaChangeEvent::Upserted(item)) {
            tracing::warn!("MediaChangeNotifier::upserted send failed: {e}");
        }
    }

    /// Notify that multiple items were inserted or updated.
    pub fn upserted_batch(&self, source: MediaChangeSource, items: Vec<MediaItem>) {
        if items.is_empty() {
            return;
        }
        if let Err(e) = self.tx.send(MediaChangeEvent::UpsertedBatch { source, items }) {
            tracing::warn!("MediaChangeNotifier::upserted_batch send failed: {e}");
        }
    }

    /// Notify that the item with the given `uri` was removed.
    pub fn removed(&self, uri: String) {
        if let Err(e) = self.tx.send(MediaChangeEvent::Removed { uri }) {
            tracing::warn!("MediaChangeNotifier::removed send failed: {e}");
        }
    }

    /// Notify that the system trash changed and the DB has been re-reconciled.
    /// Consumers refresh any visible Trash view.
    pub fn trash_changed(&self) {
        if let Err(e) = self.tx.send(MediaChangeEvent::TrashChanged) {
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
            Ok(MediaChangeEvent::Upserted(received)) => {
                assert_eq!(received.uri, item.uri);
            }
            other => panic!("expected Upserted, got {other:?}"),
        }
    }

    #[test]
    fn notifier_upserted_batch_sends_event_to_receiver() {
        let (notifier, mut rx) = MediaChangeNotifier::new();
        let items = vec![
            sample_item("file:///tmp/a.jpg"),
            sample_item("file:///tmp/b.jpg"),
        ];
        notifier.upserted_batch(MediaChangeSource::StartupScan, items.clone());

        match rx.try_recv() {
            Ok(MediaChangeEvent::UpsertedBatch { source, items: received }) => {
                assert_eq!(source, MediaChangeSource::StartupScan);
                assert_eq!(received.len(), 2);
                assert_eq!(received[0].uri, items[0].uri);
                assert_eq!(received[1].uri, items[1].uri);
            }
            other => panic!("expected UpsertedBatch, got {other:?}"),
        }
    }

    #[test]
    fn notifier_removed_sends_event_to_receiver() {
        let (notifier, mut rx) = MediaChangeNotifier::new();
        notifier.removed("file:///tmp/a.jpg".into());

        match rx.try_recv() {
            Ok(MediaChangeEvent::Removed { uri }) => assert_eq!(uri, "file:///tmp/a.jpg"),
            other => panic!("expected Removed, got {other:?}"),
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
