//! Media change notification channel
//!
//! Decouples the filesystem watcher (producer) from the GTK main thread
//! consumer that mutates the shared `gio::ListStore`. The watcher holds
//! a `MediaChangeNotifier` clone; a `glib::MainContext::spawn_local` task
//! owns the receiver and applies splice/append/remove diffs.

use crate::core::media::MediaItem;
use tokio::sync::mpsc;

/// Media change event emitted by the watcher, consumed by the UI.
#[derive(Debug, Clone)]
pub enum MediaChangeEvent {
    /// A new item was inserted, or an existing item was updated.
    /// Consumers match by `uri`: existing → splice-replace; absent → append.
    Upserted(MediaItem),
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
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "h".into(),
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
