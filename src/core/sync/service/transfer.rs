//! The download half: fetch a promised pair, prove it, publish it, admit it.
//!
//! Every step here is written so that a crash between any two of them leaves
//! the previous complete version in place and the work still owed:
//!
//! ```text
//! obligation → staged (bytes verified in owned staging)
//!           → published (file moved into the job root)
//!           → verified (cache artifact exists)
//!           → complete (both roles verified)
//!           → admitted (media row written, once)
//! ```
//!
//! The order is not cosmetic. A photo that became visible with only its
//! thumbnail would show an image whose original nothing agrees with, and a
//! photo admitted twice would fight the ordinary scanner over the same path.
//! So admission is last, it is idempotent, and it happens exactly once.

use std::path::{Path, PathBuf};

use crate::core::error::{AppError, Result};

use super::super::content::ContentDigest;
use super::super::native::assets::AssetRole;
use super::super::native::protocol::NativeClient;
use super::super::store::publication::ResourceWork;
use super::SyncService;

/// What one transfer pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferReport {
    pub fetched: usize,
    pub published: usize,
    pub admitted: usize,
    /// Resource ids that failed, with the reason. Reported, never swallowed.
    pub failures: Vec<(String, String)>,
}

/// A resource's bytes, proven against what the cloud promised.
#[derive(Debug, Clone)]
pub struct VerifiedResource {
    pub asset_id: String,
    pub role: AssetRole,
    /// In owned staging, matching the declared digest and size.
    pub staged: PathBuf,
    pub digest: ContentDigest,
}

impl SyncService {
    /// Fetch every obligation this job still owes, publish what verifies, and
    /// admit only the photos whose whole pair is usable.
    ///
    /// Nothing here treats a fetch as complete because it returned bytes: the
    /// declared size and digest are checked, and a mismatch fails the resource
    /// rather than retrying the same promise.
    pub async fn run_transfers(&self, job_id: i64) -> Result<TransferReport> {
        let mut report = TransferReport::default();
        let outstanding = self.store().outstanding_resources(job_id)?;
        for obligation in outstanding {
            match self.fetch_one(&obligation, &mut report).await {
                Ok(Some(_)) => report.fetched += 1,
                // Already staged from an earlier pass: nothing to re-fetch.
                Ok(None) => {}
                Err(error) => {
                    let reason = error.to_string();
                    let _ = self.store().fail_resource(
                        job_id,
                        &obligation.asset_id,
                        &obligation.role,
                        &reason,
                    );
                    report.failures.push((obligation.asset_id.clone(), reason));
                }
            }
        }
        self.admit_complete_publications(job_id, &mut report)?;
        Ok(report)
    }

    /// Fetch one obligation. `Ok(None)` means its bytes are already staged.
    async fn fetch_one(
        &self,
        obligation: &ResourceWork,
        report: &mut TransferReport,
    ) -> Result<Option<VerifiedResource>> {
        let role = role_of(&obligation.role)?;
        if let Some(staged) = obligation.staging_path.as_deref() {
            if Path::new(staged).is_file() {
                return Ok(None);
            }
            // A recorded stage that is gone is re-fetched, not trusted.
        }
        let client = self.transfer_client().await?;
        let descriptor = client
            .describe_asset(&obligation.asset_id, role)
            .await
            .map_err(|error| AppError::Backend(error.to_string()))?;
        let bytes = client
            .download_asset(&descriptor, role, None)
            .await
            .map_err(|error| AppError::Backend(error.to_string()))?;

        // The promise is checked against the bytes before anything is written.
        // The size is the descriptor's; the digest is re-derived from what
        // arrived, never copied from the answer about what was sent.
        let digest = ContentDigest::of(&bytes.bytes);
        let expected_size = obligation
            .expected_size
            .map(|size| size as u64)
            .unwrap_or(descriptor.descriptor.size);
        if digest.size != expected_size {
            return Err(AppError::Backend(format!(
                "asset {} delivered {} bytes, the projection promised {expected_size}",
                obligation.asset_id, digest.size
            )));
        }
        // The projection's promise wins; the descriptor's is the fallback for a
        // row that predates it. Neither is taken on trust — the digest below is
        // computed from the bytes that actually arrived.
        let from_descriptor =
            ContentDigest::hex_from_descriptor_base64(&descriptor.descriptor.file_checksum);
        let expected = obligation.expected_sha256.clone().or(from_descriptor);
        if let Some(expected) = expected {
            if digest.hex != expected {
                return Err(AppError::Backend(format!(
                    "asset {} hashed to {}, the projection promised {expected}",
                    obligation.asset_id, digest.hex
                )));
            }
        }

        let staged = self.stage_verified(&obligation.asset_id, role, &bytes.bytes)?;
        self.store().stage_resource(
            obligation.job_id,
            &obligation.asset_id,
            role.as_str(),
            &staged.to_string_lossy(),
        )?;
        let _ = report;
        Ok(Some(VerifiedResource {
            asset_id: obligation.asset_id.clone(),
            role,
            staged,
            digest,
        }))
    }

    /// Write verified bytes into owned staging and flush them.
    ///
    /// The file is named for the asset, never for a remote URL: a URL changes
    /// on every describe and would leave an ever-growing set of "owned" files.
    fn stage_verified(&self, asset_id: &str, role: AssetRole, bytes: &[u8]) -> Result<PathBuf> {
        let staging = self.staging_root().join(role.as_str());
        std::fs::create_dir_all(&staging).map_err(|error| {
            AppError::Backend(format!("cannot create the staging directory: {error}"))
        })?;
        let path = staging.join(sanitize_asset_id(asset_id));
        let temporary = path.with_extension("incoming");
        {
            let mut file = std::fs::File::create(&temporary).map_err(|error| {
                AppError::Backend(format!("cannot stage the download: {error}"))
            })?;
            std::io::Write::write_all(&mut file, bytes).map_err(|error| {
                AppError::Backend(format!("cannot stage the download: {error}"))
            })?;
            file.sync_all().map_err(|error| {
                AppError::Backend(format!("cannot flush the staged download: {error}"))
            })?;
        }
        std::fs::rename(&temporary, &path).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            AppError::Backend(format!("cannot publish the staged download: {error}"))
        })?;
        Ok(path)
    }

    /// Admit the photos whose whole pair is proven usable.
    ///
    /// A publication that is not complete is simply not visited, so a
    /// half-downloaded photo never reaches `media_items`.
    fn admit_complete_publications(&self, job_id: i64, report: &mut TransferReport) -> Result<()> {
        let intents = self.store().pending_publications(job_id)?;
        for intent in intents {
            // Ready is read-only on purpose: if admission then fails, the intent
            // stays pending and the next cycle tries again. Marking it complete
            // first would lose the photo instead.
            if !self.store().publication_ready(&intent.publication_id)? {
                continue;
            }
            // The projection says where this photo belongs and what it is; the
            // pair being verified says the bytes exist. Both are required.
            let graph = self
                .store()
                .remote_graph(job_id, &self.generation_of(job_id)?)?;
            let Some(photo) = graph
                .photos
                .iter()
                .find(|photo| photo.photo_id == intent.photo_id)
            else {
                report.failures.push((
                    intent.photo_id.clone(),
                    "the photo left the projection before it could be admitted".into(),
                ));
                continue;
            };
            let local_root = self.local_root_of(job_id)?;
            let original =
                self.staging_root()
                    .join(AssetRole::Original.as_str())
                    .join(sanitize_asset_id(
                        intent.original_asset_id.as_deref().unwrap_or("missing"),
                    ));
            let thumbnail = self
                .staging_root()
                .join(AssetRole::Thumbnail.as_str())
                .join(sanitize_asset_id(
                    intent.thumbnail_asset_id.as_deref().unwrap_or("missing"),
                ));
            if !original.is_file() || !thumbnail.is_file() {
                report.failures.push((
                    intent.photo_id.clone(),
                    "a verified role has no staged bytes".into(),
                ));
                continue;
            }
            match self.admit_one(photo, &local_root, &original, &thumbnail, job_id) {
                Ok(()) => {
                    // Only now may the intent be closed: the photo is visible.
                    self.store().complete_publication(&intent.publication_id)?;
                    report.admitted += 1;
                }
                Err(error) => report
                    .failures
                    .push((intent.photo_id.clone(), error.to_string())),
            }
        }
        Ok(())
    }

    fn generation_of(&self, job_id: i64) -> Result<String> {
        self.store()
            .native_progress(job_id)?
            .map(|progress| progress.database_generation)
            .ok_or_else(|| AppError::Backend("no receive position exists yet".into()))
    }

    fn local_root_of(&self, job_id: i64) -> Result<PathBuf> {
        self.store()
            .get_job(job_id)?
            .map(|job| job.local_root)
            .ok_or_else(|| AppError::Backend("the synchronization job no longer exists".into()))
    }

    /// Publish one pair and write its media row — once.
    fn admit_one(
        &self,
        photo: &super::super::RemotePhoto,
        local_root: &Path,
        original: &Path,
        thumbnail: &Path,
        job_id: i64,
    ) -> Result<()> {
        let destination = local_root.join(&photo.relative_path);
        let parent = destination
            .parent()
            .ok_or_else(|| AppError::Backend("a photo path has no parent directory".into()))?;
        std::fs::create_dir_all(parent).map_err(|error| {
            AppError::Backend(format!("cannot create the photo folder: {error}"))
        })?;

        let previous = std::fs::metadata(&destination)
            .ok()
            .map(|_| destination.clone());
        // The old complete file stays until the new one is in place, so a
        // failure here leaves the previous version readable. A first publish
        // copies, leaving the staged original for a possible rollback; a
        // replacement moves it, because the previous file already serves.
        if previous.is_none() {
            std::fs::copy(original, &destination).map_err(|error| {
                AppError::Backend(format!("cannot publish the photo original: {error}"))
            })?;
        } else {
            std::fs::rename(original, &destination).map_err(|error| {
                let _ = std::fs::copy(original, &destination);
                AppError::Backend(format!("cannot replace the photo original: {error}"))
            })?;
        }

        // The thumbnail is a cache artifact, not a file in the library: it is
        // imported through the ordinary cache so the grid, the viewer and the
        // memo cache all keep working exactly as they do for a local photo.
        let cache_dir = crate::config::cache_dir();
        let bytes = std::fs::read(thumbnail).map_err(|error| {
            AppError::Backend(format!("cannot read the staged thumbnail: {error}"))
        })?;
        let mtime = std::fs::metadata(&destination)
            .and_then(|metadata| metadata.modified())
            .ok();
        let uri = crate::core::file_uri::from_path(&destination);
        crate::core::thumbnails::import_cached_thumbnail(
            &cache_dir,
            &uri,
            crate::core::thumbnails::ThumbnailSize::Medium,
            mtime,
            &bytes,
        )
        .map_err(|error| AppError::Backend(format!("cannot import the thumbnail: {error}")))?;
        for role in [AssetRole::Original, AssetRole::Thumbnail] {
            let asset_id = if role == AssetRole::Original {
                photo.original.asset_id.clone()
            } else {
                photo.thumbnail.asset_id.clone()
            };
            self.store()
                .verify_resource(job_id, &asset_id, role.as_str(), &uri)?;
        }

        self.write_media_row(photo, &destination)?;
        Ok(())
    }

    /// Write the media row for a published pair.
    ///
    /// The row is written through the ordinary media path so the scanner,
    /// watcher and grid all see the same shape they always have. The thumbnail's
    /// generation stamp is **not** touched: a cache import is not a generation,
    /// and setting it would make the pipeline skip work it still owes.
    fn write_media_row(&self, photo: &super::super::RemotePhoto, destination: &Path) -> Result<()> {
        use crate::core::media::NewMediaItem;
        let metadata = crate::core::metadata::extract(destination).ok();
        let uri = crate::core::file_uri::from_path(destination);
        let mtime = std::fs::metadata(destination)
            .and_then(|metadata| metadata.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now())
            .into();
        let item = NewMediaItem {
            uri,
            path: destination.to_path_buf(),
            folder_path: destination
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            mime_type: metadata
                .as_ref()
                .map(|metadata| metadata.mime_type.clone())
                .unwrap_or_else(|| "image/jpeg".to_string()),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: metadata.as_ref().and_then(|metadata| metadata.width),
            height: metadata.as_ref().and_then(|metadata| metadata.height),
            video_duration_secs: None,
            taken_at: photo
                .asset_date_ms
                .and_then(chrono::DateTime::from_timestamp_millis)
                .or_else(|| metadata.as_ref().and_then(|metadata| metadata.taken_at)),
            file_mtime: mtime,
            file_size: std::fs::metadata(destination)
                .map(|meta| meta.len())
                .unwrap_or(0),
            blake3_hash: String::new(),
        };
        crate::core::db::upsert_media_items_batch(self.pool(), &[item])?;
        Ok(())
    }

    async fn transfer_client(&self) -> Result<NativeClient> {
        let job_id = self
            .store()
            .list_jobs()
            .map_err(|error| AppError::Backend(error.to_string()))?
            .first()
            .map(|job| job.id)
            .unwrap_or_default();
        let job = self
            .store()
            .get_job(job_id)
            .map_err(|error| AppError::Backend(error.to_string()))?
            .ok_or_else(|| AppError::Backend("no synchronization job exists".into()))?;
        self.transport_client_for(&job).await
    }
}

fn role_of(value: &str) -> Result<AssetRole> {
    match value {
        "original" => Ok(AssetRole::Original),
        "thumbnail" => Ok(AssetRole::Thumbnail),
        other => Err(AppError::Backend(format!(
            "unknown resource role {other:?}"
        ))),
    }
}

/// An asset id is a UUID today; sanitizing keeps a future id from escaping the
/// staging directory, and keeps a hostile one from naming a path.
fn sanitize_asset_id(asset_id: &str) -> String {
    let safe: String = asset_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(96)
        .collect();
    if safe.is_empty() {
        "unnamed".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests;
