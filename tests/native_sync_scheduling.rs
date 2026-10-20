use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use photo_viewer::core::sync::native::protocol::{
    NativeCapabilityBits, NativeLimits, NativeNamespace, NativePinnedScope, NativeRecordLimits,
    NativeSyncLimits,
};

use photo_viewer::core::sync::{
    debounce, NativeZone, NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope,
};

async fn advance(duration: Duration) {
    tokio::time::advance(duration).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

async fn finish_attempt(
    service: &SyncService,
    job_id: i64,
    attempts: &AtomicUsize,
    expected: usize,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while attempts.load(Ordering::SeqCst) < expected || service.has_pending_work(job_id).unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "sync worker did not finish"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(attempts.load(Ordering::SeqCst), expected);
}

#[tokio::test(start_paused = true)]
async fn automatic_sync_requires_a_local_change_and_never_polls() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    let root = sandbox.scratch("sync-change-scheduling");
    let photos = root.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let pool = photo_viewer::core::db::init_pool(&root.path().join("library.db")).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    let service = SyncService::with_staging_root_for_tests(pool, root.path().join("staging"))
        .with_credential_reader(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            // Stop at the transport boundary without contacting a server. A network
            // failure remains eligible for automatic retry, exposing any idle poll.
            Err(photo_viewer::core::error::AppError::Backend(
                "network is unavailable: scheduler regression".into(),
            ))
        });
    let job = service
        .store()
        .create_job(&NewSyncJob {
            provider_kind: ProviderKind::NativeSyncServer,
            endpoint: "https://sync.example.invalid/".into(),
            credential_ref: "scheduler-test".into(),
            local_root: photos,
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::All,
            upload_albums: Vec::new(),
            native_zone: Some(NativeZone::new("PrimarySync")),
            native_pinned: Some(demo_pinned_scope(
                "https://sync.example.invalid/",
                &["PrimarySync"],
            )),
        })
        .unwrap();
    photo_viewer::core::prefs::set_sync_enabled(true).unwrap();
    service.start_scheduler();
    advance(Duration::ZERO).await;
    assert!(
        !service.has_pending_work(job.id).unwrap(),
        "startup must not request a cycle"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 0);

    advance(Duration::from_secs(45)).await;
    assert!(!service.has_pending_work(job.id).unwrap());
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        0,
        "idle time must not start synchronization"
    );

    debounce::note_local_change();
    advance(Duration::ZERO).await;
    advance(Duration::from_secs(9)).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    advance(Duration::from_secs(1)).await;
    finish_attempt(&service, job.id, &attempts, 1).await;
    assert_eq!(
        service.store().failure_cause(job.id).unwrap(),
        Some(photo_viewer::core::sync::SyncCause::Network)
    );

    advance(Duration::from_secs(45)).await;
    assert!(!service.has_pending_work(job.id).unwrap());
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "a failed cycle must not arm a periodic retry"
    );

    debounce::note_local_change();
    advance(Duration::ZERO).await;
    advance(debounce::QUIET_PERIOD).await;
    finish_attempt(&service, job.id, &attempts, 2).await;
}

#[tokio::test(start_paused = true)]
async fn startup_resumes_durable_work_once_without_polling() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    let root = sandbox.scratch("sync-resume-scheduling");
    let photos = root.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let pool = photo_viewer::core::db::init_pool(&root.path().join("library.db")).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    let service =
        SyncService::with_staging_root_for_tests(pool.clone(), root.path().join("staging"))
            .with_credential_reader(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(photo_viewer::core::error::AppError::Backend(
                    "network is unavailable: resume regression".into(),
                ))
            });
    let job = service
        .store()
        .create_job(&NewSyncJob {
            provider_kind: ProviderKind::NativeSyncServer,
            endpoint: "https://sync.example.invalid/".into(),
            credential_ref: "resume-test".into(),
            local_root: photos,
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::All,
            upload_albums: Vec::new(),
            native_zone: Some(NativeZone::new("PrimarySync")),
            native_pinned: Some(demo_pinned_scope(
                "https://sync.example.invalid/",
                &["PrimarySync"],
            )),
        })
        .unwrap();
    photo_viewer::core::prefs::set_sync_enabled(true).unwrap();

    // A job that owes nothing keeps the launch silent.
    assert_eq!(service.resume_unfinished_work().unwrap(), 0);
    advance(Duration::ZERO).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 0);

    // Durable work left by an earlier run is resumed exactly once.
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_entries(job_id, relative_path, native_dirty, native_local_version)
             VALUES(?1, 'deferred.jpg', 2, 1)",
            [job.id],
        )
        .unwrap();
    assert_eq!(service.resume_unfinished_work().unwrap(), 1);
    finish_attempt(&service, job.id, &attempts, 1).await;

    advance(Duration::from_secs(45)).await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "resuming owed work must not arm a periodic retry"
    );

    // An open conflict needs the user's choice and blocks the upload lease, so
    // resuming behind it would spin without settling anything.
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_conflicts(job_id, entry_id, kind)
             SELECT job_id, id, 'native' FROM sync_entries WHERE job_id=?1",
            [job.id],
        )
        .unwrap();
    assert_eq!(
        service.resume_unfinished_work().unwrap(),
        0,
        "a launch must not resume behind an open conflict"
    );
    advance(Duration::from_secs(45)).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

fn demo_pinned_scope(public_base_url: &str, zones: &[&str]) -> NativePinnedScope {
    NativePinnedScope {
        authentication: None,
        public_base_url: public_base_url.into(),
        namespace: NativeNamespace {
            tenant: "local".into(),
            container: "com.apple.photos.cloud".into(),
            environment: "production".into(),
            database: "private".into(),
        },
        database_generation: "501855bb-18d3-4008-abc3-37e4948bf6f0".into(),
        protocol_version: 1,
        service_version: "0.1.0".into(),
        stage: "M4".into(),
        checksum_algorithm: "SHA256".into(),
        zones: zones.iter().map(|zone| (*zone).to_string()).collect(),
        capabilities: NativeCapabilityBits {
            zone_listing: true,
            record_crud: true,
            snapshots: true,
            incremental_changes: true,
            upload_leases: true,
            fencing: true,
            token_current_gate: true,
            upload_phases: true,
            operation_status_read: true,
            asset_upload: true,
            asset_download: true,
            photo_publication: true,
            query_batch: false,
            change_aliases: false,
            apple_authentication: false,
            sharing: false,
            upload_resume: false,
        },
        limits: NativeLimits {
            max_asset_bytes: 536_870_912,
            thumbnail_max_bytes: 2_097_152,
            upload_concurrency: 2,
            download_concurrency: 8,
            upload_idle_timeout_secs: 30,
            upload_total_timeout_secs: 3600,
            db_queue_capacity: 128,
            json_body_limit: 1_048_576,
            lease_ttl_secs: 90,
            lease_heartbeat_secs: 30,
            control_queue_capacity: 16,
        },
        sync_limits: NativeSyncLimits {
            max_page: 200,
            max_zones: 20,
        },
        record_limits: NativeRecordLimits {
            max_operations: 400,
            max_record_bytes: 1_048_576,
            max_result_bytes: 8_388_608,
        },
    }
}
