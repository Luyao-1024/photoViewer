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

#[test]
fn repository_favorite_summary_batches_ids() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-favs.db")).unwrap();
    let inserted =
        photo_viewer::core::db::upsert_media_items_batch(&pool, &[item("a", 10), item("b", 20)])
            .unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[0].id, true).unwrap();

    let repo = MediaRepository::new(pool);
    let summary = repo
        .favorite_state(&[inserted[0].id.into(), inserted[1].id.into()])
        .unwrap();

    assert!(summary.has_favorite);
    assert!(summary.has_unfavorite);
}

#[test]
fn repository_set_favorite_returns_changed_items() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-set-fav.db")).unwrap();
    let inserted =
        photo_viewer::core::db::upsert_media_items_batch(&pool, &[item("a", 10)]).unwrap();

    let repo = MediaRepository::new(pool);
    let mutation = repo.set_favorite(&[inserted[0].id.into()], true).unwrap();

    assert_eq!(mutation.changed_ids, vec![inserted[0].id.into()]);
    assert_eq!(mutation.changed_items.len(), 1);
    assert!(mutation.changed_items[0].is_favorite);
}

#[test]
fn repository_upsert_batch_returns_changed_items() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-upsert.db")).unwrap();
    let repo = MediaRepository::new(pool);

    let mutation = repo.upsert_batch(&[item("a", 10), item("b", 20)]).unwrap();

    assert_eq!(mutation.changed_ids.len(), 2);
    assert_eq!(mutation.changed_items.len(), 2);
    assert_eq!(mutation.changed_items[0].uri, "file:///tmp/a.jpg");
    assert_eq!(mutation.changed_items[1].uri, "file:///tmp/b.jpg");
}
