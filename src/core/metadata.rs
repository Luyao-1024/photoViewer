//! Image metadata extraction: dimensions, EXIF DateTimeOriginal, MIME type.
use crate::core::error::{AppError, Result};
use chrono::{DateTime, TimeZone, Utc};
use gdk_pixbuf;
use std::path::Path;

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

#[derive(Debug, Clone, Default)]
pub struct RawMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub mime_type: String,
    pub camera: Option<ExifSummary>,
}

/// Extract metadata from a file at `path`.
pub fn extract(path: &Path) -> Result<RawMetadata> {
    let mime_type = mime_from_extension(path);
    if mime_type.is_empty() {
        return Err(AppError::Decode(format!(
            "unknown extension: {}",
            path.display()
        )));
    }

    let mut meta = RawMetadata {
        mime_type,
        ..Default::default()
    };

    // 1. Read file header to get dimensions (gdk-pixbuf handles JPEG/PNG/WebP,
    //    with image::image_dimensions as a fallback that does not upscale).
    //    HEIC/HEIF may fail here when libheif isn't available via gdk-pixbuf
    //    loaders (e.g. outside Flatpak); dimension failure is non-fatal — we
    //    still want EXIF and other metadata.
    if let Ok(dim) = image::image_dimensions(path) {
        meta.width = Some(dim.0);
        meta.height = Some(dim.1);
    } else if let Ok(buf) = gdk_pixbuf::Pixbuf::from_file(path) {
        meta.width = Some(buf.width() as u32);
        meta.height = Some(buf.height() as u32);
    }

    // 2. Try to read EXIF DateTimeOriginal + a curated camera summary.
    if let Ok(exif) = read_exif(path) {
        meta.taken_at = exif_datetime(&exif);
        meta.camera = ExifSummary::from_exif(&exif);
    }

    tracing::info!(
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

fn read_exif(path: &Path) -> Result<exif::Exif> {
    // HEIC/HEIF needs a dedicated path: kamadak-exif's `read_from_container`
    // *can* parse the ISOBMFF container, but caps the Exif item at
    // `MAX_EXIF_SIZE = 65535`. Camera phones (iPhone, many Androids) embed a
    // high-resolution JPEG thumbnail inside the Exif item, pushing it to several
    // hundred KB, so kamadak-exif rejects those files with "Exif data too large"
    // and EXIF silently comes back empty. We parse the container ourselves
    // (no size cap) and hand the raw TIFF block to `Reader::read_raw`. See
    // `extract_heic_exif_tiff`.
    if mime_from_extension(path) == "image/heic" {
        tracing::info!("read_exif: HEIC path detected, parsing ISOBMFF...");
        let data = std::fs::read(path)?;
        let tiff = extract_heic_exif_tiff(&data)
            .ok_or_else(|| AppError::Exif("no Exif item in HEIC".into()))?;
        tracing::info!("read_exif: HEIC tiff extracted, {} bytes", tiff.len());
        let exif = exif::Reader::new()
            .read_raw(tiff)
            .map_err(|e| AppError::Exif(e.to_string()))?;
        let field_count = exif.fields().count();
        tracing::info!("read_exif: HEIC parsed OK, {} EXIF fields", field_count);
        return Ok(exif);
    }

    let file = std::fs::File::open(path)?;
    let mut bufreader = std::io::BufReader::new(&file);
    let exif = exif::Reader::new()
        .read_from_container(&mut bufreader)
        .map_err(|e| AppError::Exif(e.to_string()))?;
    tracing::info!(
        "read_exif: standard path OK, {} fields",
        exif.fields().count()
    );
    Ok(exif)
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

/// Extract the raw TIFF block of the first `Exif` item in a HEIC/HEIF file,
/// or `None` if the file has no such item. Returns enough for
/// `exif::Reader::read_raw` regardless of item size (no 64 KB cap).
fn extract_heic_exif_tiff(data: &[u8]) -> Option<Vec<u8>> {
    // Walk top-level boxes to the `meta` box.
    let mut off = 0usize;
    while let Some((size, typ, hdr)) = box_header(data, off) {
        if &typ == b"meta" {
            let body = data.get(off + hdr..off + size)?;
            return parse_meta(body, data);
        }
        off = match size {
            0 => break, // box extends to EOF
            _ => off.checked_add(size)?,
        };
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

fn mime_from_extension(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => "image/jpeg".into(),
            "png" => "image/png".into(),
            "webp" => "image/webp".into(),
            "heic" | "heif" => "image/heic".into(),
            _ => String::new(),
        },
        None => String::new(),
    }
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

    /// Manual verification against a real phone HEIC. Ignored by default.
    /// Point it at any HEIC on your machine, e.g. an iPhone/Android export:
    ///   HEIC_TEST_FILE=/path/to/IMG.heic \
    ///     cargo test --lib core::metadata::tests::real_heic_exif_recovers -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_heic_exif_recovers() {
        let path = match std::env::var("HEIC_TEST_FILE") {
            Ok(p) => std::path::PathBuf::from(p),
            Err(_) => {
                eprintln!("set HEIC_TEST_FILE to a .heic path; skipping");
                return;
            }
        };
        let exif = read_exif(&path).expect("read_exif should succeed on real HEIC");
        // The old path returned Err here; now DateTimeOriginal must parse:
        assert!(
            exif_datetime(&exif).is_some(),
            "DateTimeOriginal should be present"
        );
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
