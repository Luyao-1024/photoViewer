//! Shared test fixtures: generate test images (with or without EXIF).
use image::{ImageBuffer, Rgb};
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use tempfile::TempDir;

pub fn tmp_dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// Generate a solid-color JPEG test image (no EXIF).
pub fn write_plain_jpeg(dir: &std::path::Path, name: &str) -> PathBuf {
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| Rgb([128, 128, 128]));
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

/// Generate a JPEG with EXIF DateTimeOriginal set to `naive` (interpreted as local time).
pub fn write_jpeg_with_exif(
    dir: &std::path::Path,
    name: &str,
    naive: chrono::NaiveDateTime,
) -> PathBuf {
    // 1) Create a base JPEG.
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| Rgb([128, 128, 128]));
    let mut jpeg_bytes: Vec<u8> = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut jpeg_bytes), image::ImageFormat::Jpeg)
        .unwrap();

    // 2) Build the EXIF APP1 segment using kamadak-exif's Writer.
    let exif_dt = naive.format("%Y:%m:%d %H:%M:%S").to_string();
    let field = exif::Field {
        tag: exif::Tag::DateTimeOriginal,
        ifd_num: exif::In::PRIMARY,
        value: exif::Value::Ascii(vec![exif_dt.into_bytes()]),
    };
    let mut writer = exif::experimental::Writer::new();
    writer.push_field(&field);
    let mut exif_buf: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut exif_buf);
    cursor.seek(SeekFrom::Start(0)).unwrap();
    writer.write(&mut cursor, true).unwrap();
    // exif_buf now starts with TIFF_LE_SIG (II*\0). For an APP1 segment we need
    // the EXIF identifier "Exif\0\0" right after the segment length.
    // The TIFF/IFD bytes start at offset 0 of the writer output, so the
    // APP1 payload is: "Exif\0\0" + tiff_block.
    let mut app1_payload: Vec<u8> = Vec::with_capacity(6 + exif_buf.len());
    app1_payload.extend_from_slice(b"Exif\0\0");
    app1_payload.extend_from_slice(&exif_buf);
    let app1_len = (app1_payload.len() + 2) as u16; // includes the 2 length bytes

    // 3) Insert APP1 right after the SOI marker (0xFFD8) of the base JPEG.
    let mut out: Vec<u8> = Vec::with_capacity(jpeg_bytes.len() + 2 + 2 + app1_payload.len());
    out.extend_from_slice(&jpeg_bytes[..2]); // SOI
    out.extend_from_slice(&[0xFF, 0xE1]); // APP1 marker
    out.extend_from_slice(&app1_len.to_be_bytes());
    out.extend_from_slice(&app1_payload);
    out.extend_from_slice(&jpeg_bytes[2..]);

    let path = dir.join(name);
    std::fs::write(&path, &out).unwrap();
    path
}