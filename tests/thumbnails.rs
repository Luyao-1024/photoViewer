use gtk4::prelude::TextureExt;
use photo_viewer::core::db;
use photo_viewer::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use std::sync::Once;
use tempfile::tempdir;
use tokio::runtime::Runtime;

mod common;
use common::write_plain_jpeg;

static INIT: Once = Once::new();

fn ensure_gtk() {
    INIT.call_once(|| {
        gtk4::init().expect("GTK init failed");
    });
}

fn rt() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap()
}

#[test]
fn generate_and_cache() {
    ensure_gtk();
    let dir = tempdir().unwrap();
    let src = write_plain_jpeg(dir.path(), "src.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let runtime = rt();
    let _guard = runtime.enter();

    let loader = ThumbnailLoader::new(pool, dir.path().join("cache"));
    loader.spawn_workers(2);

    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request(
        format!("file://{}", src.display()),
        ThumbnailSize::Small,
        tx,
    );

    let tex = runtime.block_on(async { rx.await.unwrap() });
    drop(_guard);
    assert!(tex.width() > 0, "texture width must be > 0");
    assert!(tex.height() > 0, "texture height must be > 0");

    // 验证磁盘缓存文件存在
    let cache_dir = dir.path().join("cache/thumbnails/small");
    assert!(cache_dir.exists(), "cache dir should exist");
    let files: Vec<_> = walkdir::WalkDir::new(&cache_dir)
        .into_iter()
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "jpg").unwrap_or(false))
        .collect();
    assert!(
        !files.is_empty(),
        "expected at least one cached jpg under {:?}",
        cache_dir
    );
}

#[test]
fn cache_hit_avoids_regenerate() {
    ensure_gtk();
    let dir = tempdir().unwrap();
    let src = write_plain_jpeg(dir.path(), "src.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let runtime = rt();
    let _guard = runtime.enter();

    let loader = ThumbnailLoader::new(pool, dir.path().join("cache"));
    loader.spawn_workers(2);

    // 第一次
    let (tx1, rx1) = tokio::sync::oneshot::channel();
    loader.request(
        format!("file://{}", src.display()),
        ThumbnailSize::Medium,
        tx1,
    );
    let _ = runtime.block_on(rx1);

    // 记录缓存 mtime
    let cache_file = walkdir::WalkDir::new(dir.path().join("cache/thumbnails/medium"))
        .into_iter()
        .flatten()
        .find(|e| e.path().extension().map(|x| x == "jpg").unwrap_or(false))
        .expect("cache file should exist after first request");
    let mtime1 = std::fs::metadata(cache_file.path())
        .unwrap()
        .modified()
        .unwrap();

    // 第二次（应命中缓存）
    std::thread::sleep(std::time::Duration::from_millis(20));
    let (tx2, rx2) = tokio::sync::oneshot::channel();
    loader.request(
        format!("file://{}", src.display()),
        ThumbnailSize::Medium,
        tx2,
    );
    let _ = runtime.block_on(rx2);

    let mtime2 = std::fs::metadata(cache_file.path())
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(mtime1, mtime2, "命中缓存时不应重新生成");
}

#[test]
fn memory_cache_keeps_thumbnail_sizes_separate() {
    ensure_gtk();
    let dir = tempdir().unwrap();
    let src = dir.path().join("large-src.jpg");
    let img = image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(1800, 1200, |_, _| {
        image::Rgb([128, 128, 128])
    });
    img.save(&src).unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let runtime = rt();
    let _guard = runtime.enter();

    let loader = ThumbnailLoader::new(pool, dir.path().join("cache"));
    loader.spawn_workers(1);

    let uri = format!("file://{}", src.display());

    let (tx_small, rx_small) = tokio::sync::oneshot::channel();
    loader.request(uri.clone(), ThumbnailSize::Small, tx_small);
    let small = runtime.block_on(async { rx_small.await.unwrap() });

    let (tx_large, rx_large) = tokio::sync::oneshot::channel();
    loader.request(uri, ThumbnailSize::Large, tx_large);
    let large = runtime.block_on(async { rx_large.await.unwrap() });

    drop(_guard);
    assert!(
        large.width() > small.width(),
        "large thumbnail should not reuse the small thumbnail from memory cache"
    );
}
