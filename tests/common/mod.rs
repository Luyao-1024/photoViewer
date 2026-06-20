//! Shared test fixtures: generate test images (with or without EXIF).
use image::{ImageBuffer, Rgb};
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