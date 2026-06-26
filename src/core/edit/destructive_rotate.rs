//! 方向属性旋转（不改写像素，但保留 .bak 备份）
use std::path::Path;

use crate::core::error::Result;
use crate::core::orientation;

/// 在原文件位置更新方向属性；若已存在 `{原文件名}.bak` 则跳过备份。
pub fn rotate_in_place(path: &Path, delta_degrees: i32) -> Result<()> {
    let backup = backup_path_for(path);
    if !backup.exists() {
        std::fs::copy(path, &backup)?;
    }
    match delta_degrees.rem_euclid(360) {
        0 | 90 | 180 | 270 => orientation::rotate_orientation_in_place(path, delta_degrees)?,
        _ => {
            return Err(crate::core::error::AppError::Decode(
                "invalid rotation delta".into(),
            ))
        }
    }
    Ok(())
}

fn backup_path_for(path: &Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    std::path::PathBuf::from(s)
}
