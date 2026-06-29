use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSource {
    StartupScan,
    FilesystemWatcher,
    UserInteractive,
    TrashReconcile,
    ThumbnailWorker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaFields {
    pub favorite: bool,
    pub metadata: bool,
    pub location: bool,
    pub trash: bool,
}

impl MediaFields {
    pub const FAVORITE: Self = Self {
        favorite: true,
        metadata: false,
        location: false,
        trash: false,
    };
}

#[derive(Debug, Clone)]
pub enum DomainEvent {
    MediaUpserted {
        source: ChangeSource,
        items: Vec<MediaItem>,
    },
    MediaRemoved {
        source: ChangeSource,
        ids: Vec<MediaId>,
        uris: Vec<String>,
    },
    MediaUpdated {
        source: ChangeSource,
        items: Vec<MediaItem>,
        fields: MediaFields,
    },
    TrashChanged {
        source: ChangeSource,
    },
    AlbumsDirty {
        source: ChangeSource,
    },
    ThumbnailStatsDirty,
    LiveCountDirty,
}

#[derive(Clone)]
pub struct DomainEventSender {
    tx: mpsc::UnboundedSender<DomainEvent>,
}

impl DomainEventSender {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<DomainEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    pub fn send(&self, event: DomainEvent) {
        if let Err(err) = self.tx.send(event) {
            tracing::warn!("DomainEventSender send failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_event_sender_sends_events() {
        let (sender, mut rx) = DomainEventSender::new();
        sender.send(DomainEvent::LiveCountDirty);
        assert!(matches!(rx.try_recv(), Ok(DomainEvent::LiveCountDirty)));
    }
}
