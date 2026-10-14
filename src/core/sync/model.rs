use serde::{Deserialize, Serialize};

/// Which transport a connection speaks. The stored value is the column text, so
/// the two representations must stay in step; an unknown value is a database
/// problem rather than something to default away.
///
/// Native SyncServer is currently the only kind. The column — and this enum —
/// exist so adding a second provider later is a code change, not a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    NativeSyncServer,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NativeSyncServer => "native_syncserver",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "native_syncserver" => Some(Self::NativeSyncServer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
    Bidirectional,
    UploadOnly,
}

impl SyncDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bidirectional => "bidirectional",
            Self::UploadOnly => "upload_only",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bidirectional" => Some(Self::Bidirectional),
            "upload_only" => Some(Self::UploadOnly),
            _ => None,
        }
    }
}

/// Which local folders a job may upload. Cloud presence never implies upload
/// permission: a folder must be selected here regardless of what the cloud
/// already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadScope {
    All,
    SelectedAlbums,
}

impl UploadScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::SelectedAlbums => "selected_albums",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "selected_albums" => Some(Self::SelectedAlbums),
            _ => None,
        }
    }
}
