use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    pub fn as_db_value(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

pub const MEDIA_SUBKIND_STANDARD: &str = "standard";
pub const MEDIA_SUBKIND_MOTION_PHOTO: &str = "motion_photo";

pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "heic", "heif"];
pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "avi"];

pub fn mime_from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "heic" | "heif" => "image/heic",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        _ => return None,
    })
}

pub fn media_kind_from_mime(mime_type: &str) -> Option<MediaKind> {
    if mime_type.starts_with("image/") {
        Some(MediaKind::Image)
    } else if mime_type.starts_with("video/") {
        Some(MediaKind::Video)
    } else {
        None
    }
}

pub fn is_supported_media_path(path: &Path) -> bool {
    mime_from_extension(path).is_some()
}

/// 单张媒体项的完整元数据
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub id: i64,
    pub uri: String,
    pub path: PathBuf,
    pub folder_path: PathBuf,
    pub mime_type: String,
    pub media_subkind: String,
    pub media_attributes: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub file_mtime: DateTime<Utc>,
    pub file_size: u64,
    pub blake3_hash: String,
    pub trashed_at: Option<DateTime<Utc>>,
}

impl MediaItem {
    pub fn media_kind(&self) -> Option<MediaKind> {
        media_kind_from_mime(&self.mime_type)
    }

    pub fn is_image(&self) -> bool {
        self.media_kind() == Some(MediaKind::Image)
    }

    pub fn is_video(&self) -> bool {
        self.media_kind() == Some(MediaKind::Video)
    }

    pub fn is_motion_photo(&self) -> bool {
        self.media_subkind == MEDIA_SUBKIND_MOTION_PHOTO
    }

    pub fn display_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("(unnamed)")
    }

    pub fn sort_datetime(&self) -> DateTime<Utc> {
        self.taken_at.unwrap_or(self.file_mtime)
    }
}

/// 用于 INSERT 的新项（不含 id 和 trashed_at）
#[derive(Debug, Clone)]
pub struct NewMediaItem {
    pub uri: String,
    pub path: PathBuf,
    pub folder_path: PathBuf,
    pub mime_type: String,
    pub media_subkind: String,
    pub media_attributes: String,
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
            media_subkind: item.media_subkind.clone(),
            media_attributes: item.media_attributes.clone(),
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
            media_subkind: MEDIA_SUBKIND_STANDARD.into(),
            media_attributes: "{}".into(),
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
