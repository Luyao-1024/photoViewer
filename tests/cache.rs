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

#[cfg(unix)]
#[test]
fn cleanup_report_does_not_count_failed_deletions_as_reclaimed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let thumbs = dir.path().join("thumbnails");
    std::fs::create_dir_all(&thumbs).unwrap();
    let file = thumbs.join("locked.jpg");
    std::fs::write(&file, vec![0_u8; 128]).unwrap();

    // Directory write permission controls unlink on Unix. This assertion is
    // skipped for privileged runners, where root may still unlink the file.
    let mut permissions = std::fs::metadata(&thumbs).unwrap().permissions();
    permissions.set_mode(0o500);
    std::fs::set_permissions(&thumbs, permissions).unwrap();
    let report = cache::enforce_size_limit_report(&thumbs, 0).unwrap();
    let mut restore = std::fs::metadata(&thumbs).unwrap().permissions();
    restore.set_mode(0o700);
    std::fs::set_permissions(&thumbs, restore).unwrap();

    if file.exists() {
        assert_eq!(report.deleted_files, 0);
        assert_eq!(report.deleted_bytes, 0);
        assert_eq!(report.failed_files, 1);
        assert_eq!(report.remaining_bytes, 128);
    }
}
