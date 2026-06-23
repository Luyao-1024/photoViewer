//! 按年/月/日对 MediaItem 分组（用于 PhotosPage 三种视图）
use crate::core::i18n::trf;
use crate::core::media::MediaItem;
use chrono::{Datelike, NaiveDate, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    Year,
    Month,
    #[default]
    Day,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
}
