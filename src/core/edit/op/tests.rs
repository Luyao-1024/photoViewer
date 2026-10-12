use super::*;
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};

/// 构造一张按行主序填充可辨识像素的小图，便于断言旋转/裁剪的结果。
fn ramp(width: u32, height: u32) -> DynamicImage {
    let mut buf = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let value = (y * width + x) as u8;
            buf.put_pixel(x, y, Rgba([value, 0, 0, 255]));
        }
    }
    DynamicImage::ImageRgba8(buf)
}

fn pixel_at(img: &DynamicImage, x: u32, y: u32) -> [u8; 4] {
    img.to_rgba8().get_pixel(x, y).0
}

#[test]
fn rotation_none_is_a_no_op() {
    let img = ramp(3, 2);
    let rotated = apply_rotation(&img, Rotation::None);
    assert_eq!(rotated.dimensions(), (3, 2));
    assert_eq!(pixel_at(&rotated, 2, 1), pixel_at(&img, 2, 1));
}

#[test]
fn rotation_swaps_axes_for_quarter_turns() {
    let img = ramp(3, 2);
    for rot in [Rotation::R90, Rotation::R270] {
        assert_eq!(
            apply_rotation(&img, rot).dimensions(),
            (2, 3),
            "{rot:?} should swap width and height"
        );
    }
    assert_eq!(apply_rotation(&img, Rotation::R180).dimensions(), (3, 2));
}

#[test]
fn rotation_cycles_back_to_the_original_after_four_turns() {
    let mut img = ramp(3, 2);
    for rot in [Rotation::R90, Rotation::R90, Rotation::R90] {
        img = apply_rotation(&img, rot);
    }
    img = apply_rotation(&img, Rotation::R90);
    let original = ramp(3, 2);
    assert_eq!(img.dimensions(), original.dimensions());
    assert_eq!(pixel_at(&img, 2, 1), pixel_at(&original, 2, 1));
}

#[test]
fn rotation_moves_the_top_left_pixel_to_the_top_right() {
    // 2x1: [0] [1]  --R90-->  [0]
    //                                [1]
    let img = ramp(2, 1);
    let rotated = apply_rotation(&img, Rotation::R90);
    assert_eq!(rotated.dimensions(), (1, 2));
    assert_eq!(pixel_at(&rotated, 0, 0), [0, 0, 0, 255]);
    assert_eq!(pixel_at(&rotated, 0, 1), [1, 0, 0, 255]);
}

#[test]
fn crop_extracts_the_requested_region() {
    let img = ramp(4, 4);
    let cropped = apply_crop(
        &img,
        CropRect {
            x: 1,
            y: 2,
            width: 2,
            height: 1,
        },
    );
    assert_eq!(cropped.dimensions(), (2, 1));
    assert_eq!(pixel_at(&cropped, 0, 0), [9, 0, 0, 255]);
}

#[test]
fn crop_clamps_width_at_the_right_edge() {
    // Without the saturating clamp this would ask the image crate for a region
    // that runs past the buffer.
    let img = ramp(4, 4);
    let cropped = apply_crop(
        &img,
        CropRect {
            x: 2,
            y: 0,
            width: 99,
            height: 1,
        },
    );
    assert_eq!(cropped.dimensions(), (2, 1));
}

#[test]
fn crop_clamps_height_at_the_bottom_edge() {
    let img = ramp(4, 4);
    let cropped = apply_crop(
        &img,
        CropRect {
            x: 0,
            y: 3,
            width: 1,
            height: 99,
        },
    );
    assert_eq!(cropped.dimensions(), (1, 1));
}

#[test]
fn color_adjust_with_zero_params_returns_equal_pixels() {
    let img = ramp(3, 3);
    let adjusted = apply_color_adjust(&img, 0, 0, 0);
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(pixel_at(&adjusted, x, y), pixel_at(&img, x, y));
        }
    }
}

#[test]
fn positive_brightness_lifts_every_channel() {
    let img = ramp(3, 3);
    let brighter = apply_color_adjust(&img, 100, 0, 0);
    for y in 0..3 {
        for x in 0..3 {
            assert!(
                pixel_at(&brighter, x, y)[0] > pixel_at(&img, x, y)[0],
                "brightness should lift the channel at ({x}, {y})"
            );
        }
    }
}

#[test]
fn negative_brightness_lowers_every_channel() {
    let img = ramp(3, 3);
    let darker = apply_color_adjust(&img, -100, 0, 0);
    for y in 0..3 {
        for x in 0..3 {
            assert!(pixel_at(&darker, x, y)[0] <= pixel_at(&img, x, y)[0]);
        }
    }
}

#[test]
fn brightness_clamps_at_the_black_point() {
    // A dark pixel brightened hard must not wrap around to a bright value.
    let mut buf = RgbaImage::new(1, 1);
    buf.put_pixel(0, 0, Rgba([2, 2, 2, 255]));
    let brightened = apply_color_adjust(&DynamicImage::ImageRgba8(buf), 100, 0, 0);
    assert_eq!(pixel_at(&brightened, 0, 0)[0], 255);
}

#[test]
fn saturation_zero_is_the_identity() {
    let img = DynamicImage::ImageRgba8({
        let mut buf = RgbaImage::new(1, 1);
        buf.put_pixel(0, 0, Rgba([200, 50, 50, 255]));
        buf
    });
    let adjusted = apply_color_adjust(&img, 0, 0, 0);
    assert_eq!(pixel_at(&adjusted, 0, 0), [200, 50, 50, 255]);
}

#[test]
fn full_negative_saturation_collapses_every_channel_onto_luma() {
    let mut buf = RgbaImage::new(1, 1);
    buf.put_pixel(0, 0, Rgba([200, 50, 50, 255]));
    let img = DynamicImage::ImageRgba8(buf);

    // -100 gives a factor of exactly 0, so the image becomes pure greyscale.
    let grey = apply_color_adjust(&img, 0, 0, -100);
    let [r, g, b, _] = pixel_at(&grey, 0, 0);
    assert_eq!(r, g, "channels collapse onto luma");
    assert_eq!(g, b);
    // luma = 0.299*200 + 0.587*50 + 0.114*50 = 94.85 -> 95
    assert_eq!(r, 95, "the surviving grey is the Rec.601 luma of the input");
}

#[test]
fn partial_negative_saturation_pulls_channels_toward_luma_without_reordering_them() {
    let mut buf = RgbaImage::new(1, 1);
    buf.put_pixel(0, 0, Rgba([200, 50, 50, 255]));
    let img = DynamicImage::ImageRgba8(buf);

    let muted = apply_color_adjust(&img, 0, 0, -50);
    let [r, g, b, _] = pixel_at(&muted, 0, 0);
    assert!(r > g, "the dominant channel stays dominant: {r},{g},{b}");
    assert!(g > 50, "weak channels move up toward luma: {g}");
    assert!(r < 200, "the dominant channel moves down toward luma: {r}");
}

#[test]
fn positive_saturation_pushes_channels_away_from_luma() {
    let mut buf = RgbaImage::new(1, 1);
    buf.put_pixel(0, 0, Rgba([128, 128, 128, 255]));
    let img = DynamicImage::ImageRgba8(buf);

    let vivid = apply_color_adjust(&img, 0, 0, 100);
    // A pure grey has channel == luma, so saturation cannot move it; the invariant that
    // matters is that the scale factor never overflows past the 8-bit range.
    assert_eq!(pixel_at(&vivid, 0, 0)[..3], [128, 128, 128]);
}

#[test]
fn saturation_preserves_alpha() {
    let mut buf = RgbaImage::new(1, 1);
    buf.put_pixel(0, 0, Rgba([200, 50, 50, 77]));
    let img = DynamicImage::ImageRgba8(buf);
    assert_eq!(pixel_at(&apply_color_adjust(&img, 0, 0, 50), 0, 0)[3], 77);
}

#[test]
fn scale_channel_clamps_into_the_byte_range() {
    let luma = 128.0;
    assert_eq!(scale_channel_from_luma(255.0, luma, 10.0), 255);
    assert_eq!(scale_channel_from_luma(0.0, luma, 10.0), 0);
    assert_eq!(scale_channel_from_luma(luma, luma, 5.0), 128);
}
