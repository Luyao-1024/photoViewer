use super::*;
use crate::core::media::NewMediaItem;
use chrono::TimeZone;
use tempfile::tempdir;

fn item(uri: &str, path: &str, folder: &str) -> NewMediaItem {
    let mtime = Utc.with_ymd_and_hms(2026, 7, 5, 12, 0, 0).unwrap();
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(mtime),
        file_mtime: mtime,
        file_size: 1000,
        blake3_hash: format!("hash-{uri}"),
    }
}

#[test]
fn refresh_does_not_expose_empty_folder_album_projection_to_readers() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("albums-refresh.db")).unwrap();
    db::insert_media_item(
        &pool,
        &item("file:///pictures/a.jpg", "/pictures/a.jpg", "/pictures"),
    )
    .unwrap();
    db::insert_media_item(
        &pool,
        &item(
            "file:///screenshots/b.jpg",
            "/screenshots/b.jpg",
            "/screenshots",
        ),
    )
    .unwrap();
    refresh(&pool).unwrap();
    assert_eq!(list(&pool).unwrap().len(), 2);

    refresh_with_observer_for_tests(&pool, || {
            let visible_folder_count = list_with_favorites(&pool)
                .unwrap()
                .into_iter()
                .filter(|album| !album.is_virtual)
                .count();
            assert_eq!(
                visible_folder_count, 2,
                "readers must keep seeing the previous folder album projection while refresh is rebuilding"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn refresh_caches_virtual_albums_but_folder_list_excludes_them() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("albums-virtual-cache.db")).unwrap();
    db::insert_media_item(
        &pool,
        &item("file:///pictures/a.jpg", "/pictures/a.jpg", "/pictures"),
    )
    .unwrap();
    refresh(&pool).unwrap();

    assert_eq!(
        list(&pool).unwrap().len(),
        1,
        "folder album list should not expose cached virtual album rows"
    );

    let conn = pool.get().unwrap();
    conn.execute(
        "UPDATE albums SET photo_count = 1234 WHERE folder_path = ?1",
        [IMAGES_ALBUM_PATH],
    )
    .unwrap();
    drop(conn);

    let images = list_with_favorites(&pool)
        .unwrap()
        .into_iter()
        .find(|album| album.is_images_album())
        .expect("images virtual album should come from the cached albums projection");
    assert_eq!(
            images.photo_count, 1234,
            "list_with_favorites should use cached virtual album rows instead of rescanning media_items"
        );
}

#[test]
fn folder_cover_lookup_uses_file_mtime_index() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("albums-cover-index.db")).unwrap();
    let conn = pool.get().unwrap();
    let mut stmt = conn
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT uri FROM media_items
             WHERE folder_path = ?1 AND trashed_at IS NULL
             ORDER BY file_mtime DESC
             LIMIT 1",
        )
        .unwrap();
    let plan = stmt
        .query_map(["/pictures"], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");

    assert!(
        plan.contains("idx_media_folder_mtime"),
        "folder-cover lookup must use the ordered partial index: {plan}"
    );
    assert!(
        !plan.contains("TEMP B-TREE"),
        "folder-cover lookup must not sort each folder in a temporary B-tree: {plan}"
    );
}
