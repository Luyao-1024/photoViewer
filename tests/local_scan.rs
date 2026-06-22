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

    // 验证每项都有 hash 和 mime
    for item in &items {
        assert!(!item.blake3_hash.is_empty());
        assert!(item.mime_type.starts_with("image/"));
    }
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
