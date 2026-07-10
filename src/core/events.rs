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
    pub thumbnail: bool,
    pub attributes: bool,
}

impl MediaFields {
    pub const FAVORITE: Self = Self {
        favorite: true,
        metadata: false,
        location: false,
        trash: false,
        thumbnail: false,
        attributes: false,
    };

    pub const LOCATION: Self = Self {
        favorite: false,
        metadata: false,
        location: true,
        trash: false,
        thumbnail: false,
        attributes: false,
    };

    pub const TRASH: Self = Self {
        favorite: false,
        metadata: false,
        location: false,
        trash: true,
        thumbnail: false,
        attributes: false,
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
    MediaMovedToTrash {
        source: ChangeSource,
        items: Vec<MediaItem>,
    },
    MediaRestored {
        source: ChangeSource,
        items: Vec<MediaItem>,
    },
    MediaUpdated {
        source: ChangeSource,
        items: Vec<MediaItem>,
        fields: MediaFields,
    },
    TrashChanged {
        source: ChangeSource,
    },
    AlbumsChanged {
        source: ChangeSource,
        affected_folders: Vec<std::path::PathBuf>,
        affected_virtual: Vec<String>,
        live_count_delta: i64,
    },
    AlbumCoverChanged {
        folder_path: std::path::PathBuf,
        cover_uri: String,
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
mod tests;
