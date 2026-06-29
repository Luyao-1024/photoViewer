//! Image metadata extraction: dimensions, EXIF DateTimeOriginal, MIME type.
use crate::core::error::{AppError, Result};
use crate::core::media::{media_kind_from_mime, mime_from_extension, MediaKind};
use chrono::{DateTime, TimeZone, Utc};
use gdk_pixbuf;
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::process::Command;

/// Curated, strongly-typed camera/EXIF summary for the details panel.
/// `from_exif` returns `None` when no curated field is present, so the UI can
/// omit the whole section cleanly. Core stays free of i18n; the UI formats and
/// translates these values.
#[derive(Debug, Clone, PartialEq)]
pub struct ExifSummary {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_model: Option<String>,
    pub software: Option<String>,
    pub aperture: Option<f64>,
    pub exposure_time: Option<(u32, u32)>,
    pub iso: Option<u32>,
    pub focal_length_mm: Option<f64>,
    pub focal_length_35mm_mm: Option<u32>,
    pub exposure_bias_ev: Option<f64>,
    pub exposure_mode: Option<ExposureMode>,
    pub metering_mode: Option<MeteringMode>,
    pub flash: Option<FlashState>,
    pub white_balance: Option<WhiteBalance>,
    pub gps: Option<GpsCoord>,
    pub altitude_m: Option<f64>,
}

impl ExifSummary {
    /// Build a curated summary from parsed EXIF. Returns `None` when no
    /// recognised field is present, so the UI can drop the section cleanly.
    pub fn from_exif(exif: &exif::Exif) -> Option<Self> {
        let summary = Self {
            make: ascii(exif, exif::Tag::Make),
            model: ascii(exif, exif::Tag::Model),
            lens_model: ascii(exif, exif::Tag::LensModel),
            software: ascii(exif, exif::Tag::Software),
            aperture: rational_as_f64(exif, exif::Tag::FNumber),
            exposure_time: rational(exif, exif::Tag::ExposureTime),
            iso: short(exif, exif::Tag::PhotographicSensitivity)
                .map(|v| v as u32)
                .or_else(|| short(exif, exif::Tag::ISOSpeed).map(|v| v as u32)),
            focal_length_mm: rational_as_f64(exif, exif::Tag::FocalLength),
            focal_length_35mm_mm: short(exif, exif::Tag::FocalLengthIn35mmFilm).map(|v| v as u32),
            exposure_bias_ev: srational_as_f64(exif, exif::Tag::ExposureBiasValue),
            exposure_mode: short(exif, exif::Tag::ExposureProgram).and_then(ExposureMode::from_u16),
            metering_mode: short(exif, exif::Tag::MeteringMode)
                .map(MeteringMode::from)
                .and_then(|m| {
                    // MeteringMode::Other is only for unknown values; drop it so the
                    // row isn't shown for exotic modes nobody recognises.
                    if matches!(m, MeteringMode::Other) {
                        None
                    } else {
                        Some(m)
                    }
                }),
            flash: short(exif, exif::Tag::Flash).map(|v| {
                // bit 0 == 1 means fired
                if v & 1 == 1 {
                    FlashState::Fired
                } else {
                    FlashState::NotFired
                }
            }),
            white_balance: short(exif, exif::Tag::WhiteBalance).and_then(WhiteBalance::from_u16),
            gps: gps_coord(exif),
            altitude_m: rational_as_f64(exif, exif::Tag::GPSAltitude).map(|raw| {
                // GPSAltitudeRef: 0 = above sea level, 1 = below.
                if byte(exif, exif::Tag::GPSAltitudeRef) == Some(1) {
                    -raw
                } else {
                    raw
                }
            }),
        };

        // Return None only when literally every field is absent.
        if summary.make.is_none()
            && summary.model.is_none()
            && summary.lens_model.is_none()
            && summary.software.is_none()
            && summary.aperture.is_none()
            && summary.exposure_time.is_none()
            && summary.iso.is_none()
            && summary.focal_length_mm.is_none()
            && summary.focal_length_35mm_mm.is_none()
            && summary.exposure_bias_ev.is_none()
            && summary.exposure_mode.is_none()
            && summary.metering_mode.is_none()
            && summary.flash.is_none()
            && summary.white_balance.is_none()
            && summary.gps.is_none()
            && summary.altitude_m.is_none()
        {
            None
        } else {
            Some(summary)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureMode {
    Auto,
    Manual,
    AutoBracket,
    AperturePriority,
    ShutterPriority,
    Program,
}

impl ExposureMode {
    fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            0 => Self::Auto, // Not defined → treat as auto
            1 => Self::Manual,
            2 => Self::Program,          // Program AE
            3 => Self::AperturePriority, // Av / A
            4 => Self::ShutterPriority,  // Tv / S
            5..=8 => Self::Auto,         // Creative / Action / Portrait / Landscape → auto
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeteringMode {
    Average,
    CenterWeighted,
    Spot,
    Other,
}

impl From<u16> for MeteringMode {
    fn from(v: u16) -> Self {
        match v {
            0 | 1 => Self::Average,
            2 => Self::CenterWeighted,
            3 => Self::Spot,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashState {
    Fired,
    NotFired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteBalance {
    Auto,
    Manual,
}

impl WhiteBalance {
    fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            0 => Self::Auto,
            1 => Self::Manual,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GpsCoord {
    pub lat: GpsDms,
    pub lon: GpsDms,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GpsDms {
    pub deg: u32,
    pub min: u32,
    pub sec: f64,
    /// `true` for N / E, `false` for S / W.
    pub north_or_east: bool,
}

/// Curated video metadata summary for the details panel (duration, codec, fps,
/// bitrate, container, device). Populated by `ffprobe`; the UI omits `None`
/// fields. Like `ExifSummary`, the core stays free of i18n — the UI formats and
/// translates these values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoSummary {
    pub duration_secs: Option<f64>,
    pub codec: Option<String>,
    pub fps: Option<f64>,
    pub bitrate: Option<u64>,
    pub container: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub make: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RawMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub mime_type: String,
    pub camera: Option<ExifSummary>,
    pub video: Option<VideoSummary>,
}

/// Largest prefix of an image file the scan hot path ever pre-reads. It covers
/// JPEG's SOF marker (after one or two ≤64 KiB APP1 segments) plus the 128 KiB
/// motion-photo XMP scan, so a single read can feed dimensions + EXIF + motion
/// detection. Pathological files that need more fall back to a full read.
pub const IMAGE_HEAD_CAP: u64 = 256 * 1024;

/// Read up to [`IMAGE_HEAD_CAP`] bytes from the start of `path`. The scan passes
/// this buffer to [`extract_with_head`] and motion-photo detection so a standard
/// image is opened once instead of 2–3 times.
pub fn read_image_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take(IMAGE_HEAD_CAP).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Extract metadata from a file at `path`.
///
/// Convenience wrapper for callers that don't have a pre-read head; equivalent
/// to [`extract_with_head`] with `head = None`.
pub fn extract(path: &Path) -> Result<RawMetadata> {
    extract_with_head(path, None)
}

/// Extract metadata, optionally reusing a head buffer already read by the
/// caller.
///
/// `head` lets the scan hot path pass the one 256 KiB read it already did (and
/// shares with motion-photo detection) so a standard image is parsed from that
/// buffer instead of being opened a second and third time for `image_dimensions`
/// / `read_exif`. `None` makes this read what it needs itself — the historical
/// behaviour for non-scan callers (incremental watcher, trash reconciliation,
/// UI single-file reads, tests).
///
/// HEIC ignores `head`: its EXIF item can live anywhere in the file, so it needs
/// the whole file (read once) and is not a motion-photo candidate.
pub fn extract_with_head(path: &Path, head: Option<&[u8]>) -> Result<RawMetadata> {
    let Some(mime_type) = mime_from_extension(path).map(str::to_string) else {
        return Err(AppError::Decode(format!(
            "unknown extension: {}",
            path.display()
        )));
    };

    let mut meta = RawMetadata {
        mime_type,
        ..Default::default()
    };

    if media_kind_from_mime(&meta.mime_type) == Some(MediaKind::Video) {
        tracing::debug!(
            target: crate::core::log_targets::METADATA,
            "metadata::extract path={} mime={} video=true",
            path.display(),
            meta.mime_type
        );
        // 用 ffprobe 提取视频元数据：分辨率、录制时间、时长、编码、帧率、码率、
        // 容器、拍摄设备。失败时仅保留 mime_type（详情面板仍显示名称/类型/大小）。
        if let Some((summary, recorded)) = probe_video(path) {
            meta.width = summary.width;
            meta.height = summary.height;
            meta.taken_at = recorded;
            meta.video = Some(summary);
        }
        return Ok(meta);
    }

    if meta.mime_type == "image/heic" {
        // HEIC fast path: read the file ONCE (whole — its Exif item can be
        // anywhere) and derive both dimensions and EXIF from the ISOBMFF bytes.
        // This avoids the generic branch's two costly HEIC mistakes:
        //   - `image::image_dimensions` fails (no heif feature) → falls back to
        //     `Pixbuf::from_file`, a full libheif decode (~100+ ms/file; a
        //     prebuilt C library, so --release does not help) just for width/
        //     height. The dimensions live in the `ispe` property — a few bytes.
        //   - `read_exif` would `std::fs::read` the whole file a SECOND time.
        // `head` is intentionally not used here: 256 KiB is not enough.
        let bytes = std::fs::read(path).ok();
        if let Some(data) = &bytes {
            if let Some((w, h)) = extract_heic_dims(data) {
                meta.width = Some(w);
                meta.height = Some(h);
            } else if let Ok(buf) = gdk_pixbuf::Pixbuf::from_file(path) {
                // No `ispe` (rare/malformed): last-resort decode so we still
                // record a dimension rather than none.
                meta.width = Some(buf.width() as u32);
                meta.height = Some(buf.height() as u32);
            }
            if let Ok(exif) = read_heic_exif_from_bytes(data) {
                meta.taken_at = exif_datetime(&exif);
                meta.camera = ExifSummary::from_exif(&exif);
            }
        }
    } else {
        // Standard image: dimensions + EXIF from the shared head when present,
        // falling back to a path read if the head is too short (pathological
        // JPEG whose SOF sits past 256 KiB, or an EXIF APP1 beyond the head).
        if let Some((w, h)) = dims_from(path, head) {
            meta.width = Some(w);
            meta.height = Some(h);
        }
        if let Ok(exif) = exif_from(path, head) {
            meta.taken_at = exif_datetime(&exif);
            meta.camera = ExifSummary::from_exif(&exif);
        }
    }

    tracing::debug!(
        target: crate::core::log_targets::METADATA,
        "metadata::extract path={} mime={} dims={:?}x{:?} taken_at={:?} camera_some={}",
        path.display(),
        meta.mime_type,
        meta.width,
        meta.height,
        meta.taken_at,
        meta.camera.is_some(),
    );

    Ok(meta)
}

/// Dimensions for a standard image: from the shared head when available, then a
/// streaming `image_dimensions(path)` read, then a last-resort pixbuf decode.
fn dims_from(path: &Path, head: Option<&[u8]>) -> Option<(u32, u32)> {
    if let Some(h) = head {
        if let Some(d) = image_dims_from_cursor(h) {
            return Some(d);
        }
    }
    image::image_dimensions(path)
        .ok()
        .or_else(|| pixbuf_dims(path))
}

/// EXIF for a standard image: parsed from the shared head when present, else a
/// streaming `read_exif(path)` read.
fn exif_from(path: &Path, head: Option<&[u8]>) -> Result<exif::Exif> {
    if let Some(h) = head {
        if let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(h)) {
            return Ok(exif);
        }
    }
    read_exif(path)
}

fn image_dims_from_cursor(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn pixbuf_dims(path: &Path) -> Option<(u32, u32)> {
    gdk_pixbuf::Pixbuf::from_file(path)
        .ok()
        .map(|buf| (buf.width() as u32, buf.height() as u32))
}

fn read_exif(path: &Path) -> Result<exif::Exif> {
    // Standard (non-HEIC) path: stream the file and let kamadak-exif find the
    // EXIF segment. HEIC is handled in `extract` via `read_heic_exif_from_bytes`
    // — it parses the ISOBMFF container itself (bypassing kamadak-exif's 64 KB
    // Exif-item cap, which phone HEICs with embedded thumbnails exceed) and
    // reuses the bytes already read for dimensions, so the file is read once.
    let file = std::fs::File::open(path)?;
    let mut bufreader = std::io::BufReader::new(&file);
    let exif = exif::Reader::new()
        .read_from_container(&mut bufreader)
        .map_err(|e| AppError::Exif(e.to_string()))?;
    Ok(exif)
}

/// Parse the EXIF block of a HEIC/HEIF file from already-read bytes (no extra
/// I/O). `extract` calls this so the single `std::fs::read` feeds both
/// dimensions (`extract_heic_dims`) and EXIF.
fn read_heic_exif_from_bytes(data: &[u8]) -> Result<exif::Exif> {
    let tiff = extract_heic_exif_tiff(data)
        .ok_or_else(|| AppError::Exif("no Exif item in HEIC".into()))?;
    exif::Reader::new()
        .read_raw(tiff)
        .map_err(|e| AppError::Exif(e.to_string()))
}

// ── EXIF value helpers ──────────────────────────────────────────────────────

/// Single ASCII string value for `tag` in PRIMARY IFD.
fn ascii(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Ascii(ref vec) => vec
            .first()
            .and_then(|b| std::str::from_utf8(b).ok().map(|s| s.trim().to_string())),
        _ => None,
    }
}

/// Single unsigned rational `(num, denom)` for `tag` in PRIMARY IFD.
fn rational(exif: &exif::Exif, tag: exif::Tag) -> Option<(u32, u32)> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Rational(ref vec) if !vec.is_empty() => Some((vec[0].num, vec[0].denom)),
        _ => None,
    }
}

/// Unsigned rational as `f64` (num / denom).
fn rational_as_f64(exif: &exif::Exif, tag: exif::Tag) -> Option<f64> {
    let (n, d) = rational(exif, tag)?;
    if d == 0 {
        None
    } else {
        Some(n as f64 / d as f64)
    }
}

/// Signed rational as `f64`.
fn srational_as_f64(exif: &exif::Exif, tag: exif::Tag) -> Option<f64> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::SRational(ref vec) if !vec.is_empty() => {
            let (n, d) = (vec[0].num, vec[0].denom);
            if d == 0 {
                None
            } else {
                Some(n as f64 / d as f64)
            }
        }
        _ => None,
    }
}

/// Single unsigned short/long as `u16` for `tag` in PRIMARY IFD.
fn short(exif: &exif::Exif, tag: exif::Tag) -> Option<u16> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    // `get_uint` returns `Option<u32>` regardless of the underlying type
    // (Short / Long). On EXIF-legal values this cast is lossless.
    field.value.get_uint(0).map(|v| v as u16)
}

/// Single byte value.
fn byte(exif: &exif::Exif, tag: exif::Tag) -> Option<u8> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Byte(ref vec) if !vec.is_empty() => Some(vec[0]),
        _ => None,
    }
}

/// Assemble GPS coordinates from GPSLatitude / GPSLongitude + refs.
fn gps_coord(exif: &exif::Exif) -> Option<GpsCoord> {
    let lat_ref = ascii(exif, exif::Tag::GPSLatitudeRef)?;
    let lon_ref = ascii(exif, exif::Tag::GPSLongitudeRef)?;
    let lat = gps_dms(exif, exif::Tag::GPSLatitude)?;
    let lon = gps_dms(exif, exif::Tag::GPSLongitude)?;
    Some(GpsCoord {
        lat: GpsDms {
            deg: lat.0,
            min: lat.1,
            sec: lat.2,
            north_or_east: lat_ref.starts_with('N'),
        },
        lon: GpsDms {
            deg: lon.0,
            min: lon.1,
            sec: lon.2,
            north_or_east: lon_ref.starts_with('E'),
        },
    })
}

/// Parse three Rational values as (deg, min, sec).
fn gps_dms(exif: &exif::Exif, tag: exif::Tag) -> Option<(u32, u32, f64)> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Rational(ref vec) if vec.len() >= 3 => {
            let d = vec[0].num as f64 / vec[0].denom as f64;
            let m = vec[1].num as f64 / vec[1].denom as f64;
            let s = vec[2].num as f64 / vec[2].denom as f64;
            Some((d as u32, m as u32, s))
        }
        _ => None,
    }
}

// ── HEIC/HEIF (ISOBMFF) Exif-item extraction ───────────────────────────────
//
// A HEIC file is an ISO base media file: top-level boxes (`ftyp`, `meta`,
// `mdat`, ...). The `meta` box lists items; the one whose `infe` declares
// `item_type == "Exif"` holds the EXIF data, and `iloc` says where its bytes
// live (usually inside `mdat`). The Exif item body itself is a 4-byte
// big-endian `tiff_header_offset` followed by the TIFF block (optionally with
// an `"Exif\0\0"` prefix, as Apple writes it); we strip the leading
// `4 + tiff_header_offset` bytes and return the raw TIFF for `read_raw`.

/// Body of the top-level `meta` box, with the box header stripped (so it begins
/// at the FullBox version+flags). Shared by EXIF and dimension extraction.
fn find_meta_body(data: &[u8]) -> Option<&[u8]> {
    let mut off = 0usize;
    while let Some((size, typ, hdr)) = box_header(data, off) {
        if &typ == b"meta" {
            return data.get(off + hdr..off + size);
        }
        off = match size {
            0 => break, // box extends to EOF
            _ => off.checked_add(size)?,
        };
    }
    None
}

/// Extract the raw TIFF block of the first `Exif` item in a HEIC/HEIF file,
/// or `None` if the file has no such item. Returns enough for
/// `exif::Reader::read_raw` regardless of item size (no 64 KB cap).
pub fn extract_heic_exif_tiff(data: &[u8]) -> Option<Vec<u8>> {
    let body = find_meta_body(data)?;
    parse_meta(body, data)
}

/// Primary-image dimensions of a HEIC/HEIF from the ISOBMFF `ispe` property,
/// **without decoding the image**. Walks `meta` → `pitm` (primary item id) →
/// `iprp` → (`ipco` property list, `ipma` associations) and returns the
/// `(width, height)` of the primary item's `ispe`. Returns `None` if the
/// container lacks/omits it, so callers can fall back to a decode.
pub fn extract_heic_dims(data: &[u8]) -> Option<(u32, u32)> {
    let body = find_meta_body(data)?;
    let mut cur = Cur::new(body.get(4..)?); // skip meta FullBox version+flags
    let mut primary_id: Option<u32> = None;
    let mut ipco: Option<&[u8]> = None;
    let mut ipma: Option<&[u8]> = None;
    while cur.remaining() >= 8 {
        let child = cur.box_at_start()?;
        let typ: [u8; 4] = child.get(4..8)?.try_into().ok()?;
        match &typ {
            b"pitm" => primary_id = parse_pitm(child),
            b"iprp" => {
                // iprp holds ipco (property list) + ipma (associations).
                let mut p = Cur::new(child.get(8..)?); // skip iprp box header
                while p.remaining() >= 8 {
                    let prop = p.box_at_start()?;
                    let pt: [u8; 4] = prop.get(4..8)?.try_into().ok()?;
                    match &pt {
                        b"ipco" => ipco = Some(prop),
                        b"ipma" => ipma = Some(prop),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let primary = primary_id?;
    // ipco properties are 1-based in ipma; store 0-based and subtract.
    let props = parse_ipco_properties(ipco?)?;
    let indices = parse_ipma_for_item(ipma?, primary)?;
    for idx in indices {
        let Some(prop_idx) = usize::try_from(idx).ok().and_then(|i| i.checked_sub(1)) else {
            continue; // index 0 means "no property"
        };
        if let Some(prop) = props.get(prop_idx) {
            // ispe FullBox: 8 hdr + 4 version/flags + u32 width + u32 height.
            if prop.len() >= 20 && &prop[4..8] == b"ispe" {
                let w = u32_at(prop, 12)?;
                let h = u32_at(prop, 16)?;
                return Some((w, h));
            }
        }
    }
    None
}

/// Primary item id from a `pitm` box (whole box incl. header).
fn parse_pitm(boxb: &[u8]) -> Option<u32> {
    let mut e = Cur::new(boxb.get(8..)?); // skip box header
    let (version, _) = e.fullbox()?;
    if version == 0 {
        Some(u32::from(e.u16()?))
    } else {
        e.u32()
    }
}

/// Property boxes listed in an `ipco` box, in file order. `ipma` references
/// them 1-based. Each returned slice is the whole property box incl. its header.
fn parse_ipco_properties(boxb: &[u8]) -> Option<Vec<&[u8]>> {
    let mut cur = Cur::new(boxb.get(8..)?); // skip ipco box header
    let mut props = Vec::new();
    while cur.remaining() >= 8 {
        props.push(cur.box_at_start()?);
    }
    Some(props)
}

/// 1-based property indices associated with `want_id` in an `ipma` box.
fn parse_ipma_for_item(boxb: &[u8], want_id: u32) -> Option<Vec<u32>> {
    let mut cur = Cur::new(boxb.get(8..)?); // skip box header
    let (version, flags) = cur.fullbox()?;
    let entry_count = cur.u32()? as usize;
    for _ in 0..entry_count {
        let item_id = if version < 1 {
            u32::from(cur.u16()?)
        } else {
            cur.u32()?
        };
        let assoc_count = cur.take(1)?[0] as usize;
        let want = item_id == want_id;
        let mut indices = Vec::new();
        for _ in 0..assoc_count {
            let idx = if flags & 1 != 0 {
                // 1 essential bit + 15-bit property index.
                (cur.u16()? & 0x7fff) as u32
            } else {
                // 1 essential bit + 7-bit property index.
                (cur.take(1)?[0] & 0x7f) as u32
            };
            if want {
                indices.push(idx);
            }
        }
        if want {
            return Some(indices);
        }
    }
    None
}

/// `(full box size, type, header length)` at `off`. `size==1` means a 64-bit
/// largesize follows; `size==0` means the box runs to end of file.
fn box_header(d: &[u8], off: usize) -> Option<(usize, [u8; 4], usize)> {
    let s = u32_at(d, off)? as usize;
    let typ: [u8; 4] = d.get(off + 4..off + 8)?.try_into().ok()?;
    if s == 1 {
        Some((u64_at(d, off + 8)? as usize, typ, 16))
    } else if s == 0 {
        Some((d.len() - off, typ, 8))
    } else {
        Some((s, typ, 8))
    }
}

/// Parse a `meta` box body (FullBox: 4-byte version+flags already at the front
/// of `body`). `file` is the whole file, needed to read method-0 extents.
fn parse_meta(body: &[u8], file: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cur::new(&body[4..]); // skip FullBox version+flags
    let mut idat: Option<&[u8]> = None;
    let mut exif_item_id: Option<u32> = None;
    let mut iloc: Option<&[u8]> = None;
    while cur.remaining() >= 8 {
        let child = cur.box_at_start()?;
        let typ: [u8; 4] = child.get(4..8)?.try_into().ok()?;
        match &typ {
            b"idat" => idat = Some(child.get(12..)?), // 8 box hdr + 4 fullbox
            b"iinf" => exif_item_id = parse_iinf(child),
            b"iloc" => iloc = Some(child),
            _ => {}
        }
    }
    let loc = parse_iloc(iloc?, exif_item_id?)?;
    let src = match loc.construction_method {
        0 => file,
        1 => idat?,
        _ => return None, // method 2 (item offset) not needed for Exif items
    };
    let mut buf = Vec::new();
    for (_index, offset, length) in &loc.extents {
        let start = loc.base_offset.checked_add(*offset)? as usize;
        let seg = if *length == 0 {
            src.get(start..).unwrap_or(&[])
        } else {
            src.get(start..start + *length as usize).unwrap_or(&[])
        };
        buf.extend_from_slice(seg);
    }
    if buf.len() < 4 {
        return None;
    }
    let tiff_offset = u32_at(&buf, 0)? as usize;
    if buf.len() < 4 + tiff_offset {
        return None;
    }
    Some(buf[4 + tiff_offset..].to_vec())
}

/// Find the item id of the `Exif` item in an `iinf` box (whole box incl. header).
fn parse_iinf(boxb: &[u8]) -> Option<u32> {
    let mut cur = Cur::new(boxb.get(8..)?); // skip box header
    let (version, _) = cur.fullbox()?;
    let entry_count = cur.count(version == 0)?;
    let mut exif_id = None;
    for _ in 0..entry_count {
        let infe = cur.box_at_start()?;
        let mut e = Cur::new(infe.get(8..)?);
        let (ever, _) = e.fullbox()?;
        let item_id = match ever {
            2 => u32::from(e.u16()?),
            3 => e.u32()?,
            _ => continue, // unsupported infe version: skip, can't know its size safely
        };
        let _protection = e.u16()?;
        let item_type = e.take(4)?;
        if item_type == b"Exif" {
            exif_id = Some(item_id);
        }
    }
    exif_id
}

/// Where an ISOBMFF item's bytes live, from `iloc`. `construction_method`:
/// 0 = file offset, 1 = inside the `idat` box. Each extent is
/// `(index, offset, length)`; the item data is `base_offset + offset`.
struct ItemLocation {
    construction_method: u8,
    base_offset: u64,
    extents: Vec<(u64, u64, u64)>,
}

/// Locate `want_id` in an `iloc` box. `boxb` is the whole box including header.
fn parse_iloc(boxb: &[u8], want_id: u32) -> Option<ItemLocation> {
    let mut cur = Cur::new(boxb.get(8..)?); // skip box header
    let (version, _) = cur.fullbox()?;
    let sizes = cur.u16()?;
    let offset_size = (sizes >> 12) as usize;
    let length_size = (sizes >> 8 & 0xf) as usize;
    let base_offset_size = (sizes >> 4 & 0xf) as usize;
    let index_size = if version == 1 || version == 2 {
        (sizes & 0xf) as usize
    } else {
        0
    };
    let item_count = cur.count(version < 2)?;
    for _ in 0..item_count {
        let item_id = cur.id(version < 2)?;
        let method = if version == 1 || version == 2 {
            // 12 reserved bits + 4-bit method in a u16.
            (cur.u16()? & 0xf) as u8
        } else {
            0
        };
        let _data_ref_index = cur.u16()?;
        let base_offset = cur.sized(base_offset_size)?;
        let extent_count = cur.u16()? as usize;
        // Only fully parse the extents of the item we want; skip the rest by
        // their known per-extent byte length so we stay aligned.
        let want = item_id == want_id;
        let mut extents = Vec::new();
        for _ in 0..extent_count {
            let index = cur.sized(index_size)?;
            let offset = cur.sized(offset_size)?;
            let length = cur.sized(length_size)?;
            if want {
                extents.push((index, offset, length));
            }
        }
        if want {
            return Some(ItemLocation {
                construction_method: method,
                base_offset,
                extents,
            });
        }
    }
    None
}

// ── tiny big-endian cursor over a byte slice ────────────────────────────────
struct Cur<'a> {
    b: &'a [u8],
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b }
    }

    fn remaining(&self) -> usize {
        self.b.len()
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(..n)?;
        self.b = &self.b[n..];
        Some(s)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    /// `(version, flags)` from a FullBox header.
    fn fullbox(&mut self) -> Option<(u8, u32)> {
        let v = self.u32()?;
        Some(((v >> 24) as u8, v & 0xffffff))
    }

    /// u16 (version 0) or u32 (version >= 1) count field.
    fn count(&mut self, v0: bool) -> Option<usize> {
        if v0 {
            Some(self.u16()? as usize)
        } else {
            Some(self.u32()? as usize)
        }
    }

    /// u16 or u32 item id.
    fn id(&mut self, v0: bool) -> Option<u32> {
        if v0 {
            Some(self.u16()? as u32)
        } else {
            self.u32()
        }
    }

    /// ISOBMFF size048 field: 0/4/8-byte big-endian unsigned int.
    fn sized(&mut self, size: usize) -> Option<u64> {
        match size {
            0 => Some(0),
            4 => Some(self.u32()? as u64),
            8 => self.u64(),
            _ => None,
        }
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    /// Returns the slice covering one whole child box at the current position
    /// and advances past it. The returned slice includes the box header.
    fn box_at_start(&mut self) -> Option<&'a [u8]> {
        let start_len = self.b.len();
        let s = u32::from_be_bytes(self.b.get(..4)?.try_into().ok()?) as usize;
        let (header_len, body_len): (usize, usize) = match s {
            0 => (8, start_len.checked_sub(8)?), // box runs to end of parent
            1 => {
                let big = usize::try_from(u64::from_be_bytes(self.b.get(8..16)?.try_into().ok()?))
                    .ok()?;
                (16, big.checked_sub(16)?)
            }
            _ => (8, s.checked_sub(8)?),
        };
        let total = header_len.checked_add(body_len)?;
        let slice = self.b.get(..total)?;
        self.b = &self.b[total..];
        Some(slice)
    }
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_be_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

fn exif_datetime(exif: &exif::Exif) -> Option<DateTime<Utc>> {
    // Prefer DateTimeOriginal > DateTime > DateTimeDigitized.
    for field in [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTime,
        exif::Tag::DateTimeDigitized,
    ] {
        if let Some(v) = exif.get_field(field, exif::In::PRIMARY) {
            if let exif::Value::Ascii(ref vec) = v.value {
                if let Some(s) = vec.first() {
                    if let Ok(s) = std::str::from_utf8(s) {
                        if let Some(dt) = parse_exif_datetime(s.trim()) {
                            return Some(dt);
                        }
                    }
                }
            }
        }
    }
    None
}

/// EXIF DateTime format "YYYY:MM:DD HH:MM:SS".
fn parse_exif_datetime(s: &str) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = s.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return None;
    }
    let date: Vec<&str> = parts[0].split(':').collect();
    let time: Vec<&str> = parts[1].split(':').collect();
    if date.len() != 3 || time.len() != 3 {
        return None;
    }

    let y: i32 = date[0].parse().ok()?;
    let m: u32 = date[1].parse().ok()?;
    let d: u32 = date[2].parse().ok()?;
    let h: u32 = time[0].parse().ok()?;
    let mi: u32 = time[1].parse().ok()?;
    let s: u32 = time[2].parse().ok()?;

    // EXIF has no timezone; interpret as local time, then convert to UTC.
    use chrono::Local;
    let naive = chrono::NaiveDate::from_ymd_opt(y, m, d)?.and_hms_opt(h, mi, s)?;
    let local_dt = Local.from_local_datetime(&naive).single()?;
    Some(local_dt.with_timezone(&Utc))
}

/// 调用 `ffprobe` 提取视频元数据（JSON），返回 (摘要, 录制时间)。
/// ffprobe 不可用或解析失败时返回 `None`，调用方退化为仅含 mime_type 的元数据。
fn probe_video(path: &Path) -> Option<(VideoSummary, Option<DateTime<Utc>>)> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            "probe_video: ffprobe 失败 {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let val: Value = serde_json::from_slice(&out.stdout).ok()?;
    let summary = parse_video_probe(&val);
    let recorded = video_creation_time(&val);
    Some((summary, recorded))
}

/// 从 ffprobe JSON 解析出展示用的 [`VideoSummary`]。
fn parse_video_probe(val: &Value) -> VideoSummary {
    let vstream = val
        .get("streams")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter()
                .find(|st| st.get("codec_type").and_then(Value::as_str) == Some("video"))
        });
    let fmt = val.get("format");

    let mut s = VideoSummary::default();
    if let Some(vs) = vstream {
        s.width = ju64(Some(vs), "width").map(|n| n as u32);
        s.height = ju64(Some(vs), "height").map(|n| n as u32);
        s.codec = video_codec_label(vs);
        s.fps = jstr(Some(vs), "avg_frame_rate")
            .and_then(parse_fps)
            .or_else(|| jstr(Some(vs), "r_frame_rate").and_then(parse_fps));
        s.bitrate = ju64(Some(vs), "bit_rate");
    }
    if let Some(f) = fmt {
        s.duration_secs = jf64(Some(f), "duration");
        s.container = jstr(Some(f), "format_long_name").map(str::to_string);
        s.bitrate = s.bitrate.or_else(|| ju64(Some(f), "bit_rate"));
        let tags = f.get("tags");
        s.make = tag_str(tags, "com.apple.quicktime.make").or_else(|| tag_str(tags, "make"));
        s.model = tag_str(tags, "com.apple.quicktime.model").or_else(|| tag_str(tags, "model"));
    }
    s
}

/// 友好编码标签：把 ffprobe 的 `codec_name`（h264/hevc/…）映射为常见名，并附带
/// profile（如 "HEVC (Main 10)"）。未知编码原样返回。
fn video_codec_label(vs: &Value) -> Option<String> {
    let name = vs.get("codec_name").and_then(Value::as_str)?;
    let friendly = match name {
        "h264" => "H.264",
        "hevc" => "HEVC",
        "vp8" => "VP8",
        "vp9" => "VP9",
        "av1" => "AV1",
        "mpeg4" => "MPEG-4",
        "mpeg2video" => "MPEG-2",
        "mpeg1video" => "MPEG-1",
        "wmv1" | "wmv2" | "wmv3" => "WMV",
        _ => name,
    };
    let profile = vs
        .get("profile")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty());
    match profile {
        Some(p) if !friendly.contains(p) => Some(format!("{friendly} ({p})")),
        _ => Some(friendly.to_string()),
    }
}

/// 解析 ffprobe 的帧率分数 "30000/1001" → 29.97，或纯数字 "30" → 30.0。
fn parse_fps(s: &str) -> Option<f64> {
    match s.split_once('/') {
        Some((num, den)) => {
            let n: f64 = num.trim().parse().ok()?;
            let d: f64 = den.trim().parse().ok()?;
            if d == 0.0 {
                None
            } else {
                Some(n / d)
            }
        }
        None => s.trim().parse::<f64>().ok(),
    }
}

/// 从容器 tags 读取录制时间（creation_time），解析为 UTC。
fn video_creation_time(val: &Value) -> Option<DateTime<Utc>> {
    let tags = val.get("format").and_then(|f| f.get("tags"))?;
    let raw = tags
        .get("creation_time")
        .and_then(Value::as_str)
        .or_else(|| {
            tags.get("com.apple.quicktime.creation_date")
                .and_then(Value::as_str)
        })?;
    // ffprobe 输出 ISO 8601（带时区，如 "2024-01-01T12:00:00.000000Z"）。
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// JSON 对象中取字符串字段。
fn jstr<'a>(obj: Option<&'a Value>, key: &str) -> Option<&'a str> {
    obj.and_then(|o| o.get(key))?.as_str()
}

/// JSON 对象中取无符号整数（ffprobe 常把数值写成字符串）。
fn ju64(obj: Option<&Value>, key: &str) -> Option<u64> {
    let v = obj.and_then(|o| o.get(key))?;
    v.as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| v.as_u64())
        .or_else(|| v.as_f64().map(|f| f as u64))
}

/// JSON 对象中取浮点数。
fn jf64(obj: Option<&Value>, key: &str) -> Option<f64> {
    let v = obj.and_then(|o| o.get(key))?;
    v.as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| v.as_f64())
}

/// 从 tags 对象取非空字符串字段。
fn tag_str(tags: Option<&Value>, key: &str) -> Option<String> {
    tags.and_then(|t| t.get(key))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    //! Regression coverage for HEIC EXIF extraction.
    //!
    //! Root cause this guards against: kamadak-exif's HEIF container reader
    //! caps the Exif item at `MAX_EXIF_SIZE = 65535`. Phones that embed a large
    //! JPEG thumbnail in the Exif item exceed that, so `read_from_container`
    //! returns "Exif data too large" and EXIF silently goes empty. We build a
    //! synthetic HEIC whose Exif item is deliberately oversized and prove the
    //! dedicated parser still recovers DateTimeOriginal.
    use super::*;
    use std::io::{Cursor, Seek, SeekFrom};

    /// Minimal TIFF block (little-endian) with DateTimeOriginal set.
    fn tiff_with_datetime_original(dt: &str) -> Vec<u8> {
        let field = exif::Field {
            tag: exif::Tag::DateTimeOriginal,
            ifd_num: exif::In::PRIMARY,
            value: exif::Value::Ascii(vec![dt.as_bytes().to_vec()]),
        };
        let mut writer = exif::experimental::Writer::new();
        writer.push_field(&field);
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        cursor.seek(SeekFrom::Start(0)).unwrap();
        writer.write(&mut cursor, true).unwrap();
        assert!(
            buf.starts_with(b"II*\x00"),
            "writer should emit a TIFF LE block"
        );
        buf
    }

    fn be_u16(n: u16) -> [u8; 2] {
        n.to_be_bytes()
    }
    fn be_u32(n: u32) -> [u8; 4] {
        n.to_be_bytes()
    }
    fn box_(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(8 + body.len());
        v.extend_from_slice(&be_u32(8 + body.len() as u32));
        v.extend_from_slice(typ);
        v.extend_from_slice(body);
        v
    }
    fn fullbox(typ: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
        let mut fb = Vec::with_capacity(4 + body.len());
        fb.push(version);
        fb.extend_from_slice(&[0, 0, 0]); // flags
        fb.extend_from_slice(body);
        box_(typ, &fb)
    }

    /// Build a minimal HEIC file whose single Exif item (`item_id = 1`) carries
    /// `tiff`. The Exif item body mirrors real phone output: a 4-byte
    /// `tiff_header_offset` naming an `"Exif\0\0"` prefix before the TIFF block.
    /// `mdat_offset` is where the mdat *body* (the Exif item) starts in the file.
    fn build_heic(tiff: &[u8], mdat_offset: u32, item_len: u32) -> Vec<u8> {
        // ftyp: major brand "mif1" → kamadak-exif `is_heif` returns true.
        let mut ftyp_body = Vec::new();
        ftyp_body.extend_from_slice(b"mif1"); // major brand
        ftyp_body.extend_from_slice(&be_u32(0)); // minor version
        ftyp_body.extend_from_slice(b"mif1"); // compatible brand
        let ftyp = box_(b"ftyp", &ftyp_body);

        // infe v2: item 1, type "Exif".
        let mut infe_body = Vec::new();
        infe_body.extend_from_slice(&be_u16(1)); // item_id
        infe_body.extend_from_slice(&be_u16(0)); // item_protection_index
        infe_body.extend_from_slice(b"Exif"); // item_type
        let infe = fullbox(b"infe", 2, &infe_body);

        // iinf v0: one entry.
        let mut iinf_body = Vec::new();
        iinf_body.extend_from_slice(&be_u16(1)); // entry_count
        iinf_body.extend_from_slice(&infe);
        let iinf = fullbox(b"iinf", 0, &iinf_body);

        // iloc v1: item 1, method 0, one extent at `mdat_offset` of `item_len`.
        // sizes nibbles: offset_size=4, length_size=4, base_offset_size=0, index_size=0.
        let mut iloc_body = Vec::new();
        iloc_body.extend_from_slice(&be_u16(0x4400)); // size fields
        iloc_body.extend_from_slice(&be_u16(1)); // item_count
        iloc_body.extend_from_slice(&be_u16(1)); // item_id
        iloc_body.extend_from_slice(&be_u16(0)); // construction_method (0)
        iloc_body.extend_from_slice(&be_u16(0)); // data_reference_index
                                                 // base_offset: base_offset_size=0 → zero bytes
        iloc_body.extend_from_slice(&be_u16(1)); // extent_count
        iloc_body.extend_from_slice(&be_u32(mdat_offset)); // extent offset (abs)
        iloc_body.extend_from_slice(&be_u32(item_len)); // extent length
        let iloc = fullbox(b"iloc", 1, &iloc_body);

        let mut meta_body = Vec::new();
        meta_body.extend_from_slice(&iinf);
        meta_body.extend_from_slice(&iloc);
        let meta = fullbox(b"meta", 0, &meta_body);

        // mdat body = 4-byte tiff_header_offset(=6) + "Exif\0\0" + tiff.
        let mut mdat_body = Vec::new();
        mdat_body.extend_from_slice(&be_u32(6)); // tiff_header_offset → byte 10
        mdat_body.extend_from_slice(b"Exif\0\0");
        mdat_body.extend_from_slice(tiff);
        assert_eq!(mdat_body.len() as u32, item_len);
        let mdat = box_(b"mdat", &mdat_body);

        let mut file = Vec::new();
        file.extend_from_slice(&ftyp);
        file.extend_from_slice(&meta);
        file.extend_from_slice(&mdat);
        file
    }

    /// Assemble a HEIC around `tiff`, computing the real mdat offset by measuring
    /// the file once first (meta size does not depend on the offset value).
    fn heic_around(tiff: &[u8]) -> Vec<u8> {
        let item_len = (4 + 6 + tiff.len()) as u32;
        let probe = build_heic(tiff, 0, item_len);
        // probe layout: ftyp | meta | mdat(header 8 + item_len).
        let pre_mdat = probe.len() - 8 - item_len as usize; // ftyp + meta
        let mdat_data_abs = (pre_mdat + 8) as u32; // skip mdat's 8-byte header
        build_heic(tiff, mdat_data_abs, item_len)
    }

    #[test]
    fn oversized_heic_exif_item_is_recovered() {
        // Pad the TIFF past kamadak-exif's 64 KB cap so the container reader
        // fails — this is the regression target.
        let mut tiff = tiff_with_datetime_original("2024:05:06 07:08:09");
        tiff.resize(70_000, 0); // trailing zeros: beyond IFD0 (next_ifd=0), ignored
        let heic = heic_around(&tiff);

        // OLD path: kamadak-exif rejects the >64 KB Exif item.
        let mut cur = Cursor::new(&heic);
        let old = exif::Reader::new().read_from_container(&mut cur);
        assert!(
            old.is_err(),
            "kamadak-exif should reject the oversized Exif item"
        );

        // NEW path: our parser locates it and read_raw succeeds.
        let extracted = extract_heic_exif_tiff(&heic).expect("Exif item should be located");
        assert!(
            extracted.starts_with(b"II*\x00"),
            "extracted block is TIFF LE"
        );
        let exif = exif::Reader::new()
            .read_raw(extracted)
            .expect("read_raw should parse the extracted TIFF");
        let dto = exif
            .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
            .expect("DateTimeOriginal should be present");
        assert!(
            dto.display_value()
                .with_unit(&exif)
                .to_string()
                .contains("2024-05-06 07:08:09"),
            "DateTimeOriginal value mismatch"
        );
    }

    #[test]
    fn small_heic_exif_item_also_parsed() {
        // Sanity: the dedicated path also handles normally-sized Exif items
        // (where kamadak-exif would have succeeded on its own).
        let tiff = tiff_with_datetime_original("2023:01:02 03:04:05");
        let heic = heic_around(&tiff);

        let extracted = extract_heic_exif_tiff(&heic).expect("Exif item should be located");
        let exif = exif::Reader::new()
            .read_raw(extracted)
            .expect("read_raw should parse");
        assert!(exif
            .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
            .is_some());
    }

    /// Verifies the real HEIC path against a checked-in phone export. Set
    /// `HEIC_TEST_FILE` to run the same assertion against another HEIC.
    #[test]
    fn real_heic_exif_recovers() {
        let path = std::env::var("HEIC_TEST_FILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| media_fixture_path("real_phone.heic"));
        let meta = extract(&path).expect("extract should succeed on real HEIC");
        assert_eq!(
            meta.mime_type, "image/heic",
            "fixture should exercise the HEIC metadata path"
        );
        assert!(
            meta.taken_at.is_some(),
            "DateTimeOriginal should be present"
        );
    }

    #[test]
    fn heic_dims_read_from_ispe_without_decode() {
        // Minimal ISOBMFF: a leading ftyp, then meta{ pitm(primary=1),
        // iprp{ ipco[ ispe(1280,720) ], ipma(1 -> property 1) } }. No image
        // data, no decode — proves extract_heic_dims walks the property boxes.
        fn box_with(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let size = (8 + body.len()) as u32;
            let mut b = Vec::with_capacity(8 + body.len());
            b.extend_from_slice(&size.to_be_bytes());
            b.extend_from_slice(typ);
            b.extend_from_slice(body);
            b
        }
        // ispe FullBox: version/flags(0) + u32 width + u32 height.
        let mut ispe = Vec::new();
        ispe.extend_from_slice(&0u32.to_be_bytes());
        ispe.extend_from_slice(&1280u32.to_be_bytes());
        ispe.extend_from_slice(&720u32.to_be_bytes());
        let ispe_box = box_with(b"ispe", &ispe);
        let ipco = box_with(b"ipco", &ispe_box);
        // ipma: fullbox(v0, flags=0) + entry_count=1 + item_id=1 + assoc_count=1
        // + assoc=1 (essential=0, 7-bit index=1, since flags bit0 == 0).
        let mut ipma_body = Vec::new();
        ipma_body.extend_from_slice(&0u32.to_be_bytes()); // version+flags
        ipma_body.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        ipma_body.extend_from_slice(&1u16.to_be_bytes()); // item_id
        ipma_body.push(1); // assoc_count
        ipma_body.push(1); // assoc: essential=0, index=1
        let ipma = box_with(b"ipma", &ipma_body);
        let mut iprp_body = Vec::new();
        iprp_body.extend_from_slice(&ipco);
        iprp_body.extend_from_slice(&ipma);
        let iprp = box_with(b"iprp", &iprp_body);
        let mut pitm_body = Vec::new();
        pitm_body.extend_from_slice(&0u32.to_be_bytes()); // version+flags
        pitm_body.extend_from_slice(&1u16.to_be_bytes()); // item_id
        let pitm = box_with(b"pitm", &pitm_body);
        let mut meta_body = Vec::new();
        meta_body.extend_from_slice(&0u32.to_be_bytes()); // meta fullbox version+flags
        meta_body.extend_from_slice(&pitm);
        meta_body.extend_from_slice(&iprp);
        let meta = box_with(b"meta", &meta_body);
        let ftyp = box_with(b"ftyp", b"\0\0\0\0mif1\0\0\0\0");
        let mut file = Vec::new();
        file.extend_from_slice(&ftyp);
        file.extend_from_slice(&meta);

        assert_eq!(extract_heic_dims(&file), Some((1280, 720)));
    }

    #[test]
    fn heic_dims_none_when_ispe_absent() {
        // meta with pitm + iprp{ ipco[hvc1], ipma } but NO ispe: returns None so
        // the caller can fall back to a decode instead of emitting wrong dims.
        fn box_with(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let size = (8 + body.len()) as u32;
            let mut b = Vec::with_capacity(8 + body.len());
            b.extend_from_slice(&size.to_be_bytes());
            b.extend_from_slice(typ);
            b.extend_from_slice(body);
            b
        }
        let hvc1 = box_with(b"hvc1", &[0u8; 4]);
        let ipco = box_with(b"ipco", &hvc1);
        let mut ipma_body = Vec::new();
        ipma_body.extend_from_slice(&0u32.to_be_bytes());
        ipma_body.extend_from_slice(&1u32.to_be_bytes());
        ipma_body.extend_from_slice(&1u16.to_be_bytes());
        ipma_body.push(1);
        ipma_body.push(1);
        let ipma = box_with(b"ipma", &ipma_body);
        let mut iprp_body = Vec::new();
        iprp_body.extend_from_slice(&ipco);
        iprp_body.extend_from_slice(&ipma);
        let iprp = box_with(b"iprp", &iprp_body);
        let mut pitm_body = Vec::new();
        pitm_body.extend_from_slice(&0u32.to_be_bytes());
        pitm_body.extend_from_slice(&1u16.to_be_bytes());
        let pitm = box_with(b"pitm", &pitm_body);
        let mut meta_body = Vec::new();
        meta_body.extend_from_slice(&0u32.to_be_bytes());
        meta_body.extend_from_slice(&pitm);
        meta_body.extend_from_slice(&iprp);
        let meta = box_with(b"meta", &meta_body);

        assert_eq!(extract_heic_dims(&meta), None);
    }

    /// Verifies `ffprobe`-based video metadata extraction against a checked-in
    /// phone video. Set `VIDEO_TEST_FILE` to run against another clip.
    #[test]
    fn video_metadata_extracted() {
        let path = std::env::var("VIDEO_TEST_FILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| media_fixture_path("real_phone_video.mp4"));
        let meta = extract(&path).expect("extract should succeed on a video");
        assert_eq!(
            media_kind_from_mime(&meta.mime_type),
            Some(MediaKind::Video),
            "mime should be video"
        );
        let v = meta.video.expect("VideoSummary should be populated");
        assert!(v.width.unwrap_or(0) > 0, "width should be parsed");
        assert!(v.height.unwrap_or(0) > 0, "height should be parsed");
        assert!(v.codec.is_some(), "codec should be parsed");
        assert!(
            v.duration_secs.unwrap_or(0.0) > 0.0,
            "duration should be parsed"
        );
        println!(
            "video meta: {:?}x{:?} {:?} {:.1}s fps={:?} container={:?}",
            v.width,
            v.height,
            v.codec,
            v.duration_secs.unwrap_or(0.0),
            v.fps,
            v.container
        );
    }

    fn media_fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("media")
            .join(name)
    }

    #[test]
    fn heic_without_exif_item_returns_none() {
        // A HEIC whose only item is an image tile (hvc1), not Exif.
        let mut infe_body = Vec::new();
        infe_body.extend_from_slice(&be_u16(1));
        infe_body.extend_from_slice(&be_u16(0));
        infe_body.extend_from_slice(b"hvc1");
        let infe = fullbox(b"infe", 2, &infe_body);
        let mut iinf_body = Vec::new();
        iinf_body.extend_from_slice(&be_u16(1));
        iinf_body.extend_from_slice(&infe);
        let iinf = fullbox(b"iinf", 0, &iinf_body);
        let meta = fullbox(b"meta", 0, &iinf);
        let mut ftyp_body = Vec::new();
        ftyp_body.extend_from_slice(b"mif1");
        ftyp_body.extend_from_slice(&be_u32(0));
        ftyp_body.extend_from_slice(b"mif1");
        let file = [box_(b"ftyp", &ftyp_body), meta].concat();

        assert_eq!(extract_heic_exif_tiff(&file), None);
    }

    #[test]
    fn exif_summary_from_jpeg_with_camera_fields() {
        // Build a TIFF with common camera/shooting fields the UI should show.
        use exif::experimental::Writer;
        use std::io::Cursor;

        let fields: Vec<exif::Field> = vec![
            exif::Field {
                tag: exif::Tag::Make,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"Canon".to_vec()]),
            },
            exif::Field {
                tag: exif::Tag::Model,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"EOS R5".to_vec()]),
            },
            exif::Field {
                tag: exif::Tag::FNumber,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Rational(vec![exif::Rational { num: 28, denom: 10 }]), // f/2.8
            },
            exif::Field {
                tag: exif::Tag::ExposureTime,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Rational(vec![exif::Rational { num: 1, denom: 125 }]),
            },
            exif::Field {
                tag: exif::Tag::PhotographicSensitivity,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Short(vec![400]),
            },
            exif::Field {
                tag: exif::Tag::FocalLength,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Rational(vec![exif::Rational { num: 50, denom: 1 }]),
            },
            exif::Field {
                tag: exif::Tag::DateTimeOriginal,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"2024:05:06 07:08:09".to_vec()]),
            },
        ];

        let mut writer = Writer::new();
        for f in &fields {
            writer.push_field(f);
        }
        let mut tiff = Vec::new();
        let mut cursor = Cursor::new(&mut tiff);
        cursor.seek(SeekFrom::Start(0)).unwrap();
        writer.write(&mut cursor, true).unwrap();

        let exif = exif::Reader::new().read_raw(tiff).unwrap();
        let summary = ExifSummary::from_exif(&exif).expect("should produce summary");

        assert_eq!(summary.make.as_deref(), Some("Canon"));
        assert_eq!(summary.model.as_deref(), Some("EOS R5"));
        assert!((summary.aperture.unwrap() - 2.8).abs() < 0.01);
        assert_eq!(summary.exposure_time, Some((1, 125)));
        assert_eq!(summary.iso, Some(400));
        assert!((summary.focal_length_mm.unwrap() - 50.0).abs() < 0.01);
    }

    #[test]
    fn exif_summary_empty_when_no_relevant_fields() {
        // Only DateTimeOriginal → not enough to produce a camera summary.
        let tiff = tiff_with_datetime_original("2024:05:06 07:08:09");
        let exif = exif::Reader::new().read_raw(tiff).unwrap();
        assert!(
            exif_datetime(&exif).is_some(),
            "DateTimeOriginal should still parse"
        );
        assert_eq!(ExifSummary::from_exif(&exif), None);
    }
}
