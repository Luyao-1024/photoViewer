use chrono::{DateTime, Utc};
use std::path::PathBuf;

/// 单张媒体项的完整元数据
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub id: i64,
    pub uri: String,
    pub path: PathBuf,
    pub folder_path: PathBuf,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub file_mtime: DateTime<Utc>,
    pub file_size: u64,
    pub blake3_hash: String,
    pub trashed_at: Option<DateTime<Utc>>,
}

impl MediaItem {
    pub fn display_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("(unnamed)")
    }
}

/// 用于 INSERT 的新项（不含 id 和 trashed_at）
#[derive(Debug, Clone)]
pub struct NewMediaItem {
    pub uri: String,
    pub path: PathBuf,
    pub folder_path: PathBuf,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub file_mtime: DateTime<Utc>,
    pub file_size: u64,
    pub blake3_hash: String,
}

impl From<&MediaItem> for NewMediaItem {
    fn from(item: &MediaItem) -> Self {
        Self {
            uri: item.uri.clone(),
            path: item.path.clone(),
            folder_path: item.folder_path.clone(),
            mime_type: item.mime_type.clone(),
            width: item.width,
            height: item.height,
            taken_at: item.taken_at,
            file_mtime: item.file_mtime,
            file_size: item.file_size,
            blake3_hash: item.blake3_hash.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item() -> MediaItem {
        MediaItem {
            id: 1,
            uri: "file:///tmp/IMG_001.jpg".into(),
            path: PathBuf::from("/tmp/IMG_001.jpg"),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            width: Some(1920),
            height: Some(1080),
            taken_at: Some(Utc::now()),
            file_mtime: Utc::now(),
            file_size: 123_456,
            blake3_hash: "abc123".into(),
            trashed_at: None,
        }
    }

    #[test]
    fn display_name_from_path() {
        let item = sample_item();
        assert_eq!(item.display_name(), "IMG_001.jpg");
    }

    #[test]
    fn trashed_flag() {
        let mut item = sample_item();
        assert!(item.trashed_at.is_none());
        item.trashed_at = Some(Utc::now());
        assert!(item.trashed_at.is_some());
    }
}