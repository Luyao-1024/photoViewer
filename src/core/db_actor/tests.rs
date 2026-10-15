use super::*;
use crate::core::media::MEDIA_SUBKIND_STANDARD;
use chrono::Utc;

fn new_item(path: PathBuf) -> NewMediaItem {
    NewMediaItem {
        uri: format!("file://{}", path.display()),
        path: path.clone(),
        folder_path: path.parent().unwrap().to_path_buf(),
        mime_type: "image/jpeg".into(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: None,
        height: None,
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc::now(),
        file_size: 1,
        blake3_hash: String::new(),
    }
}

#[test]
fn write_priorities_put_interactive_work_before_background_work() {
    let thumbnail = DbCommand::MarkThumbnailsGenerated {
        ids: vec![MediaId::from(1)],
    };
    let refresh = DbCommand::RefreshAlbumsInternal;
    let scan = DbCommand::PruneMissingLiveRows {
        missing: Vec::new(),
    };
    let watcher = DbCommand::DeleteLiveByPath {
        source: ChangeSource::FilesystemWatcher,
        path: PathBuf::from("/tmp/a.jpg"),
    };
    let user = DbCommand::SetFavorite {
        ids: vec![MediaId::from(1)],
        is_favorite: true,
    };

    assert!(refresh.priority() > thumbnail.priority());
    assert!(scan.priority() > refresh.priority());
    assert!(watcher.priority() > scan.priority());
    assert!(user.priority() > watcher.priority());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_sync_writes_reply_to_runtime_worker() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let (events, _rx) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), events);
    let store = crate::core::sync::store::SyncStore::with_actor(pool, actor);

    tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        tokio::spawn(async move {
            for _ in 0..100 {
                store.mark_job_started(999).unwrap();
            }
        })
        .await
        .unwrap();
    })
    .await
    .expect("blocking database replies must wake the runtime worker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_state_writes_emit_refresh_events() {
    use crate::core::sync::native::protocol::NativePinnedScope;
    use crate::core::sync::{
        NativeCapabilityBits, NativeLimits, NativeNamespace, NativeZone, NewSyncJob, SyncDirection,
        SyncStore, UploadScope,
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir(&root).unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let (events, mut rx) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), events);
    let store = SyncStore::with_actor(pool, actor);

    tokio::task::spawn_blocking(move || {
        store
            .create_job(&NewSyncJob {
                provider_kind: crate::core::sync::ProviderKind::NativeSyncServer,
                endpoint: "https://sync.example.test/".into(),
                credential_ref: "sync-refresh-test".into(),
                local_root: root,
                direction: SyncDirection::Bidirectional,
                upload_scope: UploadScope::All,
                upload_albums: Vec::new(),
                native_zone: Some(NativeZone::new("PrimarySync")),
                native_pinned: Some(NativePinnedScope {
                    authentication: None,
                    public_base_url: "https://sync.example.test/".into(),
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
                    zones: vec!["PrimarySync".into()],
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
                    sync_limits: Default::default(),
                    record_limits: Default::default(),
                }),
            })
            .unwrap();
        store.set_upload_albums(1, &["Other".into()]).unwrap();
    })
    .await
    .unwrap();

    // Creating a connection and applying an upload-scope change both move cloud
    // state, so the grid and the overview have to be told to refresh.
    for _ in 0..2 {
        assert!(matches!(rx.try_recv(), Ok(DomainEvent::SyncStateDirty)));
    }
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn set_favorite_updates_db_and_emits_precise_event() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let id = db::insert_media_item(&pool, &new_item(dir.path().join("a.jpg"))).unwrap();
    let (events, mut rx) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), events);

    actor
        .execute(DbCommand::SetFavorite {
            ids: vec![MediaId::from(id)],
            is_favorite: true,
        })
        .await
        .unwrap();

    assert!(db::get_media_item(&pool, id).unwrap().is_favorite);
    match rx.recv().await.unwrap() {
        DomainEvent::MediaUpdated { items, fields, .. } => {
            assert_eq!(items[0].id, id);
            assert!(fields.favorite);
        }
        other => panic!("expected MediaUpdated, got {other:?}"),
    }
}

#[tokio::test]
async fn delete_live_by_path_removes_row_and_emits_uri() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.jpg");
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let uri = format!("file://{}", path.display());
    db::insert_media_item(&pool, &new_item(path.clone())).unwrap();
    let (events, mut rx) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), events);

    actor
        .execute(DbCommand::DeleteLiveByPath {
            source: ChangeSource::FilesystemWatcher,
            path,
        })
        .await
        .unwrap();

    assert!(db::list_all_media(&pool).unwrap().is_empty());
    match rx.recv().await.unwrap() {
        DomainEvent::MediaRemoved { uris, .. } => assert_eq!(uris, vec![uri]),
        other => panic!("expected MediaRemoved, got {other:?}"),
    }
}

#[tokio::test]
async fn trash_commit_emits_precise_moved_event_after_mark_without_sync_album_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let id = db::insert_media_item(&pool, &new_item(dir.path().join("a.jpg"))).unwrap();
    let (events, mut rx) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), events);
    crate::core::albums::refresh(&pool).unwrap();

    let prepared = actor
        .execute(DbCommand::MarkTrashed {
            ids: vec![MediaId::from(id)],
        })
        .await
        .unwrap();
    assert!(db::get_media_item(&pool, id).unwrap().trashed_at.is_some());

    let DbCommandResult::MediaItems(items) = prepared else {
        panic!("expected prepared media items");
    };
    actor
        .execute(DbCommand::CommitMovedToTrash {
            items: items.clone(),
        })
        .await
        .unwrap();

    let albums = crate::core::albums::list(&pool).unwrap();
    assert_eq!(
        albums[0].photo_count, 1,
        "commit should publish the media event immediately; RefreshCoordinator rebuilds the derived album projection asynchronously"
    );

    match rx.recv().await.unwrap() {
        DomainEvent::MediaMovedToTrash { items, .. } => assert_eq!(items[0].id, id),
        other => panic!("expected MediaMovedToTrash, got {other:?}"),
    }
    assert!(
        matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "the precise media event already drives RefreshCoordinator; do not queue a duplicate AlbumsChanged refresh"
    );
}

#[tokio::test]
async fn mark_thumbnails_generated_runs_through_db_actor() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let id = db::insert_media_item(&pool, &new_item(dir.path().join("thumb.jpg"))).unwrap();
    let (events, _rx) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), events);

    actor
        .execute(DbCommand::MarkThumbnailsGenerated {
            ids: vec![MediaId::from(id)],
        })
        .await
        .unwrap();

    assert_eq!(db::count_thumbnail_generated(&pool).unwrap(), 1);
}
