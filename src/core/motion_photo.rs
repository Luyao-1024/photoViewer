use crate::core::media::mime_from_extension;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
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

/// 动图（Google/Xiaomi MicroVideo、Google Motion Photo Container）只可能是 JPEG：
/// 它们的形态是「JPEG 主体 + 追加的 MP4」，XMP 标记 `GCamera:*` 位于 JPEG 头部的 XMP
/// APP1 段。iPhone Live Photo 是 HEIC + 独立 `.mov`，不走这套追加式 MP4。因此视频 /
/// HEIC / PNG / WebP 永远不会命中——跳过它们，避免对 77MB 视频做无谓解析。
fn is_motion_candidate_mime(path: &Path) -> bool {
    matches!(mime_from_extension(path), Some("image/jpeg"))
}

/// 命中即值得全量解析的 ASCII 标记片段。
const MOTION_MARKERS: &[&[u8]] = &[
    b"GCamera:MicroVideo",
    b"GCamera:MotionPhoto",
    b"Container:Directory",
];

/// 在已读字节内按字节搜索 ASCII 标记，**不做 UTF-8 转换**（整段二进制逐字节转码才是
/// 原 `from_utf8_lossy` 路径的真正 CPU 瓶颈）。只对 XMP 载荷（通常 <2KB）调用，故开销
/// 与文件大小无关。
fn contains_motion_marker(bytes: &[u8]) -> bool {
    MOTION_MARKERS.iter().any(|needle| {
        needle.len() <= bytes.len() && bytes.windows(needle.len()).any(|w| w == *needle)
    })
}

/// XMP 在 JPEG 中固定以 APP1 段携带，载荷起始为 Adobe XMP 签名。
const XMP_APP1_SIG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// 头部读取上限。XMP APP1 紧随 SOI（可能在 JFIF APP0 / EXIF APP1 之后，EXIF APP1 至多
/// 64KB），128KB 足以覆盖真实动图的 XMP 段；段遍历遇到 SOF/SOS 即停，因此不会随文件
/// 大小增长。
const HEAD_SCAN_CAP: u64 = 128 * 1024; // 128 KiB

/// 按 JPEG 段结构（SOI 后若干 `FF<marker><len:2 BE>` 段）定位 XMP APP1 段，返回其 XMP
/// 载荷的字节区间。遇 SOFn/SOS/EOI 等表示元数据区结束的标记仍未命中则返回 None——
/// 绝大多数 JPEG 没有 XMP APP1，这里只走几个段就返回，**不逐字节扫描**整个头部。
fn find_xmp_payload_range(bytes: &[u8]) -> Option<std::ops::Range<usize>> {
    const SOI: &[u8; 2] = &[0xFF, 0xD8];
    if bytes.len() < 4 || bytes[..2] != *SOI {
        return None;
    }
    let mut pos = 2usize;
    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xFF {
            return None; // 段结构损坏
        }
        let marker = bytes[pos + 1];
        // 这些标记之后不再是元数据段（XMP APP1 必须在此之前出现），到此即可判定无 XMP。
        if marker == 0x00 // FF00 填充
            || marker == 0xD8 // SOI
            || (0xD0..=0xD7).contains(&marker) // RSTn
            || marker == 0xD9 // EOI
            || marker == 0xDA // SOS：之后是熵编码数据
            || (0xC0..=0xCF).contains(&marker) // SOFn / DHT / DAC：帧/表段，XMP 必在其前
        {
            return None;
        }
        let seg_len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if seg_len < 2 {
            return None;
        }
        let next = pos + 2 + seg_len;
        if next > bytes.len() {
            return None; // 段超出已读头部
        }
        let payload = pos + 4;
        if marker == 0xE1 && bytes[payload..].starts_with(XMP_APP1_SIG) {
            return Some((payload + XMP_APP1_SIG.len())..next);
        }
        pos = next;
    }
    None
}

pub fn detect(path: &Path) -> Option<MotionPhotoInfo> {
    if !is_motion_candidate_mime(path) {
        return None;
    }

    // 只读头部 128KB，按 JPEG 段结构直接跳到 XMP APP1 载荷里搜 ASCII 标记。绝大多数
    // JPEG 没有 XMP APP1，段遍历几步即返回 None；对真正的动图，也只从这小段 XMP 载荷
    // 解析偏移，并用一次 8 字节 seek 校验尾部 MP4——**绝不整文件读取或整文件 UTF8 转码**。
    // 后者曾是对 ~520 张 10MB 真动图的主要耗时（实测动图阶段 62s 的来源），段定位 + 小载
    // 荷解析把动图阶段降到与文件大小无关。
    let file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    let mut head = Vec::new();
    file.take(HEAD_SCAN_CAP)
        .read_to_end(&mut head)
        .ok()?;
    let xmp = find_xmp_payload_range(&head)?;
    let xmp_bytes = &head[xmp];
    if !contains_motion_marker(xmp_bytes) {
        return None;
    }

    let text = String::from_utf8_lossy(xmp_bytes);
    let has_mp4_at = |offset: u64| mp4_marker_at(path, offset);
    detect_micro_video(&text, file_len, &has_mp4_at)
        .or_else(|| detect_container(&text, file_len, &has_mp4_at))
}

pub fn extract_video_to(path: &Path, info: &MotionPhotoInfo, dest: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut out = std::fs::File::create(dest)?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(info.video_offset))?;
    let mut limited = std::io::Read::take(file, info.video_length);
    std::io::copy(&mut limited, &mut out)?;
    Ok(())
}

fn detect_micro_video(
    text: &str,
    file_len: u64,
    has_mp4_at: &impl Fn(u64) -> bool,
) -> Option<MotionPhotoInfo> {
    if !text.contains("GCamera:MicroVideo=\"1\"") {
        return None;
    }
    let video_length = attr_u64(text, "GCamera:MicroVideoOffset")?;
    if video_length == 0 || video_length > file_len {
        return None;
    }
    let video_offset = file_len - video_length;
    if !has_mp4_at(video_offset) {
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

fn detect_container(
    text: &str,
    file_len: u64,
    has_mp4_at: &impl Fn(u64) -> bool,
) -> Option<MotionPhotoInfo> {
    if !text.contains("GCamera:MotionPhoto=\"1\"") || !text.contains("Container:Directory") {
        return None;
    }

    let items = parse_container_items(text);
    let tail_len: u64 = items.iter().map(|item| item.length).sum();
    if tail_len == 0 || tail_len > file_len {
        return None;
    }

    let tail_start = file_len - tail_len;
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
    if video_length == 0 || !has_mp4_at(video_offset) {
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

/// 校验 `path` 在 `offset` 处是否为 MP4（`ftyp` box）：只 seek 读 8 字节，不读整文件。
fn mp4_marker_at(path: &Path, offset: u64) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    if f.seek(SeekFrom::Start(offset)).is_err() {
        return false;
    }
    let mut buf = [0u8; 8];
    if f.read_exact(&mut buf).is_err() {
        return false;
    }
    buf[4..8] == *b"ftyp"
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
