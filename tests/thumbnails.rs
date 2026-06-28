use gtk4::prelude::TextureExt;
use photo_viewer::core::db;
use photo_viewer::core::thumbnails::{ThumbnailLoader, ThumbnailSize, TIER_NORMAL};
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
        None,
        tx,
        TIER_NORMAL,
    );

    let loaded = runtime.block_on(async { rx.await.unwrap() });
    drop(_guard);
    assert!(loaded.texture.width() > 0, "texture width must be > 0");
    assert!(loaded.texture.height() > 0, "texture height must be > 0");

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
        None,
        tx1,
        TIER_NORMAL,
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
        None,
        tx2,
        TIER_NORMAL,
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
    loader.request(
        uri.clone(),
        ThumbnailSize::Small,
        None,
        tx_small,
        TIER_NORMAL,
    );
    let small = runtime.block_on(async { rx_small.await.unwrap() });

    let (tx_large, rx_large) = tokio::sync::oneshot::channel();
    loader.request(uri, ThumbnailSize::Large, None, tx_large, TIER_NORMAL);
    let large = runtime.block_on(async { rx_large.await.unwrap() });

    drop(_guard);
    assert!(
        large.texture.width() > small.texture.width(),
        "large thumbnail should not reuse the small thumbnail from memory cache"
    );
}

/// Regression for the first-load "blank thumbnails" bug.
///
/// Three PhotosPage grids (Year/Month/Day) used to fire one request per tile
/// at rebuild time, so a single visible source could be requested thousands
/// of times in a burst. The loader's bounded queue (capacity `QUEUE_CAPACITY`)
/// silently dropped the overflow via `try_send`, leaving tiles permanently
/// blank. With in-flight dedup, every duplicate (uri, size) request attaches
/// to the single in-flight generation instead of enqueueing separately, so
/// none are dropped.
///
/// We fire `N` (> `QUEUE_CAPACITY`) requests for the SAME uri with a single
/// worker and a large source so the worker cannot drain the burst: on the old
/// code ~`N - QUEUE_CAPACITY` requests would be dropped (`rx.Err`); with dedup
/// all `N` must resolve.
#[test]
fn duplicate_requests_are_coalesced_and_never_dropped() {
    ensure_gtk();
    let dir = tempdir().unwrap();
    // Large source → non-trivial decode, so the single worker stays busy while
    // the request burst fills (and on the old code, overflows) the queue.
    let src = dir.path().join("big-src.jpg");
    let img = image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(2400, 1600, |_, _| {
        image::Rgb([128, 128, 128])
    });
    img.save(&src).unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let runtime = rt();
    let _guard = runtime.enter();
    let loader = ThumbnailLoader::new(pool, dir.path().join("cache"));
    loader.spawn_workers(1);

    let uri = format!("file://{}", src.display());
    const N: usize = 3000; // > QUEUE_CAPACITY (2048)
    let mut rxs = Vec::with_capacity(N);
    for _ in 0..N {
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(uri.clone(), ThumbnailSize::Small, None, tx, TIER_NORMAL);
        rxs.push(rx);
    }

    let mut ok = 0usize;
    for rx in rxs {
        if runtime
            .block_on(rx)
            .map(|t| t.texture.width() > 0)
            .unwrap_or(false)
        {
            ok += 1;
        }
    }
    drop(_guard);
    assert_eq!(ok, N, "all duplicate requests must resolve; none dropped");
}

/// B6 回归：`prioritize_keys` 把仍在排队的请求提到队首（BOOST），
/// 单 worker 起来后应**先**处理被提权的项，消除优先级倒置。
#[test]
fn prioritize_keys_serves_boosted_before_normal() {
    ensure_gtk();
    let dir = tempdir().unwrap();
    // A 用大图（生成慢，拉开时间窗），B 用小图。
    let src_a = dir.path().join("big-a.jpg");
    let img = image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(2400, 1600, |_, _| {
        image::Rgb([60, 90, 120])
    });
    img.save(&src_a).unwrap();
    let src_b = write_plain_jpeg(dir.path(), "b.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let runtime = rt();
    let _guard = runtime.enter();
    let loader = ThumbnailLoader::new(pool, dir.path().join("cache"));

    let uri_a = format!("file://{}", src_a.display());
    let uri_b = format!("file://{}", src_b.display());
    let key_b = ThumbnailLoader::cache_key_for(&uri_b, ThumbnailSize::Small, None).unwrap();

    // 不起 worker：两条 NORMAL 先入队（A 先、B 后）。
    let (txa, rxa) = tokio::sync::oneshot::channel();
    loader.request(uri_a, ThumbnailSize::Small, None, txa, TIER_NORMAL);
    let (txb, rxb) = tokio::sync::oneshot::channel();
    loader.request(uri_b, ThumbnailSize::Small, None, txb, TIER_NORMAL);

    // 提权 B（模拟可见）。此时两者都还在排队 → B 升 BOOST。
    loader.prioritize_keys(&[key_b]);

    // 起单 worker：应先弹 BOOST(B) 再弹 NORMAL(A)。
    loader.spawn_workers(1);

    // 谁先 resolve 谁就被先处理。B 小且被提权 → 应先完成。
    let first = runtime.block_on(async {
        tokio::select! {
            Ok(l) = rxa => { let _ = l.texture.width(); 'A' }
            Ok(l) = rxb => { let _ = l.texture.width(); 'B' }
        }
    });
    assert_eq!(first, 'B', "BOOST 项 B 应先于 NORMAL 项 A 被处理");
}
