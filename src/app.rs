//! AdwApplication lifecycle management
use crate::core::db::DbPool;
use crate::core::error::Result as CoreResult;
use crate::core::init_pool;
use crate::core::media::MediaItem;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::{MainWindow, PhotosPage};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

static TOKIO: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
const INITIAL_MEDIA_PAGE_SIZE: u32 = 200;
const BACKGROUND_MEDIA_PAGE_SIZE: u32 = 200;

fn install_tokio_runtime() -> &'static tokio::runtime::Runtime {
    TOKIO.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        // Enter the runtime context. `EnterGuard` borrows from
        // `runtime`; we `forget` it so it lives forever (and the
        // thread-local stays set for the process lifetime), then
        // hand the runtime back to be stored in the OnceLock.
        let guard = runtime.enter();
        std::mem::forget(guard);
        runtime
    })
}

pub fn build_app() -> adw::Application {
    // Build a multi-thread tokio runtime and enter its context for the
    // lifetime of the application. GTK's main loop is *not* a tokio
    // runtime, so `tokio::task::spawn_blocking` (used by the thumbnail
    // worker pool and the scan worker) would otherwise panic with
    // "there is no reactor running". We stash the runtime in a
    // process-wide `OnceLock` and `forget` the EnterGuard so the
    // thread-local stays set for the entire process.
    let _ = install_tokio_runtime();

    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer")
        .build();

    app.connect_activate(move |app| {
        // Follow the system color scheme (libadwaita picks up the user's
        // GNOME light/dark preference; this just opts in to automatic
        // tracking rather than forcing light or dark).
        let style_manager = adw::StyleManager::default();
        style_manager.set_color_scheme(adw::ColorScheme::Default);

        // Register the grid + glass CSS before the first widget is realized so
        // the user's Liquid Glass preference (prefs::liquid_glass_enabled,
        // read inside install()) is honoured from the very first frame. Later
        // defensive grid_css::install() calls from page constructors are no-ops.
        crate::ui::grid_css::install();

        let window = MainWindow::new(app);
        window.populate_sidebar();

        // 异步初始化 DB + 扫描
        let app_handle = app.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            match initialize().await {
                Ok((media_list, loader, pool, change_rx)) => {
                    let window: MainWindow = app_handle
                        .active_window()
                        .and_downcast::<MainWindow>()
                        .expect("MainWindow not found");
                    let nav = window.nav_view();
                    let photos = PhotosPage::new(media_list.clone(), loader.clone());
                    // Inject the nav view so the PhotosPage can push a ViewerPage
                    // when a tile is clicked.
                    photos.set_nav_target(&nav);
                    // Inject the DB pool so ViewerPage can launch the editor panel
                    // (the editor needs the pool for M4-T4 save logic).
                    photos.set_db_pool(pool.clone());
                    nav.push(&photos);

                    // Store DB pool + loader on the window so the sidebar can
                    // build album detail / trash pages on demand, then wire
                    // row-selected to push them onto nav_view.
                    let pool_for_consumer = pool.clone();
                    window.set_resources(pool, loader, media_list.clone());
                    // Now that the pool is available, populate the album rows
                    // nested under the sidebar's Albums group header.
                    window.populate_album_rows();
                    window.connect_sidebar(&nav);

                    // Consumer: GTK 主线程独占 media_list 写权限，所以 spawn_local
                    // 排空 change_rx。Upserted/Removed → 同步到 media_list；
                    // TrashChanged → 刷新当前可见的回收站页面（文件管理器改了回收站
                    // 后无需切换页面即可看到）。
                    let window_for_consumer = window.downgrade();
                    gtk::glib::MainContext::default().spawn_local(async move {
                        let mut rx = change_rx;
                        while let Some(event) = rx.recv().await {
                            use crate::core::media_change_notifier::{
                                MediaChangeEvent, MediaChangeSource,
                            };
                            match event {
                                MediaChangeEvent::TrashChanged => {
                                    if let Some(window) = window_for_consumer.upgrade() {
                                        window.refresh_visible_trash_page();
                                    }
                                }
                                other => {
                                    let is_startup_scan_batch = matches!(
                                        &other,
                                        MediaChangeEvent::UpsertedBatch {
                                            source: MediaChangeSource::StartupScan,
                                            ..
                                        }
                                    );
                                    let event_label = match &other {
                                        MediaChangeEvent::Upserted(_) => "upserted".to_string(),
                                        MediaChangeEvent::UpsertedBatch { source, items } => {
                                            format!("upserted_batch({source:?}, {})", items.len())
                                        }
                                        MediaChangeEvent::Removed { .. } => "removed".to_string(),
                                        MediaChangeEvent::TrashChanged => "trash_changed".to_string(),
                                    };
                                    let list_len_before = media_list.n_items();
                                    let apply_started = std::time::Instant::now();
                                    crate::ui::apply_to_media_list::apply_to_media_list(
                                        &media_list,
                                        other,
                                    );
                                    tracing::info!(
                                        target: crate::core::log_targets::BROWSING,
                                        "UI_CHANGE_APPLY event={} list_len_before={} list_len_after={} elapsed_ms={}",
                                        event_label,
                                        list_len_before,
                                        media_list.n_items(),
                                        apply_started.elapsed().as_millis()
                                    );
                                    // 文件系统监视器已更新 DB（albums::refresh），
                                    // 此处同步刷新侧栏相册行，使新增/删除的相册
                                    // 及照片计数即时反映到 UI。
                                    if !is_startup_scan_batch {
                                        if let Some(window) = window_for_consumer.upgrade() {
                                            let album_started = std::time::Instant::now();
                                            window.refresh_album_rows();
                                            tracing::info!(
                                                target: crate::core::log_targets::BROWSING,
                                                "UI_ALBUM_REFRESH after_event={} elapsed_ms={}",
                                                event_label,
                                                album_started.elapsed().as_millis()
                                            );
                                        }
                                    } else {
                                        let pool_for_albums = pool_for_consumer.clone();
                                        let window_for_albums = window_for_consumer.clone();
                                        let event_label_for_albums = event_label.clone();
                                        gtk::glib::MainContext::default().spawn_local(async move {
                                            let refresh_started = std::time::Instant::now();
                                            let result = gtk::gio::spawn_blocking(move || {
                                                crate::core::albums::refresh(&pool_for_albums)
                                            })
                                            .await;
                                            match result {
                                                Ok(Ok(())) => {
                                                    if let Some(window) = window_for_albums.upgrade()
                                                    {
                                                        window.refresh_album_rows();
                                                    }
                                                    tracing::info!(
                                                        target: crate::core::log_targets::BROWSING,
                                                        "STARTUP_ALBUM_REFRESH after_event={} elapsed_ms={}",
                                                        event_label_for_albums,
                                                        refresh_started.elapsed().as_millis()
                                                    );
                                                }
                                                Ok(Err(err)) => tracing::warn!(
                                                    "startup albums refresh failed: {err}"
                                                ),
                                                Err(err) => tracing::warn!(
                                                    "startup albums refresh join failed: {err:?}"
                                                ),
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("初始化失败: {}", e);
                }
            }
        });

        window.present();
    });

    app
}

async fn initialize() -> anyhow::Result<(
    gtk::gio::ListStore,
    Arc<ThumbnailLoader>,
    DbPool,
    tokio::sync::mpsc::UnboundedReceiver<crate::core::media_change_notifier::MediaChangeEvent>,
)> {
    let data_dir = crate::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("photos.db");
    let (pool, items) = initialize_db_once_with_retry(db_path.clone()).await?;

    // 缩略图加载器单例（M2-T1）
    let thumbnail_loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        crate::config::cache_dir(),
    ));
    // worker 数随核数取（夹在 [4, 8]）：太少首屏填不满，太多与扫描/主线程争抢
    // 且磁盘 IO 边际递减。解码是 CPU 密集突发，冷启动/滚动时才吃满。
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(4, 8);
    thumbnail_loader.spawn_workers(workers);

    let pictures = crate::config::pictures_dir();
    let media_roots = crate::config::media_roots();

    // 启动文件监听（M5-T5+）：监听媒体根的后续变更并增量 upsert。
    // 通过 `MediaChangeNotifier` 把"哪个 MediaItem 变了"推给 GTK 主线程
    // 消费者；消费者按 uri 在共享的 `media_list` 上做 splice/append/remove。
    //
    // 同时监听系统回收站根：文件管理器对回收站的还原/清空/删除只动回收站目录，
    // 必须单独监听才能实时感知（见 notify_watcher 的防抖对账）。
    let (notifier, change_rx) = crate::core::media_change_notifier::MediaChangeNotifier::new();
    let trash_roots = crate::core::trash::trash_roots();
    let mut watch_paths = media_roots.clone();
    watch_paths.extend(trash_roots.iter().filter(|r| r.exists()).cloned());
    let _watcher = crate::core::notify_watcher::start_watching(
        pool.clone(),
        watch_paths,
        trash_roots,
        pictures.clone(),
        notifier.clone(),
    );

    // 首屏只加载一页（200 张），让窗口尽快可操作；启动扫描、回收站对账和
    // 剩余 DB 分页都在后台继续。
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    append_media_items(&list, items);
    start_background_startup_work(
        pool.clone(),
        media_roots,
        pictures,
        notifier.clone(),
        list.clone(),
        INITIAL_MEDIA_PAGE_SIZE,
    );

    // change_rx 交给 activate 处的消费者：那里能拿到 MainWindow，TrashChanged 时
    // 可以刷新可见的回收站页面（相册列表的 Upserted/Removed 也由它应用到 media_list）。
    Ok((list, thumbnail_loader, pool, change_rx))
}

fn initialize_db_once_blocking(path: PathBuf) -> CoreResult<(DbPool, Vec<MediaItem>)> {
    let pool = init_pool(&path)?;
    let items = crate::core::db::list_media_page(&pool, 0, INITIAL_MEDIA_PAGE_SIZE)?;
    Ok((pool, items))
}

async fn initialize_db_once_with_retry(path: PathBuf) -> anyhow::Result<(DbPool, Vec<MediaItem>)> {
    let first = match gtk::gio::spawn_blocking({
        let path = path.clone();
        move || initialize_db_once_blocking(path)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => return Err(anyhow::anyhow!("spawn_blocking join error: {:?}", e)),
    };

    if let Ok(data) = first {
        Ok(data)
    } else {
        tracing::warn!(
            "first DB init/query attempt failed at {}: {} ; retrying once after cleanup path",
            path.display(),
            first.as_ref().err().unwrap()
        );
        let second = match gtk::gio::spawn_blocking({
            let path = path.clone();
            move || initialize_db_once_blocking(path)
        })
        .await
        {
            Ok(r) => r,
            Err(e) => return Err(anyhow::anyhow!("spawn_blocking join error: {:?}", e)),
        };
        second.map_err(anyhow::Error::from)
    }
}

fn append_media_items(list: &gtk::gio::ListStore, items: Vec<MediaItem>) {
    if items.is_empty() {
        return;
    }

    // During startup the consumer loop may have already appended an item
    // (via a file-system Upserted event) that also appears in this DB page.
    // Collect the URIs already present so we skip them here, preventing
    // duplicate grid tiles.
    let existing: std::collections::HashSet<String> = (0..list.n_items())
        .filter_map(|i| {
            list.item(i)
                .and_downcast::<glib::BoxedAnyObject>()
                .map(|obj| obj.borrow::<MediaItem>().uri.clone())
        })
        .collect();

    let additions: Vec<glib::BoxedAnyObject> = items
        .into_iter()
        .filter(|item| !existing.contains(&item.uri))
        .map(glib::BoxedAnyObject::new)
        .collect();
    if !additions.is_empty() {
        list.splice(list.n_items(), 0, &additions);
    }
}

fn load_remaining_media_pages(pool: DbPool, list: gtk::gio::ListStore, start_offset: u32) {
    glib::MainContext::default().spawn_local(async move {
        let mut offset = start_offset;
        loop {
            let pool_for_page = pool.clone();
            let result = gtk::gio::spawn_blocking(move || {
                crate::core::db::list_media_page(&pool_for_page, offset, BACKGROUND_MEDIA_PAGE_SIZE)
            })
            .await;
            let items = match result {
                Ok(Ok(items)) => items,
                Ok(Err(e)) => {
                    tracing::error!("background media page load failed: {}", e);
                    return;
                }
                Err(e) => {
                    tracing::error!("background media page load join failed: {:?}", e);
                    return;
                }
            };
            if items.is_empty() {
                return;
            }
            let count = items.len() as u32;
            append_media_items(&list, items);
            if count < BACKGROUND_MEDIA_PAGE_SIZE {
                return;
            }
            offset += count;
        }
    });
}

fn start_background_startup_work(
    pool: DbPool,
    media_roots: Vec<std::path::PathBuf>,
    pictures: std::path::PathBuf,
    notifier: crate::core::media_change_notifier::MediaChangeNotifier,
    list: gtk::gio::ListStore,
    remaining_offset: u32,
) {
    glib::MainContext::default().spawn_local(async move {
        if let Err(e) = crate::core::bootstrap::scan_and_aggregate_with_notifier(
            &pool,
            &media_roots,
            notifier.clone(),
        )
        .await
        {
            tracing::error!("后台扫描失败: {}", e);
        }

        let pool_for_trash = pool.clone();
        let pictures_for_trash = pictures.clone();
        match gtk::gio::spawn_blocking(move || {
            crate::core::trash::reconcile_trash(&pool_for_trash, &pictures_for_trash)
        })
        .await
        {
            Ok(Ok(stats)) => {
                tracing::info!(
                    "回收站对账完成：新增 {}、标记 {}、清理 {}、跳过 {}",
                    stats.inserted,
                    stats.marked,
                    stats.pruned,
                    stats.skipped
                );
                notifier.trash_changed();
            }
            Ok(Err(e)) => tracing::warn!("回收站对账失败: {e}"),
            Err(e) => tracing::warn!("回收站对账 join 失败: {e:?}"),
        }

        load_remaining_media_pages(pool, list, remaining_offset);
    });
}
