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

    pub fn removed_batch(&self, source: ChangeSource, uris: Vec<String>) {
        if uris.is_empty() {
            return;
        }
        if let Err(e) = self.tx.send(DomainEvent::MediaRemoved {
            source,
            ids: Vec::new(),
            uris,
        }) {
            tracing::warn!("MediaChangeNotifier::removed_batch send failed: {e}");
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
mod tests;
