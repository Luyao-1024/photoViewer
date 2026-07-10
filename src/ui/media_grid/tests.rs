use super::*;
use chrono::{TimeZone, Utc};
use std::path::PathBuf;

fn sample_item(id: i64, name: &str) -> MediaItem {
    let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
    MediaItem {
        id,
        uri: format!("file:///tmp/{name}"),
        path: PathBuf::from(format!("/tmp/{name}")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/png".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 100,
        blake3_hash: format!("hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}

#[test]
fn thumbnail_request_mtime_uses_indexed_file_mtime_without_stat() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.jpg");
    std::fs::write(&path, b"image").unwrap();

    let indexed_mtime = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap();
    let mut item = sample_item(99, "photo.jpg");
    item.path = path;
    item.file_mtime = indexed_mtime;

    assert_eq!(
        thumbnail_request_mtime(&item),
        std::time::SystemTime::from(indexed_mtime),
        "thumbnail requests should reuse the indexed mtime instead of stat-ing on the UI thread"
    );
}

#[test]
fn progressive_render_progress_logs_stay_debug() {
    let production_source = media_grid_production_sources();

    for message in [
        "PROGRESSIVE_RENDER seed",
        "PROGRESSIVE_RENDER done",
        "PROGRESSIVE_RENDER tick",
        "PROGRESSIVE_RENDER append",
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
            "{message} should stay out of default logs"
        );
    }
}

#[test]
fn high_frequency_thumbnail_trace_spans_stay_debug() {
    let production_source = media_grid_production_sources();

    for span_name in ["grid:reprioritize", "grid:thumb_request"] {
        let span_index = production_source
            .find(span_name)
            .unwrap_or_else(|| panic!("missing trace span {span_name}"));
        let before = &production_source[..span_index];
        let actual_macro = [
            "tracing::debug_span!(",
            "tracing::info_span!(",
            "tracing::warn_span!(",
        ]
        .iter()
        .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
        .max_by_key(|(index, _)| *index)
        .map(|(_, candidate)| candidate)
        .expect("span should be inside a tracing span macro");
        assert_eq!(
                actual_macro, "tracing::debug_span!(",
                "{span_name} is high-frequency diagnostic tracing and should stay out of default INFO logs"
            );
    }
}

fn media_grid_production_sources() -> String {
    let mut production_source = include_str!("../media_grid.rs").to_string();
    production_source.push_str(include_str!("selection.rs"));
    production_source.push_str(include_str!("updates.rs"));
    production_source.push_str(include_str!("viewport.rs"));
    production_source.push_str(include_str!("loading.rs"));
    production_source.push_str(include_str!("render.rs"));
    production_source.push_str(include_str!("virtual_paging.rs"));
    production_source
}
