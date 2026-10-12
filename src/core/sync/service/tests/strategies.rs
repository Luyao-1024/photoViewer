use super::*;

fn fixture(
    direction: SyncDirection,
    selected: bool,
) -> (tempfile::TempDir, SyncService, SyncJob, Arc<FakeProvider>) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("library");
    std::fs::create_dir_all(root.join("camera")).unwrap();
    let service = recovery_service(temp.path());
    let job = service
        .store
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "strategies".into(),
            etag_is_sha256: false,
            local_root: root,
            remote_root: "PhotoViewer".into(),
            direction,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: if selected {
                vec!["camera".into()]
            } else {
                Vec::new()
            },
        })
        .unwrap();
    (temp, service, job, Arc::new(FakeProvider::default()))
}

fn image(version: u8) -> Vec<u8> {
    let mut bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media/animated_source.gif"),
    )
    .unwrap();
    bytes.extend_from_slice(&[version; 8]);
    bytes
}

#[tokio::test]
async fn sync_strategy_deleted_local_stays_absent_until_remote_content_changes() {
    for selected in [false, true] {
        let (temp, service, job, provider) = fixture(SyncDirection::Bidirectional, selected);
        let target = job.local_root.join("camera/picture.gif");
        provider.insert("PhotoViewer/camera/picture.gif", image(1));
        assert_eq!(
            service
                .run_with_provider(&job, provider.clone())
                .await
                .unwrap()
                .downloaded,
            1
        );
        std::fs::remove_file(&target).unwrap();
        assert_eq!(
            service
                .run_with_provider(&job, provider.clone())
                .await
                .unwrap()
                .downloaded,
            0
        );
        assert!(!target.exists());
        assert_eq!(
            provider.objects.lock().unwrap()["PhotoViewer/camera/picture.gif"],
            image(1)
        );
        assert!(service.store.open_conflicts(job.id).unwrap().is_empty());
        // Reopen the database: suppression must outlive this client session and media index rows.
        let reopened = recovery_service(temp.path());
        let pool = reopened.pool.get().unwrap();
        pool.execute("DELETE FROM media_items", []).unwrap();
        drop(pool);
        assert_eq!(
            reopened
                .run_with_provider(&job, provider.clone())
                .await
                .unwrap()
                .downloaded,
            0
        );
        assert!(!target.exists());
        provider.insert("PhotoViewer/camera/picture.gif", image(2));
        assert_eq!(
            reopened
                .run_with_provider(&job, provider.clone())
                .await
                .unwrap()
                .downloaded,
            1
        );
        assert_eq!(std::fs::read(&target).unwrap(), image(2));
        std::fs::remove_file(&target).unwrap();
        assert_eq!(
            reopened
                .run_with_provider(&job, provider.clone())
                .await
                .unwrap()
                .downloaded,
            0
        );
        provider.insert("PhotoViewer/camera/picture.gif", image(3));
        assert_eq!(
            reopened
                .run_with_provider(&job, provider.clone())
                .await
                .unwrap()
                .downloaded,
            1
        );
    }
}

#[tokio::test]
async fn sync_strategy_etag_change_without_content_change_does_not_restore() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, true);
    provider.insert("PhotoViewer/camera/picture.gif", image(1));
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    let target = job.local_root.join("camera/picture.gif");
    std::fs::remove_file(&target).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    // Forget the cached remote validator to force content verification, as an ETag change does.
    service
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE sync_entries SET remote_revision='different-etag',remote_fingerprint=NULL",
            [],
        )
        .unwrap();
    assert_eq!(
        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap()
            .downloaded,
        0
    );
    assert!(!target.exists());
    let calls = provider.download_calls.load(Ordering::Relaxed);
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(
        provider.download_calls.load(Ordering::Relaxed),
        calls,
        "unchanged verified content must be cached"
    );
}

#[tokio::test]
async fn sync_strategy_unavailable_local_root_does_not_create_deletions() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, true);
    provider.insert("PhotoViewer/camera/picture.gif", image(1));
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    std::fs::remove_dir_all(&job.local_root).unwrap();
    assert!(service
        .run_with_provider(&job, provider.clone())
        .await
        .is_err());
    assert!(service
        .store
        .entries(job.id)
        .unwrap()
        .iter()
        .all(|entry| entry.suppressed.is_none()));
}

#[tokio::test]
async fn backup_strategy_uploads_only_preserves_versions_and_never_restores_deleted_local() {
    let (_temp, service, job, provider) = fixture(SyncDirection::UploadOnly, true);
    let target = job.local_root.join("camera/local.gif");
    std::fs::write(&target, image(1)).unwrap();
    provider.insert("PhotoViewer/cloud.gif", image(9));
    let first = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(first.uploaded, 1);
    assert_eq!(first.downloaded, 0);
    assert!(!job.local_root.join("cloud.gif").exists());
    std::fs::write(&target, image(2)).unwrap();
    assert_eq!(
        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap()
            .uploaded,
        1
    );
    {
        let objects = provider.objects.lock().unwrap();
        assert_eq!(objects["PhotoViewer/camera/local.gif"], image(2));
        assert!(objects
            .iter()
            .any(|(key, data)| key.contains(".photoviewer-backup/objects/") && data == &image(1)));
        assert!(objects
            .iter()
            .any(|(key, data)| key.contains(".photoviewer-backup/objects/") && data == &image(2)));
        assert_eq!(
            objects
                .keys()
                .filter(|key| key.contains(".photoviewer-backup/records/"))
                .count(),
            2
        );
    }
    let calls = provider.upload_calls.load(Ordering::Relaxed);
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), calls);
    std::fs::remove_file(&target).unwrap();
    assert_eq!(
        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap()
            .downloaded,
        0
    );
    provider.insert("PhotoViewer/camera/local.gif", image(3));
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert!(!target.exists());
    assert!(service.store.open_conflicts(job.id).unwrap().is_empty());
}

#[tokio::test]
async fn backup_strategy_initial_collision_preserves_cloud_copy_before_replacing() {
    let (_temp, service, job, provider) = fixture(SyncDirection::UploadOnly, true);
    std::fs::write(job.local_root.join("camera/picture.gif"), image(1)).unwrap();
    provider.insert("PhotoViewer/camera/picture.gif", image(2));
    assert_eq!(
        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap()
            .uploaded,
        1
    );
    let objects = provider.objects.lock().unwrap();
    assert_eq!(objects["PhotoViewer/camera/picture.gif"], image(1));
    assert!(objects
        .iter()
        .any(|(key, data)| key.contains(".photoviewer-backup/objects/") && data == &image(2)));
}

#[tokio::test]
async fn strategy_switch_preserves_suppression_and_uses_latest_desired_direction() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, true);
    provider.insert("PhotoViewer/camera/picture.gif", image(1));
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    std::fs::remove_file(job.local_root.join("camera/picture.gif")).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert!(service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::UploadOnly)
        )
        .unwrap());
    assert_eq!(
        service
            .store
            .desired_job(job.id)
            .unwrap()
            .unwrap()
            .direction,
        SyncDirection::UploadOnly
    );
    service.store.set_job_paused(job.id, true).unwrap();
    let pending = service.store.pending_change(job.id).unwrap().unwrap();
    service
        .store
        .apply_change(job.id, pending.revision)
        .unwrap();
    assert!(!service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::UploadOnly)
        )
        .unwrap());
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::Bidirectional),
        )
        .unwrap();
    service.store.set_job_paused(job.id, true).unwrap();
    let pending = service.store.pending_change(job.id).unwrap().unwrap();
    service
        .store
        .apply_change(job.id, pending.revision)
        .unwrap();
    let latest = service.store.get_job(job.id).unwrap().unwrap();
    assert_eq!(
        service
            .run_with_provider(&latest, provider.clone())
            .await
            .unwrap()
            .downloaded,
        0
    );
    assert!(!job.local_root.join("camera/picture.gif").exists());
}

#[tokio::test]
async fn backup_strategy_failed_archive_recovers_before_overwrite_without_resurrecting_local() {
    let (_temp, service, job, provider) = fixture(SyncDirection::UploadOnly, true);
    let target = job.local_root.join("camera/picture.gif");
    std::fs::write(&target, image(1)).unwrap();
    provider.insert("PhotoViewer/camera/picture.gif", image(2));
    provider.fail_archive_upload.store(true, Ordering::Relaxed);
    assert!(service
        .run_with_provider(&job, provider.clone())
        .await
        .is_err());
    assert_eq!(
        provider.objects.lock().unwrap()["PhotoViewer/camera/picture.gif"],
        image(2)
    );
    std::fs::remove_file(&target).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert!(
        !target.exists(),
        "backup recovery must not recreate an explicitly missing local file"
    );
    let objects = provider.objects.lock().unwrap();
    assert_eq!(objects["PhotoViewer/camera/picture.gif"], image(1));
    for version in [1, 2] {
        assert!(objects
            .iter()
            .any(|(key, data)| key.contains(".photoviewer-backup/objects/")
                && data == &image(version)));
    }
}

#[tokio::test]
async fn backup_strategy_switch_archives_previously_synced_unchanged_files() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, true);
    let target = job.local_root.join("camera/picture.gif");
    std::fs::write(&target, image(1)).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::UploadOnly),
        )
        .unwrap();
    service.store.set_job_paused(job.id, true).unwrap();
    let pending = service.store.pending_change(job.id).unwrap().unwrap();
    service
        .store
        .apply_change(job.id, pending.revision)
        .unwrap();
    let latest = service.store.get_job(job.id).unwrap().unwrap();
    service
        .run_with_provider(&latest, provider.clone())
        .await
        .unwrap();
    assert!(provider
        .objects
        .lock()
        .unwrap()
        .iter()
        .any(|(key, data)| key.contains(".photoviewer-backup/objects/") && data == &image(1)));
}

#[tokio::test]
async fn sync_strategy_incomplete_remote_scan_does_not_create_deletions() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, true);
    provider.insert("PhotoViewer/camera/picture.gif", image(1));
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    std::fs::remove_file(job.local_root.join("camera/picture.gif")).unwrap();
    provider.fail_listing.store(true, Ordering::Relaxed);
    assert!(service
        .run_with_provider(&job, provider.clone())
        .await
        .is_err());
    assert!(service.store.entries(job.id).unwrap()[0]
        .suppressed
        .is_none());
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert!(service.store.entries(job.id).unwrap()[0]
        .suppressed
        .is_some());
}

#[tokio::test]
async fn sync_strategy_failed_new_version_download_keeps_deletion_until_successful_publish() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, false);
    provider.insert("PhotoViewer/camera/picture.gif", image(1));
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    let target = job.local_root.join("camera/picture.gif");
    std::fs::remove_file(&target).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    let deleted = service.store.entries(job.id).unwrap()[0].suppressed.clone();
    provider.insert("PhotoViewer/camera/picture.gif", image(2));
    provider.fail_download.store(true, Ordering::Relaxed);
    assert!(service
        .run_with_provider(&job, provider.clone())
        .await
        .is_err());
    assert!(!target.exists());
    assert_eq!(
        service.store.entries(job.id).unwrap()[0].suppressed,
        deleted
    );
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), image(2));
    assert!(service.store.entries(job.id).unwrap()[0]
        .suppressed
        .is_none());
}

#[tokio::test]
async fn sync_strategy_explicit_local_recreation_clears_suppression() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, true);
    provider.insert("PhotoViewer/camera/picture.gif", image(1));
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    let target = job.local_root.join("camera/picture.gif");
    std::fs::remove_file(&target).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert!(service.store.entries(job.id).unwrap()[0]
        .suppressed
        .is_some());
    std::fs::write(&target, image(1)).unwrap();
    let summary = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 0);
    assert_eq!(summary.uploaded, 0);
    assert!(service.store.entries(job.id).unwrap()[0]
        .suppressed
        .is_none());
}

#[tokio::test]
async fn backup_strategy_switch_supersedes_old_conflict_and_protects_both_versions() {
    let (_temp, service, job, provider) = fixture(SyncDirection::Bidirectional, true);
    let target = job.local_root.join("camera/picture.gif");
    std::fs::write(&target, image(1)).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    std::fs::write(&target, image(2)).unwrap();
    provider.insert("PhotoViewer/camera/picture.gif", image(3));
    assert_eq!(
        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap()
            .conflicts,
        1
    );
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::UploadOnly),
        )
        .unwrap();
    service.store.set_job_paused(job.id, true).unwrap();
    let pending = service.store.pending_change(job.id).unwrap().unwrap();
    service
        .store
        .apply_change(job.id, pending.revision)
        .unwrap();
    let latest = service.store.get_job(job.id).unwrap().unwrap();
    service
        .run_with_provider(&latest, provider.clone())
        .await
        .unwrap();
    assert!(service.store.open_conflicts(job.id).unwrap().is_empty());
    let objects = provider.objects.lock().unwrap();
    assert_eq!(objects["PhotoViewer/camera/picture.gif"], image(2));
    for version in [2, 3] {
        assert!(objects
            .iter()
            .any(|(key, data)| key.contains(".photoviewer-backup/objects/")
                && data == &image(version)));
    }
}

#[tokio::test]
async fn backup_strategy_history_is_excluded_from_sync_after_switching_modes() {
    let (_temp, service, job, provider) = fixture(SyncDirection::UploadOnly, true);
    let target = job.local_root.join("camera/picture.gif");
    std::fs::write(&target, image(1)).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    std::fs::write(&target, image(2)).unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::Bidirectional),
        )
        .unwrap();
    service.store.set_job_paused(job.id, true).unwrap();
    let pending = service.store.pending_change(job.id).unwrap().unwrap();
    service
        .store
        .apply_change(job.id, pending.revision)
        .unwrap();
    let latest = service.store.get_job(job.id).unwrap().unwrap();
    assert_eq!(
        service
            .run_with_provider(&latest, provider.clone())
            .await
            .unwrap()
            .downloaded,
        0
    );
    assert!(!job.local_root.join(".photoviewer-backup").exists());
    assert!(service
        .store
        .entries(job.id)
        .unwrap()
        .iter()
        .all(|entry| !entry.relative_path.contains(".photoviewer-backup")));
}

#[tokio::test]
async fn backup_strategy_success_clears_old_capability_conflict() {
    let (_temp, service, job, provider) = fixture(SyncDirection::UploadOnly, true);
    let target = job.local_root.join("camera/picture.gif");
    std::fs::write(&target, image(1)).unwrap();
    provider.insert("PhotoViewer/camera/picture.gif", image(2));
    let local = local::fingerprint(&target).unwrap();
    let remote = crate::core::sync::Fingerprint {
        size: image(2).len() as u64,
        blake3: blake3::hash(&image(2)).to_hex().to_string(),
    };
    let revision = FakeProvider::revision(&image(2));
    let id = service
        .store
        .upsert_observation(
            job.id,
            "camera/picture.gif",
            Some(&local),
            None,
            Some(&remote),
            Some(&revision.value),
            false,
            "conflict",
        )
        .unwrap();
    service
        .store
        .record_conflict(
            job.id,
            id,
            "InsufficientEvidence",
            Some(&local),
            Some(&remote),
            Some(&revision.value),
        )
        .unwrap();
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert!(service.store.open_conflicts(job.id).unwrap().is_empty());
    assert_eq!(
        provider.objects.lock().unwrap()["PhotoViewer/camera/picture.gif"],
        image(1)
    );
}
