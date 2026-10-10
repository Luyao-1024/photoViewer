use super::*;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::core::sync::provider::{
    ProviderCapabilities, ProviderError, ProviderResult, WriteCondition,
};

#[test]
fn operation_id_has_restart_unique_session() {
    let first = next_operation_id(7);
    let second = next_operation_id(7);
    let (first_session, first_sequence) =
        first.strip_prefix("7-").unwrap().rsplit_once('-').unwrap();
    let (second_session, second_sequence) =
        second.strip_prefix("7-").unwrap().rsplit_once('-').unwrap();

    assert!(uuid::Uuid::parse_str(first_session).is_ok());
    assert_eq!(first_session, second_session);
    assert_ne!(first_sequence, second_sequence);
}

#[test]
fn remote_key_is_root_scoped() {
    assert_eq!(
        remote_key("Photos/Camera", "2026/a.jpg").unwrap(),
        "Photos/Camera/2026/a.jpg"
    );
    assert!(remote_key("Photos", "../secret.jpg").is_err());
}

#[test]
fn relative_remote_key_rejects_sibling_roots() {
    assert_eq!(
        relative_remote_key("Photos", "Photos/2026/a.jpg").unwrap(),
        "2026/a.jpg"
    );
    assert!(relative_remote_key("Photos", "Photos2/a.jpg").is_err());
}

#[derive(Default)]
struct FakeProvider {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    upload_calls: AtomicU64,
    download_calls: AtomicU64,
    /// Declared lengths that override the stored byte count. Lets a test place
    /// an object beyond a size threshold without allocating gigabytes for it.
    reported_sizes: Mutex<BTreeMap<String, u64>>,
}

impl FakeProvider {
    fn insert(&self, key: &str, bytes: Vec<u8>) {
        self.objects.lock().unwrap().insert(key.into(), bytes);
    }

    fn report_size(&self, key: &str, size: u64) {
        self.reported_sizes.lock().unwrap().insert(key.into(), size);
    }

    fn declared_size(&self, key: &str, actual: u64) -> u64 {
        self.reported_sizes
            .lock()
            .unwrap()
            .get(key)
            .copied()
            .unwrap_or(actual)
    }

    fn revision(bytes: &[u8]) -> Revision {
        Revision::etag(format!("\"{}\"", blake3::hash(bytes).to_hex()))
    }
}

#[async_trait]
impl SyncProvider for FakeProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            read: true,
            write_new: true,
            conditional_replace: true,
            conditional_delete: true,
            ..ProviderCapabilities::default()
        }
    }

    async fn probe(&self) -> ProviderResult<ProviderCapabilities> {
        Ok(self.capabilities())
    }

    async fn list_children(&self, collection: &str) -> ProviderResult<Vec<RemoteEntry>> {
        let prefix = format!("{}/", collection.trim_matches('/'));
        let objects = self.objects.lock().unwrap();
        let mut entries = BTreeMap::new();
        for (key, bytes) in objects.iter().filter(|(key, _)| key.starts_with(&prefix)) {
            let remainder = &key[prefix.len()..];
            if let Some((child, _)) = remainder.split_once('/') {
                let key = format!("{}{}", prefix, child);
                entries.entry(key.clone()).or_insert(RemoteEntry {
                    key,
                    is_collection: true,
                    size: 0,
                    modified_unix: None,
                    revision: None,
                });
            } else {
                entries.insert(
                    key.clone(),
                    RemoteEntry {
                        key: key.clone(),
                        is_collection: false,
                        size: self.declared_size(key, bytes.len() as u64),
                        modified_unix: None,
                        revision: Some(Self::revision(bytes)),
                    },
                );
            }
        }
        Ok(entries.into_values().collect())
    }

    async fn stat(&self, key: &str) -> ProviderResult<Option<RemoteEntry>> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|bytes| RemoteEntry {
                key: key.into(),
                is_collection: false,
                size: self.declared_size(key, bytes.len() as u64),
                modified_unix: None,
                revision: Some(Self::revision(bytes)),
            }))
    }

    async fn ensure_collection(&self, _collection: &str) -> ProviderResult<()> {
        Ok(())
    }

    async fn download(
        &self,
        key: &str,
        expected: Option<&Revision>,
        destination: &Path,
    ) -> ProviderResult<RemoteEntry> {
        self.download_calls.fetch_add(1, Ordering::Relaxed);
        let bytes = self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound(key.into()))?;
        let revision = Self::revision(&bytes);
        if expected.is_some_and(|expected| expected != &revision) {
            return Err(ProviderError::PreconditionFailed(key.into()));
        }
        tokio::fs::write(destination, &bytes).await?;
        Ok(RemoteEntry {
            key: key.into(),
            is_collection: false,
            size: bytes.len() as u64,
            modified_unix: None,
            revision: Some(revision),
        })
    }

    async fn upload(
        &self,
        key: &str,
        source: &Path,
        condition: WriteCondition,
    ) -> ProviderResult<RemoteEntry> {
        self.upload_calls.fetch_add(1, Ordering::Relaxed);
        let bytes = tokio::fs::read(source).await?;
        let mut objects = self.objects.lock().unwrap();
        match condition {
            WriteCondition::CreateOnly if objects.contains_key(key) => {
                return Err(ProviderError::PreconditionFailed(key.into()))
            }
            WriteCondition::ReplaceIf(expected) => {
                let actual = objects
                    .get(key)
                    .map(|bytes| Self::revision(bytes))
                    .ok_or_else(|| ProviderError::NotFound(key.into()))?;
                if actual != expected {
                    return Err(ProviderError::PreconditionFailed(key.into()));
                }
            }
            WriteCondition::CreateOnly => {}
        }
        objects.insert(key.into(), bytes.clone());
        Ok(RemoteEntry {
            key: key.into(),
            is_collection: false,
            size: bytes.len() as u64,
            modified_unix: None,
            revision: Some(Self::revision(&bytes)),
        })
    }

    async fn delete(&self, key: &str, expected: &Revision) -> ProviderResult<()> {
        let mut objects = self.objects.lock().unwrap();
        let actual = objects
            .get(key)
            .map(|bytes| Self::revision(bytes))
            .ok_or_else(|| ProviderError::NotFound(key.into()))?;
        if &actual != expected {
            return Err(ProviderError::PreconditionFailed(key.into()));
        }
        objects.remove(key);
        Ok(())
    }
}

#[tokio::test]
async fn synchronizes_new_files_both_directions_and_detects_later_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media/animated_source.gif");
    std::fs::copy(&fixture, local_root.join("local.gif")).unwrap();

    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let (sender, _receiver) = crate::core::events::DomainEventSender::new();
    let actor = crate::core::db_actor::start_db_actor(pool.clone(), sender);
    let service = SyncService {
        store: SyncStore::with_actor(pool.clone(), actor.clone()),
        pool,
        actor: Some(actor),
        staging_root: temp.path().join("staging"),
    };
    let job = service
        .store()
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "fake-credential".into(),
            local_root: local_root.clone(),
            remote_root: "PhotoViewer".into(),
            direction: crate::core::sync::SyncDirection::Bidirectional,
            upload_scope: crate::core::sync::UploadScope::All,
            upload_albums: Vec::new(),
        })
        .unwrap();
    let provider = Arc::new(FakeProvider::default());
    let remote_bytes = std::fs::read(&fixture).unwrap();
    provider.insert("PhotoViewer/cloud.gif", remote_bytes);

    let first = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(first.uploaded, 1);
    assert_eq!(first.downloaded, 1);
    assert!(local_root.join("cloud.gif").is_file());
    assert!(provider
        .objects
        .lock()
        .unwrap()
        .contains_key("PhotoViewer/local.gif"));

    let mut local_edit = std::fs::read(&fixture).unwrap();
    local_edit.extend_from_slice(b"local edit");
    let mut remote_edit = std::fs::read(&fixture).unwrap();
    remote_edit.extend_from_slice(b"remote edit");
    std::fs::write(local_root.join("local.gif"), &local_edit).unwrap();
    provider.insert("PhotoViewer/local.gif", remote_edit);
    let second = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(second.conflicts, 1);

    let conflict = service.store().open_conflicts(job.id).unwrap().remove(0);
    service
        .resolve_with_provider(
            &job,
            conflict,
            ConflictResolution::UseLocal,
            provider.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        provider
            .objects
            .lock()
            .unwrap()
            .get("PhotoViewer/local.gif")
            .unwrap(),
        &local_edit
    );
    assert!(service.store().open_conflicts(job.id).unwrap().is_empty());

    let mut local_second = std::fs::read(&fixture).unwrap();
    local_second.extend_from_slice(b"local second edit");
    let mut remote_second = std::fs::read(&fixture).unwrap();
    remote_second.extend_from_slice(b"remote second edit");
    std::fs::write(local_root.join("local.gif"), &local_second).unwrap();
    provider.insert("PhotoViewer/local.gif", remote_second.clone());
    assert_eq!(
        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap()
            .conflicts,
        1
    );
    let conflict = service.store().open_conflicts(job.id).unwrap().remove(0);
    let conflict_id = conflict.id;
    service
        .resolve_with_provider(
            &job,
            conflict,
            ConflictResolution::KeepBoth,
            provider.clone(),
        )
        .await
        .unwrap();
    let copy_path = format!("local.cloud-conflict-{conflict_id}.gif");
    assert_eq!(
        std::fs::read(local_root.join(&copy_path)).unwrap(),
        remote_second
    );
    {
        let objects = provider.objects.lock().unwrap();
        assert_eq!(objects.get("PhotoViewer/local.gif").unwrap(), &local_second);
        assert_eq!(
            objects.get(&format!("PhotoViewer/{copy_path}")).unwrap(),
            &remote_second
        );
    }
    assert!(service.store().open_conflicts(job.id).unwrap().is_empty());
    assert_eq!(
        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap()
            .conflicts,
        0
    );

    let mut local_third = std::fs::read(&fixture).unwrap();
    local_third.extend_from_slice(b"local third edit");
    let mut remote_third = std::fs::read(&fixture).unwrap();
    remote_third.extend_from_slice(b"remote third edit");
    std::fs::write(local_root.join("local.gif"), local_third).unwrap();
    provider.insert("PhotoViewer/local.gif", remote_third.clone());
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    let conflict = service.store().open_conflicts(job.id).unwrap().remove(0);
    service
        .resolve_with_provider(
            &job,
            conflict,
            ConflictResolution::UseRemote,
            provider.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(local_root.join("local.gif")).unwrap(),
        remote_third
    );
    assert!(service.store().open_conflicts(job.id).unwrap().is_empty());
}

#[tokio::test]
async fn remote_albums_download_without_changing_local_upload_selection() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    for album in ["Selected", "Unselected", "camera/nested"] {
        std::fs::create_dir_all(local_root.join(album)).unwrap();
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media/animated_source.gif");
    let original = std::fs::read(&fixture).unwrap();
    for path in [
        "Selected/upload.gif",
        "Unselected/local-only.gif",
        "camera/local-only.gif",
        "camera/nested/local-only.gif",
        "local-only.gif",
    ] {
        std::fs::write(local_root.join(path), &original).unwrap();
    }

    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let service = SyncService {
        store: SyncStore::new(pool.clone()),
        pool,
        actor: None,
        staging_root: temp.path().join("staging"),
    };
    let job = service
        .store()
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "selective-credential".into(),
            local_root: local_root.clone(),
            remote_root: "PhotoViewer".into(),
            direction: crate::core::sync::SyncDirection::Bidirectional,
            upload_scope: crate::core::sync::UploadScope::SelectedAlbums,
            upload_albums: vec!["Selected".into()],
        })
        .unwrap();
    let provider = Arc::new(FakeProvider::default());
    let mut cloud_bytes = original.clone();
    cloud_bytes.extend_from_slice(b"cloud");
    let cloud_paths = [
        "camera/download.gif",
        "camera/nested/download.gif",
        "download.gif",
    ];
    for path in cloud_paths {
        provider.insert(&format!("PhotoViewer/{path}"), cloud_bytes.clone());
    }

    let first = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(first.uploaded, 1);
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 1);
    assert_eq!(first.downloaded, 3);
    assert_eq!(
        service.store().upload_albums(job.id).unwrap(),
        vec!["Selected".to_string()]
    );
    assert_eq!(
        service
            .store()
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .config_generation,
        job.config_generation,
        "cloud discovery must not change the user's upload configuration"
    );
    for path in cloud_paths {
        assert_eq!(std::fs::read(local_root.join(path)).unwrap(), cloud_bytes);
    }
    let remote_after_first_run = provider.objects.lock().unwrap().clone();
    assert_eq!(remote_after_first_run.len(), 4);
    assert!(remote_after_first_run.contains_key("PhotoViewer/Selected/upload.gif"));
    assert!(
        service
            .store()
            .entries(job.id)
            .unwrap()
            .iter()
            .all(|entry| !entry.relative_path.ends_with("local-only.gif")),
        "unchecked local-only files must not be hashed into sync state"
    );

    // Local edits in unchecked cloud-backed albums (including nested/root
    // albums) never upload. The existing remote-authoritative policy downloads
    // the cloud version instead, while unrelated local-only files stay intact.
    let mut local_edit = cloud_bytes.clone();
    local_edit.extend_from_slice(b"local edit that must not upload");
    for path in cloud_paths {
        std::fs::write(local_root.join(path), &local_edit).unwrap();
    }
    let second = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(second.uploaded, 0);
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 1);
    assert_eq!(second.downloaded, 3);
    assert_eq!(second.conflicts, 0);
    assert_eq!(*provider.objects.lock().unwrap(), remote_after_first_run);
    for path in cloud_paths {
        assert_eq!(std::fs::read(local_root.join(path)).unwrap(), cloud_bytes);
    }
    for path in [
        "camera/local-only.gif",
        "camera/nested/local-only.gif",
        "local-only.gif",
    ] {
        assert_eq!(std::fs::read(local_root.join(path)).unwrap(), original);
    }
    assert_eq!(
        service.store().upload_albums(job.id).unwrap(),
        vec!["Selected"]
    );
    assert_eq!(
        service
            .store()
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .config_generation,
        job.config_generation
    );

    // Explicitly opting camera into uploads enables both its new files and
    // edits, but does not opt in its nested album or the root album.
    service
        .store()
        .set_upload_albums(job.id, &["Selected".into(), "camera".into()])
        .unwrap();
    let selected_job = service.store().get_job(job.id).unwrap().unwrap();
    std::fs::write(local_root.join("camera/download.gif"), &local_edit).unwrap();
    let third = service
        .run_with_provider(&selected_job, provider.clone())
        .await
        .unwrap();
    assert_eq!(third.uploaded, 2);
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 3);
    assert_eq!(third.conflicts, 0);
    {
        let objects = provider.objects.lock().unwrap();
        assert_eq!(objects["PhotoViewer/camera/download.gif"], local_edit);
        assert_eq!(objects["PhotoViewer/camera/local-only.gif"], original);
        assert!(!objects.contains_key("PhotoViewer/camera/nested/local-only.gif"));
        assert!(!objects.contains_key("PhotoViewer/local-only.gif"));
    }

    // Unchecking a previously synced album stays effective on later runs;
    // new remote changes still download without re-enabling uploads.
    service.store().set_upload_albums(job.id, &[]).unwrap();
    let unchecked_job = service.store().get_job(job.id).unwrap().unwrap();
    std::fs::write(local_root.join("camera/new-local.gif"), &original).unwrap();
    let mut cloud_update = cloud_bytes;
    cloud_update.extend_from_slice(b"remote update");
    provider.insert("PhotoViewer/camera/download.gif", cloud_update.clone());
    let remote_before_fourth_run = provider.objects.lock().unwrap().clone();
    let fourth = service
        .run_with_provider(&unchecked_job, provider.clone())
        .await
        .unwrap();
    assert_eq!(fourth.uploaded, 0);
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 3);
    assert_eq!(fourth.downloaded, 1);
    assert_eq!(fourth.conflicts, 0);
    assert_eq!(*provider.objects.lock().unwrap(), remote_before_fourth_run);
    assert_eq!(
        std::fs::read(local_root.join("camera/download.gif")).unwrap(),
        cloud_update
    );
    assert!(service.store().upload_albums(job.id).unwrap().is_empty());
    assert_eq!(
        service
            .store()
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .config_generation,
        unchecked_job.config_generation
    );
}

#[tokio::test]
async fn recovers_upload_that_reached_remote_before_result_commit() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media/animated_source.gif");
    std::fs::copy(&fixture, local_root.join("photo.gif")).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let service = SyncService {
        store: SyncStore::new(pool.clone()),
        pool,
        actor: None,
        staging_root: temp.path().join("staging"),
    };
    let job = service
        .store()
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "recovery-credential".into(),
            local_root: local_root.clone(),
            remote_root: "PhotoViewer".into(),
            direction: crate::core::sync::SyncDirection::Bidirectional,
            upload_scope: crate::core::sync::UploadScope::All,
            upload_albums: Vec::new(),
        })
        .unwrap();
    let provider = Arc::new(FakeProvider::default());
    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();

    let old_remote = provider
        .stat("PhotoViewer/photo.gif")
        .await
        .unwrap()
        .unwrap();
    let mut changed = std::fs::read(&fixture).unwrap();
    changed.extend_from_slice(b"uploaded before crash");
    std::fs::write(local_root.join("photo.gif"), &changed).unwrap();
    let changed_fingerprint = local::fingerprint(&local_root.join("photo.gif")).unwrap();
    let stored = service.store().entries(job.id).unwrap().remove(0);
    let entry_id = service
        .store()
        .upsert_observation(
            job.id,
            "photo.gif",
            Some(&changed_fingerprint),
            None,
            stored.remote.as_ref(),
            old_remote
                .revision
                .as_ref()
                .map(|revision| revision.value.as_str()),
            false,
            "pending",
        )
        .unwrap();
    let artifact = temp.path().join("interrupted.upload");
    std::fs::write(&artifact, &changed).unwrap();
    service
        .store()
        .prepare_task(
            "interrupted-upload",
            job.id,
            entry_id,
            "upload_replace",
            old_remote.revision.as_ref(),
            &artifact,
            &changed_fingerprint.blake3,
            job.config_generation,
            stored.generation + 1,
        )
        .unwrap();
    provider.insert("PhotoViewer/photo.gif", changed);

    // A completed old-generation write remains verifiable after explicit opt-out;
    // recovery must not re-upload it merely because the configuration changed.
    service.store().set_upload_albums(job.id, &[]).unwrap();
    let opted_out = service.store().get_job(job.id).unwrap().unwrap();
    let upload_calls = provider.upload_calls.load(Ordering::Acquire);
    let summary = service
        .run_with_provider(&opted_out, provider.clone())
        .await
        .unwrap();
    assert_eq!(provider.upload_calls.load(Ordering::Acquire), upload_calls);
    assert_eq!(summary.conflicts, 0);
    assert!(!artifact.exists());
    let connection = service.pool.get().unwrap();
    let state: String = connection
        .query_row(
            "SELECT state FROM sync_tasks WHERE operation_id = 'interrupted-upload'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "succeeded");
}

#[tokio::test]
async fn upload_recovery_waits_for_explicit_album_selection() {
    let full = b"complete image bytes from the original upload";
    let partial = &full[..17];
    for remote_content in [Some(partial.to_vec()), None] {
        let temp = tempfile::tempdir().unwrap();
        let local_root = temp.path().join("photos");
        std::fs::create_dir_all(local_root.join("OldAlbum")).unwrap();
        let local_path = local_root.join("OldAlbum/photo.jpg");
        std::fs::write(&local_path, &full[..10]).unwrap();
        let artifact = temp.path().join("original.upload");
        std::fs::write(&artifact, full).unwrap();
        let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
        let service = SyncService {
            store: SyncStore::new(pool.clone()),
            pool,
            actor: None,
            staging_root: temp.path().join("staging"),
        };
        let job = service
            .store()
            .create_job(&crate::core::sync::NewSyncJob {
                endpoint: "https://dav.example.test/".into(),
                username: "alice".into(),
                credential_ref: "recovery-test".into(),
                local_root: local_root.clone(),
                remote_root: "PhotoViewer".into(),
                direction: crate::core::sync::SyncDirection::Bidirectional,
                upload_scope: crate::core::sync::UploadScope::SelectedAlbums,
                upload_albums: vec!["OldAlbum".into()],
            })
            .unwrap();
        let fingerprint = local::fingerprint(&artifact).unwrap();
        let entry_id = service
            .store()
            .upsert_observation(
                job.id,
                "OldAlbum/photo.jpg",
                Some(&fingerprint),
                None,
                None,
                None,
                false,
                "pending",
            )
            .unwrap();
        service
            .store()
            .prepare_task(
                "partial-upload",
                job.id,
                entry_id,
                "upload_new",
                None,
                &artifact,
                &fingerprint.blake3,
                job.config_generation,
                1,
            )
            .unwrap();
        service
            .store()
            .set_task_state("partial-upload", "blocked", Some("locked"))
            .unwrap();
        service
            .store()
            .set_upload_albums(job.id, &["OtherAlbum".into()])
            .unwrap();
        let job = service.store().get_job(job.id).unwrap().unwrap();
        let provider = Arc::new(FakeProvider::default());
        if let Some(bytes) = remote_content {
            provider.insert("PhotoViewer/OldAlbum/photo.jpg", bytes);
        }
        let remote_before_recovery = provider.objects.lock().unwrap().clone();

        service
            .run_with_provider(&job, provider.clone())
            .await
            .unwrap();

        assert_eq!(std::fs::read(&local_path).unwrap(), full);
        assert_eq!(
            *provider.objects.lock().unwrap(),
            remote_before_recovery,
            "recovery must not write to an unchecked cloud album"
        );
        assert!(
            artifact.exists(),
            "the only complete snapshot must be retained"
        );
        assert_eq!(
            service.store().upload_albums(job.id).unwrap(),
            vec!["OtherAlbum"]
        );
        let conn = service.pool.get().unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM sync_tasks WHERE operation_id = 'partial-upload'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "blocked");
        assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 0);
        drop(conn);

        service
            .store()
            .set_upload_albums(job.id, &["OldAlbum".into()])
            .unwrap();
        let selected_job = service.store().get_job(job.id).unwrap().unwrap();
        service
            .run_with_provider(&selected_job, provider.clone())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&local_path).unwrap(), full);
        assert_eq!(
            provider.objects.lock().unwrap()["PhotoViewer/OldAlbum/photo.jpg"],
            full
        );
        assert!(!artifact.exists());
        let conn = service.pool.get().unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM sync_tasks WHERE operation_id = 'partial-upload'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "succeeded");
        assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test]
async fn unrelated_remote_content_does_not_replace_local_or_upload_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir_all(local_root.join("OldAlbum")).unwrap();
    let local_path = local_root.join("OldAlbum/photo.jpg");
    std::fs::write(&local_path, b"local original").unwrap();
    let artifact = temp.path().join("original.upload");
    std::fs::write(&artifact, b"complete original upload").unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let service = SyncService {
        store: SyncStore::new(pool.clone()),
        pool,
        actor: None,
        staging_root: temp.path().join("staging"),
    };
    let job = service
        .store()
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "recovery-nonprefix".into(),
            local_root: local_root.clone(),
            remote_root: "PhotoViewer".into(),
            direction: crate::core::sync::SyncDirection::Bidirectional,
            upload_scope: crate::core::sync::UploadScope::SelectedAlbums,
            upload_albums: vec!["OtherAlbum".into()],
        })
        .unwrap();
    let fingerprint = local::fingerprint(&artifact).unwrap();
    let entry_id = service
        .store()
        .upsert_observation(
            job.id,
            "OldAlbum/photo.jpg",
            Some(&fingerprint),
            None,
            None,
            None,
            false,
            "pending",
        )
        .unwrap();
    service
        .store()
        .prepare_task(
            "nonprefix-upload",
            job.id,
            entry_id,
            "upload_new",
            None,
            &artifact,
            &fingerprint.blake3,
            job.config_generation,
            1,
        )
        .unwrap();
    service
        .store()
        .set_task_state("nonprefix-upload", "blocked", Some("network error"))
        .unwrap();
    let provider = Arc::new(FakeProvider::default());
    provider.insert(
        "PhotoViewer/OldAlbum/photo.jpg",
        b"different cloud content".to_vec(),
    );

    service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();

    assert_eq!(std::fs::read(&local_path).unwrap(), b"local original");
    assert_eq!(
        std::fs::read(&artifact).unwrap(),
        b"complete original upload"
    );
    assert_eq!(
        provider.objects.lock().unwrap()["PhotoViewer/OldAlbum/photo.jpg"],
        b"different cloud content"
    );
    let conn = service.pool.get().unwrap();
    let state: String = conn
        .query_row(
            "SELECT state FROM sync_tasks WHERE operation_id = 'nonprefix-upload'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "blocked");
}

#[test]
fn live_progress_tracker_counts_transfers_by_phase() {
    let mut progress = SyncLiveProgress::preparing();
    assert_eq!(progress.phase, SyncLivePhase::Preparing);
    assert_eq!(
        progress,
        SyncLiveProgress {
            phase: SyncLivePhase::Preparing,
            downloaded: 0,
            download_total: 0,
            uploaded: 0,
            upload_total: 0,
            transfer_active: false,
            current_bytes: 0,
            current_total: 0,
        }
    );

    progress.add_totals(3, 2);
    progress.begin_transfer(SyncLivePhase::Downloading, 4096);
    assert!(progress.transfer_active);
    assert_eq!(progress.current_total, 4096);
    progress.complete_download();
    assert!(!progress.transfer_active);
    assert_eq!(progress.current_bytes, 0);
    assert_eq!(progress.current_total, 0);
    progress.begin_transfer(SyncLivePhase::Downloading, 4096);
    progress.complete_download();
    assert_eq!(progress.downloaded, 2);
    assert_eq!(progress.download_total, 3);
    assert_eq!(progress.phase, SyncLivePhase::Downloading);

    progress.begin_transfer(SyncLivePhase::Uploading, 8192);
    progress.complete_upload();
    assert_eq!(progress.uploaded, 1);
    assert_eq!(progress.phase, SyncLivePhase::Uploading);

    // A verification that ends in a conflict also resolves its planned
    // download slot, and a saturated counter cannot exceed the planned total.
    progress.download_total = progress.downloaded;
    progress.begin_transfer(SyncLivePhase::Downloading, 1);
    progress.complete_download();
    assert_eq!(progress.downloaded, progress.download_total);

    // A later job in the same pull adds its plan to the running session
    // instead of resetting the counters back to zero.
    progress.add_totals(1, 4);
    assert_eq!(progress.download_total, 3);
    assert_eq!(progress.upload_total, 6);
    assert_eq!(progress.downloaded, 2);
    assert_eq!(progress.uploaded, 1);
}

#[test]
fn transfer_progress_sink_counts_streamed_bytes() {
    let sink = TransferProgress::default();
    assert_eq!(sink.snapshot(), (0, 0));

    sink.start(1024);
    sink.record(300);
    sink.record(400);
    assert_eq!(sink.snapshot(), (700, 1024));

    // Restarting for the next transfer clears the previous counters.
    sink.start(2048);
    assert_eq!(sink.snapshot(), (0, 2048));
}

/// The progress session is process-wide; tests that open or close one hold
/// this lock so unrelated test threads cannot interleave begin/end pairs.
static PROGRESS_TEST_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test]
// The test-only lock serializes process-wide progress-session state between
// this test and the sync `#[test]` below. Nothing awaited here re-acquires
// it (current-thread runtime, plain-`#[test]` contender), so holding the
// guard across `.await` cannot deadlock.
#[allow(clippy::await_holding_lock)]
async fn run_with_provider_reports_live_transfer_progress() {
    let _session_lock = PROGRESS_TEST_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media/animated_source.gif");
    std::fs::copy(&fixture, local_root.join("local.gif")).unwrap();

    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let (sender, _receiver) = crate::core::events::DomainEventSender::new();
    let actor = crate::core::db_actor::start_db_actor(pool.clone(), sender);
    let service = SyncService {
        store: SyncStore::with_actor(pool.clone(), actor.clone()),
        pool,
        actor: Some(actor),
        staging_root: temp.path().join("staging"),
    };
    let job = service
        .store()
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "progress-credential".into(),
            local_root: local_root.clone(),
            remote_root: "PhotoViewer".into(),
            direction: crate::core::sync::SyncDirection::Bidirectional,
            upload_scope: crate::core::sync::UploadScope::All,
            upload_albums: Vec::new(),
        })
        .unwrap();
    let provider = Arc::new(FakeProvider::default());
    let remote_bytes = std::fs::read(&fixture).unwrap();
    provider.insert("PhotoViewer/cloud.gif", remote_bytes.clone());
    provider.insert("PhotoViewer/another/cloud.gif", remote_bytes);

    live_progress_begin_session();
    let summary = service.run_with_provider(&job, provider).await.unwrap();
    let progress = live_progress().expect("live progress is reported while a run is active");
    assert_eq!(summary.uploaded, 1);
    assert_eq!(summary.downloaded, 2);
    // The session is process-wide and unrelated tests may run transfers
    // concurrently, so only this run's guaranteed lower bounds are asserted.
    assert!(progress.download_total >= 2);
    assert!(progress.upload_total >= 1);
    assert!(progress.downloaded >= 2);
    assert!(progress.uploaded >= 1);
    assert!(matches!(
        progress.phase,
        SyncLivePhase::Downloading | SyncLivePhase::Uploading
    ));
    live_progress_end_session();
    assert_eq!(live_progress(), None);
}

#[tokio::test]
async fn edit_first_activity_is_scoped_to_database_not_numeric_job_id() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let a = SyncService::new(crate::core::db::init_pool(&left.path().join("photos.db")).unwrap());
    let b = SyncService::new(crate::core::db::init_pool(&right.path().join("photos.db")).unwrap());
    let active = ActiveJobGuard::acquire(a.job_key(1).unwrap()).unwrap();
    assert!(a.is_job_running(1).unwrap());
    assert!(
        !b.is_job_running(1).unwrap(),
        "a second library must not be stopped by another library's edit"
    );
    drop(active);
    assert!(!a.is_job_running(1).unwrap());
}

#[test]
fn live_progress_session_survives_a_concurrent_trigger_close() {
    let _session_lock = PROGRESS_TEST_LOCK.lock().unwrap();
    live_progress_begin_session();

    // While a run holds its job's activation slot, another trigger closing
    // its own cycle must not tear down the session that run reports through.
    let active = ActiveJobGuard::acquire((PathBuf::from("progress-contract"), 987_654)).unwrap();
    live_progress_end_session();
    assert!(
        live_progress().is_some(),
        "an active run owns the progress session"
    );

    // Once the run finishes (guard dropped), closing the session clears it.
    drop(active);
    live_progress_end_session();
    assert_eq!(live_progress(), None);
}

#[tokio::test]
async fn edit_first_noop_and_invalid_root_do_not_pause() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let service = SyncService {
        store: SyncStore::new(pool.clone()),
        pool,
        actor: None,
        staging_root: temp.path().join("staging"),
    };
    let job = service
        .store
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "edit-first".into(),
            local_root,
            remote_root: "Photos".into(),
            direction: super::super::SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: Vec::new(),
        })
        .unwrap();
    service.set_remote_root(job.id, "/Photos/").await.unwrap();
    let unchanged = service.store.get_job(job.id).unwrap().unwrap();
    assert!(
        !unchanged.paused,
        "a no-op must not stop the job before comparison"
    );
    assert_eq!(unchanged.config_generation, job.config_generation);
    assert!(service.set_remote_root(job.id, "../invalid").await.is_err());
    assert!(!service.store.get_job(job.id).unwrap().unwrap().paused);
}

fn recovery_service(root: &Path) -> SyncService {
    let pool = crate::core::db::init_pool(&root.join("photos.db")).unwrap();
    SyncService::new(pool)
}

fn recovery_job(service: &SyncService, local_root: PathBuf) -> crate::core::sync::SyncJob {
    service
        .store()
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "recovery-cap".into(),
            local_root,
            remote_root: "PhotoViewer".into(),
            direction: crate::core::sync::SyncDirection::UploadOnly,
            upload_scope: crate::core::sync::UploadScope::All,
            upload_albums: Vec::new(),
        })
        .unwrap()
}

/// Recovery of a `prepared` upload stats the remote object before planning
/// starts, and past the verification cap it records the outcome instead of
/// transferring it. The failure this prevents is not slowness but
/// non-termination — the same gigabytes refetched every round, each attempt
/// killed by the per-request timeout before it can ever resolve.
#[tokio::test]
async fn upload_recovery_records_rather_than_fetches_an_oversized_remote_object() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    let service = recovery_service(temp.path());
    let job = recovery_job(&service, local_root.clone());

    let artifact = temp.path().join("staged.upload");
    std::fs::write(&artifact, vec![7u8; 4096]).unwrap();
    let fingerprint = crate::core::sync::local::fingerprint(&artifact).unwrap();

    let entry_id = service
        .store()
        .upsert_observation(
            job.id,
            "big.mp4",
            Some(&fingerprint),
            None,
            None,
            None,
            false,
            "pending",
        )
        .unwrap();
    service
        .store()
        .prepare_task(
            "oversized-recovery",
            job.id,
            entry_id,
            "upload_new",
            None,
            &artifact,
            &fingerprint.blake3,
            job.config_generation,
            1,
        )
        .unwrap();
    let task = service
        .store()
        .unfinished_tasks(job.id)
        .unwrap()
        .into_iter()
        .find(|task| task.relative_path == "big.mp4")
        .expect("the prepared upload is pending");

    let provider = FakeProvider::default();
    provider.insert("PhotoViewer/big.mp4", vec![7u8; 4096]);
    provider.report_size("PhotoViewer/big.mp4", MAX_VERIFY_DOWNLOAD_BYTES + 1);

    service
        .recover_upload_task(&job, &provider, &task)
        .await
        .unwrap();

    assert_eq!(
        provider.upload_calls.load(Ordering::Relaxed),
        0,
        "an oversized remote object must not be re-uploaded either"
    );
    let conn = service.pool.get().unwrap();
    let state: String = conn
        .query_row(
            "SELECT state FROM sync_tasks WHERE operation_id = 'oversized-recovery'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        state, "blocked",
        "an object past the cap must be recorded as unproven, not fetched and retried forever"
    );
    let reason: String = conn
        .query_row(
            "SELECT last_error FROM sync_tasks WHERE operation_id = 'oversized-recovery'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        reason.contains("upload-recovery verification limit"),
        "the recorded reason must say why: {reason}"
    );
    assert!(
        service.store().open_conflicts(job.id).unwrap().is_empty(),
        "recovery must not invent a conflict; the reason belongs to the task"
    );
}

/// The planner emits `VerifyContent` when WebDAV cannot supply a content hash
/// for the remote side; the *client* then decides whether proving the pair is
/// worth a full download. A length difference already proves the contents
/// differ, so the run must conclude it without transferring a single byte.
#[tokio::test]
async fn verification_resolves_a_length_mismatch_without_fetching() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    let service = recovery_service(temp.path());
    let job = recovery_job(&service, local_root.clone());

    let provider = FakeProvider::default();
    provider.insert("PhotoViewer/clip.mp4", vec![9u8; 2048]);
    let remote = provider
        .stat("PhotoViewer/clip.mp4")
        .await
        .unwrap()
        .expect("the object is listed");

    let local = crate::core::sync::local::LocalEntry {
        relative_path: "clip.mp4".into(),
        absolute_path: local_root.join("clip.mp4"),
        fingerprint: crate::core::sync::Fingerprint {
            size: 4096,
            blake3: "a".repeat(64),
        },
        modified_ns: 1,
    };
    let mut summary = RunSummary::default();
    service
        .reconcile_one(
            &job,
            &provider,
            "clip.mp4",
            Some(&local),
            Some(&remote),
            None,
            true,
            &mut summary,
        )
        .await
        .unwrap();

    assert_eq!(
        provider.download_calls.load(Ordering::Relaxed),
        0,
        "different lengths already prove the contents differ; fetching cannot add evidence"
    );
    assert_eq!(summary.conflicts, 1);
    let conflict = service.store().open_conflicts(job.id).unwrap().remove(0);
    assert_eq!(conflict.kind, "InitialContentMismatch");
}

/// When the two sides claim the same length and the object is past the cap,
/// proving identity would require the download the timeout forbids. The run
/// records the evidence gap as a conflict — visible with a retry in Settings —
/// instead of refetching the giant every round until the per-request timeout
/// kills it, which is how a synchronization gets stuck on `Running` forever.
#[tokio::test]
async fn verification_stops_at_the_cap_and_records_insufficient_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    let service = recovery_service(temp.path());
    let job = recovery_job(&service, local_root.clone());

    let oversized = MAX_VERIFY_DOWNLOAD_BYTES + 1;
    let provider = FakeProvider::default();
    provider.insert("PhotoViewer/huge.mp4", vec![7u8; 4096]);
    provider.report_size("PhotoViewer/huge.mp4", oversized);
    let remote = provider
        .stat("PhotoViewer/huge.mp4")
        .await
        .unwrap()
        .expect("the object is listed");

    let local = crate::core::sync::local::LocalEntry {
        relative_path: "huge.mp4".into(),
        absolute_path: local_root.join("huge.mp4"),
        fingerprint: crate::core::sync::Fingerprint {
            size: oversized,
            blake3: "b".repeat(64),
        },
        modified_ns: 1,
    };
    let mut summary = RunSummary::default();
    service
        .reconcile_one(
            &job,
            &provider,
            "huge.mp4",
            Some(&local),
            Some(&remote),
            None,
            true,
            &mut summary,
        )
        .await
        .unwrap();

    assert_eq!(
        provider.download_calls.load(Ordering::Relaxed),
        0,
        "a past-cap verification download cannot finish inside the request timeout"
    );
    assert_eq!(summary.conflicts, 1);
    let conflict = service.store().open_conflicts(job.id).unwrap().remove(0);
    assert_eq!(conflict.kind, "InsufficientEvidence");
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 0);
}
