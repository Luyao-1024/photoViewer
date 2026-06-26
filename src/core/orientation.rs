use std::io::Cursor;
use std::path::Path;

use exif::{Field, In, Tag, Value};
use gdk_pixbuf::{Pixbuf, PixbufRotation};

use crate::core::error::{AppError, Result};
use crate::core::metadata::extract_heic_exif_tiff;
use crate::core::media::mime_from_extension;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const EXIF_PREFIX: &[u8] = b"Exif\0\0";

/// Read the image orientation property. Missing orientation is normal.
pub fn read_orientation(path: &Path) -> Result<u16> {
    let Some(exif) = read_exif(path)? else {
        return Ok(1);
    };
    Ok(exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .map(|v| v as u16)
        .filter(|v| (1..=8).contains(v))
        .unwrap_or(1))
}

/// Update orientation metadata by a clockwise delta. Pixel data is not decoded
/// or rewritten.
pub fn rotate_orientation_in_place(path: &Path, delta_degrees: i32) -> Result<()> {
    let current = read_orientation(path)?;
    let next = orientation_from_degrees(
        (degrees_from_orientation(current) + delta_degrees).rem_euclid(360),
    );
    write_orientation(path, next)
}

pub fn apply_orientation_to_pixbuf(pb: &Pixbuf, orientation: u16) -> Pixbuf {
    match orientation {
        2 => pb.flip(true).unwrap_or_else(|| pb.clone()),
        3 => pb
            .rotate_simple(PixbufRotation::Upsidedown)
            .unwrap_or_else(|| pb.clone()),
        4 => pb.flip(false).unwrap_or_else(|| pb.clone()),
        5 => pb
            .rotate_simple(PixbufRotation::Clockwise)
            .and_then(|p| p.flip(true))
            .unwrap_or_else(|| pb.clone()),
        6 => pb
            .rotate_simple(PixbufRotation::Clockwise)
            .unwrap_or_else(|| pb.clone()),
        7 => pb
            .rotate_simple(PixbufRotation::Counterclockwise)
            .and_then(|p| p.flip(true))
            .unwrap_or_else(|| pb.clone()),
        8 => pb
            .rotate_simple(PixbufRotation::Counterclockwise)
            .unwrap_or_else(|| pb.clone()),
        _ => pb.clone(),
    }
}

pub fn load_oriented_pixbuf(path: &Path) -> Result<Pixbuf> {
    let pb = Pixbuf::from_file(path).map_err(AppError::Gio)?;
    let orientation = read_orientation(path)?;
    Ok(apply_orientation_to_pixbuf(&pb, orientation))
}

fn degrees_from_orientation(orientation: u16) -> i32 {
    match orientation {
        3 => 180,
        6 => 90,
        8 => 270,
        _ => 0,
    }
}

fn orientation_from_degrees(degrees: i32) -> u16 {
    match degrees.rem_euclid(360) {
        90 => 6,
        180 => 3,
        270 => 8,
        _ => 1,
    }
}

fn read_exif(path: &Path) -> Result<Option<exif::Exif>> {
    let data = std::fs::read(path)?;
    if is_png(&data) {
        let Some(tiff) = find_png_exif_chunk(&data)? else {
            return Ok(None);
        };
        return exif::Reader::new()
            .read_raw(tiff)
            .map(Some)
            .map_err(|e| AppError::Exif(e.to_string()));
    }

    // HEIC/HEIF needs a dedicated path: kamadak-exif's `read_from_container`
    // *can* parse the ISOBMFF container, but caps the Exif item at
    // `MAX_EXIF_SIZE = 65535`. Camera phones (iPhone, many Androids) embed a
    // high-resolution JPEG thumbnail inside the Exif item, pushing it to several
    // hundred KB, so kamadak-exif rejects those files with "Exif data too large"
    // and EXIF silently comes back empty. We parse the container ourselves
    // (no size cap) and hand the raw TIFF block to `Reader::read_raw`.
    if mime_from_extension(path) == Some("image/heic") {
        let Some(tiff) = extract_heic_exif_tiff(&data) else {
            return Ok(None);
        };
        return exif::Reader::new()
            .read_raw(tiff)
            .map(Some)
            .map_err(|e| AppError::Exif(e.to_string()));
    }

    let mut cursor = Cursor::new(data);
    match exif::Reader::new().read_from_container(&mut cursor) {
        Ok(exif) => Ok(Some(exif)),
        Err(exif::Error::NotFound(_)) => Ok(None),
        Err(e) => Err(AppError::Exif(e.to_string())),
    }
}

fn write_orientation(path: &Path, orientation: u16) -> Result<()> {
    let data = std::fs::read(path)?;
    let existing = read_exif(path)?;
    let tiff = encode_exif(existing.as_ref(), orientation)?;
    let out = if is_png(&data) {
        write_png_exif_chunk(&data, &tiff)?
    } else if is_jpeg(&data) {
        write_jpeg_exif_segment(&data, &tiff)?
    } else {
        return Err(AppError::Exif(format!(
            "unsupported orientation metadata format: {}",
            path.display()
        )));
    };
    std::fs::write(path, out)?;
    Ok(())
}

fn encode_exif(existing: Option<&exif::Exif>, orientation: u16) -> Result<Vec<u8>> {
    let mut fields: Vec<Field> = existing
        .into_iter()
        .flat_map(|exif| exif.fields())
        .filter(|field| !(field.ifd_num == In::PRIMARY && field.tag == Tag::Orientation))
        .cloned()
        .collect();
    fields.push(Field {
        tag: Tag::Orientation,
        ifd_num: In::PRIMARY,
        value: Value::Short(vec![orientation]),
    });

    let mut writer = exif::experimental::Writer::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut out = Cursor::new(Vec::new());
    writer
        .write(
            &mut out,
            existing.map(|e| e.little_endian()).unwrap_or(false),
        )
        .map_err(|e| AppError::Exif(e.to_string()))?;
    Ok(out.into_inner())
}

fn is_jpeg(data: &[u8]) -> bool {
    data.starts_with(&[0xff, 0xd8])
}

fn is_png(data: &[u8]) -> bool {
    data.starts_with(PNG_SIGNATURE)
}

fn find_png_exif_chunk(data: &[u8]) -> Result<Option<Vec<u8>>> {
    if !is_png(data) {
        return Ok(None);
    }
    let mut pos = PNG_SIGNATURE.len();
    while pos + 12 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let typ = &data[pos + 4..pos + 8];
        let chunk_start = pos + 8;
        let chunk_end = chunk_start + len;
        let next = chunk_end + 4;
        if next > data.len() {
            return Err(AppError::Exif("truncated PNG chunk".into()));
        }
        if typ == b"eXIf" {
            return Ok(Some(data[chunk_start..chunk_end].to_vec()));
        }
        pos = next;
    }
    Ok(None)
}

fn write_png_exif_chunk(data: &[u8], tiff: &[u8]) -> Result<Vec<u8>> {
    if !is_png(data) {
        return Err(AppError::Exif("not a PNG file".into()));
    }
    let mut out = Vec::with_capacity(data.len() + tiff.len() + 12);
    out.extend_from_slice(PNG_SIGNATURE);

    let mut pos = PNG_SIGNATURE.len();
    let mut inserted = false;
    while pos + 12 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let typ = &data[pos + 4..pos + 8];
        let chunk_end = pos + 8 + len;
        let next = chunk_end + 4;
        if next > data.len() {
            return Err(AppError::Exif("truncated PNG chunk".into()));
        }
        if typ == b"eXIf" {
            pos = next;
            continue;
        }
        out.extend_from_slice(&data[pos..next]);
        if typ == b"IHDR" && !inserted {
            write_png_chunk(&mut out, b"eXIf", tiff);
            inserted = true;
        }
        pos = next;
    }
    if !inserted {
        return Err(AppError::Exif("PNG missing IHDR chunk".into()));
    }
    Ok(out)
}

fn write_png_chunk(out: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(data);
    let crc = crc32(typ, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(typ: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in typ.iter().chain(data.iter()) {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn write_jpeg_exif_segment(data: &[u8], tiff: &[u8]) -> Result<Vec<u8>> {
    if !is_jpeg(data) {
        return Err(AppError::Exif("not a JPEG file".into()));
    }
    let segment_len = EXIF_PREFIX.len() + tiff.len() + 2;
    if segment_len > u16::MAX as usize {
        return Err(AppError::Exif("EXIF segment too large for JPEG".into()));
    }

    let mut exif_segment = Vec::with_capacity(segment_len + 2);
    exif_segment.extend_from_slice(&[0xff, 0xe1]);
    exif_segment.extend_from_slice(&(segment_len as u16).to_be_bytes());
    exif_segment.extend_from_slice(EXIF_PREFIX);
    exif_segment.extend_from_slice(tiff);

    let mut out = Vec::with_capacity(data.len() + exif_segment.len());
    out.extend_from_slice(&data[..2]);
    out.extend_from_slice(&exif_segment);

    let mut pos = 2;
    while pos + 4 <= data.len() && data[pos] == 0xff {
        let marker = data[pos + 1];
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            out.extend_from_slice(&data[pos..pos + 2]);
            pos += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        if len < 2 || pos + 2 + len > data.len() {
            return Err(AppError::Exif("truncated JPEG segment".into()));
        }
        let payload = &data[pos + 4..pos + 2 + len];
        if marker == 0xe1 && payload.starts_with(EXIF_PREFIX) {
            pos += 2 + len;
            continue;
        }
        out.extend_from_slice(&data[pos..pos + 2 + len]);
        pos += 2 + len;
    }
    out.extend_from_slice(&data[pos..]);
    Ok(out)
}
