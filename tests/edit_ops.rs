use photo_viewer::core::edit::{EditRegistry, ParamValue, Rotation};
use image::{DynamicImage, RgbImage};

fn sample_img() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| image::Rgb([128, 128, 128])))
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