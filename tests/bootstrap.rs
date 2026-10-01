//! 启动时一次性引导：扫描 + 聚合 albums
//!
//! 验证 `core::bootstrap::scan_and_aggregate` 在临时目录里完成扫描后，
//! `albums::list` 能正确返回按 `folder_path` 聚合的相册（含中文子目录）。
mod common;
use common::*;
use photo_viewer::core::albums;
use photo_viewer::core::bootstrap;
use photo_viewer::core::db;
use photo_viewer::core::events::{ChangeSource, DomainEvent};
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::media_change_notifier::MediaChangeNotifier;

#[test]
fn scan_and_aggregate_includes_chinese_subfolder_as_album() {
    let dir = tmp_dir();
    let shots = dir.path().join("截图");
    std::fs::create_dir(&shots).unwrap();
    write_plain_png(&shots, "a.png");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        bootstrap::scan_and_aggregate(&pool, &[dir.path().to_path_buf()])
            .await
            .unwrap();
    });

    let albums = albums::list(&pool).unwrap();
    let scr = albums
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "截图")
                .unwrap_or(false)
        })
        .expect("截图 album should exist");
    assert_eq!(scr.photo_count, 1);
}

#[test]
fn scan_and_aggregate_with_notifier_emits_upserted_items() {
    let dir = tmp_dir();
    write_plain_png(dir.path(), "visible.png");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let (notifier, mut rx) = MediaChangeNotifier::new();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        bootstrap::scan_and_aggregate_with_notifier(&pool, &[dir.path().to_path_buf()], notifier)
            .await
            .unwrap();
    });

    match rx.try_recv() {
        Ok(DomainEvent::MediaUpserted { source, items }) => {
            assert_eq!(source, ChangeSource::StartupScan);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].display_name(), "visible.png");
        }
        other => panic!("expected startup scan to emit UpsertedBatch, got {other:?}"),
    }
}

/// Drain a bounded domain-event channel, dropping only the panic-on-none
/// plumbing so a test can reason about the emitted order.
fn drain_events(rx: &mut tokio::sync::mpsc::Receiver<DomainEvent>) -> Vec<DomainEvent> {
    let mut events = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(event) => events.push(event),
            Err(_) => return events,
        }
    }
}

#[test]
fn scan_and_aggregate_with_actor_emits_actor_events() {
    let dir = tmp_dir();
    write_plain_png(dir.path(), "actor-visible.png");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let (sender, mut rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(pool.clone(), sender);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        bootstrap::scan_and_aggregate_with_actor(
            &pool,
            &[dir.path().to_path_buf()],
            db_actor.clone(),
        )
        .await
        .unwrap();
    });

    // The pass brackets its upsert batch with the scan-phase events that let the
    // Photos page tell "indexing" and "indexing failed" apart from an empty
    // library; without them a 16 s scan reads as "you have no photos".
    let events = drain_events(&mut rx);
    let upsert_at = events
        .iter()
        .position(|event| matches!(event, DomainEvent::MediaUpserted { .. }))
        .expect("startup scan must emit an upsert batch");
    assert!(
        matches!(
            events[0],
            DomainEvent::ScanPhase {
                active: true,
                error: None
            }
        ),
        "the phase must open before any row arrives, got {:?}",
        events[0]
    );
    let closed_at = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                DomainEvent::ScanPhase {
                    active: false,
                    error: None
                }
            )
        })
        .expect("a completed scan must close the phase without an error");
    assert!(
        closed_at > upsert_at,
        "the phase must close after the rows it produced"
    );
    match &events[upsert_at] {
        DomainEvent::MediaUpserted { source, items } => {
            assert_eq!(source, &ChangeSource::StartupScan);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].display_name(), "actor-visible.png");
        }
        _ => unreachable!("located by the MediaUpserted filter"),
    }
}

#[test]
fn unavailable_scan_root_keeps_existing_library_rows() {
    let dir = tmp_dir();
    let unavailable = dir.path().join("offline-volume");
    let path = unavailable.join("Camera/kept.jpg");
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    db::upsert_media_items_batch(
        &pool,
        &[NewMediaItem {
            uri: format!("file://{}", path.display()),
            path: path.clone(),
            folder_path: path.parent().unwrap().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(1),
            height: Some(1),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: chrono::Utc::now(),
            file_size: 1,
            blake3_hash: "kept".into(),
        }],
    )
    .unwrap();

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        bootstrap::scan_and_aggregate(&pool, &[unavailable])
            .await
            .unwrap();
    });

    assert_eq!(db::list_all_media(&pool).unwrap().len(), 1);
}
