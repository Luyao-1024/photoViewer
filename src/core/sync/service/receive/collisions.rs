//! Receive-only path collisions. Ordinary local indexing never hashes a file.
use super::{ReceiveError, SyncService};
use crate::core::{
    backend::local::LocalBackend,
    error::{AppError, Result},
    sync::{
        native::records::RemoteGraph,
        store::{receive::LocalDigest, runtime::Command},
    },
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

fn failure(message: impl Into<String>) -> AppError {
    AppError::Backend(message.into())
}
fn receive_error(error: impl std::fmt::Display) -> ReceiveError {
    ReceiveError::Transport(error.to_string())
}

// Include inode identity so an external replacement cannot masquerade as the
// same file merely by preserving its size and modification time.
fn stamp(path: &Path) -> Result<Value> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(failure("a receive collision is not a regular file"));
    }
    Ok(json!([
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec()
    ]))
}

impl SyncService {
    pub(super) async fn prepare_local_matches(
        &self,
        job: i64,
        graph: &RemoteGraph,
    ) -> std::result::Result<(), ReceiveError> {
        let service = self.clone();
        let paths: Vec<String> = graph
            .photos
            .iter()
            .filter(|photo| !photo.is_hidden)
            .map(|photo| photo.relative_path.clone())
            .collect();
        tokio::task::spawn_blocking(move || service.index_receive_paths(job, paths))
            .await
            .map_err(receive_error)?
            .map_err(receive_error)
    }

    fn index_receive_paths(&self, job: i64, paths: Vec<String>) -> Result<()> {
        let service = self;

        let root = service
            .store()
            .get_job(job)?
            .ok_or_else(|| failure("receive job was removed"))?
            .local_root;
        let mut items = Vec::new();
        for relative in &paths {
            let path = super::super::transfer::safe_destination(&root, relative)?;
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() => meta,
                Ok(_) => return Err(failure("a local non-file occupies the remote path")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            use std::os::unix::fs::MetadataExt;
            let unchanged:bool=service.pool().get()?.query_row(
                    "SELECT EXISTS(SELECT 1 FROM media_items WHERE path=?1 AND trashed_at IS NULL AND file_size=?2 AND file_mtime_ns=?3)",
                    rusqlite::params![path.to_string_lossy(),meta.len() as i64,meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec())],|r|r.get(0))?;
            if !unchanged {
                if let Some(item) = LocalBackend::new_item_from_path(&path)? {
                    items.push(item);
                }
            }
        }
        if !items.is_empty() {
            service.store().commit_local_scan(items)?;
        }
        service
            .store()
            .runtime(Command::ObserveLocal { job, paths })
    }

    pub(super) async fn preserve_collisions(
        &self,
        job: i64,
        graph: &RemoteGraph,
        measured: &HashMap<String, LocalDigest>,
    ) -> std::result::Result<(), ReceiveError> {
        let service = self.clone();
        let graph = graph.clone();
        let measured = measured.clone();
        tokio::task::spawn_blocking(move || service.preserve_receive_paths(job, &graph, &measured))
            .await
            .map_err(receive_error)?
            .map_err(receive_error)
    }

    fn preserve_receive_paths(
        &self,
        job: i64,
        graph: &RemoteGraph,
        measured: &HashMap<String, LocalDigest>,
    ) -> Result<()> {
        let service = self;

        let _lease = crate::core::sync::publication_io::write()?;
        let current = service
            .store()
            .get_job(job)?
            .ok_or_else(|| failure("receive job was removed"))?;
        let mut reserved: HashSet<String> = graph
            .photos
            .iter()
            .map(|photo| photo.relative_path.clone())
            .collect();
        let by_path: HashMap<&str, _> = graph
            .photos
            .iter()
            .filter(|photo| !photo.is_hidden)
            .map(|photo| (photo.relative_path.as_str(), photo))
            .collect();
        let record_ids: HashSet<&str> = graph
            .photos
            .iter()
            .map(|photo| photo.photo_id.as_str())
            .collect();
        for entry in service.store().local_entries(job)? {
            let Some(photo) = by_path.get(entry.relative_path.as_str()) else {
                continue;
            };
            if entry.photo_id.as_deref() == Some(&photo.photo_id) {
                continue;
            }
            if !entry.local_path.exists() {
                continue;
            }
            let (dirty,busy):(i64,bool)=service.pool().get()?.query_row("SELECT native_dirty,EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND state='issued') OR EXISTS(SELECT 1 FROM sync_publications WHERE entry_id=?1 AND state='publishing') FROM sync_entries WHERE id=?1",[entry.entry_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if busy {
                continue;
            }
            let observation = measured.get(&entry.relative_path);
            if observation.is_some_and(|value| {
                value.entry_id != entry.entry_id || value.local_version != entry.local_version
            }) {
                return Err(failure(
                    "local edit superseded receive comparison; retry scheduled",
                ));
            }
            let same_bytes = observation.is_some_and(|value| {
                value.digest.size == photo.original.size
                    && crate::core::sync::ContentDigest::hex_from_descriptor_base64(
                        &photo.original.file_checksum,
                    )
                    .as_deref()
                        == Some(value.digest.hex.as_str())
            });
            let old_identity_still_live = match entry.photo_id.as_deref() {
                Some(id) if graph.tombstones.iter().any(|deleted|deleted==id) => false,
                Some(id) => record_ids.contains(id) || service.pool().get()?.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2 AND native_retired=0)",
                    rusqlite::params![job,id],|r|r.get::<_,bool>(0),
                )?,
                None=>false,
            };
            if same_bytes
                && !old_identity_still_live
                && dirty != super::super::super::store::NativeDirty::DELETE as i64
            {
                if entry.photo_id.is_some() {
                    service
                        .store()
                        .runtime(Command::ForgetIdentityForAdoption {
                            job,
                            entry: entry.entry_id,
                            version: entry.local_version,
                        })?;
                }
                continue;
            }
            if entry.file_size == photo.original.size && observation.is_none() {
                return Err(failure(
                    "could not compare a same-size local collision; retry scheduled",
                ));
            }
            let source_stamp = stamp(&entry.local_path)?;
            let stem = entry
                .local_path
                .file_stem()
                .ok_or_else(|| failure("collision path has no stem"))?;
            let parent = entry
                .local_path
                .parent()
                .ok_or_else(|| failure("collision path has no parent"))?;
            let mut moved = false;
            for number in 1..=u32::MAX {
                let mut name = stem.to_os_string();
                name.push(format!("_{number}"));
                if let Some(extension) = entry.local_path.extension() {
                    name.push(".");
                    name.push(extension);
                }
                let preserved = parent.join(name);
                let relative = preserved
                    .strip_prefix(&current.local_root)
                    .map_err(|_| failure("collision escaped the job root"))?
                    .to_string_lossy()
                    .to_string();
                if reserved.contains(&relative) || std::fs::symlink_metadata(&preserved).is_ok() {
                    continue;
                }
                let occupied:bool=service.pool().get()?.query_row("SELECT EXISTS(SELECT 1 FROM sync_entries WHERE job_id=?1 AND relative_path=?2) OR EXISTS(SELECT 1 FROM media_items WHERE path=?3)",rusqlite::params![job,relative,preserved.to_string_lossy()],|r|r.get(0))?;
                if occupied {
                    continue;
                }
                let id = format!("collision-{}", uuid::Uuid::new_v4());
                let capture = json!({"job":job,"entry":entry.entry_id,"local":entry.local_version,"config":current.config_generation,"dirty":dirty,"photo":photo.photo_id});
                let details = json!({"collision":true,"source":entry.local_path,"preserved":preserved,"root":current.local_root,"stamp":source_stamp});
                service.store().runtime(Command::BeginCollision {
                    id: id.clone(),
                    capture,
                    details: details.clone(),
                })?;
                if stamp(&entry.local_path)? != source_stamp {
                    service.store().runtime(Command::AbortCollision { id })?;
                    return Err(failure("local collision changed before preservation"));
                }
                match std::fs::hard_link(&entry.local_path, &preserved) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        service.store().runtime(Command::AbortCollision { id })?;
                        continue;
                    }
                    Err(error) => {
                        service.store().runtime(Command::AbortCollision { id })?;
                        return Err(error.into());
                    }
                }
                if let Err(error) = std::fs::remove_file(&entry.local_path) {
                    recover_collision(service, &id, &details)?;
                    return Err(error.into());
                }
                std::fs::File::open(parent)?.sync_all()?;
                if let Err(error) = service
                    .store()
                    .runtime(Command::FinishCollision { id: id.clone() })
                {
                    recover_collision(service, &id, &details)?;
                    return Err(error);
                }
                reserved.insert(relative);
                moved = true;
                break;
            }
            if !moved {
                return Err(failure("no free numeric suffix for the local collision"));
            }
        }
        Ok(())
    }
}

/// A collision move commits its DB paths only after its original is preserved.
/// Until then the ordinary publication recovery loop restores the original name.
pub(in crate::core::sync::service) fn recover_collision(
    service: &SyncService,
    id: &str,
    details: &Value,
) -> Result<()> {
    let source = PathBuf::from(
        details["source"]
            .as_str()
            .ok_or_else(|| failure("collision recovery has no source"))?,
    );
    let preserved = PathBuf::from(
        details["preserved"]
            .as_str()
            .ok_or_else(|| failure("collision recovery has no preserved path"))?,
    );
    let root = Path::new(
        details["root"]
            .as_str()
            .ok_or_else(|| failure("collision recovery has no root"))?,
    );
    for path in [&source, &preserved] {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| failure("collision recovery escaped the root"))?;
        if super::super::transfer::safe_destination(root, &relative.to_string_lossy())? != *path {
            return Err(failure("collision recovery path mismatch"));
        }
    }
    if source.parent() != preserved.parent() {
        return Err(failure("collision recovery changed albums"));
    }
    if std::fs::symlink_metadata(&preserved).is_ok() {
        if stamp(&preserved)? != details["stamp"] {
            return Err(failure("preserved collision changed after interruption"));
        }
        if std::fs::symlink_metadata(&source).is_ok() {
            if stamp(&source)? != details["stamp"] {
                return Err(failure("collision source changed after interruption"));
            }
        } else {
            std::fs::hard_link(&preserved, &source)?;
        }
        std::fs::remove_file(&preserved)?;
    }
    std::fs::File::open(
        source
            .parent()
            .ok_or_else(|| failure("collision recovery has no parent"))?,
    )?
    .sync_all()?;
    service
        .store()
        .runtime(Command::AbortCollision { id: id.into() })
}

#[cfg(test)]
mod tests;
