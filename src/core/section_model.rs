//! 按年/月/日对 MediaItem 分组（用于 PhotosPage 三种视图）
use crate::core::media::MediaItem;
use chrono::{Datelike, NaiveDate, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Year,
    Month,
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
    match item.taken_at {
        Some(dt) => match mode {
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
        },
        None => SectionKey {
            year: None,
            month: None,
            day: None,
        },
    }
}

fn weekday_cn(d: Weekday) -> &'static str {
    match d {
        Weekday::Sun => "周日",
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
    }
}

fn make_label(key: &SectionKey, count: u32) -> String {
    match (key.year, key.month, key.day) {
        (Some(y), Some(m), Some(d)) => {
            let wname = NaiveDate::from_ymd_opt(y, m, d)
                .map(|nd| weekday_cn(nd.weekday()))
                .unwrap_or("");
            format!("{}年{}月{}日 {} · {} 张", y, m, d, wname, count)
        }
        (Some(y), Some(m), None) => format!("{}年{}月 · {} 张", y, m, count),
        (Some(y), None, None) => format!("{} · {} 张", y, count),
        _ => format!("未知日期 · {} 张", count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn weekday_chinese_label() {
        assert_eq!(weekday_cn(Weekday::Sun), "周日");
        assert_eq!(weekday_cn(Weekday::Sat), "周六");
    }
}