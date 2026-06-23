//! 启动时一次性引导：扫描 + 聚合 albums
//!
//! 验证 `core::bootstrap::scan_and_aggregate` 在临时目录里完成扫描后，
//! `albums::list` 能正确返回按 `folder_path` 聚合的相册（含中文子目录）。
mod common;
use common::*;
use photo_viewer::core::albums;
use photo_viewer::core::bootstrap;
use photo_viewer::core::db;

#[test]
fn scan_and_aggregate_includes_chinese_subfolder_as_album() {
    let dir = tmp_dir();
    let shots = dir.path().join("截图");
    std::fs::create_dir(&shots).unwrap();
    write_plain_png(&shots, "a.png");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        bootstrap::scan_and_aggregate(&pool, &[dir.path().to_path_buf()])
            .await
            .unwrap();
    });

    let albums = albums::list(&pool).unwrap();
    let scr = albums
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "截图")
                .unwrap_or(false)
        })
        .expect("截图 album should exist");
    assert_eq!(scr.photo_count, 1);
}
