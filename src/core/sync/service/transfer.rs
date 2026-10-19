//! Bounded resource receive and recoverable, actor-committed pair publication.
use super::SyncService;
use crate::core::{
    error::{AppError, Result},
    sync::{
        native::assets::AssetRole,
        store::{runtime::Command, ResourceWork},
        ContentDigest,
    },
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TransferReport {
    pub fetched: usize,
    pub admitted: usize,
    pub failures: Vec<(String, String)>,
}
#[derive(Debug)]
pub struct VerifiedResource {
    pub asset_id: String,
    pub role: AssetRole,
    pub staged: PathBuf,
    pub digest: ContentDigest,
}
fn error(message: impl Into<String>) -> AppError {
    AppError::Backend(message.into())
}

impl SyncService {
    pub async fn run_transfers(&self, job_id: i64) -> Result<TransferReport> {
        let _slot = self.acquire_run_slot(job_id).await?;
        self.require_selected_account(job_id)?;
        self.transfer_selected(job_id).await
    }

    pub(super) async fn transfer_selected(&self, job_id: i64) -> Result<TransferReport> {
        self.require_selected_account(job_id)?;
        let service = self.clone();
        tokio::task::spawn_blocking(move || service.recover_publications(job_id))
            .await
            .map_err(|_| error("publication recovery worker stopped"))??;
        let generation = self
            .store()
            .get_job(job_id)?
            .ok_or_else(|| error("job removed"))?
            .config_generation;
        let mut report = TransferReport::default();
        let obligations = self.store().outstanding_resources(job_id)?;
        let requested = obligations.len();
        let started = Instant::now();
        if requested > 0 {
            tracing::info!(target: crate::core::log_targets::SYNC,
                job_id, requested,
                staged = obligations.iter().filter(|work| work.staging_path.is_some()).count(),
                "云同步下载批次开始");
        }
        let mut reused = 0usize;
        for obligation in obligations {
            let fetch_started = Instant::now();
            let fetched = {
                let fetch = self.fetch_one(&obligation, &mut report);
                tokio::pin!(fetch);
                let mut check = tokio::time::interval(std::time::Duration::from_millis(250));
                loop {
                    tokio::select! {
                        result=&mut fetch=>break result,
                        _=check.tick()=>{
                            let current=self.store().get_job(job_id)?;
                            if !self.store().job_is_selected(job_id)? || self.store().reset_pending()? || self.store().pending_change(job_id)?.is_some() || !current.is_some_and(|j|j.config_generation==generation && !j.paused) {return Err(error("synchronization configuration changed during download"));}
                        }
                    }
                }
            };
            match fetched {
                Ok(Some(resource)) => {
                    report.fetched += 1;
                    if resource.role == AssetRole::Thumbnail {
                        let service = self.clone();
                        let path = resource.staged.clone();
                        let asset = resource.asset_id.clone();
                        tokio::task::spawn_blocking(move || {
                            service.prepare_downloaded_thumbnail(job_id, &asset, &path)
                        })
                        .await
                        .map_err(|_| error("thumbnail publication worker stopped"))??;
                    }
                    tracing::info!(
                        target: crate::core::log_targets::SYNC,
                        job_id, asset_id = %obligation.asset_id, role = %obligation.role,
                        bytes = resource.digest.size, fetched = report.fetched,
                        elapsed_ms = fetch_started.elapsed().as_millis() as u64,
                        "云同步资源下载完成"
                    );
                }
                Ok(None) => reused += 1,
                Err(failure) => {
                    let reason = failure.to_string();
                    self.store().fail_resource(
                        job_id,
                        &obligation.asset_id,
                        &obligation.role,
                        &reason,
                    )?;
                    tracing::warn!(
                        target: crate::core::log_targets::SYNC,
                        job_id, asset_id = %obligation.asset_id, role = %obligation.role,
                        error = %reason,
                        "云同步资源下载失败"
                    );
                    report.failures.push((obligation.asset_id.clone(), reason));
                }
            }
            // Publish ready photos as their own pair arrives, without waiting
            // for the rest of the album (which can include very large files).
            let previously_admitted = report.admitted;
            let service = self.clone();
            let mut available = std::mem::take(&mut report);
            let asset = obligation.asset_id.clone();
            report = tokio::task::spawn_blocking(move || {
                service.admit_publications_for_asset(job_id, Some(&asset), &mut available)?;
                Ok::<_, AppError>(available)
            })
            .await
            .map_err(|_| error("publication worker stopped"))??;
            log_published_batch(job_id, report.admitted - previously_admitted);
        }
        let previously_admitted = report.admitted;
        let service = self.clone();
        report = tokio::task::spawn_blocking(move || {
            service.admit_complete_publications(job_id, &mut report)?;
            Ok::<_, AppError>(report)
        })
        .await
        .map_err(|_| error("publication worker stopped"))??;
        log_published_batch(job_id, report.admitted - previously_admitted);
        if report.failures.is_empty()
            && self.store().unresolved_resource_count(job_id)? == 0
            && self
                .store()
                .native_progress(job_id)?
                .is_some_and(|p| p.target_token.is_some() && p.target_token != p.applied_token)
        {
            self.store().advance_applied_token(job_id)?;
        }
        if requested > 0 || report.admitted > 0 || !report.failures.is_empty() {
            tracing::info!(
                target: crate::core::log_targets::SYNC,
                job_id, requested, fetched = report.fetched, reused, admitted = report.admitted,
                failed = report.failures.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "云同步下载批次结束"
            );
        }
        Ok(report)
    }
    async fn fetch_one(
        &self,
        work: &ResourceWork,
        _report: &mut TransferReport,
    ) -> Result<Option<VerifiedResource>> {
        let role = role_of(&work.role)?;
        let job = self
            .store()
            .get_job(work.job_id)?
            .ok_or_else(|| error("download job was removed"))?;
        let hex = work
            .expected_sha256
            .clone()
            .ok_or_else(|| error("resource projection has no checksum"))?;
        let size = work
            .expected_size
            .filter(|n| *n > 0)
            .ok_or_else(|| error("resource projection has no size"))? as u64;
        let pinned = self
            .store()
            .native_scope(job.connection_id)?
            .ok_or_else(|| error("download job has no pinned capability"))?;
        if size > pinned.limits.max_asset_bytes
            || (role == AssetRole::Thumbnail && size > pinned.limits.thumbnail_max_bytes)
        {
            return Err(error(
                "projected resource exceeds the configured size limit",
            ));
        }
        let mut expected = ContentDigest {
            size,
            hex,
            base64: String::new(),
        };
        if let Some(path) = work.staging_path.as_ref().map(PathBuf::from) {
            let expected = expected.clone();
            let check_path = path.clone();
            if tokio::task::spawn_blocking(move || verify_file(&check_path, &expected))
                .await
                .map_err(|_| error("resource verification worker stopped"))?
                .is_ok()
            {
                return Ok(None);
            }
        }
        // Locally staged bytes are checked against the durable expected digest
        // before credentials/HTTP are needed. A restart can reuse them directly.
        let client = self.transport_client_for(&job).await?;
        let descriptor = client
            .describe_asset(&work.asset_id, role)
            .await
            .map_err(|e| error(e.to_string()))?;
        expected.base64 = descriptor.descriptor.file_checksum.clone();
        if ContentDigest::hex_from_descriptor_base64(&expected.base64).as_deref()
            != Some(&expected.hex)
        {
            return Err(error("resource descriptor differs from projection"));
        }
        let folder = self
            .staging_root()
            .join(format!("job-{}", job.id))
            .join(sanitize_asset_id(&work.generation))
            .join(role.as_str());
        tokio::fs::create_dir_all(&folder)
            .await
            .map_err(|_| error("cannot create scoped download staging"))?;
        let staged = folder.join(format!(
            "{}-{}",
            sanitize_asset_id(&work.asset_id),
            uuid::Uuid::new_v4()
        ));
        if let Err(failure) = client.download_to(&descriptor, &staged, &expected).await {
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(error(failure.to_string()));
        }
        self.store().stage_resource(
            job.id,
            &work.asset_id,
            role.as_str(),
            &staged.to_string_lossy(),
        )?;
        Ok(Some(VerifiedResource {
            asset_id: work.asset_id.clone(),
            role,
            staged,
            digest: expected,
        }))
    }
    fn prepare_downloaded_thumbnail(&self, job: i64, asset: &str, path: &Path) -> Result<()> {
        let bytes = std::fs::read(path)?;
        for intent in self.store().pending_publications(job)? {
            if intent.thumbnail_asset_id.as_deref() != Some(asset) {
                continue;
            }
            let media: Option<i64> = self
                .pool()
                .get()?
                .query_row(
                    "SELECT media_id FROM sync_entries WHERE job_id=?1 AND native_photo_id=?2",
                    rusqlite::params![job, intent.photo_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let Some(media) = media else {
                continue;
            };
            let item = crate::core::db::get_media_item(self.pool(), media)?;
            if item.storage_location != crate::core::media::MediaLocation::Cloud {
                continue;
            }
            for size in [
                crate::core::thumbnails::ThumbnailSize::Small,
                crate::core::thumbnails::ThumbnailSize::Medium,
                crate::core::thumbnails::ThumbnailSize::Large,
            ] {
                crate::core::thumbnails::import_cached_thumbnail(
                    &crate::config::cache_dir(),
                    &item.uri,
                    size,
                    Some(item.file_mtime.into()),
                    &bytes,
                )
                .map_err(|e| error(format!("cannot install downloaded thumbnail: {e}")))?;
            }
            self.store().runtime(Command::ThumbnailReady {
                publication: intent.publication_id,
                media,
                mtime_ns: item.file_mtime.timestamp_nanos_opt().unwrap_or(0),
                asset: asset.into(),
            })?;
        }
        Ok(())
    }

    fn admit_complete_publications(&self, job_id: i64, report: &mut TransferReport) -> Result<()> {
        self.admit_publications_for_asset(job_id, None, report)
    }

    fn admit_publications_for_asset(
        &self,
        job_id: i64,
        asset: Option<&str>,
        report: &mut TransferReport,
    ) -> Result<()> {
        let staged_thumbnails = {
            let conn = self.pool().get()?;
            let mut stmt=conn.prepare("SELECT asset_id,staging_path FROM sync_native_resources WHERE job_id=?1 AND role='thumbnail' AND state IN ('staged','verified') AND staging_path IS NOT NULL AND (?2 IS NULL OR asset_id=?2) AND EXISTS(SELECT 1 FROM sync_publications p JOIN sync_entries e ON e.id=p.entry_id JOIN media_items m ON m.id=e.media_id WHERE p.job_id=?1 AND p.thumbnail_asset_id=sync_native_resources.asset_id AND p.state='pending' AND m.storage_location=2 AND m.thumbnail_ready=0)")?;
            let rows = stmt.query_map(rusqlite::params![job_id, asset], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    PathBuf::from(r.get::<_, String>(1)?),
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (asset, path) in staged_thumbnails {
            self.prepare_downloaded_thumbnail(job_id, &asset, &path)?;
        }
        let intents = self.store().publications_for_asset(job_id, asset)?;
        if intents.is_empty() {
            return Ok(());
        }
        for intent in intents {
            if !self.store().publication_ready(&intent.publication_id)? {
                continue;
            }
            let Some(photo) = self
                .store()
                .current_remote_photo(job_id, &intent.photo_id)?
                .filter(|p| {
                    intent.original_asset_id.as_deref() == Some(&p.original.asset_id)
                        && intent.thumbnail_asset_id.as_deref() == Some(&p.thumbnail.asset_id)
                })
            else {
                report.failures.push((
                    intent.photo_id,
                    "publication no longer matches its photo metadata".into(),
                ));
                continue;
            };
            let conn = self.pool().get()?;
            let path = |asset: &str, role: &str| -> Result<PathBuf> {
                conn.query_row("SELECT staging_path FROM sync_native_resources WHERE job_id=?1 AND generation=?2 AND asset_id=?3 AND role=?4",rusqlite::params![job_id,self.generation_of(job_id)?,asset,role],|r|r.get::<_,String>(0)).map(PathBuf::from).map_err(Into::into)
            };
            let original = path(&photo.original.asset_id, "original")?;
            let thumbnail = path(&photo.thumbnail.asset_id, "thumbnail")?;
            drop(conn);
            match self.publish_pair(
                &photo,
                &self.local_root_of(job_id)?,
                &original,
                &thumbnail,
                job_id,
                &intent.publication_id,
            ) {
                Ok(()) => report.admitted += 1,
                Err(failure) => report
                    .failures
                    .push((photo.photo_id.clone(), failure.to_string())),
            }
        }
        Ok(())
    }
    fn publish_pair(
        &self,
        photo: &super::super::RemotePhoto,
        root: &Path,
        original: &Path,
        thumbnail: &Path,
        job: i64,
        id: &str,
    ) -> Result<()> {
        if photo.is_hidden {
            return Err(error("hidden media requires explicit conflict resolution"));
        }
        verify_descriptor(original, &photo.original)?;
        verify_descriptor(thumbnail, &photo.thumbnail)?;
        let capture = super::super::store::recovery::capture(&*self.pool().get()?, id)?
            .ok_or_else(|| error("missing publication capture"))?;
        let relative = if capture["preserveMetadata"] == true {
            capture["localRelativePath"]
                .as_str()
                .ok_or_else(|| error("missing local metadata path"))?
        } else {
            &photo.relative_path
        };
        let target = safe_destination(root, relative)?;
        let parent = target
            .parent()
            .ok_or_else(|| error("publication has no parent"))?;
        std::fs::create_dir_all(parent)?;
        let extension = target
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        let next = parent.join(format!(
            ".photoviewer-sync-{}.new.{extension}",
            uuid::Uuid::new_v4()
        ));
        let backup = parent.join(format!(".photoviewer-sync-{}.old", uuid::Uuid::new_v4()));
        std::fs::copy(original, &next)?;
        let old = std::fs::symlink_metadata(&target).ok();
        if old.as_ref().is_some_and(|m| !m.is_file()) {
            let _ = std::fs::remove_file(&next);
            return Err(error("publication target is not an owned regular file"));
        }
        let conn = self.pool().get()?;
        let (baseline, old_path, mtime_seconds, mtime_ns): (
            bool,
            Option<String>,
            Option<i64>,
            Option<i64>,
        ) = conn.query_row(
            "SELECT e.native_has_baseline,m.path,m.file_mtime,m.file_mtime_ns
                 FROM sync_entries e LEFT JOIN media_items m ON m.id=e.media_id
                 WHERE e.job_id=?1 AND e.native_photo_id=?2",
            rusqlite::params![job, photo.photo_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        drop(conn);
        // The metadata row already determines the photo's fallback sort time.
        // Restore that timestamp on the owned temporary before cache import,
        // fsync and rename; a download must never become a new modification.
        if let Some(seconds) = mtime_seconds {
            let expected = match mtime_ns.filter(|ns| *ns != 0) {
                Some(ns) => chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(ns),
                None => chrono::DateTime::from_timestamp(seconds, 0)
                    .ok_or_else(|| error("invalid stored original modification time"))?,
            };
            std::fs::File::open(&next)?.set_modified(std::time::SystemTime::from(expected))?;
        }
        std::fs::File::open(&next)?.sync_all()?;
        if old.is_some() && (!baseline || old_path.as_deref() != target.to_str()) {
            let _ = std::fs::remove_file(&next);
            return Err(error("an unowned local file occupies the remote path"));
        }
        let uri = crate::core::file_uri::from_path(&target);
        let mtime = std::fs::metadata(&next)?.modified()?;
        let thumbnail_bytes = std::fs::read(thumbnail)?;
        for size in [
            crate::core::thumbnails::ThumbnailSize::Small,
            crate::core::thumbnails::ThumbnailSize::Medium,
            crate::core::thumbnails::ThumbnailSize::Large,
        ] {
            crate::core::thumbnails::import_cached_thumbnail(
                &crate::config::cache_dir(),
                &uri,
                size,
                Some(mtime),
                &thumbnail_bytes,
            )
            .map_err(|_| error("thumbnail cache preparation failed"))?;
        }
        let mut item = self.media_item(photo, &target, &next)?;
        if capture["preserveMetadata"] == true {
            item.taken_at = capture["takenAt"]
                .as_i64()
                .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0));
        }
        let old_local_stamp = old_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .as_ref()
            .and_then(|meta| stamp(Some(meta)));

        let details = json!({"target":target,"new":next,"backup":backup,"root":root,"hadOld":old.is_some(),"oldStamp":stamp(old.as_ref()),"oldPath":old_path,"oldLocalStamp":old_local_stamp,"sha256":photo.thumbnail_source_sha256,"size":photo.original.size});
        self.store().runtime(Command::Publishing {
            id: id.into(),
            details: details.clone(),
        })?;
        let _publication = super::super::publication_io::write()?;
        // Recheck the observed local file immediately before destructive placement.
        let now = std::fs::symlink_metadata(&target).ok();
        if stamp(old.as_ref()) != stamp(now.as_ref()) {
            return Err(error("local file changed during publication preparation"));
        }
        if old.is_some() {
            std::fs::hard_link(&target, &backup)?;
            std::fs::File::open(parent)?.sync_all()?;
        }
        std::fs::rename(&next, &target)?;
        std::fs::File::open(parent)?.sync_all()?;
        if let Err(failure) = self.store().runtime(Command::Admit {
            id: id.into(),
            photo: photo.clone(),
            item: Box::new(item),
        }) {
            if !matches_published(&target, &details)? {
                return Err(error(
                    "local file changed before rollback; previous complete backup retained",
                ));
            }
            if backup.is_file() {
                std::fs::rename(&backup, &target)?;
            } else if old.is_none() {
                std::fs::remove_file(&target)?;
            }
            std::fs::File::open(parent)?.sync_all()?;
            return Err(failure);
        }
        if backup.exists() {
            std::fs::remove_file(&backup)?;
            std::fs::File::open(parent)?.sync_all()?;
        }
        cleanup_renamed_source(&details)?;
        self.store()
            .runtime(Command::CleanPublication { id: id.into() })?;
        Ok(())
    }
    pub(super) fn recover_publications(&self, job: i64) -> Result<()> {
        let conn = self.pool().get()?;
        let rows = {
            let mut statement=conn.prepare("SELECT publication_id,publication_id,state FROM sync_publications WHERE job_id=?1 AND recovery_present=1 AND state IN ('publishing','complete')")?;
            let rows = statement
                .query_map([job], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        drop(conn);
        let _publication = super::super::publication_io::write()?;
        for (id, encoded, state) in rows {
            let details = super::super::store::recovery::details(&*self.pool().get()?, &encoded)?
                .ok_or_else(|| error("missing publication recovery details"))?;
            let value = |field: &str| -> Result<PathBuf> {
                details[field]
                    .as_str()
                    .map(PathBuf::from)
                    .ok_or_else(|| error("publication recovery path is missing"))
            };
            if details["collision"] == true {
                super::receive::collisions::recover_collision(self, &id, &details)?;
                continue;
            }
            let target = value("target")?;
            let backup = value("backup")?;
            let next = value("new")?;
            let root = value("root")?;
            let relative = target
                .strip_prefix(&root)
                .map_err(|_| error("publication recovery escaped its root"))?;
            if safe_destination(&root, &relative.to_string_lossy())? != target
                || backup.parent() != target.parent()
                || next.parent() != target.parent()
            {
                return Err(error("publication recovery path ownership mismatch"));
            }
            if state == "complete" {
                cleanup_renamed_source(&details)?;
                if backup.is_file() {
                    std::fs::remove_file(&backup)?;
                }
                if next.is_file() {
                    std::fs::remove_file(&next)?;
                }
                self.store().runtime(Command::CleanPublication { id })?;
                continue;
            }
            if backup.is_file() {
                // An external edit after the interruption is never discarded.
                let unchanged_old =
                    serde_json::to_value(stamp(std::fs::metadata(&target).ok().as_ref()))
                        .map_err(|_| error("invalid recovery file stamp"))?
                        == details["oldStamp"];
                if target.is_file() && !unchanged_old && !matches_published(&target, &details)? {
                    return Err(error(
                        "interrupted publication has a newer local edit; backup retained",
                    ));
                }
                std::fs::rename(&backup, &target)?;
            } else if details["hadOld"] == false && target.exists() {
                if !matches_published(&target, &details)? {
                    return Err(error("unowned file at interrupted publication target"));
                }
                std::fs::remove_file(&target)?;
            }
            if next.exists() {
                std::fs::remove_file(&next)?;
            }
            std::fs::File::open(
                target
                    .parent()
                    .ok_or_else(|| error("publication recovery has no parent"))?,
            )?
            .sync_all()?;
            self.store().runtime(Command::Rollback { id })?;
        }
        Ok(())
    }
    fn generation_of(&self, job: i64) -> Result<String> {
        self.store()
            .native_progress(job)?
            .map(|p| p.database_generation)
            .ok_or_else(|| error("job has no receive progress"))
    }
    fn local_root_of(&self, job: i64) -> Result<PathBuf> {
        self.store()
            .get_job(job)?
            .map(|j| j.local_root)
            .ok_or_else(|| error("job was removed"))
    }
    fn media_item(
        &self,
        photo: &super::super::RemotePhoto,
        target: &Path,
        source: &Path,
    ) -> Result<crate::core::media::NewMediaItem> {
        let metadata = crate::core::metadata::extract(source)?;
        Ok(crate::core::media::NewMediaItem {
            uri: crate::core::file_uri::from_path(target),
            path: target.to_path_buf(),
            folder_path: target.parent().unwrap_or(Path::new("/")).to_path_buf(),
            mime_type: metadata.mime_type,
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: metadata.width,
            height: metadata.height,
            video_duration_secs: None,
            taken_at: photo
                .asset_date_ms
                .and_then(chrono::DateTime::from_timestamp_millis)
                .or(metadata.taken_at),
            file_mtime: std::fs::metadata(source)?.modified()?.into(),
            file_size: photo.original.size,
            blake3_hash: String::new(),
        })
    }
    #[cfg(test)]
    fn stage_verified(&self, asset: &str, role: AssetRole, bytes: &[u8]) -> Result<PathBuf> {
        let folder = self.staging_root().join(role.as_str());
        std::fs::create_dir_all(&folder)?;
        let path = folder.join(sanitize_asset_id(asset));
        std::fs::write(&path, bytes)?;
        Ok(path)
    }
}
fn role_of(role: &str) -> Result<AssetRole> {
    match role {
        "original" => Ok(AssetRole::Original),
        "thumbnail" => Ok(AssetRole::Thumbnail),
        _ => Err(error("unknown asset role")),
    }
}
fn sanitize_asset_id(value: &str) -> String {
    if value.is_empty() {
        return "unnamed".into();
    }
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
pub(super) fn safe_destination(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() || relative.contains(['\\', '\0']) {
        return Err(error("unsafe relative photo path"));
    }
    let mut path = root.to_path_buf();
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        return Err(error("publication root is a symlink"));
    }
    for part in Path::new(relative).components() {
        match part {
            std::path::Component::Normal(part) => path.push(part),
            _ => return Err(error("photo path escapes its root")),
        }
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(error("photo path crosses a symlink"));
        }
    }
    Ok(path)
}
fn stamp(meta: Option<&std::fs::Metadata>) -> Option<(u64, u128)> {
    meta.map(|m| {
        (
            m.len(),
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos()),
        )
    })
}
fn verify_file(path: &Path, expected: &ContentDigest) -> Result<()> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(error("resource staging is a symlink"));
    }
    // The one hashing implementation in the tree, so a digest compared against
    // a descriptor here and a digest compared against local content during
    // identity adoption can never disagree about the same bytes.
    let actual = crate::core::sync::content::digest_file(path)?;
    if actual.size != expected.size {
        return Err(error("staged resource size mismatch"));
    }
    if actual.hex != expected.hex {
        return Err(error("staged resource checksum mismatch"));
    }
    Ok(())
}
fn verify_descriptor(path: &Path, descriptor: &super::super::AssetDescriptor) -> Result<()> {
    verify_file(
        path,
        &ContentDigest {
            size: descriptor.size,
            hex: ContentDigest::hex_from_descriptor_base64(&descriptor.file_checksum)
                .ok_or_else(|| error("invalid resource checksum"))?,
            base64: descriptor.file_checksum.clone(),
        },
    )
}
fn matches_published(path: &Path, details: &Value) -> Result<bool> {
    Ok(verify_file(
        path,
        &ContentDigest {
            size: details["size"]
                .as_u64()
                .ok_or_else(|| error("missing recovery size"))?,
            hex: details["sha256"]
                .as_str()
                .ok_or_else(|| error("missing recovery digest"))?
                .into(),
            base64: String::new(),
        },
    )
    .is_ok())
}

#[cfg(test)]
mod tests;

fn cleanup_renamed_source(details: &Value) -> Result<()> {
    let Some(old) = details["oldPath"].as_str() else {
        return Ok(());
    };
    let Some(target) = details["target"].as_str() else {
        return Err(error("publication target is missing"));
    };
    if old == target {
        return Ok(());
    }
    let old = Path::new(old);
    let root = Path::new(
        details["root"]
            .as_str()
            .ok_or_else(|| error("publication root is missing"))?,
    );
    let relative = old
        .strip_prefix(root)
        .map_err(|_| error("renamed source escaped its job root"))?;
    safe_destination(root, &relative.to_string_lossy())?;
    if let Ok(metadata) = std::fs::symlink_metadata(old) {
        let current = serde_json::to_value(stamp(Some(&metadata)))
            .map_err(|_| error("invalid renamed file stamp"))?;
        if current != details["oldLocalStamp"] {
            return Err(error(
                "previous path was edited during remote rename; file retained",
            ));
        }
        std::fs::remove_file(old)?;
        if let Some(parent) = old.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

fn log_published_batch(job_id: i64, admitted: usize) {
    if admitted > 0 {
        tracing::info!(
            target: crate::core::log_targets::SYNC,
            job_id, admitted,
            "云同步照片发布批次完成"
        );
    }
}
