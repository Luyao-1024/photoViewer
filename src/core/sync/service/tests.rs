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
