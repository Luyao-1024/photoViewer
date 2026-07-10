use super::*;
use crate::core::media::{NewMediaItem, MEDIA_SUBKIND_STANDARD};
use chrono::{TimeZone, Utc};

fn sample_new_media(path: &std::path::Path) -> NewMediaItem {
    let dt = Utc.with_ymd_and_hms(2026, 7, 3, 12, 0, 0).unwrap();
    NewMediaItem {
        uri: format!("file://{}", path.display()),
        path: path.to_path_buf(),
        folder_path: path.parent().unwrap().to_path_buf(),
        mime_type: "image/png".into(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 123,
        blake3_hash: String::new(),
    }
}

#[test]
fn startup_preload_reconcile_runs_before_initial_live_page_query() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("photos.db");
    let media_path = dir.path().join("camera.png");

    let pool = init_pool(&db_path).unwrap();
    let id = crate::core::db::insert_media_item(&pool, &sample_new_media(&media_path)).unwrap();
    drop(pool);

    let (_pool, items) = initialize_db_once_blocking_with_preload(db_path, 500, |pool| {
        crate::core::db::mark_trashed(pool, id)
    })
    .unwrap();

    assert!(
        items.is_empty(),
        "startup reconcile/preload work must happen before the first live media page is read"
    );
}

#[test]
fn legacy_ui_event_progress_logs_stay_debug() {
    let source = include_str!("../app.rs");
    let production_source = source
        .split("\n#[cfg(test)]\nmod tests;")
        .next()
        .expect("app.rs must contain production code");

    for message in [
        "SIDEBAR_TRACE album_refresh_callback_refresh_sidebar_snapshot",
        "SIDEBAR_TRACE domain_event_received",
        "SIDEBAR_TRACE schedule_album_refresh",
        "UI_CHANGE_APPLY",
    ] {
        let message_index = production_source
            .find(message)
            .unwrap_or_else(|| panic!("missing log message {message}"));
        let before = &production_source[..message_index];
        let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
            .iter()
            .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, candidate)| candidate)
            .expect("log message should be inside a tracing macro");
        assert_eq!(
                actual_macro, "tracing::debug!(",
                "{message} is high-volume UI event diagnostics and should stay out of default INFO logs"
            );
    }
}

#[test]
fn high_volume_trace_logs_stay_debug_across_modules() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "ui/apply_to_media_list.rs",
            include_str!("../ui/apply_to_media_list.rs"),
            &[
                "TRASH_TRACE ui_apply_moved_to_trash_begin",
                "TRASH_TRACE ui_apply_moved_to_trash_done",
                "TRASH_TRACE ui_remove_uris_batch",
            ],
        ),
        (
            "ui/photos_page.rs",
            include_str!("../ui/photos_page.rs"),
            &["TRASH_TRACE photos_delete_requested count="],
        ),
        (
            "ui/search_page.rs",
            include_str!("../ui/search_page.rs"),
            &["TRASH_TRACE search_delete_requested count="],
        ),
        (
            "ui/album_detail_page.rs",
            include_str!("../ui/album_detail_page.rs"),
            &["TRASH_TRACE album_detail_delete_requested count="],
        ),
        (
            "ui/viewer/actions.rs",
            include_str!("../ui/viewer/actions.rs"),
            &["TRASH_TRACE viewer_delete_requested id="],
        ),
        (
            "ui/window/albums.rs",
            include_str!("../ui/window/albums.rs"),
            &["PHOTO_REFRESH_TRACE refresh_after_album_operation"],
        ),
        (
            "core/bootstrap.rs",
            include_str!("../core/bootstrap.rs"),
            &["STARTUP_SCAN_NOTIFY interval_flush"],
        ),
    ];

    for (path, source, messages) in cases {
        let production_source = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .expect("source file must contain production code");
        for message in *messages {
            let message_index = production_source
                .find(message)
                .unwrap_or_else(|| panic!("missing log message {message} in {path}"));
            let before = &production_source[..message_index];
            let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
                .iter()
                .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
                .max_by_key(|(index, _)| *index)
                .map(|(_, candidate)| candidate)
                .unwrap_or_else(|| panic!("{message} in {path} should be inside tracing macro"));
            assert_eq!(
                    actual_macro, "tracing::debug!(",
                    "{message} in {path} can scale with batch size or media count and should stay out of default INFO logs"
                );
        }
    }
}
