//! Year/Month/Day section 计数来自 DB 聚合，而非虚拟分页窗口。
//! 回归 guard：某年实际照片数 > virtual_media_page_size(默认 500) 时，
//! `section_counts` 仍返回真实总数（对应"年视图只显示 500"的 bug）。
mod common;
use chrono::{DateTime, NaiveDate, Utc};
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::repository::MediaRepository;
use photo_viewer::core::section_model::{GroupBy, SectionKey};
use std::path::PathBuf;

fn dt(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
}

fn new_item(
    idx: usize,
    taken_at: Option<DateTime<Utc>>,
    file_mtime: DateTime<Utc>,
) -> NewMediaItem {
    NewMediaItem {
        uri: format!("file:///tmp/section_{idx}.jpg"),
        path: PathBuf::from(format!("/tmp/section_{idx}.jpg")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at,
        file_mtime,
        file_size: 1,
        blake3_hash: format!("h{idx}"),
    }
}

#[test]
fn section_counts_come_from_db_not_window() {
    let dir = tmp_dir();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    // 600 张 2025 年的图：超过默认 virtual_media_page_size(500) —— 直接对应 bug 场景。
    let mut items: Vec<NewMediaItem> = (0..600)
        .map(|i| new_item(i, Some(dt(2025, 3, 1)), dt(2025, 3, 1)))
        .collect();
    // 1 张 2024 年
    items.push(new_item(600, Some(dt(2024, 12, 25)), dt(2024, 12, 25)));
    // 1 张 taken_at=None → 走 file_mtime(2023) 分组（验证 COALESCE）
    items.push(new_item(601, None, dt(2023, 6, 15)));

    let inserted = db::upsert_media_items_batch(&pool, &items).unwrap();
    assert_eq!(inserted.len(), items.len());

    // 1 张 2020 年的图，但已进回收站 → 不应计入
    let trashed = db::upsert_media_items_batch(
        &pool,
        &[new_item(602, Some(dt(2020, 1, 1)), dt(2020, 1, 1))],
    )
    .unwrap();
    db::mark_trashed(&pool, trashed[0].id).unwrap();

    // 1) 按完整日期分组：2025-03-01 = 600，2024-12-25 = 1，2023-06-15 = 1；2020 不计。
    let by_date = db::count_live_media_by_date(&pool).unwrap();
    let find = |y: i32, m: u32, d: u32| {
        by_date
            .iter()
            .find(|&&(yy, mm, dd, _)| yy == y && mm == m && dd == d)
            .map(|&(_, _, _, c)| c)
    };
    assert_eq!(find(2025, 3, 1), Some(600));
    assert_eq!(find(2024, 12, 25), Some(1));
    assert_eq!(find(2023, 6, 15), Some(1));
    assert!(find(2020, 1, 1).is_none(), "trashed row must be excluded");
    assert_eq!(
        by_date.iter().map(|&(_, _, _, c)| c).sum::<u32>(),
        602,
        "total live = 600 + 1 + 1"
    );

    // 2) Year 折叠：2025 = 600（> 窗口 500），2024 = 1，2023 = 1
    let repo = MediaRepository::new(pool.clone());
    let year = repo.section_counts(GroupBy::Year).unwrap();
    assert_eq!(
        year.get(&SectionKey {
            year: Some(2025),
            month: None,
            day: None
        }),
        Some(&600),
        "year count must be the true DB total, not the 500-item window cap"
    );
    assert_eq!(
        year.get(&SectionKey {
            year: Some(2024),
            month: None,
            day: None
        }),
        Some(&1)
    );
    assert_eq!(
        year.get(&SectionKey {
            year: Some(2023),
            month: None,
            day: None
        }),
        Some(&1),
        "taken_at=NULL must fall back to file_mtime (COALESCE) -> 2023"
    );
    assert!(!year.contains_key(&SectionKey {
        year: Some(2020),
        month: None,
        day: None
    }));

    // 3) Month 折叠：(2025,3)=600，(2024,12)=1，(2023,6)=1
    let month = repo.section_counts(GroupBy::Month).unwrap();
    assert_eq!(
        month.get(&SectionKey {
            year: Some(2025),
            month: Some(3),
            day: None
        }),
        Some(&600)
    );
    assert_eq!(month.len(), 3);

    // 4) Day 折叠：三组各 600/1/1
    let day = repo.section_counts(GroupBy::Day).unwrap();
    assert_eq!(
        day.get(&SectionKey {
            year: Some(2025),
            month: Some(3),
            day: Some(1)
        }),
        Some(&600)
    );
    assert_eq!(day.len(), 3);
}
