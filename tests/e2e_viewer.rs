mod common;
use common::*;
use gtk4 as gtk;
use gtk::gio;
use gtk::gio::prelude::ListModelExt;
use gtk::glib;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;
use photo_viewer::core::thumbnails::ThumbnailLoader;
use std::sync::Arc;

#[test]
fn scan_then_generate_thumbnails() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "a.jpg");
    write_plain_jpeg(root, "b.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());
    let items = backend.scan_dir(root).unwrap();
    for it in &items { backend.upsert(it).unwrap(); }

    let _loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        dir.path().join("cache"),
    ));

    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let loaded = db::list_all_media(&pool).unwrap();
    for it in loaded {
        list.append(&glib::BoxedAnyObject::new(it));
    }

    assert_eq!(list.n_items(), 2);
    // 缩略图生成异步测试由 thumbnails.rs 覆盖
}