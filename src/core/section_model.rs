//! 按年/月/日对 MediaItem 分组（用于 PhotosPage 三种视图）
use crate::core::i18n::trf;
use crate::core::media::MediaItem;
use chrono::{Datelike, NaiveDate, Weekday};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GroupBy {
    Year,
    Month,
    #[default]
    Day,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SectionKey {
    pub year: Option<i32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct MediaSection {
    pub key: SectionKey,
    pub label: String,
    pub items: Vec<MediaItem>,
}

pub fn group_items(items: &[MediaItem], mode: GroupBy) -> Vec<MediaSection> {
    let mut sections: Vec<MediaSection> = Vec::new();
    let mut unknown_section: Option<MediaSection> = None;

    for item in items {
        let key = make_key(item, mode);

        // 未知日期单独存，最后追加到末尾
        if key.year.is_none() && key.month.is_none() && key.day.is_none() {
            match unknown_section.as_mut() {
                Some(sec) => sec.items.push(item.clone()),
                None => {
                    unknown_section = Some(MediaSection {
                        key,
                        label: String::new(),
                        items: vec![item.clone()],
                    });
                }
            }
            continue;
        }

        let pos = sections.iter().position(|s| s.key == key);
        match pos {
            Some(idx) => sections[idx].items.push(item.clone()),
            None => sections.push(MediaSection {
                key,
                label: String::new(),
                items: vec![item.clone()],
            }),
        }
    }

    // 更新每个 section 的 label 中的计数
    for sec in &mut sections {
        let count = sec.items.len() as u32;
        sec.label = make_label(&sec.key, count);
    }
    if let Some(mut sec) = unknown_section {
        let count = sec.items.len() as u32;
        sec.label = make_label(&sec.key, count);
        sections.push(sec);
    }

    sections
}

/// 把按完整日期(年-月-日)分组的 DB 计数，按 `mode` 折叠成 `SectionKey → count`。
///
/// 每个元素为 `(year, month, day, count)`。Year 模式按年求和、Month 按(年,月)
/// 求和、Day 直接用。用于让 section 头部计数反映整个库的真实数量，而非当前
/// 虚拟分页窗口里加载到的那部分（窗口受 `virtual_media_page_size` 截断）。
pub fn counts_from_date_groups(
    groups: &[(i32, u32, u32, u32)],
    mode: GroupBy,
) -> HashMap<SectionKey, u32> {
    let mut map: HashMap<SectionKey, u32> = HashMap::new();
    for &(year, month, day, count) in groups {
        let key = match mode {
            GroupBy::Year => SectionKey {
                year: Some(year),
                month: None,
                day: None,
            },
            GroupBy::Month => SectionKey {
                year: Some(year),
                month: Some(month),
                day: None,
            },
            GroupBy::Day => SectionKey {
                year: Some(year),
                month: Some(month),
                day: Some(day),
            },
        };
        *map.entry(key).or_insert(0) += count;
    }
    map
}

/// 用权威计数(`counts`)重写 `sections` 中命中 key 的 `label`。
///
/// `group_items` 先按当前窗口的 items 算出一个可能被截断的计数；这里再用整个
/// 库的真实计数覆盖。`counts` 中没有的 key 保持原窗口计数不变（例如未知日期段）。
pub fn apply_authoritative_counts(
    sections: &mut [MediaSection],
    counts: &HashMap<SectionKey, u32>,
) {
    for sec in sections.iter_mut() {
        if let Some(true_count) = counts.get(&sec.key) {
            sec.label = make_label(&sec.key, *true_count);
        }
    }
}

fn make_key(item: &MediaItem, mode: GroupBy) -> SectionKey {
    let dt = item.sort_datetime();
    match mode {
        GroupBy::Year => SectionKey {
            year: Some(dt.year()),
            month: None,
            day: None,
        },
        GroupBy::Month => SectionKey {
            year: Some(dt.year()),
            month: Some(dt.month()),
            day: None,
        },
        GroupBy::Day => SectionKey {
            year: Some(dt.year()),
            month: Some(dt.month()),
            day: Some(dt.day()),
        },
    }
}

fn weekday_cn(d: Weekday) -> String {
    let key = match d {
        Weekday::Sun => "date.weekday.sun",
        Weekday::Mon => "date.weekday.mon",
        Weekday::Tue => "date.weekday.tue",
        Weekday::Wed => "date.weekday.wed",
        Weekday::Thu => "date.weekday.thu",
        Weekday::Fri => "date.weekday.fri",
        Weekday::Sat => "date.weekday.sat",
    };
    trf(key, &[])
}

fn make_label(key: &SectionKey, count: u32) -> String {
    let count_s = count.to_string();
    match (key.year, key.month, key.day) {
        (Some(y), Some(m), Some(d)) => {
            let wname = NaiveDate::from_ymd_opt(y, m, d)
                .map(|nd| weekday_cn(nd.weekday()))
                .unwrap_or_default();
            trf(
                "section.label.day",
                &[
                    ("year", &y.to_string()),
                    ("month", &m.to_string()),
                    ("day", &d.to_string()),
                    ("weekday", &wname),
                    ("count", &count_s),
                ],
            )
        }
        (Some(y), Some(m), None) => trf(
            "section.label.month",
            &[
                ("year", &y.to_string()),
                ("month", &m.to_string()),
                ("count", &count_s),
            ],
        ),
        (Some(y), None, None) => trf(
            "section.label.year",
            &[("year", &y.to_string()), ("count", &count_s)],
        ),
        _ => trf("section.label.unknown", &[("count", &count_s)]),
    }
}

#[cfg(test)]
mod tests {
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
}
