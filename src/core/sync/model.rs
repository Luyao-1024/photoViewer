use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
    Bidirectional,
    UploadOnly,
    DownloadOnly,
}

impl SyncDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bidirectional => "bidirectional",
            Self::UploadOnly => "upload_only",
            Self::DownloadOnly => "download_only",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bidirectional" => Some(Self::Bidirectional),
            "upload_only" => Some(Self::UploadOnly),
            "download_only" => Some(Self::DownloadOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub blake3: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStrength {
    Strong,
    Weak,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub value: String,
    pub strength: RevisionStrength,
}

impl Revision {
    pub fn etag(value: impl Into<String>) -> Self {
        let value = value.into();
        let strength = if value.trim_start().starts_with("W/") {
            RevisionStrength::Weak
        } else {
            RevisionStrength::Strong
        };
        Self { value, strength }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Observation {
    Unknown,
    Absent,
    Present {
        fingerprint: Option<Fingerprint>,
        revision: Option<Revision>,
        size: u64,
        modified_unix: Option<i64>,
    },
}

impl Observation {
    pub fn fingerprint(&self) -> Option<&Fingerprint> {
        match self {
            Self::Present { fingerprint, .. } => fingerprint.as_ref(),
            Self::Unknown | Self::Absent => None,
        }
    }

    pub fn revision(&self) -> Option<&Revision> {
        match self {
            Self::Present { revision, .. } => revision.as_ref(),
            Self::Unknown | Self::Absent => None,
        }
    }

    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub fingerprint: Fingerprint,
    pub local_revision: Option<Revision>,
    pub remote_revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrySnapshot {
    pub relative_path: String,
    pub local: Observation,
    pub remote: Observation,
    pub baseline: Option<Baseline>,
    pub direction: SyncDirection,
    pub propagate_deletes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    InitialContentMismatch,
    BothModified,
    DeleteVsModify,
    MissingWithDeletesDisabled,
    InsufficientEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanAction {
    Noop,
    UploadNew,
    UploadReplace { expected: Revision },
    DownloadNew,
    DownloadReplace,
    DeleteLocal,
    DeleteRemote { expected: Revision },
    VerifyContent,
    WaitForCompleteObservation,
    Conflict(ConflictKind),
}
