//! 缓存清理（LRU 按 mtime）
use crate::core::error::Result;
use std::path::Path;

pub fn enforce_size_limit(cache_dir: &Path, max_bytes: u64) -> Result<usize> {
    if !cache_dir.exists() {
        return Ok(0);
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
    let mut deleted = 0;
    for (path, _, size) in &files {
        if total <= max_bytes {
            break;
        }
        std::fs::remove_file(path).ok();
        total -= size;
        deleted += 1;
    }
    Ok(deleted)
}
