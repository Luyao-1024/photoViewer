use super::super::test_support::*;
use super::super::*;
use super::*;

use std::rc::Rc;
use std::sync::Arc;

#[gtk::test]
fn mem_cached_thumbnail_tile_is_built_without_loading_class() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let cache_dir = dir.path().join("thumbs");
    let loader = Arc::new(ThumbnailLoader::new(pool, cache_dir.clone()));
    let src = dir.path().join("cached.png");
    let img = image::RgbaImage::from_pixel(32, 32, image::Rgba([20, 40, 60, 255]));
    image::DynamicImage::ImageRgba8(img).save(&src).unwrap();
    let mut item = sample_item(7, "cached.png");
    item.uri = format!("file://{}", src.display());
    item.path = src;
    let mtime = thumbnail_request_mtime(&item);
    crate::core::thumbnails::generate_for_tests(
        &cache_dir,
        &item.uri,
        ThumbnailSize::Medium,
        Some(mtime),
    )
    .expect("test should pre-create thumbnail cache");
    loader
        .try_load_cached(&item.uri, ThumbnailSize::Medium, Some(mtime))
        .expect("test should load disk cache into the memory LRU");
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(item.clone()));

    let tile = build_photo_picture(
        spec_for_mode(GroupBy::Day),
        item,
        media_list,
        0,
        loader,
        Rc::new(|| {}),
        true,
    );

    assert!(
        !tile.has_css_class("thumb-loading"),
        "cached thumbnails should paint immediately instead of flashing the loading placeholder"
    );
}

#[gtk::test]
fn uncached_thumbnail_tile_stays_hidden_until_result_arrives() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let src = dir.path().join("uncached.png");
    let img = image::RgbaImage::from_pixel(32, 32, image::Rgba([20, 40, 60, 255]));
    image::DynamicImage::ImageRgba8(img).save(&src).unwrap();
    let mut item = sample_item(8, "uncached.png");
    item.uri = format!("file://{}", src.display());
    item.path = src;
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(item.clone()));

    let tile = build_photo_picture(
        spec_for_mode(GroupBy::Day),
        item,
        media_list,
        0,
        loader,
        Rc::new(|| {}),
        true,
    );

    // The tile is hidden via CSS (`.glass-thumb-card.thumb-loading` sets
    // opacity:0 in grid_css), not via widget.set_opacity — that lets the
    // fade-in transition fire when set_paintable drops the class. So this
    // headless test asserts the CSS hook (.thumb-loading) is present rather
    // than a widget opacity value.
    assert!(
        tile.has_css_class("thumb-loading"),
        "uncached thumbnails must carry .thumb-loading so CSS hides them until generation finishes"
    );
    let flow = gtk::FlowBox::new();
    flow.append(&tile);
    let flow_child = tile
        .parent()
        .and_then(|w| w.downcast::<gtk::FlowBoxChild>().ok())
        .expect("FlowBox should wrap tile in a FlowBoxChild");
    sync_flow_child_visibility_for_tile(&tile, &flow_child);
    assert_eq!(
        flow_child.opacity(),
        0.0,
        "uncached thumbnails should hide the FlowBoxChild wrapper as well as the tile"
    );
    tile.set_paintable(Some(&gray_placeholder_texture()));
    assert!(
                !tile.has_css_class("thumb-loading"),
                "thumbnail success or failure should drop .thumb-loading so CSS reveals (and fades in) the tile"
            );
    assert_eq!(
        flow_child.opacity(),
        1.0,
        "thumbnail success or failure should reveal the FlowBoxChild wrapper"
    );
}

#[gtk::test]
fn uncached_existing_thumbnail_keeps_its_loading_border_visible() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let item = sample_item(9, "existing.png");
    media_list.append(&glib::BoxedAnyObject::new(item.clone()));

    let tile = build_photo_picture(
        spec_for_mode(GroupBy::Day),
        item,
        media_list,
        0,
        loader,
        Rc::new(|| {}),
        false,
    );
    let flow = gtk::FlowBox::new();
    flow.append(&tile);
    let flow_child = tile
        .parent()
        .and_then(|w| w.downcast::<gtk::FlowBoxChild>().ok())
        .expect("FlowBox should wrap tile in a FlowBoxChild");

    sync_flow_child_visibility_for_tile(&tile, &flow_child);
    assert!(tile.has_css_class("thumb-loading"));
    assert!(tile.has_css_class("thumb-placeholder"));
    assert_eq!(
        flow_child.opacity(),
        1.0,
        "existing uncached media should show its loading border before the thumbnail arrives"
    );

    tile.set_paintable(Some(&gray_placeholder_texture()));
    assert!(!tile.has_css_class("thumb-loading"));
    assert!(!tile.has_css_class("thumb-placeholder"));
}

#[test]
fn tile_duration_formats_minutes_and_hours() {
    assert_eq!(format_tile_duration(83.2).as_deref(), Some("01:23"));
    assert_eq!(format_tile_duration(3_661.0).as_deref(), Some("1:01:01"));
    assert_eq!(format_tile_duration(f64::NAN), None);
}
