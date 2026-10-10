use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use walkdir::WalkDir;

use crate::core::error::{AppError, Result};
use crate::core::media::is_supported_media_path;

use super::model::Fingerprint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEntry {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub fingerprint: Fingerprint,
    pub modified_ns: i64,
}

pub fn scan(root: &Path) -> Result<BTreeMap<String, LocalEntry>> {
    scan_matching(root, |_| true)
}

pub fn scan_matching(
    root: &Path,
    include: impl Fn(&str) -> bool,
) -> Result<BTreeMap<String, LocalEntry>> {
    scan_matching_cached(root, include, |_| None)
}

/// Scan the local tree, reusing stored fingerprints for files whose size and
/// nanosecond mtime still match the observation they were hashed under.
///
/// Content hashing is the dominant cost of a run that changes nothing. The
/// cache is only trusted as a pair: a hit requires the byte length and the
/// exact mtime to both equal the values stored alongside the hash, so any
/// write that moves either falls back to a full re-hash. `cached` receives
/// the normalized relative path and returns the previously hashed
/// `(Fingerprint, modified_ns)` when one exists.
pub fn scan_matching_cached(
    root: &Path,
    include: impl Fn(&str) -> bool,
    cached: impl Fn(&str) -> Option<(Fingerprint, i64)>,
) -> Result<BTreeMap<String, LocalEntry>> {
    if !root.is_absolute() || !root.is_dir() {
        return Err(AppError::Backend(format!(
            "synchronization root is not an available absolute directory: {}",
            root.display()
        )));
    }
    let mut result = BTreeMap::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return Err(AppError::Backend(format!(
                    "cannot enumerate synchronization root: {error}"
                )))
            }
        };
        let file_type = entry.file_type();
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if !is_supported_media_path(path) || is_internal_sync_name(path) {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| AppError::Backend("file escaped synchronization root".into()))?;
        let relative_path = normalized_relative_path(relative)?;
        if !include(&relative_path) {
            continue;
        }
        let metadata_before = std::fs::metadata(path)?;
        let before_time = modified_ns(&metadata_before)?;
        if let Some((fingerprint, cached_mtime)) = cached(&relative_path) {
            if fingerprint.size == metadata_before.len() && cached_mtime == before_time {
                result.insert(
                    relative_path.clone(),
                    LocalEntry {
                        relative_path,
                        absolute_path: path.to_path_buf(),
                        fingerprint,
                        modified_ns: before_time,
                    },
                );
                continue;
            }
        }
        let fingerprint = fingerprint(path)?;
        let metadata_after = std::fs::metadata(path)?;
        let after_time = modified_ns(&metadata_after)?;
        if metadata_before.len() != metadata_after.len() || before_time != after_time {
            return Err(AppError::Backend(format!(
                "file changed while it was being observed: {}",
                path.display()
            )));
        }
        result.insert(
            relative_path.clone(),
            LocalEntry {
                relative_path,
                absolute_path: path.to_path_buf(),
                fingerprint,
                modified_ns: after_time,
            },
        );
    }
    Ok(result)
}

pub fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    Ok(Fingerprint {
        size,
        blake3: hasher.finalize().to_hex().to_string(),
    })
}

pub fn destination(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let relative = Path::new(relative_path);
    let normalized = normalized_relative_path(relative)?;
    Ok(root.join(normalized))
}

pub fn atomic_publish_new(staged: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Backend("synchronization target has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    if target.exists() {
        return Err(AppError::Backend(format!(
            "refusing to overwrite local file: {}",
            target.display()
        )));
    }
    let sibling = copy_staged_to_sibling(staged, parent)?;
    sibling
        .persist_noclobber(target)
        .map_err(|error| AppError::Io(error.error))?;
    Ok(())
}

pub fn atomic_publish_replace(staged: &Path, target: &Path, operation_id: &str) -> Result<PathBuf> {
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Backend("synchronization target has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    if !target.is_file() {
        return Err(AppError::Backend(format!(
            "cannot replace missing local file: {}",
            target.display()
        )));
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Backend("synchronization target name is not UTF-8".into()))?;
    let backup = parent.join(format!(".photoviewer-sync-backup-{operation_id}-{name}"));
    if backup.exists() {
        return Err(AppError::Backend(format!(
            "synchronization backup already exists: {}",
            backup.display()
        )));
    }
    let sibling = copy_staged_to_sibling(staged, parent)?;
    std::fs::rename(target, &backup)?;
    if let Err(error) = sibling.persist_noclobber(target) {
        let rollback = std::fs::hard_link(&backup, target);
        return match rollback {
            Ok(()) => {
                std::fs::remove_file(&backup)?;
                Err(error.error.into())
            }
            Err(rollback) => Err(AppError::Backend(format!(
                "publish failed: {}; rollback failed: {rollback}; backup remains at {}",
                error.error,
                backup.display()
            ))),
        };
    }
    Ok(backup)
}

fn copy_staged_to_sibling(staged: &Path, parent: &Path) -> Result<tempfile::NamedTempFile> {
    let mut source = File::open(staged)?;
    let mut sibling = tempfile::Builder::new()
        .prefix(".photoviewer-sync-")
        .tempfile_in(parent)?;
    std::io::copy(&mut source, sibling.as_file_mut())?;
    sibling
        .as_file()
        .set_permissions(source.metadata()?.permissions())?;
    sibling.as_file().sync_all()?;
    Ok(sibling)
}

fn normalized_relative_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| {
                    AppError::Backend("synchronization path is not valid UTF-8".into())
                })?;
                if part.is_empty() {
                    return Err(AppError::Backend(
                        "empty synchronization path segment".into(),
                    ));
                }
                parts.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::Backend(
                    "synchronization path escapes its configured root".into(),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(AppError::Backend("empty synchronization path".into()));
    }
    Ok(parts.join("/"))
}

fn modified_ns(metadata: &std::fs::Metadata) -> Result<i64> {
    let duration = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::Backend(format!("invalid file modification time: {error}")))?;
    i64::try_from(duration.as_nanos())
        .map_err(|_| AppError::Backend("file modification time is out of range".into()))
}

fn is_internal_sync_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".photoviewer-sync-"))
}

#[cfg(test)]
mod tests;
