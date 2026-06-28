mod common;
use common::*;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;

#[test]
fn scan_finds_jpeg_png() {
    let dir = tmp_dir();
    let root = dir.path();

    // 创建测试图片：3 张 JPEG + 1 张 PNG + 1 个非图片文件
    for name in &["a.jpg", "b.jpg", "c.jpeg"] {
        write_plain_jpeg(root, name);
    }
    let png_path = root.join("d.png");
    image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(10, 10, |_, _| image::Rgb([255, 0, 0]))
        .save(&png_path)
        .unwrap();
    std::fs::write(root.join("readme.txt"), b"text").unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(
        items.len(),
        4,
        "应识别 4 张图片（JPEG×3 + PNG×1），忽略 .txt"
    );

    // 验证每项的 mime（blake3_hash 已不在扫描时计算，恒为空字符串）
    for item in &items {
        assert!(item.mime_type.starts_with("image/"));
    }
}

#[test]
fn scan_finds_images_and_videos_in_same_directory() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "photo.jpg");
    std::fs::write(root.join("clip.mp4"), b"fake mp4 bytes").unwrap();
    std::fs::write(root.join("notes.txt"), b"text").unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let mut items = backend.scan_dir(root).unwrap();
    items.sort_by(|a, b| a.path.cmp(&b.path));

    assert_eq!(
        items.len(),
        2,
        "scan should include one image and one video"
    );
    assert!(items.iter().any(|item| item.mime_type == "image/jpeg"));
    assert!(items.iter().any(|item| item.mime_type == "video/mp4"));
}

#[test]
fn upsert_inserts_and_updates_by_uri() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "x.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 1);

    let id1 = backend.upsert(&items[0]).unwrap();
    let id2 = backend.upsert(&items[0]).unwrap();
    assert_eq!(id1, id2, "同 URI 应返回相同 id（INSERT OR REPLACE）");
}

#[test]
fn scan_recursive_subdirs() {
    let dir = tmp_dir();
    let root = dir.path();
    let sub = root.join("sub");
    std::fs::create_dir(&sub).unwrap();
    write_plain_jpeg(root, "top.jpg");
    write_plain_jpeg(&sub, "nested.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 2);
}

/// Regression for the startup re-hash cost: on a warm DB the scan must NOT
/// re-read every file's bytes to recompute its blake3 hash. `scan_and_upsert_dir`
/// skips any file whose `(uri, file_mtime, file_size)` already matches a row —
/// the file is unchanged, so its hash/metadata are still valid.
#[test]
fn scan_and_upsert_skips_unchanged_files() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "a.jpg");
    write_plain_jpeg(root, "b.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    // 首次扫描：两张都新增。
    let n1 = backend.scan_and_upsert_dir(root).unwrap();
    assert_eq!(n1, 2, "首次扫描应索引 2 张");
    assert_eq!(db::list_all_media(&pool).unwrap().len(), 2);

    // 第二次扫描：文件未改动 → 全部跳过（不重新哈希/不重新提取）。
    let n2 = backend.scan_and_upsert_dir(root).unwrap();
    assert_eq!(n2, 0, "未改动文件应被跳过，避免重复全文件哈希");
    assert_eq!(
        db::list_all_media(&pool).unwrap().len(),
        2,
        "跳过不应改变行数"
    );

    // 新增一张：仅新的被索引，旧的仍跳过。
    write_plain_jpeg(root, "c.jpg");
    let n3 = backend.scan_and_upsert_dir(root).unwrap();
    assert_eq!(n3, 1, "仅新增的那张应被索引");
    assert_eq!(db::list_all_media(&pool).unwrap().len(), 3);
}
