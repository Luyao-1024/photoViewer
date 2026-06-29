mod common;

use chrono::{TimeZone, Utc};
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::repository::{MediaQuery, MediaRepository};
use photo_viewer::ui::models::media_window_model::MediaWindowModel;

fn item(name: &str, ts: i64) -> NewMediaItem {
    let path = std::path::PathBuf::from(format!("/tmp/{name}.jpg"));
    NewMediaItem {
        uri: format!("file:///tmp/{name}.jpg"),
        path: path.clone(),
        folder_path: std::path::PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc.timestamp_opt(ts, 0).unwrap(),
        file_size: 1,
        blake3_hash: String::new(),
    }
}

#[test]
fn media_window_selection_survives_window_replacement_by_id() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("window.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("a", 30), item("b", 20), item("c", 10)],
    )
    .unwrap();
    let selected = inserted[1].id.into();

    let repo = MediaRepository::new(pool);
    let mut model = MediaWindowModel::new(MediaQuery::LiveAll, 2);
    model.load_sync(&repo, 0).unwrap();
    model.select(selected);
    model.load_sync(&repo, 1).unwrap();

    assert!(model.is_selected(selected));
    assert_eq!(model.window_start(), 1);
}

#[test]
fn stale_generation_does_not_replace_newer_window() {
    let mut model = MediaWindowModel::new(MediaQuery::LiveAll, 2);
    let old = model.next_generation_for_tests();
    let new = model.next_generation_for_tests();

    assert!(!model.generation_is_current_for_tests(old));
    assert!(model.generation_is_current_for_tests(new));
}
