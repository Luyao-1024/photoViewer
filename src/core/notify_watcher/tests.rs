use super::*;
use crate::core::db;
use crate::core::events::DomainEvent;
use crate::core::media::NewMediaItem;
use chrono::Utc;
use notify::{event::RemoveKind, Event};

fn actor_for(pool: db::DbPool) -> (DbActorHandle, tokio::sync::mpsc::Receiver<DomainEvent>) {
    let (sender, rx) = crate::core::events::DomainEventSender::new();
    (crate::core::db_actor::start_db_actor(pool, sender), rx)
}

#[test]
fn remove_event_deletes_media_row_and_emits_removed_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.jpg");
    std::fs::write(&path, b"not actually decoded in this test").unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let uri = format!("file://{}", path.display());
    db::insert_media_item(
        &pool,
        &NewMediaItem {
            uri: uri.clone(),
            path: path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "hash".into(),
        },
    )
    .unwrap();
    std::fs::remove_file(&path).unwrap();

    let (db_actor, mut rx) = actor_for(pool.clone());
    handle_event(
        &db_actor,
        Ok(Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![path],
            attrs: Default::default(),
        }),
    );

    assert!(db::list_all_media(&pool).unwrap().is_empty());
    match rx.try_recv() {
        Ok(DomainEvent::MediaRemoved { uris, .. }) => assert_eq!(uris, vec![uri]),
        other => panic!("expected Removed, got {other:?}"),
    }
}

#[test]
fn remove_event_accepts_video_media_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.mp4");
    std::fs::write(&path, b"not actually decoded in this test").unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let uri = format!("file://{}", path.display());
    db::insert_media_item(
        &pool,
        &NewMediaItem {
            uri: uri.clone(),
            path: path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "video/mp4".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "hash".into(),
        },
    )
    .unwrap();
    std::fs::remove_file(&path).unwrap();

    let (db_actor, mut rx) = actor_for(pool.clone());
    handle_event(
        &db_actor,
        Ok(Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![path],
            attrs: Default::default(),
        }),
    );

    assert!(db::list_all_media(&pool).unwrap().is_empty());
    match rx.try_recv() {
        Ok(DomainEvent::MediaRemoved { uris, .. }) => assert_eq!(uris, vec![uri]),
        other => panic!("expected Removed for video, got {other:?}"),
    }
}

#[test]
fn event_burst_coalesces_paths_into_one_database_delete() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let paths = [dir.path().join("a.jpg"), dir.path().join("b.jpg")];
    for path in &paths {
        db::insert_media_item(
            &pool,
            &NewMediaItem {
                uri: format!("file://{}", path.display()),
                path: path.clone(),
                folder_path: dir.path().to_path_buf(),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: None,
                height: None,
                video_duration_secs: None,
                taken_at: None,
                file_mtime: Utc::now(),
                file_size: 1,
                blake3_hash: String::new(),
            },
        )
        .unwrap();
    }
    let (actor, mut receiver) = actor_for(pool.clone());
    let mut trash_dirty = false;
    flush_event_burst(
        &actor,
        paths
            .iter()
            .cloned()
            .map(|path| {
                Ok(Event {
                    kind: EventKind::Remove(RemoveKind::File),
                    paths: vec![path],
                    attrs: Default::default(),
                })
            })
            .collect(),
        &[],
        &[],
        &mut trash_dirty,
    );

    assert!(db::list_all_media(&pool).unwrap().is_empty());
    let DomainEvent::MediaRemoved { mut uris, .. } = receiver.try_recv().unwrap() else {
        panic!("expected one batched remove event");
    };
    uris.sort();
    let mut expected: Vec<String> = paths
        .iter()
        .map(|path| format!("file://{}", path.display()))
        .collect();
    expected.sort();
    assert_eq!(uris, expected);
}

/// Regression: 删除到回收站后 gio 把文件移出受监听目录，watcher 会收到
/// 原路径的 Remove 事件。但该行的 `trashed_at` 已被应用置位（`move_to_trash`
/// 之后的 `db::mark_trashed`）——watcher 绝不能把它从 DB 硬删，否则
/// `list_trashed_media` 返回空、回收站页面"看不见图片"。之前这正是
/// `delete_media_by_path` 无条件按 path/uri 删行导致的。
#[test]
fn remove_event_keeps_trashed_row_for_trash_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trashed.jpg");
    std::fs::write(&path, b"not actually decoded in this test").unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let id = db::insert_media_item(
        &pool,
        &NewMediaItem {
            uri: format!("file://{}", path.display()),
            path: path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "hash".into(),
        },
    )
    .unwrap();
    // 应用侧已标记回收站（move_to_trash 之后的 db::mark_trashed）
    db::mark_trashed(&pool, id).unwrap();
    // gio 已把文件移走 → watcher 看到原路径的 Remove 事件
    std::fs::remove_file(&path).unwrap();

    let (db_actor, mut rx) = actor_for(pool.clone());
    handle_event(
        &db_actor,
        Ok(Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![path],
            attrs: Default::default(),
        }),
    );

    // 回收站行必须保留，回收站页面才看得见
    assert_eq!(
        db::list_trashed_media(&pool).unwrap().len(),
        1,
        "watcher must not hard-delete a row the app marked trashed"
    );
    // 且不应广播 Removed 事件让 UI 把它也从实时列表抹掉
    assert!(
        rx.try_recv().is_err(),
        "watcher must not emit Removed for a trashed row"
    );
}

#[test]
fn is_under_trash_classifies_paths_under_trash_roots() {
    let roots = vec![PathBuf::from("/home/u/.local/share/Trash")];
    assert!(is_under_trash(
        Path::new("/home/u/.local/share/Trash/files/x.jpg"),
        &roots
    ));
    assert!(is_under_trash(
        Path::new("/home/u/.local/share/Trash/info/x.jpg.trashinfo"),
        &roots
    ));
    assert!(!is_under_trash(
        Path::new("/home/u/图片/Camera/x.jpg"),
        &roots
    ));
    assert!(!is_under_trash(
        Path::new("/home/u/.local/share/other"),
        &roots
    ));
}

/// 落在回收站根下的事件只置脏位（等防抖对账），不应走 handle_event——否则会把
/// 回收站里的文件当相册文件 upsert/delete。
#[test]
fn dispatch_event_marks_trash_dirty_without_running_handle_event() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let (db_actor, mut rx) = actor_for(pool);
    let trash_roots = vec![dir.path().join("Trash")];
    let mut dirty = false;

    dispatch_event(
        &db_actor,
        Ok(Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![dir.path().join("Trash").join("files").join("x.jpg")],
            attrs: Default::default(),
        }),
        &trash_roots,
        &[],
        &mut dirty,
    );

    assert!(dirty, "a trash-dir event must set the dirty flag");
    assert!(
        rx.try_recv().is_err(),
        "a trash-dir event must not trigger handle_event / notifier"
    );
}

#[test]
fn scan_excluded_events_are_ignored_without_media_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
    let (db_actor, mut rx) = actor_for(pool);
    let trash_roots = Vec::new();
    let excluded_roots = vec![dir.path().join("Private")];
    let mut dirty = false;

    dispatch_event(
        &db_actor,
        Ok(Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![dir.path().join("Private").join("x.jpg")],
            attrs: Default::default(),
        }),
        &trash_roots,
        &excluded_roots,
        &mut dirty,
    );

    assert!(!dirty, "scan exclusions are separate from trash events");
    assert!(
        rx.try_recv().is_err(),
        "excluded scan events must not trigger media notifications"
    );
}

#[test]
fn effective_exclusions_include_paths_added_after_watcher_start() {
    let startup_excluded = vec![PathBuf::from("/library/OldPrivate")];
    let current_excluded = vec![PathBuf::from("/library/NewPrivate")];

    assert!(is_under_effective_excluded(
        Path::new("/library/OldPrivate/a.jpg"),
        &startup_excluded,
        &current_excluded
    ));
    assert!(is_under_effective_excluded(
        Path::new("/library/NewPrivate/a.jpg"),
        &startup_excluded,
        &current_excluded
    ));
    assert!(!is_under_effective_excluded(
        Path::new("/library/Public/a.jpg"),
        &startup_excluded,
        &current_excluded
    ));
}
