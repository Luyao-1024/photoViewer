//! Destructive rotate + undo tests.
//!
//! Verifies that `rotate_in_place` updates orientation metadata without
//! rotating encoded pixels.
mod common;

use common::*;
use photo_viewer::core::edit::destructive_rotate;
use photo_viewer::core::orientation::{load_oriented_pixbuf, read_orientation};
use tempfile::tempdir;

#[test]
fn rotate_jpeg_updates_orientation_without_changing_pixels() {
    let dir = tempdir().unwrap();
    let src = write_plain_jpeg(dir.path(), "rot.jpg");
    let original = image::open(&src).unwrap().to_rgb8();

    destructive_rotate::rotate_in_place(&src, 90).unwrap();

    let after = image::open(&src).unwrap().to_rgb8();
    assert_eq!(after.dimensions(), original.dimensions());
    assert_eq!(after.as_raw(), original.as_raw());
    assert_eq!(read_orientation(&src).unwrap(), 6);
    assert!(src.with_extension("jpg.bak").exists());

    let displayed = load_oriented_pixbuf(&src).unwrap();
    assert_eq!((displayed.width(), displayed.height()), (48, 64));

    destructive_rotate::rotate_in_place(&src, -90).unwrap();
    assert_eq!(read_orientation(&src).unwrap(), 1);
}

#[test]
fn rotate_png_updates_orientation_without_changing_pixels() {
    let dir = tempdir().unwrap();
    let src = write_plain_png(dir.path(), "rot.png");
    let original = image::open(&src).unwrap().to_rgb8();

    destructive_rotate::rotate_in_place(&src, 90).unwrap();

    let after = image::open(&src).unwrap().to_rgb8();
    assert_eq!(after.dimensions(), original.dimensions());
    assert_eq!(after.as_raw(), original.as_raw());
    assert_eq!(read_orientation(&src).unwrap(), 6);
    assert!(src.with_extension("png.bak").exists());

    let displayed = load_oriented_pixbuf(&src).unwrap();
    assert_eq!((displayed.width(), displayed.height()), (48, 64));
}
