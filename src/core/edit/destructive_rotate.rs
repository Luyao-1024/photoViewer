//! 破坏性旋转（覆盖原图，但保留 .bak 备份用于撤销）
use std::path::Path;

use crate::core::edit::op::apply_rotation;
use crate::core::edit::Rotation;
use crate::core::error::Result;

/// 在原文件位置旋转；若已存在 .jpg.bak 则跳过备份（避免链式覆盖备份）
pub fn rotate_in_place(path: &Path, delta_degrees: i32) -> Result<()> {
    let backup = path.with_extension("jpg.bak");
    if !backup.exists() {
        std::fs::copy(path, &backup)?;
    }
    let img = image::open(path)?;
    let rot = match delta_degrees.rem_euclid(360) {
        0 => Rotation::None,
        90 => Rotation::R90,
        180 => Rotation::R180,
        270 => Rotation::R270,
        _ => {
            return Err(crate::core::error::AppError::Decode(
                "invalid rotation delta".into(),
            ));
        }
    };
    let rotated = apply_rotation(&img, rot);
    rotated.save_with_format(path, image::ImageFormat::Jpeg)?;
    Ok(())
}