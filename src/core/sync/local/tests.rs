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

#[test]
fn publishing_new_file_keeps_recovery_artifact_until_commit() {
    let temp = tempfile::tempdir().unwrap();
    let staged = temp.path().join("staging/download");
    let target = temp.path().join("photos/album/photo.jpg");
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, b"cloud photo").unwrap();

    atomic_publish_new(&staged, &target).unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"cloud photo");
    assert_eq!(std::fs::read(&staged).unwrap(), b"cloud photo");
    assert!(std::fs::read_dir(target.parent().unwrap())
        .unwrap()
        .all(|entry| entry.unwrap().path() == target));
}

#[test]
fn replacing_file_keeps_backup_and_recovery_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let staged = temp.path().join("staging/download");
    let target = temp.path().join("photos/album/photo.jpg");
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&staged, b"new version").unwrap();
    std::fs::write(&target, b"old version").unwrap();

    let backup = atomic_publish_replace(&staged, &target, "operation-1").unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new version");
    assert_eq!(std::fs::read(&backup).unwrap(), b"old version");
    assert_eq!(std::fs::read(&staged).unwrap(), b"new version");
}
