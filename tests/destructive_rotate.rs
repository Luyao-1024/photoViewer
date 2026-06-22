//! Destructive rotate + undo tests.
//!
//! Verifies that `rotate_in_place` overwrites the original file while
//! preserving a `.jpg.bak` snapshot of the pre-rotation bytes, and that
//! applying the inverse rotation restores the file to its pre-first-rotation
//! contents.
mod common;

use common::*;
use photo_viewer::core::edit::destructive_rotate;
use tempfile::tempdir;

#[test]
fn rotate_creates_backup_and_unrotate_restores() {
    let dir = tempdir().unwrap();
    let src = write_plain_jpeg(dir.path(), "rot.jpg");
    let original_bytes = std::fs::read(&src).unwrap();

    destructive_rotate::rotate_in_place(&src, 90).unwrap();

    let rotated_bytes = std::fs::read(&src).unwrap();
    let backup_bytes = std::fs::read(src.with_extension("jpg.bak")).unwrap();
    // 备份 = 旋转前原图（original_bytes），新文件 = 旋转后内容
    assert_eq!(backup_bytes, original_bytes); // backup = pre-rotation
    assert_ne!(rotated_bytes, original_bytes); // file changed

    // 反向旋转还原
    destructive_rotate::rotate_in_place(&src, -90).unwrap();
    let restored_bytes = std::fs::read(&src).unwrap();
    assert_eq!(restored_bytes, backup_bytes); // restored to pre-first-rotation
}
