use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionPhotoFormat {
    GoogleMicroVideo,
    GoogleMotionPhotoContainer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotionPhotoInfo {
    pub format: MotionPhotoFormat,
    pub video_offset: u64,
    pub video_length: u64,
    pub presentation_timestamp_us: Option<u64>,
    pub gain_map_offset: Option<u64>,
    pub gain_map_length: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaAttributes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_photo: Option<MotionPhotoInfo>,
}

impl MediaAttributes {
    pub fn standard_json() -> String {
        "{}".to_string()
    }

    pub fn motion_photo_json(info: MotionPhotoInfo) -> String {
        serde_json::to_string(&Self {
            motion_photo: Some(info),
        })
        .unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }
}

#[derive(Debug)]
struct ContainerItem {
    mime: String,
    semantic: String,
    length: u64,
}

pub fn detect(path: &Path) -> Option<MotionPhotoInfo> {
    let bytes = std::fs::read(path).ok()?;
    detect_bytes(&bytes)
}

pub fn extract_video_to(path: &Path, info: &MotionPhotoInfo, dest: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut out = std::fs::File::create(dest)?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(info.video_offset))?;
    let mut limited = std::io::Read::take(file, info.video_length);
    std::io::copy(&mut limited, &mut out)?;
    Ok(())
}

fn detect_bytes(bytes: &[u8]) -> Option<MotionPhotoInfo> {
    let text = String::from_utf8_lossy(bytes);
    detect_micro_video(bytes, &text).or_else(|| detect_container(bytes, &text))
}

fn detect_micro_video(bytes: &[u8], text: &str) -> Option<MotionPhotoInfo> {
    if !text.contains("GCamera:MicroVideo=\"1\"") {
        return None;
    }
    let video_length = attr_u64(text, "GCamera:MicroVideoOffset")?;
    if video_length == 0 || video_length > bytes.len() as u64 {
        return None;
    }
    let video_offset = bytes.len() as u64 - video_length;
    if !looks_like_mp4_at(bytes, video_offset) {
        return None;
    }
    Some(MotionPhotoInfo {
        format: MotionPhotoFormat::GoogleMicroVideo,
        video_offset,
        video_length,
        presentation_timestamp_us: attr_u64(text, "GCamera:MicroVideoPresentationTimestampUs"),
        gain_map_offset: None,
        gain_map_length: None,
    })
}

fn detect_container(bytes: &[u8], text: &str) -> Option<MotionPhotoInfo> {
    if !text.contains("GCamera:MotionPhoto=\"1\"") || !text.contains("Container:Directory") {
        return None;
    }

    let items = parse_container_items(text);
    let tail_len: u64 = items.iter().map(|item| item.length).sum();
    if tail_len == 0 || tail_len > bytes.len() as u64 {
        return None;
    }

    let tail_start = bytes.len() as u64 - tail_len;
    let mut cursor = tail_start;
    let mut video = None;
    let mut gain_map = None;
    for item in items {
        if item.semantic.eq_ignore_ascii_case("GainMap") {
            gain_map = Some((cursor, item.length));
        }
        if item.mime == "video/mp4" || item.semantic.eq_ignore_ascii_case("MotionPhoto") {
            video = Some((cursor, item.length));
        }
        cursor = cursor.saturating_add(item.length);
    }

    let (video_offset, video_length) = video?;
    if video_length == 0 || !looks_like_mp4_at(bytes, video_offset) {
        return None;
    }

    Some(MotionPhotoInfo {
        format: MotionPhotoFormat::GoogleMotionPhotoContainer,
        video_offset,
        video_length,
        presentation_timestamp_us: attr_u64(text, "GCamera:MotionPhotoPresentationTimestampUs"),
        gain_map_offset: gain_map.map(|(offset, _)| offset),
        gain_map_length: gain_map.map(|(_, length)| length),
    })
}

fn parse_container_items(text: &str) -> Vec<ContainerItem> {
    let mut items = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<Container:Item") {
        rest = &rest[start..];
        let Some(end) = rest.find("/>") else {
            break;
        };
        let tag = &rest[..end + 2];
        if let (Some(mime), Some(semantic), Some(length)) = (
            attr_string(tag, "Item:Mime"),
            attr_string(tag, "Item:Semantic"),
            attr_u64(tag, "Item:Length"),
        ) {
            items.push(ContainerItem {
                mime,
                semantic,
                length,
            });
        }
        rest = &rest[end + 2..];
    }
    items
}

fn attr_u64(text: &str, name: &str) -> Option<u64> {
    attr_string(text, name)?.parse().ok()
}

fn attr_string(text: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..].find('"')?;
    Some(text[start..start + end].to_string())
}

fn looks_like_mp4_at(bytes: &[u8], offset: u64) -> bool {
    let offset = offset as usize;
    bytes
        .get(offset + 4..offset + 8)
        .map(|marker| marker == b"ftyp")
        .unwrap_or(false)
}
