use super::*;
use crate::core::events::DomainEvent;
use crate::core::media::{NewMediaItem, MEDIA_SUBKIND_STANDARD};
use chrono::Utc;

#[test]
fn notify_interval_picks_tier_by_cumulative_count() {
    // < 5 000 → 2s
    assert_eq!(notify_interval(0), NOTIFY_INTERVAL_SMALL);
    assert_eq!(notify_interval(4_999), NOTIFY_INTERVAL_SMALL);
    // 5 000–19 999 → 5s
    assert_eq!(notify_interval(5_000), NOTIFY_INTERVAL_MEDIUM);
    assert_eq!(notify_interval(19_999), NOTIFY_INTERVAL_MEDIUM);
    // >= 20 000 → 10s
    assert_eq!(notify_interval(20_000), NOTIFY_INTERVAL_LARGE);
    assert_eq!(notify_interval(500_000), NOTIFY_INTERVAL_LARGE);
}

#[test]
fn startup_scan_prunes_missing_live_rows_and_notifies_ui() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleted-while-closed.jpg");
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let uri = format!("file://{}", path.display());
    crate::core::db::insert_media_item(
        &pool,
        &NewMediaItem {
            uri: uri.clone(),
            path,
            folder_path: dir.path().to_path_buf(),
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
        },
    )
    .unwrap();

    let (notifier, mut rx) = MediaChangeNotifier::new();
    scan_and_aggregate_with_notifier_blocking(
        pool.clone(),
        vec![dir.path().to_path_buf()],
        notifier,
    )
    .unwrap();

    assert!(crate::core::db::list_all_media(&pool).unwrap().is_empty());
    match rx.try_recv() {
        Ok(DomainEvent::MediaRemoved { uris, .. }) => assert_eq!(uris, vec![uri]),
        other => panic!("expected MediaRemoved for startup prune, got {other:?}"),
    }
}
