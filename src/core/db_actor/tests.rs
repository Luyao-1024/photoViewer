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
    use crate::core::sync::{Fingerprint, NewSyncJob, SyncDirection, SyncStore, UploadScope};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir(&root).unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let (events, mut rx) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), events);
    let store = SyncStore::with_actor(pool, actor);

    tokio::task::spawn_blocking(move || {
        let job = store
            .create_job(&NewSyncJob {
                endpoint: "https://dav.example.test/root/".into(),
                username: "alice".into(),
                credential_ref: "sync-refresh-test".into(),
                local_root: root,
                remote_root: "PhotoViewer".into(),
                direction: SyncDirection::Bidirectional,
                upload_scope: UploadScope::All,
                upload_albums: Vec::new(),
            })
            .unwrap();
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
        store.set_upload_albums(job.id, &["Other".into()]).unwrap();
    })
    .await
    .unwrap();

    for _ in 0..4 {
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
