//! Functional cleanup: real local DB/files with an isolated in-memory keyring.
mod common;
use photo_viewer::{
    core::{
        db,
        db_actor::start_db_actor,
        events::DomainEventSender,
        prefs,
        sync::{
            service::cleanup::{database_usage, DataKind},
            SyncService,
        },
    },
    platform::credentials,
};
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn categories_preserve_or_remove_login_records_and_secrets_together() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    common::credentials::install();
    for (name, kind) in [
        ("library", DataKind::Library),
        ("accounts", DataKind::Accounts),
        ("all", DataKind::All),
    ] {
        let directory = sandbox.scratch(&format!("cleanup-fixture-{name}"));
        let root = directory.path().join("originals");
        std::fs::create_dir_all(&root).unwrap();
        let original = common::write_plain_jpeg(&root, "keep.jpg");
        let original_bytes = std::fs::read(&original).unwrap();
        let pool = db::init_pool(&directory.path().join("photos.db")).unwrap();
        let (events, _receiver) = DomainEventSender::new();
        let actor = start_db_actor(pool.clone(), events);
        let service = SyncService::with_actor(pool.clone(), actor.clone());
        let reference = format!("credential-to-remove-{name}");
        let connection = {
            let conn = pool.get().unwrap();
            conn.execute("INSERT INTO sync_connections(endpoint,credential_ref) VALUES('https://sync.example.invalid/',?1)", [&reference]).unwrap();
            conn.last_insert_rowid()
        };
        let job = {
            let conn = pool.get().unwrap();
            conn.execute("INSERT INTO sync_jobs(connection_id,local_root,native_zone_name) VALUES(?1,?2,'PrimarySync')", rusqlite::params![connection,root.to_string_lossy()]).unwrap();
            conn.last_insert_rowid()
        };
        let orphan = format!("{reference}-without-job");
        pool.get().unwrap().execute("INSERT INTO sync_connections(endpoint,credential_ref,enabled) VALUES('https://sync.example.invalid/',?1,0)", [&orphan]).unwrap();
        credentials::store_api_key(&orphan, "orphan-login-token").unwrap();
        credentials::store_api_key(&reference, "isolated-login-token").unwrap();
        pool.get().unwrap().execute(
            "INSERT INTO media_items(uri,path,folder_path,mime_type,file_mtime,file_size,blake3_hash,indexed_at) VALUES(?1,?2,?3,'image/jpeg',0,1,'',0)",
            rusqlite::params![photo_viewer::core::file_uri::from_path(&original), original.to_string_lossy(),original.parent().unwrap().to_string_lossy()],
        ).unwrap();
        let thumbnail = photo_viewer::config::cache_dir().join("thumbnails/test.jpg");
        std::fs::create_dir_all(thumbnail.parent().unwrap()).unwrap();
        std::fs::write(&thumbnail, b"thumbnail").unwrap();
        let log = photo_viewer::config::cache_dir().join("logs/manual.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, b"keep unless clearing all").unwrap();
        assert!(
            database_usage(&pool, true) > 0,
            "account space must be measurable"
        );
        prefs::set_sync_enabled(kind == DataKind::Library).unwrap();
        service.clear_local_data(actor, kind).await.unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
        let connections: i64 = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_connections", [], |r| r.get(0))
            .unwrap();
        let media = db::list_all_media(&pool).unwrap();
        if kind == DataKind::Library {
            assert_eq!(connections, 2);
            assert!(service.store().get_job(job).unwrap().is_some());
            assert_eq!(
                credentials::load_api_key(&reference).unwrap(),
                "isolated-login-token"
            );
            assert!(media.is_empty());
            assert!(!thumbnail.exists());
            assert!(log.exists());
            assert_eq!(
                credentials::load_api_key(&orphan).unwrap(),
                "orphan-login-token"
            );
            credentials::delete_api_key(&reference).unwrap();
            credentials::delete_api_key(&orphan).unwrap();
        } else {
            assert_eq!(connections, 0);
            assert!(credentials::load_api_key(&reference).is_err());
            assert!(credentials::load_api_key(&orphan).is_err());
            // Login references must be reclaimed from DB pages/WAL, not just
            // hidden from SELECT results while stale bytes remain on disk.
            for filename in ["photos.db", "photos.db-wal"] {
                let bytes = std::fs::read(directory.path().join(filename)).unwrap_or_default();
                assert!(
                    !bytes
                        .windows(reference.len())
                        .any(|part| part == reference.as_bytes()),
                    "login reference remains in {filename}"
                );
            }
            for table in [
                "sync_jobs",
                "sync_entries",
                "sync_tasks",
                "sync_native_progress",
                "sync_native_records",
                "sync_native_resources",
                "sync_publications",
                "sync_job_changes",
                "settings",
            ] {
                let count: i64 = pool
                    .get()
                    .unwrap()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 0, "{table} still retains account state");
            }
            if kind == DataKind::Accounts {
                assert_eq!(media.len(), 1);
                assert!(thumbnail.exists());
                assert!(log.exists());
            } else {
                assert!(media.is_empty());
                assert!(!thumbnail.exists());
                assert!(!log.exists());
            }
        }
        assert_eq!(prefs::sync_enabled(), kind == DataKind::Library);
    }
}
#[test]
fn clearing_logs_alone_keeps_login_and_media_records() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    common::credentials::install();
    let directory = sandbox.scratch("cleanup-logs-only");
    let pool = db::init_pool(&directory.path().join("photos.db")).unwrap();
    let original = common::write_plain_jpeg(directory.path(), "keep.jpg");
    pool.get().unwrap().execute("INSERT INTO sync_connections(endpoint,credential_ref) VALUES('https://sync.invalid/','keep-reference')", []).unwrap();
    pool.get().unwrap().execute(
        "INSERT INTO media_items(uri,path,folder_path,mime_type,file_mtime,file_size,blake3_hash,indexed_at) VALUES(?1,?2,?3,'image/jpeg',0,1,'',0)",
        rusqlite::params![photo_viewer::core::file_uri::from_path(&original), original.to_string_lossy(),directory.path().to_string_lossy()],
    ).unwrap();
    credentials::store_api_key("keep-reference", "keep-token").unwrap();
    photo_viewer::core::diagnostics::clear_logs().unwrap();
    assert_eq!(
        pool.get()
            .unwrap()
            .query_row("SELECT credential_ref FROM sync_connections", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "keep-reference"
    );
    assert_eq!(
        credentials::load_api_key("keep-reference").unwrap(),
        "keep-token"
    );
    assert_eq!(db::list_all_media(&pool).unwrap().len(), 1);
    assert!(original.exists());
    credentials::delete_api_key("keep-reference").unwrap();
}
