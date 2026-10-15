//! Shared test fixtures: generate test images (with or without EXIF).
//! Each test binary is compiled separately, so individual helpers may appear
//! unused to the dead-code lint in some binaries even though they are used in
//! others. Allow the warning at module level.
#![allow(dead_code)]
pub mod credentials;
pub mod db;
pub mod interaction;
pub mod shell;
pub mod sidebar;
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

/// Generate a solid-color PNG test image (no EXIF). Used by tests that need a
/// non-JPEG image (e.g. Chinese-named folder roundtrip via notify watcher).
pub fn write_plain_png(dir: &std::path::Path, name: &str) -> PathBuf {
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| Rgb([128, 128, 128]));
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

/// Overwrite an existing file at `path` with a different-sized, different-colored
/// JPEG so its bytes differ (triggering a notify Modify event) while remaining a
/// valid, decodable image for `metadata::extract` / `upsert_from_path`.
pub fn write_distinct_jpeg(path: &std::path::Path, w: u32, h: u32, color: [u8; 3]) {
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(w, h, |_, _| Rgb(color));
    img.save(path).unwrap();
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
    img.write_to(
        &mut std::io::Cursor::new(&mut jpeg_bytes),
        image::ImageFormat::Jpeg,
    )
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

/// A capability scope shaped like a real M4 server's, for job fixtures.
///
/// Job creation validates the pinned document, so a fixture has to supply a
/// complete one. Only the fields that validation reads are meaningful here.
pub fn pinned_scope(base_url: &str, zones: &[&str]) -> photo_viewer::core::sync::NativePinnedScope {
    use photo_viewer::core::sync::{
        NativeCapabilityBits, NativeLimits, NativeNamespace, NativePinnedScope, NativeSyncLimits,
    };
    NativePinnedScope {
        authentication: None,
        public_base_url: base_url.into(),
        namespace: NativeNamespace {
            tenant: "local".into(),
            container: "com.apple.photos.cloud".into(),
            environment: "production".into(),
            database: "private".into(),
        },
        database_generation: "501855bb-18d3-4008-abc3-37e4948bf6f0".into(),
        protocol_version: 1,
        service_version: "0.1.0".into(),
        stage: "M4".into(),
        checksum_algorithm: "SHA256".into(),
        zones: zones.iter().map(|zone| (*zone).to_string()).collect(),
        capabilities: NativeCapabilityBits {
            zone_listing: true,
            record_crud: true,
            snapshots: true,
            incremental_changes: true,
            upload_leases: true,
            fencing: true,
            token_current_gate: true,
            upload_phases: true,
            operation_status_read: true,
            asset_upload: true,
            asset_download: true,
            photo_publication: true,
            query_batch: false,
            change_aliases: false,
            apple_authentication: false,
            sharing: false,
            upload_resume: false,
        },
        limits: NativeLimits {
            max_asset_bytes: 536_870_912,
            thumbnail_max_bytes: 2_097_152,
            upload_concurrency: 2,
            download_concurrency: 8,
            upload_idle_timeout_secs: 30,
            upload_total_timeout_secs: 3600,
            db_queue_capacity: 128,
            json_body_limit: 1_048_576,
            lease_ttl_secs: 90,
            lease_heartbeat_secs: 30,
            control_queue_capacity: 16,
        },
        sync_limits: NativeSyncLimits {
            max_page: 200,
            max_zones: 20,
        },
        record_limits: Default::default(),
    }
}
