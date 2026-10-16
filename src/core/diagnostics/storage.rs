//! Timestamped run files, daily gzip archives and a shared disk budget.
use anyhow::{bail, Result};
use chrono::{DateTime, Local, NaiveDate};
use flate2::{write::GzEncoder, Compression};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
pub const MAX_BYTES: u64 = 500_000_000;
#[derive(Debug)]
pub(super) struct Entry {
    pub time_ms: i64,
    pub day: String,
    pub level: String,
    pub target: String,
    pub message: String,
}
pub(super) struct Store {
    pub dir: PathBuf,
    file: File,
    path: PathBuf,
    _lock: File,
    limit: u64,
    stamp: String,
    sequence: u64,
    occupied: u64,
}
impl Store {
    pub fn open(dir: &Path, limit: u64) -> Result<Option<Self>> {
        Self::open_at(dir, limit, Local::now())
    }
    fn open_at(dir: &Path, limit: u64, now: DateTime<Local>) -> Result<Option<Self>> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join(".lock"))?;
        match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(rustix::io::Errno::WOULDBLOCK) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let limit = limit.clamp(1_000_000, MAX_BYTES);
        // Preserve today's legacy log rather than truncating it at launch.
        let leadup = dir.join("app.log");
        if let Ok(meta) = fs::symlink_metadata(&leadup) {
            if meta.file_type().is_symlink() {
                fs::remove_file(&leadup)?;
            } else if meta.is_file() {
                fs::rename(
                    &leadup,
                    dir.join(format!("legacy-{}.log", now.timestamp_micros())),
                )?;
            }
        }
        let trace = dir.join("trace.json");
        if let Ok(meta) = fs::symlink_metadata(&trace) {
            if meta.file_type().is_symlink() {
                fs::remove_file(&trace)?;
            } else if meta.is_file() {
                fs::rename(
                    &trace,
                    dir.join(format!("trace-{}.json", now.timestamp_micros())),
                )?;
            }
        }
        compress_before(dir, limit, now.date_naive())?;
        // Each process gets a new filename, including launches in the same second.
        let stamp = format!(
            "{}-{}",
            now.format("%Y-%m-%d_%H-%M-%S%.6f"),
            std::process::id()
        );
        let folder = dir.join(now.format("%Y-%m-%d").to_string());
        fs::create_dir_all(&folder)?;
        let path = folder.join(format!("{stamp}.log"));
        let file = open_file(&path)?;
        let mut store = Self {
            dir: dir.to_owned(),
            file,
            path,
            _lock: lock,
            limit,
            stamp,
            sequence: 0,
            occupied: 0,
        };
        store.link_tail()?;
        store.resize(limit)?;
        Ok(Some(store))
    }
    fn segment_limit(&self) -> u64 {
        (self.limit / 8).min(8 * 1024 * 1024)
    }
    fn link_tail(&self) -> Result<()> {
        let temporary = self.dir.join(".app.log.next");
        if temporary.exists() || fs::symlink_metadata(&temporary).is_ok() {
            fs::remove_file(&temporary)?;
        }
        std::os::unix::fs::symlink(self.path.strip_prefix(&self.dir)?, &temporary)?;
        fs::rename(temporary, self.dir.join("app.log"))?;
        Ok(())
    }
    fn rotate(&mut self) -> Result<()> {
        self.file.sync_data()?;
        self.sequence += 1;
        let path = self
            .path
            .parent()
            .unwrap()
            .join(format!("{}-{:04}.log", self.stamp, self.sequence));
        self.file = open_file(&path)?;
        self.path = path;
        self.link_tail()
    }
    pub fn append(&mut self, entry: &Entry) -> Result<()> {
        let max = (self.segment_limit() / 4).min(16 * 1024) as usize;
        let mut end = entry.message.len().min(max);
        while !entry.message.is_char_boundary(end) {
            end -= 1;
        }
        let timestamp = chrono::DateTime::from_timestamp_millis(entry.time_ms)
            .map(|time| time.to_rfc3339())
            .unwrap_or_else(|| entry.time_ms.to_string());
        // Escaped newlines prevent a field from forging another log record.
        let message = entry.message[..end]
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        let line = format!(
            "{timestamp} {} {} {}: {message}\n",
            entry.day, entry.level, entry.target
        );
        if self.file.metadata()?.len() + line.len() as u64 > self.segment_limit() {
            self.rotate()?;
        }
        // Track our writes; scan the archive tree only when reclaiming space.
        // The auxiliary allowance covers bounded crash/trace growth between scans.
        let reserve = auxiliary_limit(self.limit) + line.len() as u64;
        if self.occupied.saturating_add(reserve) > self.limit {
            if !prune(&self.dir, self.limit, reserve, &[&self.path])? {
                bail!("log storage cannot reclaim its disk budget");
            }
            self.occupied = size(&self.dir);
        }
        // Charge before writing, so a partial IO failure cannot undercount usage.
        self.occupied = self.occupied.saturating_add(line.len() as u64);
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        Ok(())
    }
    pub fn resize(&mut self, limit: u64) -> Result<()> {
        self.limit = limit.clamp(1_000_000, MAX_BYTES);
        if self.file.metadata()?.len() > self.segment_limit() {
            self.rotate()?;
        }
        if !prune(
            &self.dir,
            self.limit,
            auxiliary_limit(self.limit),
            &[&self.path],
        )? {
            bail!("log storage cannot reclaim its disk budget");
        }
        self.occupied = size(&self.dir);
        Ok(())
    }
    pub fn flush(&self) -> Result<()> {
        self.file.sync_data().map_err(Into::into)
    }
    pub fn clear(&mut self) -> Result<()> {
        // Truncate the writer's actual inode: unlinking a live file would hide
        // subsequent writes and retain its disk space until process exit.
        self.file.set_len(0)?;
        for (path, _, _) in files(&self.dir)? {
            if path != self.path && path != self.dir.join("trace.json") {
                fs::remove_file(path)?;
            }
        }
        self.file.sync_data()?;
        remove_empty_directories(&self.dir);
        self.occupied = size(&self.dir);
        Ok(())
    }
}
fn open_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new()
        .append(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
fn files(dir: &Path) -> Result<Vec<(PathBuf, std::time::SystemTime, u64)>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(dir).follow_links(false).into_iter() {
        let entry = entry?;
        if !entry.file_type().is_file() || entry.file_name() == ".lock" {
            continue;
        }
        let meta = entry.metadata()?;
        files.push((entry.path().to_owned(), meta.modified()?, meta.len()));
    }
    // Compression updates mtime, but must not make yesterday's archive newer
    // than today's raw logs for retention purposes.
    files.sort_by_key(|(path, modified, _)| (file_day(path, *modified), *modified));
    Ok(files)
}
pub(super) fn size(dir: &Path) -> u64 {
    // app.log is a symlink to the current segment, not a second copy.
    files(dir)
        .map(|files| files.iter().map(|(_, _, size)| size).sum())
        .unwrap_or(0)
}
fn prune(dir: &Path, limit: u64, reserve: u64, protected: &[&Path]) -> Result<bool> {
    let files = files(dir)?;
    let mut bytes: u64 = files.iter().map(|(_, _, size)| size).sum();
    for (path, _, size) in files {
        if bytes.saturating_add(reserve) <= limit {
            return Ok(true);
        }
        if protected.contains(&path.as_path()) || path == dir.join("trace.json") {
            continue;
        }
        fs::remove_file(path)?;
        bytes = bytes.saturating_sub(size);
    }
    Ok(bytes.saturating_add(reserve) <= limit)
}
fn auxiliary_limit(limit: u64) -> u64 {
    let crash = 4 * (limit / 128).min(32 * 1024) + 32 * 1024 + 4096;
    6 * crash + super::trace::limit(limit)
}
fn file_day(path: &Path, modified: std::time::SystemTime) -> NaiveDate {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
        .unwrap_or_else(|| DateTime::<Local>::from(modified).date_naive())
}
fn compress_before(dir: &Path, limit: u64, today: NaiveDate) -> Result<()> {
    for (path, modified, bytes) in files(dir)? {
        if !path.exists() {
            continue;
        } // An earlier quota pass may have evicted it.
        let name = path.file_name().unwrap().to_string_lossy();
        if name.starts_with(".archive-") {
            fs::remove_file(&path)?;
            continue;
        }
        if !name.ends_with(".log") && !name.ends_with(".json") {
            continue;
        }
        let day = file_day(&path, modified);
        if day >= today {
            continue;
        }
        // Compression must respect the same cap even while source and gzip
        // temporarily coexist; all normal segments have bounded source size.
        let reserve = auxiliary_limit(limit) + bytes + bytes / 100 + 65536;
        if !prune(dir, limit, reserve, &[&path])? {
            fs::remove_file(&path)?;
            continue;
        }
        let folder = dir.join("archive").join(day.format("%Y-%m-%d").to_string());
        fs::create_dir_all(&folder)?;
        let target = folder.join(format!("{name}.gz"));
        let staged = tempfile::Builder::new()
            .prefix(".archive-")
            .tempfile_in(&folder)?;
        let mut encoder = GzEncoder::new(staged, Compression::fast());
        std::io::copy(&mut File::open(&path)?, &mut encoder)?;
        let staged = encoder.finish()?;
        staged.as_file().sync_all()?;
        // Atomic publication: on interruption the original is still recoverable.
        staged.persist(&target)?;
        File::open(&folder)?.sync_all()?;
        fs::remove_file(path)?;
    }
    remove_empty_directories(dir);
    Ok(())
}
fn remove_empty_directories(dir: &Path) {
    for entry in walkdir::WalkDir::new(dir)
        .min_depth(1)
        .contents_first(true)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_dir() {
            let _ = fs::remove_dir(entry.path());
        }
    }
}
#[cfg(test)]
mod tests;
