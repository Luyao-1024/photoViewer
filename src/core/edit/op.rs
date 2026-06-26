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

/// 应用亮度/对比度/饱和度调整
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
        out = adjust_saturation(&out, saturation);
    }
    out
}

fn adjust_saturation(img: &DynamicImage, saturation: i32) -> DynamicImage {
    let factor = 1.0 + (saturation.clamp(-100, 100) as f32 / 100.0);
    let mut rgba = img.to_rgba8();

    for pixel in rgba.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let luma = 0.299 * r + 0.587 * g + 0.114 * b;

        pixel.0 = [
            scale_channel_from_luma(r, luma, factor),
            scale_channel_from_luma(g, luma, factor),
            scale_channel_from_luma(b, luma, factor),
            a,
        ];
    }

    DynamicImage::ImageRgba8(rgba)
}

fn scale_channel_from_luma(channel: f32, luma: f32, factor: f32) -> u8 {
    (luma + (channel - luma) * factor).round().clamp(0.0, 255.0) as u8
}
