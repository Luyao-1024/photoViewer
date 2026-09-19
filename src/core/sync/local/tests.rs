use super::*;

#[test]
fn scan_hashes_supported_media_and_ignores_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("photos");
    std::fs::create_dir_all(root.join("相册")).unwrap();
    std::fs::write(root.join("相册/a.jpg"), b"photo").unwrap();
    std::fs::write(root.join("notes.txt"), b"no").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("相册/a.jpg"), root.join("link.jpg")).unwrap();

    let entries = scan(&root).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries.contains_key("相册/a.jpg"));
}

#[test]
fn destination_rejects_parent_traversal() {
    let root = Path::new("/tmp/photos");
    assert!(destination(root, "../secret.jpg").is_err());
    assert_eq!(
        destination(root, "album/a.jpg").unwrap(),
        root.join("album/a.jpg")
    );
}
