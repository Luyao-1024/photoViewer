use super::*;
use crate::core::i18n::tr;

#[test]
fn make_key_year_only() {
    // 仅通过 mode 验证字段集合
    let k = SectionKey {
        year: Some(2025),
        month: None,
        day: None,
    };
    assert_eq!(k.year, Some(2025));
    assert!(k.month.is_none());
    assert!(k.day.is_none());
}

#[test]
fn weekday_label_keys() {
    assert_eq!(weekday_cn(Weekday::Sun), tr("date.weekday.sun"));
    assert_eq!(weekday_cn(Weekday::Sat), tr("date.weekday.sat"));
}

#[test]
fn groupby_default_is_day() {
    assert_eq!(GroupBy::default(), GroupBy::Day);
}

#[test]
fn group_items_preserves_first_seen_section_order_and_counts() {
    let mk = |id: i64, y: i32, m: u32, d: u32| MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: format!("/tmp/{id}.jpg").into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: Some(
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        ),
        file_mtime: NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc(),
        file_size: 1,
        blake3_hash: format!("h{id}"),
        is_favorite: false,
        trashed_at: None,
    };
    let items = vec![
        mk(1, 2026, 5, 2),
        mk(2, 2025, 12, 31),
        mk(3, 2026, 5, 2),
        mk(4, 2026, 5, 1),
    ];

    let sections = group_items(&items, GroupBy::Day);
    let keys: Vec<_> = sections.iter().map(|section| section.key.clone()).collect();

    assert_eq!(
        keys,
        vec![
            SectionKey {
                year: Some(2026),
                month: Some(5),
                day: Some(2)
            },
            SectionKey {
                year: Some(2025),
                month: Some(12),
                day: Some(31)
            },
            SectionKey {
                year: Some(2026),
                month: Some(5),
                day: Some(1)
            },
        ]
    );
    assert_eq!(sections[0].items.len(), 2);
    assert_eq!(sections[1].items.len(), 1);
    assert_eq!(sections[2].items.len(), 1);
}

#[test]
fn counts_from_date_groups_folds_by_mode() {
    // (year, month, day, count)：2025-03 两天 + 2025-01 一天 + 2024-12 一天
    let groups = vec![
        (2025, 3, 1, 7),
        (2025, 3, 15, 3),
        (2025, 1, 10, 4),
        (2024, 12, 25, 5),
    ];

    // Year：2025 = 7+3+4 = 14，2024 = 5
    let year = counts_from_date_groups(&groups, GroupBy::Year);
    assert_eq!(
        year.get(&SectionKey {
            year: Some(2025),
            month: None,
            day: None
        }),
        Some(&14)
    );
    assert_eq!(
        year.get(&SectionKey {
            year: Some(2024),
            month: None,
            day: None
        }),
        Some(&5)
    );
    assert_eq!(year.len(), 2);

    // Month：2025-03 = 7+3 = 10，2025-01 = 4，2024-12 = 5
    let month = counts_from_date_groups(&groups, GroupBy::Month);
    assert_eq!(
        month.get(&SectionKey {
            year: Some(2025),
            month: Some(3),
            day: None
        }),
        Some(&10)
    );
    assert_eq!(
        month.get(&SectionKey {
            year: Some(2024),
            month: Some(12),
            day: None
        }),
        Some(&5)
    );
    assert_eq!(month.len(), 3);

    // Day：逐日直传
    let day = counts_from_date_groups(&groups, GroupBy::Day);
    assert_eq!(
        day.get(&SectionKey {
            year: Some(2025),
            month: Some(3),
            day: Some(1)
        }),
        Some(&7)
    );
    assert_eq!(day.len(), 4);
}

#[test]
fn counts_from_date_groups_empty() {
    assert!(counts_from_date_groups(&[], GroupBy::Year).is_empty());
}

#[test]
fn apply_authoritative_counts_overrides_present_keys() {
    // 用真实 MediaItem 构造窗口分组：两天的项，窗口只各含一部分。
    let mk = |id: i64, y: i32, m: u32, d: u32| MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: format!("/tmp/{id}.jpg").into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: Some(
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        ),
        file_mtime: NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc(),
        file_size: 1,
        blake3_hash: format!("h{id}"),
        is_favorite: false,
        trashed_at: None,
    };

    let items = vec![mk(1, 2025, 3, 1), mk(2, 2025, 3, 1), mk(3, 2024, 12, 25)];
    let mut sections = group_items(&items, GroupBy::Year);
    let before = sections
        .iter()
        .find(|s| s.key.year == Some(2025))
        .expect("2025 section exists")
        .label
        .clone();

    // 真实计数：2025 远多于窗口里的 2 张
    let mut counts = HashMap::new();
    counts.insert(
        SectionKey {
            year: Some(2025),
            month: None,
            day: None,
        },
        1234,
    );
    apply_authoritative_counts(&mut sections, &counts);

    let y2025 = sections
        .iter()
        .find(|s| s.key.year == Some(2025))
        .expect("2025 section exists");
    assert!(y2025.label.contains("1234"), "label was: {}", y2025.label);
    assert_ne!(
        y2025.label, before,
        "label must change from window count to authoritative count"
    );
}

#[test]
fn apply_authoritative_counts_leaves_unmapped_keys() {
    let mk = |id: i64, y: i32, m: u32, d: u32| MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: format!("/tmp/{id}.jpg").into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: Some(
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        ),
        file_mtime: NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc(),
        file_size: 1,
        blake3_hash: format!("h{id}"),
        is_favorite: false,
        trashed_at: None,
    };
    let items = vec![mk(1, 2025, 3, 1), mk(2, 2025, 3, 1)];
    let mut sections = group_items(&items, GroupBy::Year);
    let before = sections[0].label.clone();
    // 空 map：不应改动任何 label
    apply_authoritative_counts(&mut sections, &HashMap::new());
    assert_eq!(sections[0].label, before);
}

#[test]
fn section_for_global_offset_empty_returns_none() {
    let counts: HashMap<SectionKey, u32> = HashMap::new();
    assert!(section_for_global_offset(&counts, 0).is_none());
}

#[test]
fn section_for_global_offset_maps_boundaries_and_midpoints() {
    // Library ordered newest-first. Counts: 2026-07=3, 2026-06=2, 2025=4 (total 9).
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: Some(7),
            day: None,
        },
        3,
    );
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: Some(6),
            day: None,
        },
        2,
    );
    counts.insert(
        SectionKey {
            year: Some(2025),
            month: None,
            day: None,
        },
        4,
    );

    let jul = SectionKey {
        year: Some(2026),
        month: Some(7),
        day: None,
    };
    let jun = SectionKey {
        year: Some(2026),
        month: Some(6),
        day: None,
    };
    let y2025 = SectionKey {
        year: Some(2025),
        month: None,
        day: None,
    };

    assert_eq!(section_for_global_offset(&counts, 0), Some(jul.clone())); // first
    assert_eq!(section_for_global_offset(&counts, 2), Some(jul.clone())); // last of jul
    assert_eq!(section_for_global_offset(&counts, 3), Some(jun.clone())); // first of jun (boundary → next)
    assert_eq!(section_for_global_offset(&counts, 4), Some(jun.clone())); // last of jun
    assert_eq!(section_for_global_offset(&counts, 5), Some(y2025.clone())); // first of 2025
    assert_eq!(section_for_global_offset(&counts, 8), Some(y2025.clone())); // last overall
}

#[test]
fn section_for_global_offset_clamps_beyond_total_to_oldest() {
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: None,
            day: None,
        },
        2,
    );
    // Oldest (smallest year) when more than one section exists.
    counts.insert(
        SectionKey {
            year: Some(2020),
            month: None,
            day: None,
        },
        1,
    );
    // offset past total (3) → oldest section (2020), not None.
    assert_eq!(
        section_for_global_offset(&counts, 99),
        Some(SectionKey {
            year: Some(2020),
            month: None,
            day: None
        }),
    );
}

#[test]
fn section_for_global_offset_orders_newest_first_regardless_of_input_order() {
    // Insert oldest first; result must still treat newest as offset 0.
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(
        SectionKey {
            year: Some(2020),
            month: None,
            day: None,
        },
        1,
    );
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: None,
            day: None,
        },
        1,
    );
    assert_eq!(
        section_for_global_offset(&counts, 0),
        Some(SectionKey {
            year: Some(2026),
            month: None,
            day: None
        }),
    );
    assert_eq!(
        section_for_global_offset(&counts, 1),
        Some(SectionKey {
            year: Some(2020),
            month: None,
            day: None
        }),
    );
}

#[test]
fn make_label_nocount_has_no_count_and_matches_mode() {
    let year = make_label_nocount(&SectionKey {
        year: Some(2026),
        month: None,
        day: None,
    });
    let month = make_label_nocount(&SectionKey {
        year: Some(2026),
        month: Some(7),
        day: None,
    });
    let day = make_label_nocount(&SectionKey {
        year: Some(2026),
        month: Some(7),
        day: Some(12),
    });
    assert!(
        !year.contains("·"),
        "year label must not include the count separator: {year}"
    );
    assert!(
        !month.contains("·"),
        "month label must not include the count separator: {month}"
    );
    assert!(
        !day.contains("·"),
        "day label must not include the count separator: {day}"
    );
    assert!(year.contains("2026"));
    assert!(month.contains("7"));
    assert!(day.contains("12"));
}

#[test]
fn visible_range_label_is_chronological_and_collapses_one_section() {
    let newer = SectionKey {
        year: Some(2026),
        month: Some(7),
        day: Some(12),
    };
    let older = SectionKey {
        year: Some(2026),
        month: Some(7),
        day: Some(10),
    };

    assert_eq!(
        make_visible_range_label(&newer, &older),
        format!(
            "{} – {}",
            make_label_nocount(&older),
            make_label_nocount(&newer)
        ),
        "the newest-first grid should still present its coverage oldest to newest"
    );
    assert_eq!(
        make_visible_range_label(&newer, &newer),
        make_label_nocount(&newer),
        "a viewport contained by one date section should not repeat the date"
    );
}
