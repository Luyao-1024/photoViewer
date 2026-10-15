//! Stable content for one upload: a snapshot, its digests, and its thumbnail.
//!
//! The service needs the same digest expressed two ways — lowercase hex for a
//! reservation's `expectedSha256` and for the photo's
//! `thumbnailSourceOriginalSHA256`, canonical Base64 for a descriptor's
//! `fileChecksum` — so both are computed together, once, from one pass over
//! one file.
//!
//! ## Why the bytes are copied first
//!
//! An upload takes minutes. A user editing the original while it is in flight
//! would otherwise produce a body that no longer matches the digest that was
//! declared: the service rejects that with `CHECKSUM_MISMATCH`, and the retry
//! would fail the same way. Copying into owned staging first makes the upload
//! read a file nothing else can change, and the thumbnail is generated from
//! *that* copy — so the pair describes one version, never two.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::core::error::{AppError, Result};

/// Bounded copy: a staged snapshot never outgrows what the service would accept.
const COPY_CHUNK: usize = 256 * 1024;

/// Attempts before a thumbnail is reported as over the ceiling rather than
/// retried forever.
const MAX_THUMBNAIL_ATTEMPTS: usize = 10;
/// Below this the result is visibly degraded, so the size lever is used instead.
const MIN_THUMBNAIL_QUALITY: u8 = 40;
/// Going below the service's own dimension floor gains nothing.
const MIN_THUMBNAIL_EDGE: u32 = 64;

/// A SHA-256 in both of the encodings the wire uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDigest {
    pub size: u64,
    /// Lowercase hex. Reservation and photo-field form.
    pub hex: String,
    /// Canonical padded Base64. Descriptor form.
    pub base64: String,
}

impl ContentDigest {
    pub fn of(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self {
            size: bytes.len() as u64,
            hex: to_hex(&digest),
            base64: to_base64(&digest),
        }
    }

    /// Re-encode a descriptor's Base64 checksum as the hex a reservation and a
    /// photo field expect.
    ///
    /// The service sends Base64 and expects hex for a *different* field, so
    /// this conversion is the join between them — and the one place it must
    /// not be done by re-hashing the bytes.
    pub fn hex_from_descriptor_base64(checksum: &str) -> Option<String> {
        Some(to_hex(&from_base64(checksum)?))
    }
}

/// One owned, immutable copy of a photo's original, with its digest.
#[derive(Debug, Clone)]
pub struct OwnedSnapshot {
    /// Inside the staging root, in this application's reserved namespace.
    pub path: PathBuf,
    pub digest: ContentDigest,
    /// The source it was copied from, for diagnostics only.
    pub source: PathBuf,
}

impl OwnedSnapshot {
    /// Read the staged bytes.
    pub fn read(&self) -> Result<Vec<u8>> {
        std::fs::read(&self.path)
            .map_err(|error| AppError::Backend(format!("cannot read the staged original: {error}")))
    }
}

/// Copy a source file into owned staging and digest it in one bounded pass.
///
/// `staging` must be this application's own directory: the copy is what the
/// upload reads, and it must not land where a later scan would treat it as
/// user media.
pub fn snapshot_source(
    source: &Path,
    staging: &Path,
    name_hint: &str,
    max_bytes: u64,
) -> Result<OwnedSnapshot> {
    if !source.is_file() {
        return Err(AppError::Backend(format!(
            "the original to upload is not a readable file: {}",
            source.display()
        )));
    }
    std::fs::create_dir_all(staging).map_err(|error| {
        AppError::Backend(format!("cannot create the staging directory: {error}"))
    })?;
    let name = crate::core::sync::remote_path::staged_name(
        name_hint,
        &crate::core::sync::device::new_event_id(),
    );
    let extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| ext.chars().all(|c| c.is_ascii_alphanumeric()));
    let staged =
        staging.join(extension.map_or(name.clone(), |extension| format!("{name}.{extension}")));

    let before = std::fs::metadata(source)?;
    let mut reader = std::fs::File::open(source)
        .map_err(|error| AppError::Backend(format!("cannot open the original: {error}")))?;
    let mut writer = std::fs::File::create(&staged)
        .map_err(|error| AppError::Backend(format!("cannot stage the original: {error}")))?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut size: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| AppError::Backend(format!("cannot read the original: {error}")))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > max_bytes {
            // Oversized is a refusal, not something to upload and let the
            // service reject: stopping here leaves no half-copied stage.
            drop(writer);
            let _ = std::fs::remove_file(&staged);
            return Err(AppError::Backend(format!(
                "the original is larger than the {max_bytes}-byte limit"
            )));
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| AppError::Backend(format!("cannot stage the original: {error}")))?;
    }
    writer
        .sync_all()
        .map_err(|error| AppError::Backend(format!("cannot flush the staged original: {error}")))?;
    drop(writer);

    let after = std::fs::metadata(source)?;
    use std::os::unix::fs::MetadataExt;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.modified()? != after.modified()?
    {
        let _ = std::fs::remove_file(&staged);
        return Err(AppError::Backend(
            "source changed while preparing the upload snapshot".into(),
        ));
    }
    let digest = hasher.finalize();
    Ok(OwnedSnapshot {
        path: staged,
        digest: ContentDigest {
            size,
            hex: to_hex(&digest),
            base64: to_base64(&digest),
        },
        source: source.to_path_buf(),
    })
}

/// A photo's thumbnail, generated from its staged original.
///
/// Quality is lowered before size, and the number of attempts is bounded: an
/// over-ceiling thumbnail is a limit to report, not something to retry forever.
/// A loop that re-rendered the same dimensions indefinitely would hang the
/// cycle rather than refuse it.
pub fn thumbnail_from(
    snapshot: &OwnedSnapshot,
    max_edge: u32,
    max_bytes: u64,
) -> Result<(Vec<u8>, ContentDigest)> {
    let mut quality = 90u8;
    let mut edge = max_edge;
    for _ in 0..MAX_THUMBNAIL_ATTEMPTS {
        let bytes = render(snapshot, edge, quality)?;
        if bytes.len() as u64 <= max_bytes {
            let digest = ContentDigest::of(&bytes);
            return Ok((bytes, digest));
        }
        if quality > MIN_THUMBNAIL_QUALITY {
            quality -= 10;
        } else {
            edge = (edge / 2).max(MIN_THUMBNAIL_EDGE);
            quality = 80;
        }
    }
    Err(AppError::Backend(format!(
        "the thumbnail could not be rendered within {max_bytes} bytes"
    )))
}

fn render(snapshot: &OwnedSnapshot, edge: u32, quality: u8) -> Result<Vec<u8>> {
    crate::core::thumbnails::render_jpeg_bytes_at_quality(&snapshot.path, edge, quality)
        .map_err(|error| AppError::Backend(format!("cannot render the thumbnail: {error}")))
}

/// Lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Canonical padded Base64, matching the service's encoder.
pub fn to_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(BASE64[(n >> 18) as usize & 63] as char);
        out.push(BASE64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn from_base64(value: &str) -> Option<Vec<u8>> {
    // Decoded in whole groups of four. A streaming decoder that emits a byte
    // per eight accumulated bits silently drops the tail of any input whose
    // length is not a multiple of three — which is every padded input.
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for group in bytes.chunks(4) {
        let padding = group.iter().filter(|byte| **byte == b'=').count();
        if padding > 2 {
            return None;
        }
        let mut buffer = 0u32;
        for byte in group {
            // Padding contributes no bits; it must not be looked up in the
            // alphabet, or a well-formed padded input fails to decode at all.
            if *byte == b'=' {
                buffer <<= 6;
                continue;
            }
            let index = BASE64.iter().position(|candidate| candidate == byte)?;
            buffer = (buffer << 6) | index as u32;
        }
        let significant = 3 - padding;
        for index in 0..significant {
            out.push((buffer >> (16 - index * 8)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
