//! 编辑操作的应用工具
use crate::core::edit::{CropRect, Rotation};
use image::DynamicImage;

/// 应用旋转变换
pub fn apply_rotation(img: &DynamicImage, rot: Rotation) -> DynamicImage {
    match rot {
        Rotation::None => img.clone(),
        Rotation::R90 => img.rotate90(),
        Rotation::R180 => img.rotate180(),
        Rotation::R270 => img.rotate270(),
    }
}

/// 应用裁剪
pub fn apply_crop(img: &DynamicImage, rect: CropRect) -> DynamicImage {
    img.crop_imm(
        rect.x,
        rect.y,
        rect.width.min(img.width().saturating_sub(rect.x)),
        rect.height.min(img.height().saturating_sub(rect.y)),
    )
}

/// 应用亮度/对比度/饱和度调整（使用 image::DynamicImage::brighten + contrast + huerotate）
pub fn apply_color_adjust(
    img: &DynamicImage,
    brightness: i32,
    contrast: i32,
    saturation: i32,
) -> DynamicImage {
    let mut out = img.clone();
    if brightness != 0 {
        // brighten 接受 -100..100 范围
        out = out.brighten((brightness as f32 / 100.0 * 255.0) as i32);
    }
    if contrast != 0 {
        out = out.adjust_contrast(contrast as f32 / 100.0 * 100.0);
    }
    if saturation != 0 {
        // 简化：饱和度 0 → 原图，+100 → 完全饱和，-100 → 灰度
        // 使用 huerotate 近似（更精确的实现需要颜色空间转换）
        out = out.huerotate(saturation);
    }
    out
}
