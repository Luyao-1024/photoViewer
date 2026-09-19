//! 缓存清理（LRU 按 mtime）
use crate::core::error::Result;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheCleanupReport {
    pub deleted_files: usize,
    pub deleted_bytes: u64,
    pub failed_files: usize,
    pub remaining_bytes: u64,
}

/// 计算目录的总大小（字节）。
pub fn dir_size(cache_dir: &Path) -> u64 {
    if !cache_dir.exists() {
        return 0;
    }
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(cache_dir).into_iter().flatten() {
        let p = entry.path();
        if p.is_file() {
            if let Ok(meta) = std::fs::metadata(p) {
                total += meta.len();
            }
        }
    }
    total
}

pub fn enforce_size_limit(cache_dir: &Path, max_bytes: u64) -> Result<usize> {
    Ok(enforce_size_limit_report(cache_dir, max_bytes)?.deleted_files)
}

/// Enforce the disk-cache limit and report only removals that actually
/// succeeded. Failed removals remain part of `remaining_bytes` and are never
/// counted as reclaimed capacity.
pub fn enforce_size_limit_report(cache_dir: &Path, max_bytes: u64) -> Result<CacheCleanupReport> {
    if !cache_dir.exists() {
        return Ok(CacheCleanupReport::default());
    }

    // 递归收集所有文件 + mtime + size
    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime, u64)> = Vec::new();
    for entry in walkdir::WalkDir::new(cache_dir).into_iter().flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(p) {
            files.push((
                p.to_path_buf(),
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                meta.len(),
            ));
        }
    }

    // 按 mtime 升序排序（最旧在前）
    files.sort_by_key(|(_, mtime, _)| *mtime);

    let mut total: u64 = files.iter().map(|(_, _, s)| *s).sum();
    let mut report = CacheCleanupReport::default();
    for (path, _, size) in &files {
        if total <= max_bytes {
            break;
        }
        match std::fs::remove_file(path) {
            Ok(()) => {
                total = total.saturating_sub(*size);
                report.deleted_files += 1;
                report.deleted_bytes = report.deleted_bytes.saturating_add(*size);
            }
            Err(error) => {
                report.failed_files += 1;
                tracing::warn!(
                    target: crate::core::log_targets::STORAGE,
                    path = %path.display(),
                    %error,
                    "cache cleanup could not remove file"
                );
            }
        }
    }
    report.remaining_bytes = total;
    Ok(report)
}
