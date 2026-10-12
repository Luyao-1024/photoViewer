//! Per-entry planning, verification and transfer execution.
//!
//! The coordinator persists the observation before resolving verification and
//! dispatching the resulting action. Helpers retain the original write, staging,
//! progress and rollback order.

use super::super::model::ConflictKind;
use super::*;

impl SyncService {
    pub(super) async fn reconcile_one(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        local_entry: Option<&LocalEntry>,
        remote_entry: Option<&RemoteEntry>,
        stored: Option<&StoredEntry>,
        upload_allowed: bool,
        summary: &mut RunSummary,
    ) -> Result<()> {
        let local_observation = observed_local(local_entry);
        let remote_observation = observed_remote(remote_entry, stored);
        let mut action = plan_entry(
            job,
            relative_path,
            local_observation,
            remote_observation.clone(),
            stored,
            upload_allowed,
        );

        let remote_revision = remote_entry
            .and_then(|entry| entry.revision.as_ref())
            .map(|revision| revision.value.as_str());
        let remote_revision_weak = remote_entry
            .and_then(|entry| entry.revision.as_ref())
            .is_some_and(|revision| revision.strength == RevisionStrength::Weak);
        let observed_state = action_state(&action);
        // The hot path of a no-change run: skip the write entirely when the
        // computed observation equals what the row already holds. At library
        // scale an unconditional upsert per path would rewrite every row and
        // emit every dirty event each round, even when nothing moved.
        let entry_id = match stored {
            Some(stored)
                if stored.local.as_ref() == local_entry.map(|entry| &entry.fingerprint)
                    && stored.local_mtime_ns == local_entry.map(|entry| entry.modified_ns)
                    && stored.remote.as_ref() == remote_observation.fingerprint()
                    && stored.remote_revision.as_deref() == remote_revision
                    && stored.remote_revision_weak == remote_revision_weak
                    && stored.state == observed_state =>
            {
                stored.id
            }
            _ => self.store.upsert_observation(
                job.id,
                relative_path,
                local_entry.map(|entry| &entry.fingerprint),
                local_entry.map(|entry| entry.modified_ns),
                remote_observation.fingerprint(),
                remote_revision,
                remote_revision_weak,
                observed_state,
            )?,
        };

        let verified_download = if matches!(
            action,
            PlanAction::SuppressLocalDeletion | PlanAction::VerifySuppressedRemoteContent
        ) {
            let Some(staged) = self
                .reconcile_suppressed_remote(
                    job,
                    provider,
                    relative_path,
                    remote_entry,
                    stored,
                    entry_id,
                    action,
                    summary,
                )
                .await?
            else {
                return Ok(());
            };
            action = PlanAction::DownloadNew;
            Some(staged)
        } else {
            None
        };

        if action == PlanAction::VerifyContent {
            let Some(verified_action) = self
                .verify_remote_content(
                    job,
                    provider,
                    relative_path,
                    local_entry,
                    remote_entry,
                    entry_id,
                    upload_allowed,
                    summary,
                )
                .await?
            else {
                return Ok(());
            };
            action = verified_action;
        }

        match action {
            PlanAction::Noop => self.reconcile_unchanged(
                job,
                local_entry,
                remote_entry,
                stored,
                &remote_observation,
                entry_id,
                summary,
            )?,
            PlanAction::UploadNew | PlanAction::UploadReplace { .. } => {
                self.reconcile_upload(
                    job,
                    provider,
                    relative_path,
                    local_entry,
                    remote_entry,
                    stored,
                    entry_id,
                    action,
                    summary,
                )
                .await?;
            }
            PlanAction::DownloadNew | PlanAction::DownloadReplace => {
                self.reconcile_download(
                    job,
                    provider,
                    relative_path,
                    local_entry,
                    remote_entry,
                    stored,
                    entry_id,
                    action,
                    verified_download,
                    summary,
                )
                .await?;
            }
            PlanAction::Conflict(kind) => {
                self.store.record_conflict(
                    job.id,
                    entry_id,
                    &format!("{kind:?}"),
                    local_entry.map(|entry| &entry.fingerprint),
                    remote_observation.fingerprint(),
                    remote_entry
                        .and_then(|entry| entry.revision.as_ref().map(|r| r.value.as_str())),
                )?;
                summary.conflicts += 1;
            }
            PlanAction::WaitForCompleteObservation => {}
            PlanAction::DeleteLocal | PlanAction::DeleteRemote { .. } => {
                return Err(AppError::Backend(
                    "automatic deletion is not enabled in the first synchronization release".into(),
                ));
            }
            PlanAction::VerifyContent
            | PlanAction::SuppressLocalDeletion
            | PlanAction::VerifySuppressedRemoteContent => unreachable!(),
        }
        Ok(())
    }

    async fn reconcile_suppressed_remote(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        remote_entry: Option<&RemoteEntry>,
        stored: Option<&StoredEntry>,
        entry_id: i64,
        action: PlanAction,
        summary: &mut RunSummary,
    ) -> Result<Option<PathBuf>> {
        let remote_revision = remote_entry
            .and_then(|entry| entry.revision.as_ref())
            .map(|revision| revision.value.as_str());
        let remote_revision_weak = remote_entry
            .and_then(|entry| entry.revision.as_ref())
            .is_some_and(|revision| revision.strength == RevisionStrength::Weak);
        let deleted = stored
            .and_then(|entry| entry.suppressed.as_ref().or(entry.baseline.as_ref()))
            .ok_or_else(|| {
                AppError::Backend("local deletion has no proven previous version".into())
            })?;
        if stored.is_none_or(|entry| entry.suppressed.as_ref() != Some(deleted)) {
            self.store.suppress_local_deletion(entry_id, deleted)?;
        }
        if action == PlanAction::SuppressLocalDeletion {
            summary.unchanged += 1;
            return Ok(None);
        }
        let remote = remote_entry.ok_or_else(|| {
            AppError::Backend("deleted version verification has no cloud file".into())
        })?;
        live_progress_begin(SyncLivePhase::Downloading, remote.size);
        let staged = self
            .download_to_staging(job, provider, relative_path, remote.revision.as_ref())
            .await?;
        let fingerprint = local::fingerprint(&staged)?;
        self.store.upsert_observation(
            job.id,
            relative_path,
            None,
            None,
            Some(&fingerprint),
            remote_revision,
            remote_revision_weak,
            "local_deleted",
        )?;
        if &fingerprint == deleted {
            remove_file_if_exists(&staged)?;
            live_progress_complete_download();
            summary.unchanged += 1;
            return Ok(None);
        }
        Ok(Some(staged))
    }

    async fn verify_remote_content(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        local_entry: Option<&LocalEntry>,
        remote_entry: Option<&RemoteEntry>,
        entry_id: i64,
        upload_allowed: bool,
        summary: &mut RunSummary,
    ) -> Result<Option<PlanAction>> {
        let remote_revision = remote_entry
            .and_then(|entry| entry.revision.as_ref())
            .map(|revision| revision.value.as_str());
        let action;
        let remote = remote_entry
            .ok_or_else(|| AppError::Backend("verification requires a remote object".into()))?;

        // Proving two sides are *identical* costs a full remote download,
        // because WebDAV listings carry no content hash — only size, mtime
        // and ETag. Two guards keep that cost bounded:
        //
        // 1. A length mismatch already proves the contents differ. The
        //    cheap answer must not be paid for with the expensive one.
        // 2. Above the cap, the download cannot finish inside the single
        //    request timeout, so attempting it burns the whole budget and
        //    fails every round. That is not "slow"; it is a livelock on one
        //    path that no amount of waiting resolves, so report the
        //    evidence gap instead of transferring gigabytes to learn
        //    nothing. The conflict surfaces in Settings with a retry.
        let local_size = local_entry.map(|entry| entry.fingerprint.size);
        if local_size.is_some_and(|size| size != remote.size) {
            tracing::info!(
                target: crate::core::log_targets::STORAGE,
                job_id = job.id,
                path = %relative_path,
                local_bytes = ?local_size,
                remote_bytes = remote.size,
                "content differs by length; skipping remote hash"
            );
            action = if upload_allowed {
                // No transfer happened, but this path still resolved to a
                // conflict; count it so the planned download total advances.
                live_progress_complete_download();
                PlanAction::Conflict(ConflictKind::InitialContentMismatch)
            } else {
                PlanAction::DownloadReplace
            };
        } else if let Some(remote_sha256) =
            provider.content_sha256(remote).map_err(provider_error)?
        {
            let local = local_entry.ok_or_else(|| {
                AppError::Backend("SHA-256 verification requires a local file".into())
            })?;
            if local::content_sha256(local)? == remote_sha256 {
                self.store
                    .commit_baseline(entry_id, &local.fingerprint, remote_revision)?;
                summary.verified += 1;
                live_progress_complete_download();
                tracing::info!(target: crate::core::log_targets::STORAGE, job_id = job.id,
                    path = %relative_path, bytes = remote.size,
                    "verified cloud copy matches local using SHA-256; no download needed");
                return Ok(None);
            }
            if upload_allowed {
                live_progress_complete_download();
                action = PlanAction::Conflict(ConflictKind::InitialContentMismatch);
            } else {
                action = PlanAction::DownloadReplace;
            }
        } else if remote.size > MAX_VERIFY_DOWNLOAD_BYTES {
            tracing::warn!(
                target: crate::core::log_targets::STORAGE,
                job_id = job.id,
                path = %relative_path,
                bytes = remote.size,
                "remote object exceeds the content-verification cap; recording insufficient evidence"
            );
            action = PlanAction::Conflict(ConflictKind::InsufficientEvidence);
        } else {
            live_progress_begin(SyncLivePhase::Downloading, remote.size);
            let verified = self
                .download_to_staging(job, provider, relative_path, remote.revision.as_ref())
                .await?;
            let remote_fingerprint = local::fingerprint(&verified)?;
            if local_entry.is_some_and(|entry| entry.fingerprint == remote_fingerprint) {
                self.store.commit_baseline(
                    entry_id,
                    &remote_fingerprint,
                    remote
                        .revision
                        .as_ref()
                        .map(|revision| revision.value.as_str()),
                )?;
                remove_file_if_exists(&verified)?;
                summary.verified += 1;
                live_progress_complete_download();
                tracing::info!(
                    target: crate::core::log_targets::STORAGE,
                    job_id = job.id,
                    path = %relative_path,
                    bytes = remote.size,
                    "verified cloud copy matches local"
                );
                return Ok(None);
            }
            remove_file_if_exists(&verified)?;
            action = if upload_allowed {
                // Verification consumed this path's planned download and
                // resolved it as a conflict; count it so the label advances.
                live_progress_complete_download();
                PlanAction::Conflict(ConflictKind::InitialContentMismatch)
            } else {
                // The replacement below re-downloads and counts on publish.
                PlanAction::DownloadReplace
            };
        }

        Ok(Some(action))
    }

    fn reconcile_unchanged(
        &self,
        job: &SyncJob,
        local_entry: Option<&LocalEntry>,
        remote_entry: Option<&RemoteEntry>,
        stored: Option<&StoredEntry>,
        remote_observation: &Observation,
        entry_id: i64,
        summary: &mut RunSummary,
    ) -> Result<()> {
        if let (Some(local), Some(remote)) = (local_entry, remote_entry) {
            if remote_observation
                .fingerprint()
                .is_some_and(|fingerprint| fingerprint == &local.fingerprint)
            {
                let revision = remote
                    .revision
                    .as_ref()
                    .map(|revision| revision.value.as_str());
                // Same idea as the observation guard above: advancing
                // a baseline that already names this exact fingerprint
                // and revision is a write with no effect.
                let committed = stored.is_some_and(|stored| {
                    let fingerprint = &local.fingerprint;
                    stored.local.as_ref() == Some(fingerprint)
                        && stored.remote.as_ref() == Some(fingerprint)
                        && stored.baseline.as_ref() == Some(fingerprint)
                        && stored.remote_revision.as_deref() == revision
                        && stored.baseline_remote_revision.as_deref() == revision
                        && stored.state == "synced"
                });
                if !committed {
                    self.store
                        .commit_baseline(entry_id, &local.fingerprint, revision)?;
                }
                if job.direction == SyncDirection::UploadOnly
                    && stored.is_some_and(|entry| entry.state == "conflict")
                {
                    self.finish_backup_conflicts(job, entry_id, &local.fingerprint, revision)?;
                }
            }
        }
        summary.unchanged += 1;

        Ok(())
    }

    async fn reconcile_upload(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        local_entry: Option<&LocalEntry>,
        remote_entry: Option<&RemoteEntry>,
        stored: Option<&StoredEntry>,
        entry_id: i64,
        action: PlanAction,
        summary: &mut RunSummary,
    ) -> Result<()> {
        let local = local_entry
            .ok_or_else(|| AppError::Backend("upload plan has no local source".into()))?;
        let operation_id = next_operation_id(job.id);
        let snapshot_path = self.create_upload_snapshot(job, local, &operation_id)?;
        let snapshot_fingerprint = local::fingerprint(&snapshot_path)?;
        if snapshot_fingerprint != local.fingerprint {
            remove_file_if_exists(&snapshot_path)?;
            return Err(AppError::Backend(format!(
                "local file changed before upload: {}",
                local.absolute_path.display()
            )));
        }
        let (condition, expected, task_action) = match action {
            PlanAction::UploadNew => (WriteCondition::CreateOnly, None, "upload_new"),
            PlanAction::UploadReplace { expected } => (
                WriteCondition::ReplaceIf(expected.clone()),
                Some(expected),
                "upload_replace",
            ),
            _ => unreachable!(),
        };
        self.store.prepare_task(
            &operation_id,
            job.id,
            entry_id,
            task_action,
            expected.as_ref(),
            &snapshot_path,
            &snapshot_fingerprint.blake3,
            job.config_generation,
            stored.map_or(1, |entry| entry.generation.saturating_add(1)),
        )?;
        if job.direction == SyncDirection::UploadOnly {
            self.archive_backup_upload(
                job,
                provider,
                relative_path,
                remote_entry,
                &snapshot_path,
                &snapshot_fingerprint,
                &operation_id,
            )
            .await?;
        }
        let key = remote_key(&job.remote_root, relative_path)?;
        live_progress_begin(SyncLivePhase::Uploading, snapshot_fingerprint.size);
        let uploaded = match provider.upload(&key, &snapshot_path, condition).await {
            Ok(uploaded) => uploaded,
            Err(error) => {
                self.store.set_task_state(
                    &operation_id,
                    "reconciling",
                    Some(&error.to_string()),
                )?;
                return Err(provider_error(error));
            }
        };
        let current = local::fingerprint(&local.absolute_path)?;
        if let Err(error) = self.store.commit_baseline(
            entry_id,
            &snapshot_fingerprint,
            uploaded
                .revision
                .as_ref()
                .map(|revision| revision.value.as_str()),
        ) {
            self.store
                .set_task_state(&operation_id, "reconciling", Some(&error.to_string()))?;
            return Err(error);
        }
        if job.direction == SyncDirection::UploadOnly
            && stored.is_some_and(|entry| entry.state == "conflict")
        {
            self.finish_backup_conflicts(
                job,
                entry_id,
                &snapshot_fingerprint,
                uploaded
                    .revision
                    .as_ref()
                    .map(|revision| revision.value.as_str()),
            )?;
        }
        if current != snapshot_fingerprint {
            self.store.upsert_observation(
                job.id,
                relative_path,
                Some(&current),
                None,
                Some(&snapshot_fingerprint),
                uploaded
                    .revision
                    .as_ref()
                    .map(|revision| revision.value.as_str()),
                uploaded
                    .revision
                    .as_ref()
                    .is_some_and(|revision| revision.strength == RevisionStrength::Weak),
                "pending",
            )?;
        }
        self.store
            .set_task_state(&operation_id, "succeeded", None)?;
        remove_file_if_exists(&snapshot_path)?;
        summary.uploaded += 1;
        live_progress_complete_upload();
        tracing::info!(
            target: crate::core::log_targets::STORAGE,
            job_id = job.id,
            path = %relative_path,
            bytes = snapshot_fingerprint.size,
            "uploaded to cloud"
        );

        Ok(())
    }

    async fn reconcile_download(
        &self,
        job: &SyncJob,
        provider: &dyn SyncProvider,
        relative_path: &str,
        local_entry: Option<&LocalEntry>,
        remote_entry: Option<&RemoteEntry>,
        stored: Option<&StoredEntry>,
        entry_id: i64,
        action: PlanAction,
        mut verified_download: Option<PathBuf>,
        summary: &mut RunSummary,
    ) -> Result<()> {
        let remote = remote_entry
            .ok_or_else(|| AppError::Backend("download plan has no remote source".into()))?;
        live_progress_begin(SyncLivePhase::Downloading, remote.size);
        let staged = match verified_download.take() {
            Some(staged) => staged,
            None => {
                self.download_to_staging(job, provider, relative_path, remote.revision.as_ref())
                    .await?
            }
        };
        let fingerprint = local::fingerprint(&staged)?;
        let operation_id = staged
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| AppError::Backend("invalid staging artifact name".into()))?
            .to_string();
        self.store.prepare_task(
            &operation_id,
            job.id,
            entry_id,
            if action == PlanAction::DownloadNew {
                "download_new"
            } else {
                "download_replace"
            },
            remote.revision.as_ref(),
            &staged,
            &fingerprint.blake3,
            job.config_generation,
            stored.map_or(1, |entry| entry.generation.saturating_add(1)),
        )?;
        let target = local::destination(&job.local_root, relative_path)?;
        let backup = match action {
            PlanAction::DownloadNew => {
                local::atomic_publish_new(&staged, &target)?;
                None
            }
            PlanAction::DownloadReplace => {
                if let Some(local) = local_entry {
                    let current = local::fingerprint(&local.absolute_path)?;
                    if current != local.fingerprint {
                        remove_file_if_exists(&staged)?;
                        return Err(AppError::Backend(format!(
                            "local file changed during download: {}",
                            target.display()
                        )));
                    }
                }
                Some(local::atomic_publish_replace(
                    &staged,
                    &target,
                    &operation_id,
                )?)
            }
            _ => unreachable!(),
        };
        let commit: Result<()> = (|| {
            if let Some(item) = LocalBackend::new_item_from_path(&target)? {
                if let Some(actor) = &self.actor {
                    actor.execute_blocking(crate::core::db_actor::DbCommand::UpsertMediaBatch {
                        source: crate::core::events::ChangeSource::Synchronization,
                        items: vec![item],
                    })?;
                } else {
                    LocalBackend::new(self.pool.clone()).upsert(&item)?;
                }
            }
            self.store.commit_baseline(
                entry_id,
                &fingerprint,
                remote
                    .revision
                    .as_ref()
                    .map(|revision| revision.value.as_str()),
            )?;
            Ok(())
        })();
        if let Err(error) = commit {
            self.store
                .set_task_state(&operation_id, "reconciling", Some(&error.to_string()))?;
            if let Some(backup) = &backup {
                let failed = backup.with_extension("downloaded-uncommitted");
                let preserve = std::fs::rename(&target, &failed);
                let restore = std::fs::rename(backup, &target);
                if let (Err(preserve), Err(restore)) = (preserve, restore) {
                    return Err(AppError::Backend(format!(
                        "download commit failed: {error}; preserving new file failed: {preserve}; restoring old file failed: {restore}; backup remains at {}",
                        backup.display()
                    )));
                }
            }
            return Err(error);
        }
        if let Some(backup) = backup {
            remove_file_if_exists(&backup)?;
        }
        self.store
            .set_task_state(&operation_id, "succeeded", None)?;
        remove_file_if_exists(&staged)?;
        summary.downloaded += 1;
        live_progress_complete_download();
        tracing::info!(
            target: crate::core::log_targets::STORAGE,
            job_id = job.id,
            path = %relative_path,
            bytes = remote.size,
            "downloaded from cloud"
        );

        Ok(())
    }
}
