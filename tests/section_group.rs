use chrono::{TimeZone, Utc};
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::section_model::{group_items, GroupBy};
use std::path::PathBuf;

fn item(id: i64, year: i32, month: u32, day: u32) -> MediaItem {
    let dt = Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap();
    MediaItem {
        id,
        uri: format!("file:///test/{id}.jpg"),
        path: PathBuf::from(format!("/test/{id}.jpg")),
        folder_path: PathBuf::from("/test"),
        mime_type: "image/jpeg".into(),
        width: Some(100),
        height: Some(100),
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 1000,
        blake3_hash: format!("h{id}"),
        trashed_at: None,
    }
}

fn item_without_taken_at(id: i64, year: i32, month: u32, day: u32) -> MediaItem {
    let file_time = Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap();
    MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: format!("/tmp/{id}.jpg").into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        width: None,
        height: None,
        taken_at: None,
        file_mtime: file_time,
        file_size: 0,
        blake3_hash: format!("hash-{id}"),
        trashed_at: None,
    }
}

#[test]
fn group_by_year() {
    let items = vec![
        item(1, 2025, 3, 1),
        item(2, 2025, 8, 1),
        item(3, 2024, 1, 1),
    ];
    let sections = group_items(&items, GroupBy::Year);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].label, "2025 · 2 张");
    assert_eq!(sections[1].label, "2024 · 1 张");
    assert_eq!(sections[0].items.len(), 2);
}

#[test]
fn group_by_month() {
    let items = vec![
        item(1, 2025, 3, 1),
        item(2, 2025, 3, 15),
        item(3, 2025, 4, 1),
        item(4, 2024, 12, 31),
    ];
    let sections = group_items(&items, GroupBy::Month);
    assert_eq!(sections.len(), 3);
    assert_eq!(sections[0].label, "2025年3月 · 2 张");
    assert_eq!(sections[1].label, "2025年4月 · 1 张");
    assert_eq!(sections[2].label, "2024年12月 · 1 张");
}

#[test]
fn group_by_day() {
    // 2025-03-02 实际是周日；2025-03-15 实际是周六
    let items = vec![
        item(1, 2025, 3, 2),
        item(2, 2025, 3, 2),
        item(3, 2025, 3, 15),
    ];
    let sections = group_items(&items, GroupBy::Day);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].label, "2025年3月2日 周日 · 2 张");
    assert_eq!(sections[1].label, "2025年3月15日 周六 · 1 张");
}

#[test]
fn missing_taken_at_uses_file_time_instead_of_unknown_date() {
    let mut a = item(1, 2025, 3, 1);
    a.taken_at = None;
    let b = item(2, 2025, 3, 2);
    let sections = group_items(&[a, b], GroupBy::Year);
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].label, "2025 · 2 张");
}

#[test]
fn missing_taken_at_groups_by_file_time() {
    let items = vec![
        item_without_taken_at(1, 2024, 6, 1),
        item_without_taken_at(2, 2024, 6, 2),
    ];

    let sections = group_items(&items, GroupBy::Day);

    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].key.year, Some(2024));
    assert_eq!(sections[0].key.month, Some(6));
    assert_eq!(sections[0].key.day, Some(1));
    assert_eq!(sections[1].key.day, Some(2));
}
