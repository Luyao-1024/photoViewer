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
        "INSERT INTO sync_publications(publication_id,job_id,state,role_state) VALUES('broken-old-publication',?1,'publishing','{}')", [b.id]
    ).unwrap();
    assert!(reopened.activate_native_account(a.id).await.is_err());
    assert!(reopened.store().job_is_selected(b.id).unwrap());
    assert!(!reopened.store().job_is_selected(a.id).unwrap());
}
