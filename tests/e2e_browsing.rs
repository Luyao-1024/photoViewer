//! 端到端：扫描测试目录 + 加载到 GListStore + 分组验证
mod common;
use chrono::NaiveDate;
use common::*;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;
use photo_viewer::core::section_model::{group_items, GroupBy};

#[test]
fn full_flow_scan_and_group() {
    let dir = tmp_dir();
    let root = dir.path();

    // 准备测试数据：3 个日期 / 2 个月份 / 2 个年份
    // 用 EXIF DateTimeOriginal 标记每张图的拍摄时间。
    write_jpeg_with_exif(
        root,
        "a.jpg",
        NaiveDate::from_ymd_opt(2025, 3, 1)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap(),
    );
    write_jpeg_with_exif(
        root,
        "b.jpg",
        NaiveDate::from_ymd_opt(2025, 3, 1)
            .unwrap()
            .and_hms_opt(11, 0, 0)
            .unwrap(),
    );
    write_jpeg_with_exif(
        root,
        "c.jpg",
        NaiveDate::from_ymd_opt(2025, 3, 15)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap(),
    );
    write_jpeg_with_exif(
        root,
        "d.jpg",
        NaiveDate::from_ymd_opt(2024, 12, 25)
            .unwrap()
            .and_hms_opt(13, 0, 0)
            .unwrap(),
    );

    // 1. 扫描
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());
    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 4);

    // 2. upsert
    for item in &items {
        backend.upsert(item).unwrap();
    }

    // 3. 加载
    let loaded = db::list_all_media(&pool).unwrap();
    assert_eq!(loaded.len(), 4);

    // 4. 按日分组
    let sections = group_items(&loaded, GroupBy::Day);
    assert_eq!(sections.len(), 3, "应有 3 个不同日期");

    // 5. 按月分组
    let sections = group_items(&loaded, GroupBy::Month);
    assert_eq!(sections.len(), 2, "应有 2 个不同月份");

    // 6. 按年分组
    let sections = group_items(&loaded, GroupBy::Year);
    assert_eq!(sections.len(), 2, "应有 2 个不同年份");
}
