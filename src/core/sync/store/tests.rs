use super::*;

fn new_job(root: PathBuf) -> NewSyncJob {
    NewSyncJob {
        endpoint: "https://dav.example.test/root/".into(),
        username: "alice".into(),
        credential_ref: "credential-test".into(),
        local_root: root,
        remote_root: "PhotoViewer".into(),
        direction: SyncDirection::Bidirectional,
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
