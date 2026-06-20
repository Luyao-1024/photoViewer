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

/// 两个不同目录下同名文件同时存在于 trash（gio 在 basename 冲突时把第二个
/// 改名为 `foo.2.jpg`）。验证我们的 `restore_from_trash` 能正确读取
/// trashinfo 找到实际文件名（A 对应 `dup.jpg` 而非 `dup.2.jpg`）、还原 A 的
/// 内容、不会误删 B 的 trashinfo。
///
/// 本测试在 trash 中手动构造两条冲突条目：
/// * `dup.jpg` + `dup.jpg.trashinfo` → 指向 file_a（原 A 路径）
/// * `dup.2.jpg` + `dup.2.jpg.trashinfo` → 指向 file_b（原 B 路径）
///
/// 然后只走 `restore_from_trash(A)`，断言 B 的条目原封不动。
#[test]
fn basename_collision_restore_one_keeps_other() {
    let parent = scratch_dir();
    let dir_a = parent.join("a");
    let dir_b = parent.join("b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let file_a = dir_a.join("dup.jpg");
    let file_b = dir_b.join("dup.jpg");
    std::fs::write(&file_a, b"data-from-A").unwrap();
    std::fs::write(&file_b, b"data-from-B").unwrap();
    let uri_a = format!("file://{}", file_a.display());
    let _uri_b = format!("file://{}", file_b.display());

    // 手动在 trash 中塞两份条目（手工模拟 gio 冲突命名后的状态）
    let home = std::env::var_os("HOME")
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let trash_files = std::path::PathBuf::from(format!("{}/.local/share/Trash/files", home));
    let trash_info = std::path::PathBuf::from(format!("{}/.local/share/Trash/info", home));
    std::fs::create_dir_all(&trash_files).unwrap();
    std::fs::create_dir_all(&trash_info).unwrap();

    let a_files = trash_files.join("dup.jpg");
    let a_info = trash_info.join("dup.jpg.trashinfo");
    let b_files = trash_files.join("dup.2.jpg");
    let b_info = trash_info.join("dup.2.jpg.trashinfo");
    std::fs::write(&a_files, b"data-from-A").unwrap();
    std::fs::write(&b_files, b"data-from-B").unwrap();
    std::fs::write(
        &a_info,
        format!(
            "[Trash Info]\nPath={}\nDeletionDate=2026-06-20T00:00:00\n",
            file_a.display()
        ),
    )
    .unwrap();
    std::fs::write(
        &b_info,
        format!(
            "[Trash Info]\nPath={}\nDeletionDate=2026-06-20T00:00:00\n",
            file_b.display()
        ),
    )
    .unwrap();

    // 现在 file_a 仍在 dir_a 中 —— restore_from_trash 应把 trash 里的 A
    // 移回 file_a，并把对应的 dup.jpg.trashinfo 删掉。
    trash::restore_from_trash(&uri_a).unwrap();

    // A 应还原到原位置，内容必须是 A 的内容（不是 B 的）
    assert!(file_a.exists(), "A should be restored to original path");
    let restored_a = std::fs::read(&file_a).unwrap();
    assert_eq!(
        restored_a, b"data-from-A",
        "A contents must be the correct one (not B's)"
    );

    // A 对应的 dup.jpg.trashinfo 应被清理
    assert!(
        !a_info.exists(),
        "A's trashinfo should be removed (got {})",
        a_info.display()
    );

    // B 的两条条目必须原封不动（不能被 A 的 restore 误删）
    assert!(
        b_files.exists(),
        "B's files entry dup.2.jpg should still be in trash"
    );
    assert!(
        b_info.exists(),
        "B's trashinfo dup.2.jpg.trashinfo should still be in trash (got exists={})",
        b_info.exists()
    );

    // 清理
    let _ = std::fs::remove_file(&b_files);
    let _ = std::fs::remove_file(&b_info);
    let _ = std::fs::remove_file(&file_a);
    let _ = std::fs::remove_dir_all(&parent);
}