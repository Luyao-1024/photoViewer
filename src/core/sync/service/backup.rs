//! Immutable, content-addressed backups published before the current cloud copy.
use super::super::model::Fingerprint;
use super::*;

pub(super) const BACKUP_DIRECTORY: &str = ".photoviewer-backup";

impl SyncService {
    pub(super) fn finish_backup_conflicts(
        &self,
        job: &SyncJob,
        entry_id: i64,
        fingerprint: &Fingerprint,
        revision: Option<&str>,
    ) -> Result<()> {
        for conflict in self.store.open_conflicts(job.id)? {
            if conflict.entry_id == entry_id {
                self.store.resolve_conflict(
                    conflict.id,
                    entry_id,
                    "backup",
                    fingerprint,
                    revision,
                )?;
            }
        }
        Ok(())
    }

    pub(super) async fn archive_backup_upload(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        remote: Option<&RemoteEntry>,
        snapshot: &Path,
        fingerprint: &Fingerprint,
        operation_id: &str,
    ) -> Result<()> {
        // A pre-existing or externally modified cloud version also deserves a
        // recoverable copy before conditional replacement.
        if let Some(remote) = remote {
            let existing = self
                .download_to_staging(job, provider, relative_path, remote.revision.as_ref())
                .await?;
            let old = local::fingerprint(&existing)?;
            let result = self
                .archive_backup_version(job, provider, relative_path, &existing, &old, operation_id)
                .await;
            remove_file_if_exists(&existing)?;
            result?;
        }
        self.archive_backup_version(
            job,
            provider,
            relative_path,
            snapshot,
            fingerprint,
            operation_id,
        )
        .await
    }

    pub(super) async fn archive_backup_version(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        snapshot: &Path,
        fingerprint: &Fingerprint,
        operation_id: &str,
    ) -> Result<()> {
        let directory = remote_key(&job.remote_root, BACKUP_DIRECTORY)?;
        for key in [
            &directory,
            &format!("{directory}/objects"),
            &format!("{directory}/records"),
        ] {
            provider
                .ensure_collection(key)
                .await
                .map_err(provider_error)?;
        }
        let extension = Path::new(relative_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("bin");
        let object_relative = format!(
            "{BACKUP_DIRECTORY}/objects/{}.{}",
            fingerprint.blake3, extension
        );
        let key = remote_key(&job.remote_root, &object_relative)?;
        // Always verify an existing archive: size/ETag alone cannot establish its
        // contents after external modification. Do not overwrite a damaged archive.
        self.upload_new_or_verify(provider, &key, snapshot, fingerprint, job)
            .await?;
        let identity =
            blake3::hash(format!("{relative_path}\0{}", fingerprint.blake3).as_bytes()).to_hex();
        let record_key = format!("{directory}/records/{identity}.json");
        let record = serde_json::to_vec(&serde_json::json!({
            "format": 1, "relative_path": relative_path,
            "blake3": fingerprint.blake3, "size": fingerprint.size,
            "object": object_relative,
        }))
        .map_err(|e| AppError::Backend(format!("cannot encode backup version: {e}")))?;
        let directory = self.staging_root.join(job.id.to_string());
        std::fs::create_dir_all(&directory)?;
        let artifact = directory.join(format!("{operation_id}.backup-record"));
        std::fs::write(&artifact, record)?;
        let record_fingerprint = local::fingerprint(&artifact)?;
        let result = self
            .upload_new_or_verify(provider, &record_key, &artifact, &record_fingerprint, job)
            .await;
        remove_file_if_exists(&artifact)?;
        result?;
        Ok(())
    }
}
