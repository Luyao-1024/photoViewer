mod common;

use chrono::{TimeZone, Utc};
use photo_viewer::core::identity::MediaId;
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::repository::{MediaNeighbor, MediaQuery, MediaRepository};
use std::path::Path;

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

fn item_at(dir: &Path, file_name: &str, ts: i64) -> NewMediaItem {
    let path = dir.join(file_name);
    NewMediaItem {
        uri: format!("file://{}", path.display()),
        path: path.clone(),
        folder_path: dir.to_path_buf(),
        mime_type: if file_name.ends_with(".mp4") {
            "video/mp4".into()
        } else {
            "image/jpeg".into()
        },
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
fn repository_items_returns_ordered_rows_without_page_total() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-items.db")).unwrap();
    photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("older", 10), item("newer", 20), item("middle", 15)],
    )
    .unwrap();

    let repo = MediaRepository::new(pool);
    let items = repo.items(MediaQuery::LiveAll, 0, 2).unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].uri, "file:///tmp/newer.jpg");
    assert_eq!(items[1].uri, "file:///tmp/middle.jpg");
}

#[test]
fn repository_searches_live_media_by_file_name_and_capture_date() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-search.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[
            item("Summer_Beach", 10),
            item("winter", 90_000),
            item("family_summer", 172_800),
            item("summer_trashed", 40),
        ],
    )
    .unwrap();
    photo_viewer::core::db::mark_trashed(&pool, inserted[3].id).unwrap();

    let repo = MediaRepository::new(pool);
    let page = repo
        .page(MediaQuery::Search("summer".into()), 0, 10)
        .unwrap();

    assert_eq!(page.total, 2);
    let names: Vec<_> = page
        .items
        .iter()
        .map(|item| item.display_name().to_string())
        .collect();
    assert_eq!(names, vec!["family_summer.jpg", "Summer_Beach.jpg"]);

    let page = repo
        .page(MediaQuery::Search("1970-01-01".into()), 0, 10)
        .unwrap();

    let names: Vec<_> = page
        .items
        .iter()
        .map(|item| item.display_name().to_string())
        .collect();
    assert_eq!(names, vec!["Summer_Beach.jpg"]);
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

#[test]
fn repository_rename_media_file_preserves_original_extension() {
    let dir = common::tmp_dir();
    let media_path = dir.path().join("IMG_001.jpg");
    std::fs::write(&media_path, b"jpeg").unwrap();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-rename.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item_at(dir.path(), "IMG_001.jpg", 10)],
    )
    .unwrap();
    let repo = MediaRepository::new(pool);

    let mutation = repo
        .rename_media_file(MediaId::from(inserted[0].id), "holiday.png")
        .unwrap();

    let renamed = dir.path().join("holiday.jpg");
    assert!(
        renamed.exists(),
        "rename should keep the original .jpg suffix"
    );
    assert!(!dir.path().join("holiday.png").exists());
    assert_eq!(mutation.changed_items[0].path, renamed);
    assert_eq!(mutation.changed_items[0].display_name(), "holiday.jpg");
}

#[test]
fn repository_library_stats_counts_only_current_generated_thumbnails() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-stats.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("fresh", 30), item("stale", 20), item("missing", 10)],
    )
    .unwrap();
    photo_viewer::core::db::mark_thumbnails_generated(&pool, &[inserted[0].id]).unwrap();
    photo_viewer::core::db::set_thumbnail_generated_at_for_tests(&pool, inserted[1].id, 1).unwrap();

    let repo = MediaRepository::new(pool);
    let stats = repo.library_stats().unwrap();

    assert_eq!(stats.live_total, 3);
    assert_eq!(stats.thumbnails_generated, 1);
}

#[test]
fn repository_neighbor_returns_adjacent_media_for_live_query_order() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-neighbor.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("old", 10), item("middle", 20), item("new", 30)],
    )
    .unwrap();
    let repo = MediaRepository::new(pool);

    let neighbor = repo
        .neighbor(MediaQuery::LiveAll, MediaId::from(inserted[1].id), 1)
        .unwrap();

    assert!(matches!(
        neighbor,
        Some(MediaNeighbor {
            index: 2,
            total: 3,
            ..
        })
    ));
    assert_eq!(neighbor.unwrap().item.uri, "file:///tmp/old.jpg");

    let neighbor = repo
        .neighbor(MediaQuery::LiveAll, MediaId::from(inserted[1].id), -1)
        .unwrap()
        .unwrap();
    assert_eq!(neighbor.index, 0);
    assert_eq!(neighbor.item.uri, "file:///tmp/new.jpg");
}
