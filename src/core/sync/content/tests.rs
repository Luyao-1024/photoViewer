use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "photoviewer-content-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp dir");
    root
}

/// A real JPEG, so the thumbnail path is exercised against bytes the decoder
/// actually accepts rather than a fabricated header.
fn write_jpeg(path: &Path) -> Vec<u8> {
    let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 64, 48)
        .expect("build fixture");
    image.fill(0x3366CC);
    let bytes = image
        .save_to_bufferv("jpeg", &[("quality", "90")])
        .expect("encode fixture");
    std::fs::write(path, &bytes).expect("write fixture");
    bytes
}

#[test]
fn a_digest_is_stated_the_way_each_field_expects() {
    let digest = ContentDigest::of(b"hello");
    assert_eq!(digest.size, 5);
    // hex, as a reservation's expectedSha256 and the photo field require
    assert_eq!(
        digest.hex,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert!(digest.hex.chars().all(|c| !c.is_ascii_uppercase()));
    // base64, as a descriptor's fileChecksum carries it
    assert_eq!(
        digest.base64,
        "LPJNul+wow4m6DsqxbninhsWHlwfp0JecwQzYpOLmCQ="
    );
    // The two describe one digest, not two.
    assert_eq!(
        ContentDigest::hex_from_descriptor_base64(&digest.base64).as_deref(),
        Some(digest.hex.as_str())
    );
}

#[test]
fn an_empty_file_still_digests() {
    let digest = ContentDigest::of(b"");
    assert_eq!(digest.size, 0);
    assert_eq!(
        digest.hex,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn the_base64_encoder_matches_the_services_padded_form() {
    for length in 0..40usize {
        let bytes: Vec<u8> = (0..length).map(|index| index as u8).collect();
        let encoded = to_base64(&bytes);
        assert_eq!(encoded.len() % 4, 0, "length {length}: {encoded}");
        assert_eq!(
            from_base64(&encoded).unwrap_or_default(),
            bytes,
            "length {length}"
        );
    }
    assert!(to_base64(&[0u8; 32]).ends_with('='), "32 bytes must pad");
}

#[test]
fn a_snapshot_copies_the_bytes_and_digests_the_copy() {
    let dir = temp_dir("snapshot");
    let source = dir.join("source.jpg");
    let bytes = write_jpeg(&source);
    let staging = dir.join("staging");

    let snapshot = snapshot_source(&source, &staging, "source", 10 * 1024 * 1024).unwrap();
    assert_ne!(snapshot.path, source, "the upload must read a copy");
    assert_eq!(snapshot.digest.size, bytes.len() as u64);
    assert_eq!(snapshot.digest, ContentDigest::of(&bytes));
    assert_eq!(snapshot.read().unwrap(), bytes);
    // The copy lives in this application's reserved namespace, so a later scan
    // cannot mistake it for user media.
    let name = snapshot
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap()
        .to_string();
    assert!(
        crate::core::sync::remote_path::is_internal_sync_name(Path::new(&name)),
        "{name}"
    );
}

#[test]
fn a_snapshot_is_stable_when_the_source_changes_underneath_it() {
    // This is the whole reason the bytes are copied: an upload that read the
    // source twice — once to hash, once to send — would send content that no
    // longer matches the digest it declared.
    let dir = temp_dir("stable");
    let source = dir.join("source.jpg");
    write_jpeg(&source);
    let staging = dir.join("staging");
    let snapshot = snapshot_source(&source, &staging, "source", 10 * 1024 * 1024).unwrap();
    let before = snapshot.read().unwrap();

    write_jpeg(&source);
    let after = snapshot.read().unwrap();
    assert_eq!(before, after, "the staged copy must not follow the source");
    assert_eq!(snapshot.digest, ContentDigest::of(&before));
}

#[test]
fn a_snapshot_refuses_an_oversized_source_and_leaves_nothing_staged() {
    let dir = temp_dir("oversized");
    let source = dir.join("big.jpg");
    std::fs::write(&source, vec![7u8; 4096]).unwrap();
    let staging = dir.join("staging");

    let error = snapshot_source(&source, &staging, "big", 1024).unwrap_err();
    assert!(error.to_string().contains("1024"), "{error}");
    // A refused copy is not left behind: a partial stage would look like a
    // usable upload to a later cycle.
    let leftovers: Vec<_> = std::fs::read_dir(&staging)
        .map(|entries| entries.flatten().collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_missing_source_is_refused_before_anything_is_created() {
    let dir = temp_dir("missing");
    let staging = dir.join("staging");
    let error = snapshot_source(&dir.join("absent.jpg"), &staging, "absent", 1024).unwrap_err();
    assert!(error.to_string().contains("not a readable file"), "{error}");
}

#[test]
fn a_thumbnail_is_generated_from_the_staged_copy_and_bounded() {
    let dir = temp_dir("thumbnail");
    let source = dir.join("source.jpg");
    let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 1600, 1200)
        .expect("build fixture");
    image.fill(0x99AA55);
    std::fs::write(&source, image.save_to_bufferv("jpeg", &[]).unwrap()).unwrap();

    let snapshot =
        snapshot_source(&source, &dir.join("staging"), "source", 10 * 1024 * 1024).unwrap();
    let (bytes, digest) = thumbnail_from(&snapshot, 2048, 2 * 1024 * 1024).unwrap();

    assert!(!bytes.is_empty());
    assert_eq!(digest, ContentDigest::of(&bytes));
    assert!(digest.size <= 2 * 1024 * 1024);

    // Decodable, and within the long-edge bound the service enforces.
    let glib_bytes = glib::Bytes::from(&bytes);
    let stream = gtk4::gio::MemoryInputStream::from_bytes(&glib_bytes);
    let decoded = gdk_pixbuf::Pixbuf::from_stream(&stream, None::<&gtk4::gio::Cancellable>)
        .expect("the thumbnail must decode");
    assert!(decoded.width() <= 2048 && decoded.height() <= 2048);
}

#[test]
fn a_thumbnail_stays_under_a_tight_byte_ceiling() {
    let dir = temp_dir("tight");
    let source = dir.join("noise.jpg");
    // Noise is the hard case: it compresses badly, so a naive encoder would
    // blow a small ceiling instead of shrinking.
    let mut rgb = vec![0u8; 400 * 400 * 3];
    for index in 0..400 * 400 {
        rgb[index * 3] = ((index * 31) % 251) as u8;
        rgb[index * 3 + 1] = ((index * 17) % 253) as u8;
        rgb[index * 3 + 2] = ((index * 11) % 247) as u8;
    }
    let rgb_image = gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from_owned(rgb),
        gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        400,
        400,
        400 * 3,
    );
    std::fs::write(&source, rgb_image.save_to_bufferv("jpeg", &[]).unwrap()).unwrap();

    let snapshot =
        snapshot_source(&source, &dir.join("staging"), "noise", 10 * 1024 * 1024).unwrap();
    // First measure what an unconstrained render costs, then set a ceiling
    // below it so only shrinking can meet the bound.
    let (unbounded, _) = thumbnail_from(&snapshot, 512, 64 * 1024 * 1024).unwrap();
    let ceiling = unbounded.len() as u64 / 2;
    assert!(ceiling > 0, "the fixture must produce something to shrink");

    let (bytes, digest) = thumbnail_from(&snapshot, 512, ceiling).unwrap();
    assert!(
        digest.size <= ceiling,
        "the ceiling must be honoured: {} > {ceiling}",
        digest.size
    );
    assert!(
        bytes.len() < unbounded.len(),
        "meeting the ceiling must have cost something"
    );
    assert_eq!(digest, ContentDigest::of(&bytes));

    // And a ceiling nothing can meet is reported, never looped on.
    assert!(thumbnail_from(&snapshot, 512, 64).is_err());
}

#[test]
fn a_source_that_is_not_an_image_refuses_to_produce_a_thumbnail() {
    let dir = temp_dir("not-an-image");
    let source = dir.join("notes.jpg");
    std::fs::write(&source, b"this is not an image at all").unwrap();
    let snapshot = snapshot_source(&source, &dir.join("staging"), "notes", 1024 * 1024).unwrap();
    // The original still digests — a failed thumbnail must not lose the fact
    // that these exact bytes exist.
    let payload = b"this is not an image at all";
    assert_eq!(snapshot.digest, ContentDigest::of(payload));
    assert!(thumbnail_from(&snapshot, 512, 1024 * 1024).is_err());
}
