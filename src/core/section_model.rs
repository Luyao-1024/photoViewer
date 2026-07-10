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
    let mut section_positions: HashMap<SectionKey, usize> = HashMap::new();
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

        match section_positions.get(&key).copied() {
            Some(idx) => sections[idx].items.push(item.clone()),
            None => {
                let idx = sections.len();
                section_positions.insert(key.clone(), idx);
                sections.push(MediaSection {
                    key,
                    label: String::new(),
                    items: vec![item.clone()],
                });
            }
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
mod tests;
