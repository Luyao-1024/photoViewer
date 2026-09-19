use super::*;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::core::sync::provider::{
    ProviderCapabilities, ProviderError, ProviderResult, WriteCondition,
};

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
}

impl FakeProvider {
    fn insert(&self, key: &str, bytes: Vec<u8>) {
        self.objects.lock().unwrap().insert(key.into(), bytes);
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
        Ok(self
            .objects
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .filter(|(key, _)| !key[prefix.len()..].contains('/'))
            .map(|(key, bytes)| RemoteEntry {
                key: key.clone(),
                is_collection: false,
                size: bytes.len() as u64,
                modified_unix: None,
                revision: Some(Self::revision(bytes)),
            })
            .collect())
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
                size: bytes.len() as u64,
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

    let summary = service.run_with_provider(&job, provider).await.unwrap();
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
