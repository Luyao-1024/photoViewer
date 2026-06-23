mod common;
use common::*;
use photo_viewer::core::metadata;

#[test]
fn plain_jpeg_returns_no_taken_at() {
    let dir = tmp_dir();
    let path = write_plain_jpeg(dir.path(), "plain.jpg");

    let meta = metadata::extract(&path).unwrap();
    assert_eq!(meta.mime_type, "image/jpeg");
    assert_eq!(meta.width, Some(64));
    assert_eq!(meta.height, Some(48));
    assert!(meta.taken_at.is_none(), "无 EXIF 数据应返回 None");
    assert!(meta.exif_fields.is_empty(), "无 EXIF 数据应返回空字段列表");
}

#[test]
fn unknown_extension_returns_error() {
    let dir = tmp_dir();
    let path = dir.path().join("garbage.xyz");
    std::fs::write(&path, b"not an image").unwrap();

    let result = metadata::extract(&path);
    assert!(result.is_err());
}

#[test]
fn mime_type_inferred_from_extension() {
    let dir = tmp_dir();
    let png_path = dir.path().join("test.png");
    image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(10, 10, |_, _| image::Rgb([0, 0, 0]))
        .save(&png_path)
        .unwrap();

    let meta = metadata::extract(&png_path).unwrap();
    assert_eq!(meta.mime_type, "image/png");
}

#[test]
fn exif_fields_include_datetime_original() {
    let dir = tmp_dir();
    let naive = chrono::NaiveDate::from_ymd_opt(2024, 5, 6)
        .unwrap()
        .and_hms_opt(7, 8, 9)
        .unwrap();
    let path = write_jpeg_with_exif(dir.path(), "with-exif.jpg", naive);

    let meta = metadata::extract(&path).unwrap();

    assert!(
        meta.exif_fields.iter().any(|field| {
            field.tag == "DateTimeOriginal" && field.value.contains("2024-05-06 07:08:09")
        }),
        "EXIF 字段列表应包含 DateTimeOriginal, got {:?}",
        meta.exif_fields
    );
}
