//! The actual local mutation path must retain deletion state until cloud success.
use super::*;

#[test]
fn permanent_deletion_survives_failure_and_reopen_until_cloud_confirmation() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    photo_viewer::core::prefs::set_trash_backend(photo_viewer::core::prefs::TrashBackend::App)
        .unwrap();
    let Some(server) = Server::start() else {
        return skip();
    };
    let root = sandbox.scratch("durable-permanent-delete");
    let source = root.path().join("photos");
    std::fs::create_dir_all(&source).unwrap();
    let path = common::write_plain_jpeg(&source, "photo.jpg");
    common::cache_local_thumbnail(&path);
    block_on(async {
        use photo_viewer::core::{
            db,
            db_actor::{start_db_actor, DbCommand},
            events::{ChangeSource, DomainEventSender},
            identity::MediaId,
            repository::MediaRepository,
            sync::{NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
        };
        let database = root.path().join("library.db");
        let pool = db::init_pool(&database).unwrap();
        let (sender, _events) = DomainEventSender::new();
        let actor = start_db_actor(pool.clone(), sender);
        let key = server.api_key.clone();
        let service = SyncService::with_actor(pool.clone(), actor.clone())
            .with_credential_reader(move |_| Ok(key.clone()));
        actor
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items: vec![
                    photo_viewer::core::backend::local::LocalBackend::new_item_from_path(&path)
                        .unwrap()
                        .unwrap(),
                ],
            })
            .unwrap();
        let job = service
            .store()
            .create_job(&NewSyncJob {
                provider_kind: ProviderKind::NativeSyncServer,
                endpoint: server.base_url.clone(),
                credential_ref: "deletion-test".into(),
                local_root: source.clone(),
                direction: SyncDirection::Bidirectional,
                upload_scope: UploadScope::All,
                upload_albums: vec![],
                native_zone: Some(NativeZone::new("PrimarySync")),
                native_pinned: Some(server.client().probe().await.unwrap()),
            })
            .unwrap();
        assert_eq!(
            service.run_cycle(job.id).await.unwrap().mutations_confirmed,
            1
        );
        let media = db::list_all_media(&pool).unwrap().remove(0);
        assert_eq!(
            media.storage_location,
            photo_viewer::core::media::MediaLocation::Both
        );
        let repository = MediaRepository::new(pool.clone());
        repository
            .move_to_trash(&[MediaId::from(media.id)])
            .unwrap();
        let dirty: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT native_dirty FROM sync_entries WHERE job_id=?1",
                [job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dirty, 2, "trash is metadata, not deletion");
        let trashed = service.run_cycle(job.id).await.unwrap();
        assert!(trashed.failures.is_empty(), "{trashed:?}");
        let deleted = repository.delete_batch(&[MediaId::from(media.id)], Some(&actor));
        assert!(deleted.failures.is_empty(), "{deleted:?}");
        assert!(!path.exists());
        assert!(photo_viewer::core::trash::trashed_file_uri(&media.uri).is_err());
        let state = |pool: &db::DbPool| {
            pool.get().unwrap().query_row("SELECT native_dirty,native_photo_id,native_master_id FROM sync_entries WHERE job_id=?1",[job.id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).unwrap()
        };
        let pending = state(&pool);
        assert_eq!(pending.0, 4);
        let failed = service.clone().with_credential_reader(|_| {
            Err(photo_viewer::core::error::AppError::Backend(
                "test unavailable credential".into(),
            ))
        });
        assert!(failed.run_cycle(job.id).await.is_err());
        assert_eq!(state(&pool), pending);
        let reopened = db::init_pool(&database).unwrap();
        assert_eq!(state(&reopened), pending);
        let confirmed = service.run_cycle(job.id).await.unwrap();
        assert!(confirmed.failures.is_empty(), "{confirmed:?}");
        assert_eq!(confirmed.outstanding, 0);
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM sync_entries WHERE job_id=?1",
                    [job.id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    });
}
