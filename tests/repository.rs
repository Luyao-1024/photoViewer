mod common;

use chrono::{TimeZone, Utc};
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::repository::{MediaQuery, MediaRepository};

fn item(id_name: &str, ts: i64) -> NewMediaItem {
    let path = std::path::PathBuf::from(format!("/tmp/{id_name}.jpg"));
    NewMediaItem {
        uri: format!("file:///tmp/{id_name}.jpg"),
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
fn repository_live_page_returns_total_and_ordered_rows() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo.db")).unwrap();
    photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("older", 10), item("newer", 20), item("middle", 15)],
    )
    .unwrap();

    let repo = MediaRepository::new(pool);
    let page = repo.page(MediaQuery::LiveAll, 0, 2).unwrap();

    assert_eq!(page.total, 3);
    assert_eq!(page.start, 0);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].uri, "file:///tmp/newer.jpg");
    assert_eq!(page.items[1].uri, "file:///tmp/middle.jpg");
}
