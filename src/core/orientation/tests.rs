//! Regression coverage for `read_orientation` on JPEG files whose EXIF block
//! is structurally valid in the primary IFD but has a truncated secondary IFD.
//!
//! Root cause this guards against: WeChat (and other phone galleries) re-encode
//! JPEGs and often leave a half-written tail IFD behind. `kamadak-exif`'s
//! default `read_from_container` returns `Err(InvalidFormat("Truncated IFD
//! count"))` for the whole file in that case, so the viewer previously failed
//! to open these images at original resolution.
//!
//! The display path must recover the orientation from the primary IFD and
//! fall back to ignoring the broken secondary IFD. The strict
//! `read_exif` used by the write path (`write_orientation`) must still reject
//! such files so a half-broken EXIF block is never silently overwritten.

use super::*;
use std::fs;
use std::io::Write;
use tempfile::NamedTempFile;

/// Build the TIFF block of a JPEG's APP1 EXIF segment with one valid primary
/// IFD entry (`Orientation = ORIENT`) and a `next_ifd` pointer that lands
/// past the end of the data. kamadak-exif's
/// `Parser::parse_ifd` guards its first read with
/// `data.len() < offset || data.len() - offset < 2`, so an offset beyond EOF
/// produces the exact `"Truncated IFD count"` error we want to recover from.
fn le_tiff_with_orientation_and_truncated_next_ifd(orientation: u16) -> Vec<u8> {
    let mut tiff = Vec::with_capacity(26);
    // Header: byte order, TIFF magic (42), offset of IFD0.
    tiff.extend_from_slice(b"II"); // little-endian
    tiff.extend_from_slice(&0x002Au16.to_le_bytes()); // magic 42
    tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8

    // IFD0: 1 entry.
    tiff.extend_from_slice(&1u16.to_le_bytes());
    // Orientation (tag 0x0112), SHORT (3), count=1, value = orientation.
    // The value field is always 4 bytes wide; for an in-line SHORT count=1,
    // the SHORT occupies the first 2 bytes LE and the upper 2 are padding.
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    let mut value_slot = [0u8; 4];
    value_slot[..2].copy_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&value_slot);

    // Next IFD offset — deliberate: it points past the end of the TIFF block
    // so kamadak-exif fails with "Truncated IFD count". 100 is comfortably
    // beyond the 26 bytes we've actually written so far.
    tiff.extend_from_slice(&100u32.to_le_bytes());

    tiff
}

/// Wrap a TIFF block in a minimal JPEG with SOI + APP1 + EOI. The body
/// between APP1 and EOI is empty (no SOS), so gdk-pixbuf can't decode
/// pixels — we only feed the file to `read_exif` / `read_orientation`, which
/// never touch the pixels.
fn jpeg_with_app1_exif(tiff: &[u8]) -> Vec<u8> {
    let segment_payload_len = 2 + EXIF_PREFIX.len() + tiff.len(); // length field is self-inclusive
    assert!(
        segment_payload_len <= u16::MAX as usize,
        "APP1 segment must fit in u16 length field"
    );

    let mut jpeg = Vec::new();
    jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
    jpeg.extend_from_slice(&[0xFF, 0xE1]); // APP1 marker
    jpeg.extend_from_slice(&(segment_payload_len as u16).to_be_bytes());
    jpeg.extend_from_slice(EXIF_PREFIX); // "Exif\0\0"
    jpeg.extend_from_slice(tiff);
    jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
    jpeg
}

fn write_jpeg_to_temp(jpeg: &[u8]) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create temp file");
    file.write_all(jpeg).expect("write JPEG bytes");
    file.flush().expect("flush");
    file
}

/// What `kamadak-exif`'s reader emits verbatim when the next-IFD pointer is
/// past EOF. Pinned so a future kamadak-exif upgrade that renames the format
/// string shows up here rather than silently changing our behaviour.
const TRUNCATED_IFD_COUNT_MESSAGE: &str = "Truncated IFD count";

#[test]
fn read_orientation_recovers_from_truncated_secondary_ifd() {
    let tiff = le_tiff_with_orientation_and_truncated_next_ifd(6);
    let jpeg = jpeg_with_app1_exif(&tiff);
    let file = write_jpeg_to_temp(&jpeg);
    let path = file.path().to_path_buf();

    // Strict path: the file's TIFF block has a truncated secondary IFD, so
    // kamadak-exif returns `Err(InvalidFormat("Truncated IFD count"))`.
    // `orientation::read_exif` propagates that as `AppError::Exif(...)`.
    let strict_err = read_exif(&path)
        .err()
        .expect("strict read_exif must fail on truncated tail IFD");
    let strict_msg = match strict_err {
        AppError::Exif(msg) => msg,
        other => panic!("strict read_exif expected Err(AppError::Exif(_)), got {other:?}"),
    };
    assert!(
        strict_msg.contains(TRUNCATED_IFD_COUNT_MESSAGE),
        "strict path should surface the truncated secondary IFD; got: {strict_msg}"
    );

    // Display path: `read_orientation` must recover Orientation = 6 from the
    // intact primary IFD despite the broken tail. Without the soft wrapper
    // this returns `Err(AppError::Exif(_))` and the viewer shows a blank stage.
    let recovered =
        read_orientation(&path).expect("read_orientation should not fail on recoverable EXIF");
    assert_eq!(
        recovered, 6,
        "primary IFD Orientation should survive a truncated tail IFD"
    );
}

#[test]
fn read_orientation_handles_well_formed_orientation_3() {
    // Single Orientation=3 (180°) entry, no next-IFD pointer (offset 0).
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&0x002Au16.to_le_bytes());
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    let mut value_slot = [0u8; 4];
    value_slot[..2].copy_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&value_slot);
    tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD

    let file = write_jpeg_to_temp(&jpeg_with_app1_exif(&tiff));
    let got = read_orientation(file.path()).expect("well-formed EXIF must decode");
    assert_eq!(got, 3);
}

#[test]
fn read_orientation_returns_one_for_non_jpeg_file() {
    // No SOI: kamadak-exif returns `NotFound` for this case, which the
    // existing arms already map to `Ok(None)` → `read_orientation` → `Ok(1)`.
    let mut file = NamedTempFile::new().expect("create temp file");
    file.write_all(b"not really a JPEG, just text bytes")
        .expect("write text");
    file.flush().expect("flush");

    let got = read_orientation(file.path()).expect("non-image should not error");
    assert_eq!(got, 1);
}

// Sanity: the lenient-recovery wiring lives in `core::metadata`
// (`lenient_exif_reader` opts into kamadak-exif's `continue_on_error`), and
// the orientation display path must delegate to it rather than roll its own
// strict reader. Double-check the source so a refactor that silently drops the
// delegation shows up here.
#[test]
fn display_path_delegates_to_shared_lenient_exif_reader() {
    // Anchor against the manifest dir so this works whether `cargo test` is
    // invoked from the repo root or somewhere else.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let base = std::path::Path::new(manifest_dir);

    let metadata_src =
        fs::read_to_string(base.join("src/core/metadata.rs")).expect("read metadata.rs");
    assert!(
        metadata_src.contains("continue_on_error"),
        "shared lenient EXIF reader in metadata.rs must opt into kamadak-exif's partial-result recovery"
    );

    let orientation_src =
        fs::read_to_string(base.join("src/core/orientation.rs")).expect("read orientation.rs");
    assert!(
        orientation_src.contains("lenient_exif_reader"),
        "orientation's display path must delegate to the shared lenient reader"
    );
}
