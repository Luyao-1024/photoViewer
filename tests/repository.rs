mod common;

use chrono::{TimeZone, Utc};
use photo_viewer::core::db::SearchField;
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

fn item_with_attrs(id_name: &str, ts: i64, attrs: &str) -> NewMediaItem {
    let mut item = item(id_name, ts);
    item.media_attributes = attrs.into();
    item
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
fn repository_attribute_page_returns_only_matching_live_media() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-attribute.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[
            item_with_attrs("animated_old", 10, r#"{"animated":true}"#),
            item("plain_new", 30),
            item_with_attrs("animated_new", 40, r#"{"animated":true}"#),
        ],
    )
    .unwrap();
    photo_viewer::core::db::mark_trashed(&pool, inserted[0].id).unwrap();

    let repo = MediaRepository::new(pool);
    let page = repo
        .page(
            MediaQuery::Attribute(photo_viewer::core::media::MEDIA_ATTRIBUTE_ANIMATED.into()),
            0,
            10,
        )
        .unwrap();

    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].uri, "file:///tmp/animated_new.jpg");
}

#[test]
fn repository_attribute_neighbor_stays_inside_attribute_projection() {
    let dir = common::tmp_dir();
    let pool =
        photo_viewer::core::db::init_pool(&dir.path().join("repo-attribute-neighbor.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[
            item_with_attrs("animated_old", 10, r#"{"animated":true}"#),
            item("plain_middle", 20),
            item_with_attrs("animated_new", 30, r#"{"animated":true}"#),
        ],
    )
    .unwrap();
    let query = MediaQuery::Attribute(photo_viewer::core::media::MEDIA_ATTRIBUTE_ANIMATED.into());
    let repo = MediaRepository::new(pool);

    let neighbor = repo
        .neighbor(query.clone(), MediaId::from(inserted[2].id), 1)
        .unwrap()
        .unwrap();

    assert_eq!(neighbor.query, query);
    assert_eq!(neighbor.index, 1);
    assert_eq!(neighbor.total, 2);
    assert_eq!(neighbor.item.uri, "file:///tmp/animated_old.jpg");
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
        .page(
            MediaQuery::Search {
                term: "summer".into(),
                field: SearchField::All,
            },
            0,
            10,
        )
        .unwrap();

    assert_eq!(page.total, 2);
    let names: Vec<_> = page
        .items
        .iter()
        .map(|item| item.display_name().to_string())
        .collect();
    assert_eq!(names, vec!["family_summer.jpg", "Summer_Beach.jpg"]);

    // Search by date only.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "1970-01-01".into(),
                field: SearchField::All,
            },
            0,
            10,
        )
        .unwrap();

    let names: Vec<_> = page
        .items
        .iter()
        .map(|item| item.display_name().to_string())
        .collect();
    assert_eq!(names, vec!["Summer_Beach.jpg"]);

    // Search by filename only — date term should not match.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "1970-01-01".into(),
                field: SearchField::Name,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(page.total, 0);

    // Search by filename only — name term should match.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "summer".into(),
                field: SearchField::Name,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(page.total, 2);

    // Search by date only — name term should not match.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "summer".into(),
                field: SearchField::Date,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(page.total, 0);

    // Search by date only — date term should match.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "1970-01-01".into(),
                field: SearchField::Date,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(page.total, 1);
}

#[test]
fn repository_date_search_supports_year_month_day_granularity() {
    let dir = common::tmp_dir();
    let pool =
        photo_viewer::core::db::init_pool(&dir.path().join("repo-date-granularity.db")).unwrap();

    // Create items with different dates using timestamps.
    // 2025-01-15 00:00:00 UTC = 1736899200
    // 2025-06-20 00:00:00 UTC = 1750377600
    // 2025-10-01 00:00:00 UTC = 1759276800
    // 2024-03-10 00:00:00 UTC = 1710028800
    photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[
            item("photo_jan", 1736899200),
            item("photo_jun", 1750377600),
            item("photo_oct", 1759276800),
            item("photo_2024", 1710028800),
        ],
    )
    .unwrap();

    let repo = MediaRepository::new(pool);

    // Search by year only — should match all 2025 items.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "2025".into(),
                field: SearchField::Date,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(page.total, 3, "Year search should match 3 items from 2025");

    // Search by year/month with slash separator.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "2025/06".into(),
                field: SearchField::Date,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(
        page.total, 1,
        "Year/month search should match 1 item from June 2025"
    );

    // Search by year/month with dash separator.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "2025-10".into(),
                field: SearchField::Date,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(
        page.total, 1,
        "Year/month search should match 1 item from October 2025"
    );

    // Search by full date with slash.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "2025/01/15".into(),
                field: SearchField::Date,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(
        page.total, 1,
        "Full date search should match exactly Jan 15, 2025"
    );

    // Search for different year.
    let page = repo
        .page(
            MediaQuery::Search {
                term: "2024".into(),
                field: SearchField::Date,
            },
            0,
            10,
        )
        .unwrap();
    assert_eq!(page.total, 1, "Year search should match 1 item from 2024");
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

#[test]
fn db_favorite_neighbor_uses_favorite_projection_order() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-fav-neighbor.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[
            item("old_fav", 10),
            item("middle_fav", 20),
            item("new_unfav", 30),
            item("new_fav", 40),
        ],
    )
    .unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[0].id, true).unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[1].id, true).unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[3].id, true).unwrap();

    let neighbor =
        photo_viewer::core::db::favorite_media_neighbor(&pool, inserted[1].id, 1).unwrap();

    let (index, total, item) = neighbor.unwrap();
    assert_eq!(index, 2);
    assert_eq!(total, 3);
    assert_eq!(item.uri, "file:///tmp/old_fav.jpg");
}

#[test]
fn repository_neighbor_returns_adjacent_media_for_favorites_query_order() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-fav-neighbor-wrapper.db"))
        .unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[
            item("old_fav", 10),
            item("middle_fav", 20),
            item("new_unfav", 30),
            item("new_fav", 40),
        ],
    )
    .unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[0].id, true).unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[1].id, true).unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[3].id, true).unwrap();
    let repo = MediaRepository::new(pool);

    let neighbor = repo
        .neighbor(MediaQuery::Favorites, MediaId::from(inserted[1].id), -1)
        .unwrap()
        .unwrap();

    assert_eq!(neighbor.query, MediaQuery::Favorites);
    assert_eq!(neighbor.index, 0);
    assert_eq!(neighbor.total, 3);
    assert_eq!(neighbor.item.uri, "file:///tmp/new_fav.jpg");
}

#[test]
fn db_search_neighbor_uses_search_projection_order() {
    let dir = common::tmp_dir();
    let pool =
        photo_viewer::core::db::init_pool(&dir.path().join("repo-search-neighbor.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[
            item("summer_old", 10),
            item("summer_middle", 20),
            item("winter_new", 30),
            item("summer_new", 40),
        ],
    )
    .unwrap();

    let neighbor = photo_viewer::core::db::search_media_neighbor(
        &pool,
        "summer",
        None,
        SearchField::Name,
        inserted[1].id,
        -1,
    )
    .unwrap()
    .unwrap();

    let (index, total, item) = neighbor;
    assert_eq!(index, 0);
    assert_eq!(total, 3);
    assert_eq!(item.uri, "file:///tmp/summer_new.jpg");
}

#[test]
fn db_trashed_media_page_returns_bounded_rows() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-trash-page.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("old", 10), item("middle", 20), item("new", 30)],
    )
    .unwrap();
    for item in &inserted {
        photo_viewer::core::db::mark_trashed(&pool, item.id).unwrap();
    }

    let page = photo_viewer::core::db::list_trashed_media_page(&pool, 0, 2).unwrap();

    assert_eq!(page.len(), 2);
    assert_eq!(page[0].uri, "file:///tmp/new.jpg");
    assert_eq!(page[1].uri, "file:///tmp/middle.jpg");
}

#[test]
fn repository_neighbor_returns_adjacent_media_for_trash_query_order() {
    let dir = common::tmp_dir();
    let pool =
        photo_viewer::core::db::init_pool(&dir.path().join("repo-trash-neighbor.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("old", 10), item("middle", 20), item("new", 30)],
    )
    .unwrap();
    for item in &inserted {
        photo_viewer::core::db::mark_trashed(&pool, item.id).unwrap();
    }
    let repo = MediaRepository::new(pool);

    let neighbor = repo
        .neighbor(MediaQuery::Trash, MediaId::from(inserted[1].id), -1)
        .unwrap()
        .unwrap();

    assert_eq!(neighbor.query, MediaQuery::Trash);
    assert_eq!(neighbor.index, 0);
    assert_eq!(neighbor.total, 3);
    assert_eq!(neighbor.item.uri, "file:///tmp/new.jpg");
}
