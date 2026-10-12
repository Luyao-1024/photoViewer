use super::*;

fn reset_fixture() -> (tempfile::TempDir, SyncService, SyncJob) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    std::fs::create_dir_all(root.join("camera")).unwrap();
    let mut service = recovery_service(tmp.path());
    service.staging_root = tmp.path().join("sync-staging");
    let job = service
        .store
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "alice".into(),
            credential_ref: "retain-this-credential".into(),
            local_root: root,
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: vec!["camera".into()],
        })
        .unwrap();
    (tmp, service, job)
}

fn seed_history(service: &SyncService, job: &SyncJob) {
    std::fs::create_dir_all(&service.staging_root).unwrap();
    let media = job.local_root.join("camera/picture.gif");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media/animated_source.gif"),
        &media,
    )
    .unwrap();
    let fingerprint = local::fingerprint(&media).unwrap();
    let id = service
        .store
        .upsert_observation(
            job.id,
            "camera/picture.gif",
            Some(&fingerprint),
            Some(1),
            Some(&fingerprint),
            Some("\"v1\""),
            false,
            "pending",
        )
        .unwrap();
    service
        .store
        .commit_baseline(id, &fingerprint, Some("\"v1\""))
        .unwrap();
    service
        .store
        .suppress_local_deletion(id, &fingerprint)
        .unwrap();
    let artifact = service.staging_root.join("unfinished.upload");
    std::fs::copy(&media, &artifact).unwrap();
    service
        .store
        .prepare_task(
            "reset-fixture",
            job.id,
            id,
            "upload_new",
            None,
            &artifact,
            &fingerprint.blake3,
            job.config_generation,
            1,
        )
        .unwrap();
    service
        .store
        .record_conflict(
            job.id,
            id,
            "BothModified",
            Some(&fingerprint),
            Some(&fingerprint),
            Some("\"v1\""),
        )
        .unwrap();
    service.store.mark_job_started(job.id).unwrap();
    service
        .store
        .mark_job_failed(job.id, "old failure")
        .unwrap();
    std::fs::write(
        service.staging_root.join("unreferenced.download"),
        b"partial",
    )
    .unwrap();
    std::fs::write(
        job.local_root.join("camera/.photoviewer-sync-unfinished"),
        b"temporary publication",
    )
    .unwrap();
    std::fs::write(
        job.local_root
            .join("camera/.photoviewer-sync-backup-reset-old.gif"),
        b"temporary rollback copy",
    )
    .unwrap();
}

fn assert_clean(service: &SyncService, job: &SyncJob) {
    assert!(service.store.entries(job.id).unwrap().is_empty());
    assert!(service.store.open_conflicts(job.id).unwrap().is_empty());
    assert!(service.store.unfinished_tasks(job.id).unwrap().is_empty());
    assert!(service.store.pending_change(job.id).unwrap().is_none());
    let conn = service.pool.get().unwrap();
    for table in [
        "sync_entries",
        "sync_tasks",
        "sync_conflicts",
        "sync_job_changes",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table} must be empty"
        );
    }
    let runtime: (Option<i64>, Option<i64>, Option<String>, bool) = conn
        .query_row(
            "SELECT last_started_at,last_completed_at,last_error,paused FROM sync_jobs WHERE id=?1",
            [job.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(runtime, (None, None, None, false));
    assert!(!service.store.reset_pending().unwrap());
    assert!(!service.staging_root.exists());
    assert!(!job
        .local_root
        .join("camera/.photoviewer-sync-unfinished")
        .exists());
    assert!(!job
        .local_root
        .join("camera/.photoviewer-sync-backup-reset-old.gif")
        .exists());
    assert!(
        job.local_root.join("camera/picture.gif").exists(),
        "local media must remain"
    );
}

#[tokio::test]
async fn reset_keeps_accounts_and_task_configuration_but_clears_all_history() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    let actor = crate::core::db_actor::start_db_actor(
        service.pool.clone(),
        crate::core::events::DomainEventSender::new().0,
    );
    let service = SyncService {
        store: SyncStore::with_actor(service.pool.clone(), actor.clone()),
        actor: Some(actor),
        ..service
    };
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
    let saved = service.store.get_job(job.id).unwrap().unwrap();
    assert_eq!(saved.endpoint, job.endpoint);
    assert_eq!(saved.username, job.username);
    assert_eq!(saved.credential_ref, job.credential_ref);
    assert_eq!(saved.connection_id, job.connection_id);
    assert_eq!(saved.local_root, job.local_root);
    assert_eq!(saved.remote_root, job.remote_root);
    assert_eq!(saved.direction, job.direction);
    assert_eq!(saved.upload_scope, job.upload_scope);
    assert_eq!(service.store.upload_albums(job.id).unwrap(), vec!["camera"]);
    assert!(saved.config_generation > job.config_generation);
    // Repeated resets have the same visible result.
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
}

#[tokio::test]
async fn reset_preserves_latest_accepted_configuration_and_discards_pending_actions() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::RemoteRoot("New".into()),
        )
        .unwrap();
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::UploadOnly),
        )
        .unwrap();
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::UploadAlbums(vec!["camera".into(), "second".into()]),
        )
        .unwrap();
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
    let saved = service.store.get_job(job.id).unwrap().unwrap();
    assert_eq!(saved.remote_root, "New");
    assert_eq!(saved.direction, SyncDirection::UploadOnly);
    assert_eq!(
        service.store.upload_albums(job.id).unwrap(),
        vec!["camera", "second"]
    );
}

#[tokio::test]
async fn reset_waits_for_active_operations_and_removes_their_late_writes() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    let active = service.acquire_run_slot(job.id).await.unwrap();
    service.store.begin_reset().unwrap();
    assert!(service.acquire_run_slot(job.id).await.is_err());
    let worker_service = service.clone();
    let reset = tokio::spawn(async move { worker_service.perform_sync_reset().await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!reset.is_finished());
    assert!(service.staging_root.exists());
    let f = crate::core::sync::Fingerprint {
        size: 4,
        blake3: "late".into(),
    };
    service
        .store
        .upsert_observation(
            job.id,
            "camera/late.gif",
            Some(&f),
            None,
            Some(&f),
            Some("\"late\""),
            false,
            "synced",
        )
        .unwrap();
    std::fs::write(service.staging_root.join("late.download"), b"late").unwrap();
    drop(active);
    tokio::time::timeout(Duration::from_secs(2), reset)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_clean(&service, &job);
}

#[tokio::test]
async fn reset_is_durable_across_reopening_and_old_tokens_cannot_finish_a_new_reset() {
    let (tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    service.store.begin_reset().unwrap();
    let first = service.store.reset_intent().unwrap().unwrap();
    service.store.begin_reset().unwrap();
    assert!(!service.store.finish_reset(&first.token).unwrap());
    assert!(!service.store.entries(job.id).unwrap().is_empty());
    let mut reopened = recovery_service(tmp.path());
    reopened.staging_root = service.staging_root.clone();
    assert!(reopened.store.reset_pending().unwrap());
    reopened.perform_sync_reset().await.unwrap();
    assert_clean(&reopened, &job);
}

#[tokio::test]
async fn reset_cleanup_failure_keeps_recoverable_intent_and_reports_the_error() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    // A regular file in place of the owned staging directory must not be removed
    // as though it were a valid staging tree.
    std::fs::remove_dir_all(&service.staging_root).unwrap();
    std::fs::write(&service.staging_root, b"unexpected file").unwrap();
    service.store.begin_reset().unwrap();
    assert!(service.perform_sync_reset().await.is_err());
    assert!(service.store.reset_pending().unwrap());
    assert!(service.store.reset_error().unwrap().is_some());
    assert!(!service.store.entries(job.id).unwrap().is_empty());
    std::fs::remove_file(&service.staging_root).unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
}

#[tokio::test]
async fn reset_waits_for_configuration_operations_and_preserves_their_latest_fields() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    let transition = service
        .acquire_configuration_transition()
        .await
        .unwrap()
        .unwrap();
    service.store.begin_reset().unwrap();
    let worker_service = service.clone();
    let reset = tokio::spawn(async move { worker_service.perform_sync_reset().await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!reset.is_finished());
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::UploadOnly),
        )
        .unwrap();
    drop(transition);
    tokio::time::timeout(Duration::from_secs(2), reset)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_clean(&service, &job);
    assert_eq!(
        service.store.get_job(job.id).unwrap().unwrap().direction,
        SyncDirection::UploadOnly
    );
}

#[tokio::test]
async fn reset_is_scoped_to_its_database_and_owned_staging_tree() {
    let (_left, left, left_job) = reset_fixture();
    let (_right, right, right_job) = reset_fixture();
    seed_history(&left, &left_job);
    seed_history(&right, &right_job);
    let active = right.acquire_run_slot(right_job.id).await.unwrap();
    left.store.begin_reset().unwrap();
    tokio::time::timeout(Duration::from_secs(2), left.perform_sync_reset())
        .await
        .unwrap()
        .unwrap();
    assert_clean(&left, &left_job);
    assert!(!right.store.entries(right_job.id).unwrap().is_empty());
    assert!(right.staging_root.exists());
    assert!(!right.store.reset_pending().unwrap());
    drop(active);
}

#[tokio::test]
async fn reset_removes_nested_staging_symlinks_without_touching_media() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    std::os::unix::fs::symlink(&job.local_root, service.staging_root.join("link-to-media"))
        .unwrap();
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
}

#[tokio::test]
async fn reset_also_cleans_orphans_when_no_task_exists() {
    let temp = tempfile::tempdir().unwrap();
    let mut service = recovery_service(temp.path());
    service.staging_root = temp.path().join("staging");
    std::fs::create_dir_all(&service.staging_root).unwrap();
    std::fs::write(service.staging_root.join("orphan.upload"), b"orphan").unwrap();
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert!(!service.staging_root.exists());
    assert!(!service.store.reset_pending().unwrap());
    assert!(service.store.list_jobs().unwrap().is_empty());
}

#[tokio::test]
async fn reset_includes_disabled_accounts_without_altering_their_configuration() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    service
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE sync_connections SET enabled=0 WHERE id=?1",
            [job.connection_id],
        )
        .unwrap();
    assert!(service.store.list_jobs().unwrap().is_empty());
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
    let enabled: bool = service
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT enabled FROM sync_connections WHERE id=?1",
            [job.connection_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!enabled);
    assert_eq!(
        service
            .store
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .credential_ref,
        job.credential_ref
    );
}

#[tokio::test]
async fn reset_discards_accepted_conflict_choices_without_executing_them() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    let conflict = service.store.open_conflicts(job.id).unwrap().remove(0);
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Conflict(conflict, "keep_both".into()),
        )
        .unwrap();
    assert!(service
        .store
        .pending_change(job.id)
        .unwrap()
        .unwrap()
        .conflict
        .is_some());
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
    assert_eq!(
        service.store.get_job(job.id).unwrap().unwrap().direction,
        job.direction
    );
}

#[tokio::test]
async fn reset_rejects_late_scheduler_errors_without_recreating_runtime_fields() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    service
        .store
        .fail_scheduled_run(job.id, job.config_generation, "old failure after off")
        .unwrap();
    assert_clean(&service, &job);
    let fresh = service.store.get_job(job.id).unwrap().unwrap();
    service
        .store
        .fail_scheduled_run(job.id, fresh.config_generation, "new credential error")
        .unwrap();
    let state: (Option<i64>, Option<String>) = service
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT last_completed_at,last_error FROM sync_jobs WHERE id=?1",
            [job.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, (None, Some("new credential error".into())));
}

#[tokio::test]
async fn reset_blocks_old_configuration_installation_without_losing_accepted_fields() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    service
        .store
        .queue_edit(
            job.id,
            crate::core::sync::store::JobEdit::Direction(SyncDirection::UploadOnly),
        )
        .unwrap();
    let revision = service
        .store
        .pending_change(job.id)
        .unwrap()
        .unwrap()
        .revision;
    service.store.set_job_paused(job.id, true).unwrap();
    service.store.begin_reset().unwrap();
    service.store.begin_reset().unwrap();
    let generation = service
        .store
        .get_job(job.id)
        .unwrap()
        .unwrap()
        .config_generation;
    assert!(matches!(
        service.store.apply_change(job.id, revision).unwrap(),
        crate::core::sync::store::SyncWriteResult::Changed(false)
    ));
    assert_eq!(
        service
            .store
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .config_generation,
        generation
    );
    assert!(service.store.pending_change(job.id).unwrap().is_some());
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
    let saved = service.store.get_job(job.id).unwrap().unwrap();
    assert_eq!(saved.direction, SyncDirection::UploadOnly);
    assert!(saved.config_generation > generation);
}

#[tokio::test]
async fn reset_refuses_to_sweep_a_staging_tree_that_contains_configured_source_media() {
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    let photo = service.staging_root.join("source.gif");
    std::fs::copy(job.local_root.join("camera/picture.gif"), &photo).unwrap();
    let before = std::fs::read(&photo).unwrap();
    service
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE sync_jobs SET local_root=?1 WHERE id=?2",
            rusqlite::params![service.staging_root.to_string_lossy(), job.id],
        )
        .unwrap();
    service.store.begin_reset().unwrap();
    assert!(service.perform_sync_reset().await.is_err());
    assert_eq!(std::fs::read(&photo).unwrap(), before);
    assert!(service.store.reset_pending().unwrap());
    assert!(service.store.reset_error().unwrap().is_some());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn reset_does_not_close_another_librarys_preparing_progress_session() {
    // This current-thread fixture owns the process-wide progress test lock;
    // no operation awaited below tries to acquire that lock.
    let _progress = PROGRESS_TEST_LOCK.lock().unwrap();
    let (_tmp, service, job) = reset_fixture();
    seed_history(&service, &job);
    let other = PathBuf::from("different-library-preparing");
    live_progress_begin_session(&other);
    service.store.begin_reset().unwrap();
    service.perform_sync_reset().await.unwrap();
    assert_clean(&service, &job);
    assert!(
        live_progress().is_some(),
        "clearing one library must not clear another library's preparation session"
    );
    live_progress_end_session(&other);
    assert_eq!(live_progress(), None);
}
