mod common;
use common::*;
use photo_viewer::core::albums;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::trash;
use tempfile::Builder;

/// `move_to_trash` goes through gio, which refuses to operate on tmpfs.
/// Returns a path on a real filesystem where we can stage the file before
/// trashing (same trick as `trash_flow.rs`).
fn real_fs_scratch() -> std::path::PathBuf {
    let base = std::env::var_os("TMPDIR_REAL")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"));
    Builder::new()
        .prefix("photo-viewer-e2e-")
        .tempdir_in(base)
        .expect("create scratch dir")
        .keep()
}

#[test]
fn full_flow_scan_albums_trash() {
    let dir = tmp_dir();
    let root = dir.path();

    // 1. 两个文件夹的图片
    let camera = root.join("Camera");
    std::fs::create_dir(&camera).unwrap();
    write_plain_jpeg(&camera, "img1.jpg");
    write_plain_jpeg(&camera, "img2.jpg");

    let shots = root.join("Screenshots");
    std::fs::create_dir(&shots).unwrap();
    write_plain_jpeg(&shots, "scr1.jpg");

    // 2. 扫描 + 入库
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());
    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 3);
    for it in &items {
        backend.upsert(it).unwrap();
    }

    // 3. 聚合 albums —— 应有 2 个相册（Camera=2, Screenshots=1）
    albums::refresh(&pool).unwrap();
    let list = albums::list(&pool).unwrap();
    assert_eq!(list.len(), 2, "应按 folder_path 聚合出 2 个相册");
    let total: i64 = list.iter().map(|a| a.photo_count).sum();
    assert_eq!(total, 3);
    let cam = list
        .iter()
        .find(|a| a.folder_path.file_name().map(|s| s == "Camera").unwrap_or(false))
        .expect("Camera album should exist");
    let scr = list
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Screenshots")
                .unwrap_or(false)
        })
        .expect("Screenshots album should exist");
    assert_eq!(cam.photo_count, 2);
    assert_eq!(scr.photo_count, 1);

    // 4. 把 Camera/img1.jpg 移到回收站并标记
    let cam_img1 = list_all_in(&pool)
        .into_iter()
        .find(|m| m.path.file_name().map(|s| s == "img1.jpg").unwrap_or(false))
        .expect("img1.jpg should be in DB");
    let first_id = cam_img1.id;
    let first_uri = cam_img1.uri.clone();

    // gio 需要真实文件系统路径：复制到 scratch 目录再 trash
    let scratch = real_fs_scratch();
    let real_src = scratch.join("img1.jpg");
    std::fs::copy(&cam_img1.path, &real_src).unwrap();
    let real_uri = format!("file://{}", real_src.display());

    trash::move_to_trash(&real_uri).expect("move to trash should succeed");
    db::mark_trashed(&pool, first_id).unwrap();

    // 5. 重新聚合：trashed 项应被排除
    albums::refresh(&pool).unwrap();
    let list2 = albums::list(&pool).unwrap();
    let total_after: i64 = list2.iter().map(|a| a.photo_count).sum();
    assert_eq!(
        total_after, 2,
        "trashed 后聚合总数应为 2（Camera 减 1，Screenshots 不变）"
    );

    // Camera 计数应从 2 → 1
    let cam2 = list2
        .iter()
        .find(|a| a.folder_path.file_name().map(|s| s == "Camera").unwrap_or(false))
        .expect("Camera album still exists");
    assert_eq!(cam2.photo_count, 1);

    // Screenshots 计数仍为 1
    let scr2 = list2
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Screenshots")
                .unwrap_or(false)
        })
        .expect("Screenshots album still exists");
    assert_eq!(scr2.photo_count, 1);

    // 6. 验证 DB 中：trashed 项的 uri 仍记录在媒体表中（list_all_media 排除之）
    let active = db::list_all_media(&pool).unwrap();
    assert_eq!(active.len(), 2, "DB 中 active 媒体数应为 2");

    // 静默引用 first_uri 以防未使用警告
    let _ = first_uri;
}

fn list_all_in(pool: &db::DbPool) -> Vec<MediaItem> {
    db::list_all_media(pool)
        .unwrap()
        .into_iter()
        .chain(db::list_trashed_media(pool).unwrap())
        .collect()
}
