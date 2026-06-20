//! Image metadata extraction: dimensions, EXIF DateTimeOriginal, MIME type.
use crate::core::error::{AppError, Result};
use chrono::{DateTime, TimeZone, Utc};
use gdk_pixbuf;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct RawMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub mime_type: String,
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
    if let Ok(dim) = image::image_dimensions(path) {
        meta.width = Some(dim.0);
        meta.height = Some(dim.1);
    } else if let Ok(buf) = gdk_pixbuf::Pixbuf::from_file(path) {
        meta.width = Some(buf.width() as u32);
        meta.height = Some(buf.height() as u32);
    } else {
        return Err(AppError::Decode(format!(
            "cannot decode dimensions: {}",
            path.display()
        )));
    }

    // 2. Try to read EXIF DateTimeOriginal.
    if let Ok(exif) = exif_reader(path) {
        meta.taken_at = Some(exif);
    }

    Ok(meta)
}

fn exif_reader(path: &Path) -> Result<DateTime<Utc>> {
    let file = std::fs::File::open(path)?;
    let mut bufreader = std::io::BufReader::new(&file);
    let exif = exif::Reader::new()
        .read_from_container(&mut bufreader)
        .map_err(|e| AppError::Exif(e.to_string()))?;

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
                            return Ok(dt);
                        }
                    }
                }
            }
        }
    }
    Err(AppError::Exif("no datetime field".into()))
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