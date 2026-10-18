//! Service-level behavior that does not need a network or a keyring.
//!
//! Everything that does need a server lives in `tests/native_sync_probe.rs`,
//! which drives the real `syncserver` binary.

use super::*;

#[test]
fn a_fresh_database_has_no_jobs_and_synchronization_is_off() {
    let root =
        std::env::temp_dir().join(format!("photoviewer-sync-service-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp root");
    let pool = crate::core::db::init_pool(&root.join("library.db")).unwrap();
    let service = SyncService::new(pool);
    assert!(service.store().list_jobs().unwrap().is_empty());
    assert_eq!(service.pending_configuration_edits().unwrap(), 0);
}

#[tokio::test]
async fn probing_an_endpoint_that_cannot_be_reached_never_writes_a_row() {
    let root = std::env::temp_dir().join(format!("photoviewer-sync-probe-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp root");
    let pool = crate::core::db::init_pool(&root.join("library.db")).unwrap();
    let service = SyncService::new(pool.clone());

    // Rejected before any request leaves the process, so this needs no server.
    let error = service
        .probe_native_connection("http://sync.example.invalid/", &"a".repeat(32))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("HTTPS"), "{error}");

    let jobs: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_jobs", [], |row| row.get(0))
        .unwrap();
    let connections: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_connections", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!((jobs, connections), (0, 0));
}

#[tokio::test]
async fn native_backup_is_refused_before_it_can_probe_or_write() {
    let root = std::env::temp_dir().join(format!("photoviewer-sync-backup-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp root");
    let pool = crate::core::db::init_pool(&root.join("library.db")).unwrap();
    let service = SyncService::new(pool.clone());

    let error = service
        .create_native_job(
            "https://sync.example.invalid/",
            "ref-backup",
            &"a".repeat(32),
            &root,
            "PrimarySync",
            SyncDirection::UploadOnly,
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Backup"), "{error}");

    let jobs: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_jobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(jobs, 0);
}

#[tokio::test]
async fn single_account_switch_drains_old_runs_before_admitting_the_new_account() {
    let _sandbox = crate::testing::Sandbox::install();
    let root = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&root.path().join("accounts.db")).unwrap();
    let service =
        SyncService::with_staging_root_for_tests(pool.clone(), root.path().join("staging"));
    std::fs::create_dir(root.path().join("photos")).unwrap();
    let mut first = super::super::store::tests::new_job(root.path().join("photos"));
    first.credential_ref = "account-one".into();
    let a = service.store().create_job(&first).unwrap();
    let mut second = first.clone();
    second.credential_ref = "account-two".into();
    second.native_pinned.as_mut().unwrap().namespace.tenant = "another-account".into();
    let b = service.store().create_job(&second).unwrap();
    assert!(service.store().job_is_selected(a.id).unwrap());
    assert!(!service.store().job_is_selected(b.id).unwrap());
    assert!(
        service.acquire_run_slot(b.id).await.is_err(),
        "inactive accounts are refused while admission is still locked"
    );
    let slot = service.acquire_run_slot(a.id).await.unwrap();
    let switching = service.clone();
    let switch = tokio::spawn(async move { switching.activate_native_account(b.id).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while service.store().job_is_selected(a.id).unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !switch.is_finished(),
        "accepted switch waits for the old run"
    );
    let admitting = service.clone();
    let new_run = tokio::spawn(async move { admitting.acquire_run_slot(b.id).await });
    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    assert!(
        !new_run.is_finished(),
        "new account cannot overlap the old run"
    );
    drop(slot);
    tokio::time::timeout(std::time::Duration::from_secs(2), switch)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(new_run.await.unwrap().unwrap());
    let reopened = SyncService::with_staging_root_for_tests(pool, root.path().join("staging"));
    assert!(!reopened.store().job_is_selected(a.id).unwrap());
    assert!(reopened.store().job_is_selected(b.id).unwrap());
    assert!(reopened
        .run_cycle(a.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("inactive"));
    // An old publication that cannot be safely settled rejects the switch.
    reopened.pool().get().unwrap().execute(
        "INSERT INTO sync_publications(publication_id,job_id,state,recovery_present) VALUES('broken-old-publication',?1,'publishing',1)", [b.id]
    ).unwrap();
    assert!(reopened.activate_native_account(a.id).await.is_err());
    assert!(reopened.store().job_is_selected(b.id).unwrap());
    assert!(!reopened.store().job_is_selected(a.id).unwrap());
}

/// Every cause is answered by the retry policy on purpose, so this table is the
/// policy: a cause added later has to be placed here deliberately.
#[test]
fn only_failures_the_user_can_clear_stop_the_unattended_scheduler() {
    use crate::core::sync::{SyncCause, SyncClass};
    let stops = |cause: SyncCause| !super::scheduling::unattended_retry_is_worthwhile(Some(cause));

    // The service answers these again exactly as before until the user acts, so
    // a timer that keeps asking spends a lease and a round trip to be told the
    // same thing.
    assert!(stops(SyncCause::Quota));
    assert!(stops(SyncCause::Credential));
    assert_eq!(SyncCause::Quota.class(), SyncClass::NeedsUser);
    assert_eq!(SyncCause::Credential.class(), SyncClass::NeedsUser);

    // These may well clear on their own, so the scheduler keeps trying them.
    for cause in [
        SyncCause::Network,
        SyncCause::ServerBusy,
        SyncCause::Conflict,
        SyncCause::Checksum,
        SyncCause::Unknown,
    ] {
        assert!(
            super::scheduling::unattended_retry_is_worthwhile(Some(cause)),
            "{cause:?} should still be retried unattended"
        );
    }
    // A job that has not failed yet is always worth a cycle.
    assert!(super::scheduling::unattended_retry_is_worthwhile(None));
}

#[test]
fn a_stored_quota_rejection_is_recognised_from_the_job_error_alone() {
    let root = std::env::temp_dir().join(format!("photoviewer-sync-cause-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    let pool = crate::core::db::init_pool(&root.join("library.db")).unwrap();
    let store = crate::core::sync::SyncStore::new(pool);
    let job = store
        .create_job(&crate::core::sync::NewSyncJob {
            provider_kind: crate::core::sync::ProviderKind::NativeSyncServer,
            endpoint: "https://sync.example.test/".into(),
            credential_ref: "cause-credential".into(),
            local_root: root.clone(),
            native_zone: Some(crate::core::sync::NativeZone::new("PrimarySync")),
            native_pinned: Some(crate::core::sync::native::demo_pinned_scope(
                "https://sync.example.test/",
                &["PrimarySync"],
            )),
            direction: crate::core::sync::SyncDirection::Bidirectional,
            upload_scope: crate::core::sync::UploadScope::All,
            upload_albums: Vec::new(),
        })
        .unwrap();

    assert_eq!(store.failure_cause(job.id).unwrap(), None);
    store
        .mark_job_failed(
            job.id,
            "a/photo.png: transport failed: remote version changed: \
             HTTP 409 STORAGE_QUOTA_EXCEEDED: Account storage quota exceeded",
        )
        .unwrap();
    assert_eq!(
        store.failure_cause(job.id).unwrap(),
        Some(crate::core::sync::SyncCause::Quota)
    );
    // A clean run clears the recorded failure, so the scheduler resumes on its
    // own once the cause is gone.
    store.mark_job_completed(job.id).unwrap();
    assert_eq!(store.failure_cause(job.id).unwrap(), None);
}
