mod common;
use common::*;
use photo_viewer::core::album_ops::{add_to_album, AlbumOpMode};
use photo_viewer::core::albums;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::trash;
use tempfile::{Builder, TempDir};

/// `move_to_trash` goes through gio, which refuses to operate on tmpfs.
/// Returns a `TempDir` on a real filesystem; the caller holds it so the
/// directory is cleaned up automatically when the guard is dropped.
fn real_fs_scratch() -> TempDir {
    let base = std::env::var_os("TMPDIR_REAL")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"));
    Builder::new()
        .prefix("photo-viewer-e2e-")
        .tempdir_in(base)
        .expect("create scratch dir")
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
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Camera")
                .unwrap_or(false)
        })
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
    let real_src = scratch.path().join("img1.jpg");
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
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Camera")
                .unwrap_or(false)
        })
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

/// End-to-end: scan → move via album_ops → trash → verify album counts
/// are still consistent. Specifically:
///   * Camera starts with 2 photos
///   * Move img1.jpg to Screenshots (so Camera=1, Screenshots=2)
///   * Trash img2.jpg in Camera (so Camera=0, Screenshots=2)
///   * Final albums list: Camera=0, Screenshots=2 (sum=2)
#[test]
fn end_to_end_move_then_trash_then_album_count_consistent() {
    let dir = tmp_dir();
    let root = dir.path();

    // 1. Camera (2 photos) + Screenshots (1 photo)
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
    albums::refresh(&pool).unwrap();

    let initial = albums::list(&pool).unwrap();
    let cam_initial = initial
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Camera")
                .unwrap_or(false)
        })
        .map(|a| a.photo_count)
        .unwrap();
    let scr_initial = initial
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Screenshots")
                .unwrap_or(false)
        })
        .map(|a| a.photo_count)
        .unwrap();
    assert_eq!(cam_initial, 2);
    assert_eq!(scr_initial, 1);

    // 3. Move img1.jpg from Camera → Screenshots
    let img1 = list_all_in(&pool)
        .into_iter()
        .find(|m| {
            m.folder_path
                .file_name()
                .map(|s| s == "Camera")
                .unwrap_or(false)
                && m.path.file_name().map(|s| s == "img1.jpg").unwrap_or(false)
        })
        .expect("Camera/img1.jpg should exist");
    add_to_album(&pool, &[img1.id], &shots, AlbumOpMode::Move).unwrap();

    let after_move = albums::list(&pool).unwrap();
    let cam_after_move = after_move
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Camera")
                .unwrap_or(false)
        })
        .map(|a| a.photo_count)
        .unwrap();
    let scr_after_move = after_move
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Screenshots")
                .unwrap_or(false)
        })
        .map(|a| a.photo_count)
        .unwrap();
    assert_eq!(cam_after_move, 1, "Camera 减 1");
    assert_eq!(scr_after_move, 2, "Screenshots 加 1");

    // 4. Trash the remaining Camera photo (img2.jpg). The move already
    //    removed img1.jpg from Camera, so this targets the only one left.
    let img2 = list_all_in(&pool)
        .into_iter()
        .find(|m| {
            m.folder_path
                .file_name()
                .map(|s| s == "Camera")
                .unwrap_or(false)
                && m.path.file_name().map(|s| s == "img2.jpg").unwrap_or(false)
        })
        .expect("Camera/img2.jpg should exist (move didn't touch it)");

    // gio trashing needs a real fs path; copy to scratch and trash
    let scratch = real_fs_scratch();
    let real_src = scratch.path().join("img2.jpg");
    std::fs::copy(&img2.path, &real_src).unwrap();
    let real_uri = format!("file://{}", real_src.display());
    trash::move_to_trash(&real_uri).expect("move to trash should succeed");
    db::mark_trashed(&pool, img2.id).unwrap();

    // 5. Final album state
    albums::refresh(&pool).unwrap();
    let final_list = albums::list(&pool).unwrap();
    let cam_final = final_list
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Camera")
                .unwrap_or(false)
        })
        .map(|a| a.photo_count)
        .unwrap_or(0);
    let scr_final = final_list
        .iter()
        .find(|a| {
            a.folder_path
                .file_name()
                .map(|s| s == "Screenshots")
                .unwrap_or(false)
        })
        .map(|a| a.photo_count)
        .unwrap_or(0);
    assert_eq!(cam_final, 0, "Camera 全部 move+trash 后应为 0");
    assert_eq!(scr_final, 2, "Screenshots 保留 2 张(原 1 + 移入 1)");

    // total: 2 (move 不变,trash 仅影响计数)
    let total_final: i64 = final_list.iter().map(|a| a.photo_count).sum();
    assert_eq!(total_final, 2);

    // Camera 还存在但 photo_count = 0(因为 folder_path 已经被 move 后的
    // 媒体行所引用,而 trashed 那一行原本在 Camera)。refresh 会清空
    // albums 然后重算,空 folder 不进表 — 所以 Camera 实际可能不在 final
    // list 中。如果还在则 photo_count=0,断言其一即可。
    let _ = cam_final;
}
