//! Regression coverage for HEIC EXIF extraction.
//!
//! Root cause this guards against: kamadak-exif's HEIF container reader
//! caps the Exif item at `MAX_EXIF_SIZE = 65535`. Phones that embed a large
//! JPEG thumbnail in the Exif item exceed that, so `read_from_container`
//! returns "Exif data too large" and EXIF silently goes empty. We build a
//! synthetic HEIC whose Exif item is deliberately oversized and prove the
//! dedicated parser still recovers DateTimeOriginal.
use super::*;
use std::io::{Cursor, Seek, SeekFrom, Write};
use tempfile::NamedTempFile;

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

// ── lenient EXIF recovery for the details-panel / scanner path ─────────────
// Same root cause as the viewer (see `core::orientation::tests`): a JPEG whose
// EXIF has an intact primary IFD but a truncated secondary (thumbnail) IFD.
// `metadata::extract` feeds these files to the details panel and the scanner's
// `taken_at`, so it must recover the primary-IFD fields (here DateTimeOriginal)
// instead of dropping them. Before the lenient reader, `read_exif` returned
// `Err(InvalidFormat("Truncated IFD count"))` and `taken_at` came back empty.

/// Little-endian TIFF block: one primary-IFD entry (DateTime, 0x0132 — a
/// Tiff-context tag that lives in IFD0, unlike DateTimeOriginal which is an
/// Exif sub-IFD tag) plus a `next_ifd` pointer past EOF, so kamadak-exif's
/// strict reader fails with "Truncated IFD count" while the primary IFD still
/// parses cleanly.
fn le_tiff_with_datetime_and_truncated_next_ifd(dt: &str) -> Vec<u8> {
    let mut ascii = dt.as_bytes().to_vec();
    ascii.push(0); // EXIF ASCII NUL terminator
    let ascii_len = ascii.len() as u32;
    assert!(
        ascii_len > 4,
        "value must be offset-stored to exercise the pointer path"
    );

    let mut tiff = Vec::new();
    // Header: "II", magic 42, IFD0 at offset 8.
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&0x002Au16.to_le_bytes());
    tiff.extend_from_slice(&8u32.to_le_bytes());
    // IFD0: 1 entry.
    tiff.extend_from_slice(&1u16.to_le_bytes());
    // DateTime (0x0132), ASCII (2), count, offset = 26
    // (header 8 + count 2 + entry 12 + next_ifd 4).
    tiff.extend_from_slice(&0x0132u16.to_le_bytes());
    tiff.extend_from_slice(&2u16.to_le_bytes());
    tiff.extend_from_slice(&ascii_len.to_le_bytes());
    tiff.extend_from_slice(&26u32.to_le_bytes());
    // next_ifd — points past EOF (total length = 26 + ascii_len) so the strict
    // reader rejects with "Truncated IFD count".
    tiff.extend_from_slice(&100u32.to_le_bytes());
    // ASCII payload at offset 26.
    tiff.extend_from_slice(&ascii);
    tiff
}

/// Wrap a TIFF block in a minimal JPEG (SOI + APP1 + EOI) the way phone
/// galleries do. No pixel body — `metadata` never decodes pixels here.
fn jpeg_with_app1_exif(tiff: &[u8]) -> Vec<u8> {
    const EXIF_PREFIX: &[u8; 6] = b"Exif\0\0";
    let segment_len = 2 + EXIF_PREFIX.len() + tiff.len(); // length field is self-inclusive
    assert!(segment_len <= u16::MAX as usize);

    let mut jpeg = Vec::new();
    jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
    jpeg.extend_from_slice(&[0xFF, 0xE1]); // APP1
    jpeg.extend_from_slice(&(segment_len as u16).to_be_bytes());
    jpeg.extend_from_slice(EXIF_PREFIX);
    jpeg.extend_from_slice(tiff);
    jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
    jpeg
}

#[test]
fn exif_from_recovers_datetime_from_truncated_secondary_ifd() {
    let tiff = le_tiff_with_datetime_and_truncated_next_ifd("2024:05:06 07:08:09");
    let jpeg = jpeg_with_app1_exif(&tiff);
    let mut file = NamedTempFile::with_suffix(".jpg").expect("create temp .jpg");
    file.write_all(&jpeg).expect("write JPEG");
    file.flush().expect("flush");
    let path = file.path();

    // Strict read reproduces the original failure.
    let strict_err = {
        let mut f = std::io::BufReader::new(std::fs::File::open(path).expect("open temp file"));
        exif::Reader::new()
            .read_from_container(&mut f)
            .err()
            .expect("strict reader must fail on truncated tail IFD")
    };
    assert!(
        strict_err.to_string().contains("Truncated IFD count"),
        "strict path should surface the truncated secondary IFD; got: {strict_err}"
    );

    // Lenient path (used by `extract` → details panel + scanner `taken_at`)
    // recovers DateTime from the intact primary IFD, both via the shared-head
    // fast path and the streaming fallback.
    let from_head = exif_from(path, Some(&jpeg)).expect("head path should recover partial EXIF");
    assert!(
        exif_datetime(&from_head).is_some(),
        "DateTime must survive a truncated tail IFD (head path)"
    );

    let streamed = exif_from(path, None).expect("streaming path should recover partial EXIF");
    assert!(
        exif_datetime(&streamed).is_some(),
        "DateTime must survive a truncated tail IFD (streaming path)"
    );
}

/// Build a TIFF block (little-endian) carrying an IFD1 JPEG thumbnail and the
/// given primary-IFD orientation, mirroring how phone cameras embed a low-res
/// preview. The thumbnail payload is arbitrary bytes here — the offset/length
/// extraction is what's under test, not the JPEG decode.
fn tiff_with_jpeg_thumb(thumb: &[u8], orientation: u16) -> Vec<u8> {
    let orientation_field = exif::Field {
        tag: exif::Tag::Orientation,
        ifd_num: exif::In::PRIMARY,
        value: exif::Value::Short(vec![orientation]),
    };
    let mut writer = exif::experimental::Writer::new();
    writer.push_field(&orientation_field);
    writer.set_jpeg(thumb, exif::In::THUMBNAIL);
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

#[test]
fn exif_thumbnail_helper_finds_embedded_jpeg_thumb_and_orientation() {
    let thumb = b"JPEG-BYTES";
    let tiff = tiff_with_jpeg_thumb(thumb, 6);

    let (found, orientation) = super::exif_thumbnail_jpeg_and_orientation(&tiff)
        .expect("IFD1 JPEG thumbnail must be located");
    assert_eq!(
        found, thumb,
        "the exact embedded thumb bytes must be sliced"
    );
    assert_eq!(orientation, 6, "primary-IFD orientation must be read back");
}

#[test]
fn exif_thumbnail_helper_returns_none_without_ifd1_thumb() {
    // Primary-only TIFF: no THUMBNAIL IFD, so no JPEGInterchangeFormat.
    let field = exif::Field {
        tag: exif::Tag::ImageDescription,
        ifd_num: exif::In::PRIMARY,
        value: exif::Value::Ascii(vec![b"no thumb".to_vec()]),
    };
    let mut writer = exif::experimental::Writer::new();
    writer.push_field(&field);
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);
    cursor.seek(SeekFrom::Start(0)).unwrap();
    writer.write(&mut cursor, true).unwrap();

    assert!(
        super::exif_thumbnail_jpeg_and_orientation(&buf).is_none(),
        "a TIFF without an IFD1 JPEG thumb must yield None"
    );
}

#[test]
fn extract_exif_thumbnail_returns_none_for_non_jpeg_extension() {
    // The mime guard short-circuits before any read, so a `.png` path with no
    // real image content is enough to prove non-JPEG files are skipped.
    let png = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .expect("create temp png");
    assert!(extract_exif_thumbnail(png.path()).is_none());
}
