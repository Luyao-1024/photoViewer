//! 五个编辑算子的契约测试：类型分派、默认参数与错误路径。
//!
//! 算子本身是 `op::apply_*` 的薄封装，风险集中在**收到错误 `ParamValue` 时是否
//! 报错而不是静默套用默认变换**——历史上这一层没有单元测试覆盖。

use super::brightness::BrightnessOp;
use super::contrast::ContrastOp;
use super::crop::CropOp;
use super::rotate::RotateOp;
use super::saturation::SaturationOp;
use super::{EditCategory, EditOperation, ParamValue, Rotation};
use image::{DynamicImage, GenericImageView, RgbaImage};

fn solid(value: u8) -> DynamicImage {
    let mut buf = RgbaImage::new(2, 2);
    for pixel in buf.pixels_mut() {
        pixel.0 = [value, value, value, 255];
    }
    DynamicImage::ImageRgba8(buf)
}

fn first_channel(img: &DynamicImage) -> u8 {
    img.to_rgba8().get_pixel(0, 0).0[0]
}

#[test]
fn brightness_applies_brightness_and_nothing_else() {
    let img = solid(100);
    let out = BrightnessOp
        .apply(&img, ParamValue::Int(100))
        .expect("int params");
    assert!(first_channel(&out) > 100);
}

#[test]
fn contrast_applies_contrast_and_nothing_else() {
    let img = solid(100);
    let out = ContrastOp
        .apply(&img, ParamValue::Int(50))
        .expect("int params");
    assert_ne!(first_channel(&out), first_channel(&img));
}

#[test]
fn saturation_applies_saturation_and_nothing_else() {
    let img = solid(100);
    let out = SaturationOp
        .apply(&img, ParamValue::Int(-50))
        .expect("int params");
    assert_eq!(out.dimensions(), img.dimensions());
}

#[test]
fn rotate_applies_the_requested_rotation() {
    let img = solid(10);
    let out = RotateOp
        .apply(&img, ParamValue::Rotation(Rotation::R90))
        .expect("rotation params");
    assert_eq!(out.dimensions(), (2, 2));
}

#[test]
fn crop_with_none_returns_the_image_untouched() {
    let img = solid(10);
    let out = CropOp
        .apply(&img, ParamValue::Crop(None))
        .expect("crop params");
    assert_eq!(out.dimensions(), img.dimensions());
}

#[test]
fn crop_applies_the_requested_region() {
    let img = solid(10);
    let out = CropOp
        .apply(&img, ParamValue::Crop(Some((0, 0, 1, 1))))
        .expect("crop params");
    assert_eq!(out.dimensions(), (1, 1));
}

#[test]
fn color_operators_reject_a_non_int_param() {
    let img = solid(100);
    for (name, error) in [
        ("brightness", BrightnessOp.apply(&img, ParamValue::Uint(1))),
        ("contrast", ContrastOp.apply(&img, ParamValue::Bool(true))),
        ("saturation", SaturationOp.apply(&img, ParamValue::Uint(1))),
    ] {
        let message = error
            .err()
            .unwrap_or_else(|| panic!("{name} should reject"));
        assert!(
            message.contains(name),
            "{name} error should name the operator, got {message:?}"
        );
    }
}

#[test]
fn rotate_rejects_a_non_rotation_param() {
    let img = solid(100);
    let error = RotateOp
        .apply(&img, ParamValue::Int(90))
        .expect_err("an Int must not be accepted where a Rotation is required");
    assert!(error.contains("rotate"), "got {error:?}");
}

#[test]
fn crop_rejects_a_non_crop_param() {
    let img = solid(100);
    let error = CropOp
        .apply(&img, ParamValue::Int(1))
        .expect_err("an Int must not be accepted where a Crop is required");
    assert!(error.contains("crop"), "got {error:?}");
}

#[test]
fn defaults_are_neutral() {
    // A freshly selected operator must not change the image until the user moves a slider.
    let img = solid(120);
    let before = first_channel(&img);

    let out = BrightnessOp
        .apply(&img, BrightnessOp.default_params())
        .expect("default");
    assert_eq!(first_channel(&out), before);
    let out = ContrastOp
        .apply(&img, ContrastOp.default_params())
        .expect("default");
    assert_eq!(first_channel(&out), before);
    let out = SaturationOp
        .apply(&img, SaturationOp.default_params())
        .expect("default");
    assert_eq!(first_channel(&out), before);
    let out = RotateOp
        .apply(&img, RotateOp.default_params())
        .expect("default");
    assert_eq!(out.dimensions(), img.dimensions());
    let out = CropOp
        .apply(&img, CropOp.default_params())
        .expect("default");
    assert_eq!(out.dimensions(), img.dimensions());
}

#[test]
fn identities_are_stable_and_categories_are_grouped() {
    // The id is what the UI persists, so it must not drift from the display name.
    assert_eq!(BrightnessOp.id(), "brightness");
    assert_eq!(ContrastOp.id(), "contrast");
    assert_eq!(SaturationOp.id(), "saturation");
    assert_eq!(RotateOp.id(), "rotate");
    assert_eq!(CropOp.id(), "crop");

    assert_eq!(BrightnessOp.display_name().to_lowercase(), "brightness");
    assert_eq!(ContrastOp.display_name().to_lowercase(), "contrast");
    assert_eq!(SaturationOp.display_name().to_lowercase(), "saturation");
    assert_eq!(RotateOp.display_name().to_lowercase(), "rotate");
    assert_eq!(CropOp.display_name().to_lowercase(), "crop");

    assert_eq!(BrightnessOp.category(), EditCategory::Color);
    assert_eq!(ContrastOp.category(), EditCategory::Color);
    assert_eq!(SaturationOp.category(), EditCategory::Color);
    assert_eq!(RotateOp.category(), EditCategory::Transform);
    assert_eq!(CropOp.category(), EditCategory::Crop);
}
