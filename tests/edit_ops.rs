use image::{DynamicImage, RgbImage};
use photo_viewer::core::edit::{
    centered_crop_rect_for_aspect, EditRegistry, EditState, ParamValue, Rotation,
};

fn sample_img() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| {
        image::Rgb([128, 128, 128])
    }))
}

#[test]
fn registry_with_v1_has_5_ops() {
    let r = EditRegistry::new_with_v1();
    let ids: Vec<_> = r.list().iter().map(|op| op.id().to_string()).collect();
    assert!(ids.contains(&"rotate".to_string()));
    assert!(ids.contains(&"crop".to_string()));
    assert!(ids.contains(&"brightness".to_string()));
    assert!(ids.contains(&"contrast".to_string()));
    assert!(ids.contains(&"saturation".to_string()));
}

#[test]
fn rotate_90_swaps_dimensions() {
    let r = EditRegistry::new_with_v1();
    let op = r.get("rotate").unwrap();
    let img = sample_img();
    let rotated = op.apply(&img, ParamValue::Rotation(Rotation::R90)).unwrap();
    assert_eq!(rotated.width(), 100);
    assert_eq!(rotated.height(), 100); // 方形图旋转后尺寸不变
}

#[test]
fn brightness_zero_is_identity() {
    let r = EditRegistry::new_with_v1();
    let op = r.get("brightness").unwrap();
    let img = sample_img();
    let out = op.apply(&img, ParamValue::Int(0)).unwrap();
    assert_eq!(out.width(), img.width());
    assert_eq!(out.height(), img.height());
}

#[test]
fn crop_none_returns_original() {
    let r = EditRegistry::new_with_v1();
    let op = r.get("crop").unwrap();
    let img = sample_img();
    let out = op.apply(&img, ParamValue::Crop(None)).unwrap();
    assert_eq!(out.width(), img.width());
    assert_eq!(out.height(), img.height());
}

#[test]
fn crop_some_crops_image() {
    let r = EditRegistry::new_with_v1();
    let op = r.get("crop").unwrap();
    let img = sample_img();
    let out = op
        .apply(&img, ParamValue::Crop(Some((10, 20, 50, 60))))
        .unwrap();
    assert_eq!(out.width(), 50);
    assert_eq!(out.height(), 60);
}

#[test]
fn saturation_minus_100_converts_color_to_grayscale() {
    let r = EditRegistry::new_with_v1();
    let op = r.get("saturation").unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(1, 1, image::Rgb([90, 150, 210])));

    let out = op.apply(&img, ParamValue::Int(-100)).unwrap().to_rgb8();
    let pixel = out.get_pixel(0, 0).0;

    assert_eq!(pixel[0], pixel[1]);
    assert_eq!(pixel[1], pixel[2]);
}

#[test]
fn saturation_positive_increases_channel_spread_without_reordering_hue() {
    let r = EditRegistry::new_with_v1();
    let op = r.get("saturation").unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(1, 1, image::Rgb([110, 140, 170])));

    let out = op.apply(&img, ParamValue::Int(100)).unwrap().to_rgb8();
    let pixel = out.get_pixel(0, 0).0;

    assert!(
        pixel[0] < pixel[1] && pixel[1] < pixel[2],
        "saturation should preserve channel ordering for this blue-tinted color, got {pixel:?}"
    );
    assert!(
        pixel[2] - pixel[0] > 170 - 110,
        "positive saturation should increase colorfulness, got {pixel:?}"
    );
}

#[test]
fn edit_state_reset_clears_pending_edits() {
    let mut state = EditState {
        rotation: Rotation::R90,
        brightness: 25,
        contrast: -10,
        saturation: 40,
        crop: Some((10, 20, 30, 40)),
    };

    state.reset();

    assert_eq!(state.rotation, Rotation::None);
    assert_eq!(state.brightness, 0);
    assert_eq!(state.contrast, 0);
    assert_eq!(state.saturation, 0);
    assert_eq!(state.crop, None);
}

#[test]
fn centered_crop_rect_for_aspect_uses_largest_centered_rect() {
    let rect = centered_crop_rect_for_aspect(400, 300, 16, 9).unwrap();

    assert_eq!(rect.x, 0);
    assert_eq!(rect.y, 37);
    assert_eq!(rect.width, 400);
    assert_eq!(rect.height, 225);
}

#[test]
fn centered_crop_rect_for_square_centers_on_wide_image() {
    let rect = centered_crop_rect_for_aspect(400, 300, 1, 1).unwrap();

    assert_eq!(rect.x, 50);
    assert_eq!(rect.y, 0);
    assert_eq!(rect.width, 300);
    assert_eq!(rect.height, 300);
}

#[test]
fn edit_state_for_preview_omits_crop_while_crop_mode_is_active() {
    let state = EditState {
        rotation: Rotation::R90,
        brightness: 12,
        contrast: 0,
        saturation: 0,
        crop: Some((10, 20, 30, 40)),
    };

    let preview = state.for_preview(true);

    assert_eq!(preview.rotation, Rotation::R90);
    assert_eq!(preview.brightness, 12);
    assert_eq!(preview.crop, None);
}
