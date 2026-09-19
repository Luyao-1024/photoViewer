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
        let metadata_before = std::fs::metadata(path)?;
        let fingerprint = fingerprint(path)?;
        let metadata_after = std::fs::metadata(path)?;
        let before_time = modified_ns(&metadata_before)?;
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
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if target.exists() {
        return Err(AppError::Backend(format!(
            "refusing to overwrite local file: {}",
            target.display()
        )));
    }
    std::fs::rename(staged, target)?;
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
    std::fs::rename(target, &backup)?;
    if let Err(error) = std::fs::rename(staged, target) {
        let rollback = std::fs::rename(&backup, target);
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(rollback) => Err(AppError::Backend(format!(
                "publish failed: {error}; rollback failed: {rollback}; backup remains at {}",
                backup.display()
            ))),
        };
    }
    Ok(backup)
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
