//! Opt-in, repeatable large-library benchmark.
//!
//! Run with:
//! `PHOTOVIEWER_BENCH_ITEMS=100000 cargo test --release --test library_benchmark -- --ignored --nocapture`

use photo_viewer::core::db;
use photo_viewer::core::db::SearchField;
use photo_viewer::core::repository::{MediaQuery, MediaRepository};
use std::time::Instant;

#[test]
#[ignore = "opt-in performance benchmark"]
fn large_library_query_baseline() {
    let count = std::env::var("PHOTOVIEWER_BENCH_ITEMS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(10_000)
        .clamp(1_000, 1_000_000);
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("benchmark.db")).unwrap();

    let insert_started = Instant::now();
    {
        let mut conn = pool.get().unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO media_items
                     (uri, path, folder_path, mime_type, media_kind, media_subkind,
                      media_attributes, media_type_flags, width, height, taken_at,
                      file_mtime, file_mtime_ns, file_size, blake3_hash, indexed_at)
                     VALUES (?1, ?2, ?3, 'image/jpeg', 'image', 'standard', '{}', 0,
                             1920, 1080, ?4, ?4, ?5, 1024, '', unixepoch())",
                )
                .unwrap();
            for index in 0..count {
                let name = if index == count / 2 {
                    format!("benchmark-needle-{index}.jpg")
                } else {
                    format!("benchmark-{index}.jpg")
                };
                let path = format!("/benchmark/album-{}/{name}", index % 100);
                let uri = format!("file://{path}");
                let timestamp = 1_700_000_000_i64 + index as i64;
                statement
                    .execute(rusqlite::params![
                        uri,
                        path,
                        format!("/benchmark/album-{}", index % 100),
                        timestamp,
                        timestamp.saturating_mul(1_000_000_000),
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }

    let repository = MediaRepository::new(pool.clone());
    let first_page_started = Instant::now();
    let first_page = repository.page(MediaQuery::LiveAll, 0, 200).unwrap();
    let first_page_elapsed = first_page_started.elapsed();

    let random_page_started = Instant::now();
    let random_page = repository
        .page(MediaQuery::LiveAll, (count / 2) as u32, 200)
        .unwrap();
    let random_page_elapsed = random_page_started.elapsed();

    let search_started = Instant::now();
    let search = repository
        .page(
            MediaQuery::Search {
                term: "needle".into(),
                field: SearchField::Name,
            },
            0,
            200,
        )
        .unwrap();
    let search_elapsed = search_started.elapsed();

    let unchanged_started = Instant::now();
    let unchanged = db::load_unchanged_index(&pool).unwrap();
    let unchanged_elapsed = unchanged_started.elapsed();

    assert_eq!(first_page.total, count as u32);
    assert!(!random_page.items.is_empty());
    assert_eq!(search.total, 1);
    assert_eq!(unchanged.len(), count);
    println!(
        "items={count} insert_ms={} first_page_ms={} random_offset_ms={} search_ms={} unchanged_snapshot_ms={}",
        insert_started.elapsed().as_millis(),
        first_page_elapsed.as_millis(),
        random_page_elapsed.as_millis(),
        search_elapsed.as_millis(),
        unchanged_elapsed.as_millis(),
    );
}
