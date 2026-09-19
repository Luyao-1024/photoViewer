use super::*;

fn new_job(root: PathBuf) -> NewSyncJob {
    NewSyncJob {
        endpoint: "https://dav.example.test/root/".into(),
        username: "alice".into(),
        credential_ref: "credential-test".into(),
        local_root: root,
        remote_root: "PhotoViewer".into(),
        direction: SyncDirection::Bidirectional,
        upload_scope: UploadScope::All,
        upload_albums: Vec::new(),
    }
}

#[test]
fn stores_job_and_baseline_independently_from_media_rows() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store.create_job(&new_job(local)).unwrap();
    let fingerprint = Fingerprint {
        size: 5,
        blake3: "abc".into(),
    };
    let entry = store
        .upsert_observation(
            job.id,
            "a.jpg",
            Some(&fingerprint),
            Some(1),
            Some(&fingerprint),
            Some("\"etag\""),
            false,
            "pending",
        )
        .unwrap();
    store
        .commit_baseline(entry, &fingerprint, Some("\"etag\""))
        .unwrap();

    crate::core::db_actor::start_db_actor(
        pool.clone(),
        crate::core::events::DomainEventSender::new().0,
    )
    .execute_blocking(crate::core::db_actor::DbCommand::ResetLibraryDatabase)
    .unwrap();

    let entries = store.entries(job.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].baseline.as_ref(), Some(&fingerprint));
    assert_eq!(entries[0].state, "synced");
}

#[test]
fn rejects_insecure_non_local_endpoint() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool);
    let mut job = new_job(local);
    job.endpoint = "http://dav.example.test/".into();
    assert!(store.create_job(&job).is_err());
}

#[test]
fn stores_selected_upload_albums_and_advances_configuration_generation() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool);
    let mut requested = new_job(local);
    requested.upload_scope = UploadScope::SelectedAlbums;
    requested.upload_albums = vec!["Trips/2026".into(), "Camera".into(), "Camera".into()];

    let job = store.create_job(&requested).unwrap();
    assert_eq!(job.upload_scope, UploadScope::SelectedAlbums);
    assert_eq!(
        store.upload_albums(job.id).unwrap(),
        vec!["Camera".to_string(), "Trips/2026".to_string()]
    );

    store
        .set_upload_albums(job.id, &["Screenshots".into()])
        .unwrap();
    let updated = store.get_job(job.id).unwrap().unwrap();
    assert_eq!(updated.upload_scope, UploadScope::SelectedAlbums);
    assert_eq!(updated.config_generation, job.config_generation + 1);
    assert_eq!(
        store.upload_albums(job.id).unwrap(),
        vec!["Screenshots".to_string()]
    );
}

#[test]
fn rejects_upload_album_parent_traversal() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool);
    let mut requested = new_job(local);
    requested.upload_albums = vec!["../outside".into()];
    assert!(store.create_job(&requested).is_err());
}

#[test]
fn overview_reports_persisted_job_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool.clone());

    assert_eq!(
        store.overview().unwrap(),
        SyncOverview {
            status: SyncOverviewStatus::NotConfigured,
            job_count: 0,
        }
    );

    let job = store.create_job(&new_job(local)).unwrap();
    assert_eq!(store.overview().unwrap().status, SyncOverviewStatus::Ready);

    store.mark_job_started(job.id).unwrap();
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE sync_jobs SET last_started_at = 20, last_completed_at = 10 WHERE id = ?1",
            [job.id],
        )
        .unwrap();
    }
    assert_eq!(
        store.overview().unwrap().status,
        SyncOverviewStatus::Running
    );

    store
        .mark_job_failed(job.id, "network unavailable")
        .unwrap();
    assert_eq!(store.overview().unwrap().status, SyncOverviewStatus::Failed);

    store.mark_job_completed(job.id).unwrap();
    assert_eq!(
        store.overview().unwrap().status,
        SyncOverviewStatus::Completed
    );

    store.set_job_paused(job.id, true).unwrap();
    assert_eq!(store.overview().unwrap().status, SyncOverviewStatus::Paused);
}
