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
fn cloud_state_respects_baseline_media_changes_and_selected_albums() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store.create_job(&new_job(local.clone())).unwrap();
    let fingerprint = Fingerprint {
        size: 5,
        blake3: "abc".into(),
    };
    let entry = store
        .upsert_observation(
            job.id,
            "a.jpg",
            Some(&fingerprint),
            Some(123),
            Some(&fingerprint),
            Some("etag"),
            false,
            "pending",
        )
        .unwrap();
    store
        .commit_baseline(entry, &fingerprint, Some("etag"))
        .unwrap();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO media_items
         (uri, path, folder_path, mime_type, media_kind, file_mtime, file_mtime_ns, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, 'image/jpeg', 'image', 1, 123, 5, '', 1)",
        params!["file:///a.jpg", local.join("a.jpg").to_string_lossy(), local.to_string_lossy()],
    ).unwrap();
    let id = conn.last_insert_rowid();
    assert_eq!(
        store.media_cloud_states(&[id]).unwrap(),
        HashMap::from([(id, CloudState::Synced)])
    );
    conn.execute(
        "UPDATE media_items SET file_mtime_ns = 124 WHERE id = ?1",
        [id],
    )
    .unwrap();
    assert_eq!(
        store.media_cloud_states(&[id]).unwrap()[&id],
        CloudState::Off
    );
    conn.execute(
        "UPDATE media_items SET file_mtime_ns = 123 WHERE id = ?1",
        [id],
    )
    .unwrap();
    conn.execute(
        "UPDATE sync_entries SET state = 'conflict' WHERE id = ?1",
        [entry],
    )
    .unwrap();
    assert_eq!(
        store.media_cloud_states(&[id]).unwrap()[&id],
        CloudState::Off
    );
    store.set_upload_albums(job.id, &["Other".into()]).unwrap();
    assert!(store.media_cloud_states(&[id]).unwrap().is_empty());
    store.set_upload_albums(job.id, &["".into()]).unwrap();
    assert_eq!(
        store.media_cloud_states(&[id]).unwrap()[&id],
        CloudState::Off
    );

    conn.execute(
        "INSERT INTO media_items
         (uri, path, folder_path, mime_type, media_kind, file_mtime, file_mtime_ns, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, 'video/mp4', 'video', 1, 123, 5, '', 1)",
        params!["file:///v.mp4", local.join("v.mp4").to_string_lossy(), local.to_string_lossy()],
    ).unwrap();
    let video_id = conn.last_insert_rowid();
    assert_eq!(
        store.media_cloud_states(&[video_id]).unwrap()[&video_id],
        CloudState::Off
    );
    let video_entry = store
        .upsert_observation(
            job.id,
            "v.mp4",
            Some(&fingerprint),
            Some(123),
            Some(&fingerprint),
            Some("etag"),
            false,
            "pending",
        )
        .unwrap();
    store
        .commit_baseline(video_entry, &fingerprint, Some("etag"))
        .unwrap();
    assert_eq!(
        store.media_cloud_states(&[video_id]).unwrap()[&video_id],
        CloudState::Synced
    );
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
        store.overview_with_sync_enabled(true).unwrap(),
        SyncOverview {
            status: SyncOverviewStatus::NotConfigured,
            job_count: 0,
            synced_items: 0,
            conflict_images: 0,
        }
    );
    assert_eq!(
        store.overview_with_sync_enabled(false).unwrap().status,
        SyncOverviewStatus::Disabled
    );

    let job = store.create_job(&new_job(local)).unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(false).unwrap().status,
        SyncOverviewStatus::Disabled
    );
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Ready
    );

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
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Running
    );

    store
        .mark_job_failed(job.id, "network unavailable")
        .unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Failed
    );

    store.mark_job_completed(job.id).unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Completed
    );

    store.set_job_paused(job.id, true).unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Paused
    );
}

#[test]
fn overview_counts_synced_media_and_open_image_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool);
    let job = store.create_job(&new_job(local)).unwrap();
    let fingerprint = Fingerprint {
        size: 4,
        blake3: "test".into(),
    };
    for path in ["A/photo.JPG", "A/movie.mp4"] {
        let entry = store
            .upsert_observation(
                job.id,
                path,
                Some(&fingerprint),
                None,
                Some(&fingerprint),
                Some("strong"),
                false,
                "pending",
            )
            .unwrap();
        store
            .commit_baseline(entry, &fingerprint, Some("strong"))
            .unwrap();
    }
    let photo = store
        .entries(job.id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.relative_path == "A/photo.JPG")
        .unwrap();
    store
        .record_conflict(
            job.id,
            photo.id,
            "both_changed",
            Some(&fingerprint),
            Some(&fingerprint),
            Some("strong"),
        )
        .unwrap();
    let movie = store
        .entries(job.id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.relative_path == "A/movie.mp4")
        .unwrap();
    store
        .record_conflict(
            job.id,
            movie.id,
            "both_changed",
            Some(&fingerprint),
            Some(&fingerprint),
            Some("strong"),
        )
        .unwrap();
    let overview = store.overview_with_sync_enabled(true).unwrap();
    assert_eq!(overview.synced_items, 2);
    assert_eq!(overview.conflict_images, 1);
}

#[test]
fn overview_does_not_report_completion_with_blocked_uploads() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool);
    let job = store.create_job(&new_job(local)).unwrap();
    let fingerprint = Fingerprint {
        size: 4,
        blake3: "test".into(),
    };
    let entry = store
        .upsert_observation(
            job.id,
            "A/photo.jpg",
            Some(&fingerprint),
            None,
            None,
            None,
            false,
            "pending",
        )
        .unwrap();
    let artifact = temp.path().join("photo.upload");
    std::fs::write(&artifact, b"test").unwrap();
    store
        .prepare_task(
            "pending-upload",
            job.id,
            entry,
            "upload_new",
            None,
            &artifact,
            &fingerprint.blake3,
            job.config_generation,
            1,
        )
        .unwrap();
    store
        .set_task_state("pending-upload", "blocked", Some("locked"))
        .unwrap();
    store.mark_job_completed(job.id).unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Failed
    );
}

#[test]
fn edit_first_pending_edits_merge_and_survive_reopening_without_mutating_active_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("photos");
    std::fs::create_dir(&root).unwrap();
    let path = temp.path().join("photos.db");
    let pool = crate::core::db::init_pool(&path).unwrap();
    let store = SyncStore::new(pool.clone());
    let mut requested = new_job(root);
    requested.upload_scope = UploadScope::SelectedAlbums;
    let job = store.create_job(&requested).unwrap();
    assert!(!store
        .queue_edit(job.id, JobEdit::UploadAlbums(Vec::new()))
        .unwrap());
    assert!(!store
        .queue_edit(job.id, JobEdit::RemoteRoot("/PhotoViewer/".into()))
        .unwrap());
    assert!(store.pending_change(job.id).unwrap().is_none());
    assert!(store
        .queue_edit(job.id, JobEdit::RemoteRoot("NewRoot".into()))
        .unwrap());
    assert!(store
        .queue_edit(
            job.id,
            JobEdit::UploadAlbums(vec!["camera".into(), "camera".into()])
        )
        .unwrap());
    let pending = store.pending_change(job.id).unwrap().unwrap();
    assert_eq!(pending.remote_root.as_deref(), Some("NewRoot"));
    assert_eq!(pending.upload_albums, Some(vec!["camera".into()]));
    assert_eq!(pending.revision, job.config_generation + 2);
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().remote_root,
        "PhotoViewer"
    );
    assert!(!store.get_job(job.id).unwrap().unwrap().paused);
    assert_eq!(
        store.desired_job(job.id).unwrap().unwrap().remote_root,
        "NewRoot"
    );
    assert!(!store
        .queue_edit(job.id, JobEdit::UploadAlbums(vec!["camera".into()]))
        .unwrap());
    assert!(store
        .queue_edit(job.id, JobEdit::UploadAlbums(vec!["../invalid".into()]))
        .is_err());
    drop(store);
    drop(pool);
    let store = SyncStore::new(crate::core::db::init_pool(&path).unwrap());
    assert_eq!(store.pending_change(job.id).unwrap().unwrap(), pending);
    store.set_job_paused(job.id, true).unwrap();
    assert!(matches!(
        store.apply_change(job.id, pending.revision - 1).unwrap(),
        SyncWriteResult::Changed(false)
    ));
    assert!(matches!(
        store.apply_change(job.id, pending.revision).unwrap(),
        SyncWriteResult::Changed(true)
    ));
    assert!(store.pending_change(job.id).unwrap().is_none());
    assert_eq!(store.upload_albums(job.id).unwrap(), vec!["camera"]);
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().remote_root,
        "NewRoot"
    );
    assert!(!store.get_job(job.id).unwrap().unwrap().paused);
}

#[test]
fn edit_first_pending_roots_reserve_their_namespace_and_recovery_is_not_discarded() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("one");
    let other = temp.path().join("two");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&other).unwrap();
    let store = SyncStore::new(crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap());
    let job = store.create_job(&new_job(root)).unwrap();
    let mut requested = new_job(other);
    requested.remote_root = "Other".into();
    let second = store.create_job(&requested).unwrap();
    store
        .queue_edit(second.id, JobEdit::RemoteRoot("Reserved/Album".into()))
        .unwrap();
    assert!(store
        .queue_edit(job.id, JobEdit::RemoteRoot("Reserved".into()))
        .is_err());
    assert!(store.pending_change(job.id).unwrap().is_none());
    let artifact = temp.path().join("only-copy.upload");
    std::fs::write(&artifact, b"only complete copy").unwrap();
    let fp = super::super::local::fingerprint(&artifact).unwrap();
    let entry = store
        .upsert_observation(
            job.id,
            "photo.jpg",
            Some(&fp),
            None,
            None,
            None,
            false,
            "pending",
        )
        .unwrap();
    store
        .prepare_task(
            "uncertain",
            job.id,
            entry,
            "upload_new",
            None,
            &artifact,
            &fp.blake3,
            job.config_generation,
            1,
        )
        .unwrap();
    store
        .queue_edit(job.id, JobEdit::RemoteRoot("NewRoot".into()))
        .unwrap();
    let pending = store.pending_change(job.id).unwrap().unwrap();
    store.set_job_paused(job.id, true).unwrap();
    assert!(store.apply_change(job.id, pending.revision).is_err());
    assert!(artifact.exists());
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().remote_root,
        "PhotoViewer"
    );
    assert_eq!(store.unfinished_tasks(job.id).unwrap().len(), 1);
    assert!(store.pending_change(job.id).unwrap().is_some());
}

#[test]
fn edit_first_overview_distinguishes_applying_from_paused_and_failed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("photos");
    std::fs::create_dir(&root).unwrap();
    let store = SyncStore::new(crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap());
    let job = store.create_job(&new_job(root)).unwrap();
    store
        .queue_edit(job.id, JobEdit::UploadAlbums(vec!["camera".into()]))
        .unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Applying
    );
    store.set_job_paused(job.id, true).unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Applying
    );
    store
        .mark_job_failed(job.id, "old write cannot be verified")
        .unwrap();
    assert_eq!(
        store.overview_with_sync_enabled(true).unwrap().status,
        SyncOverviewStatus::Failed
    );
    assert_eq!(
        store.overview_with_sync_enabled(false).unwrap().status,
        SyncOverviewStatus::Disabled
    );
}
