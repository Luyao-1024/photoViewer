use super::*;
use tempfile::tempdir;

fn item(name: &str, taken_at: Option<DateTime<Utc>>, file_mtime: DateTime<Utc>) -> NewMediaItem {
    NewMediaItem {
        uri: format!("file:///library/{name}.jpg"),
        path: PathBuf::from(format!("/library/{name}.jpg")),
        folder_path: PathBuf::from("/library"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at,
        file_mtime,
        file_size: 1000,
        blake3_hash: format!("hash-{name}"),
    }
}

#[test]
fn batch_ranks_match_the_full_position_projection() {
    let dir = tempdir().unwrap();
    let pool = init_pool(&dir.path().join("position.db")).unwrap();
    let base = Utc.with_ymd_and_hms(2026, 7, 5, 12, 0, 0).unwrap();
    let items = [
        item("newest", Some(base), base),
        item("night", Some(base - chrono::Duration::hours(18)), base),
        item("tie-a", Some(base - chrono::Duration::days(1)), base),
        item("tie-b", Some(base - chrono::Duration::days(1)), base),
        item("mtime", None, base - chrono::Duration::days(2)),
    ];
    let ids = items
        .iter()
        .map(|item| insert_media_item(&pool, item).unwrap())
        .collect::<Vec<_>>();

    let ranks = media_ranks_in_order(&pool, &ids, "trashed_at IS NULL", &[], false).unwrap();
    assert_eq!(ranks.len(), ids.len());
    for id in &ids {
        let (one_based, _) = media_position(&pool, *id, "trashed_at IS NULL", &[], false)
            .unwrap()
            .expect("fixture row is live");
        assert_eq!(
            ranks.get(id).copied(),
            Some(one_based - 1),
            "rank mismatch for media {id}"
        );
    }
    assert!(!ranks.contains_key(&(ids.len() as i64 + 1_000)));

    // A filter that matches none of the batch yields no ranks, so the grid
    // treats every event as belonging to another query.
    assert!(
        media_ranks_in_order(&pool, &ids, "trashed_at IS NOT NULL", &[], true)
            .unwrap()
            .is_empty()
    );
}
