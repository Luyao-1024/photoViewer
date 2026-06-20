use photo_viewer::core::cache;
use tempfile::tempdir;

#[test]
fn enforce_limit_deletes_oldest_until_under() {
    let dir = tempdir().unwrap();
    let thumbs = dir.path().join("thumbnails");
    std::fs::create_dir_all(&thumbs).unwrap();

    // 创建 3 个不同 mtime 的文件
    let f1 = thumbs.join("a.jpg");
    let f2 = thumbs.join("b.jpg");
    let f3 = thumbs.join("c.jpg");
    std::fs::write(&f1, vec![0u8; 100]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&f2, vec![0u8; 200]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&f3, vec![0u8; 300]).unwrap();

    // 上限 350 字节 → 应删除最旧的 f1 (100) → 总 500? 不，应删除 f1+f2 留下 f3 (300)
    let deleted = cache::enforce_size_limit(&thumbs, 350).unwrap();
    assert_eq!(deleted, 2);
    assert!(!f1.exists());
    assert!(!f2.exists());
    assert!(f3.exists());
}
