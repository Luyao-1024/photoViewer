use gtk::gio::prelude::*;
use gtk4 as gtk;
use photo_viewer::core::trash;
use tempfile::Builder;

/// 选取 gio 可支持的真实文件系统路径（拒绝 tmpfs）。
fn scratch_dir() -> std::path::PathBuf {
    let base = std::env::var_os("TMPDIR_REAL")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"));
    let tmp = Builder::new()
        .prefix("photo-viewer-trash-")
        .tempdir_in(base)
        .expect("create scratch dir");
    let path = tmp.keep();
    // tmpdir dropped; we manually clean up at end of each test.
    path
}

#[test]
fn move_and_restore_file() {
    let dir = scratch_dir();
    let src = dir.join("test.jpg");
    std::fs::write(&src, b"fake jpeg data").unwrap();
    let uri = format!("file://{}", src.display());

    // 移到回收站
    trash::move_to_trash(&uri).unwrap();

    // 原位置应不存在
    assert!(!src.exists());

    // 还原
    trash::restore_from_trash(&uri).unwrap();
    assert!(src.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn permanent_delete() {
    let dir = scratch_dir();
    let src = dir.join("perm.jpg");
    std::fs::write(&src, b"x").unwrap();
    let uri = format!("file://{}", src.display());

    trash::move_to_trash(&uri).unwrap();
    // 原位置应不存在（已 trash）
    assert!(!src.exists());

    trash::delete_permanently(&uri).unwrap();

    // 文件在 trash:/// 中也应不存在
    let trash_child = gtk::gio::File::for_uri("trash:///").child("perm.jpg");
    assert!(!trash_child.query_exists(gtk::gio::Cancellable::NONE));

    let _ = std::fs::remove_dir_all(&dir);
}