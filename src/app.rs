//! AdwApplication lifecycle management
use crate::core::db::DbPool;
use crate::core::init_pool;
use crate::core::media::MediaItem;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::{MainWindow, PhotosPage};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::sync::{Arc, OnceLock};

static TOKIO: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
const INITIAL_MEDIA_PAGE_SIZE: u32 = 1_000;
const BACKGROUND_MEDIA_PAGE_SIZE: u32 = 2_000;

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
                    // construct AlbumsPage/TrashPage on demand, then wire
                    // row-selected to push them onto nav_view.
                    window.set_resources(pool, loader, media_list.clone());
                    window.connect_sidebar(&nav);

                    // Consumer: GTK 主线程独占 media_list 写权限，所以 spawn_local
                    // 排空 change_rx。Upserted/Removed → 同步到 media_list；
                    // TrashChanged → 刷新当前可见的回收站页面（文件管理器改了回收站
                    // 后无需切换页面即可看到）。
                    let window_for_consumer = window.downgrade();
                    gtk::glib::MainContext::default().spawn_local(async move {
                        let mut rx = change_rx;
                        while let Some(event) = rx.recv().await {
                            use crate::core::media_change_notifier::MediaChangeEvent;
                            match event {
                                MediaChangeEvent::TrashChanged => {
                                    if let Some(window) = window_for_consumer.upgrade() {
                                        window.refresh_visible_trash_page();
                                    }
                                }
                                other => crate::ui::apply_to_media_list::apply_to_media_list(
                                    &media_list,
                                    other,
                                ),
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
    use crate::core::db;

    let data_dir = crate::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let pool = init_pool(&data_dir.join("photos.db"))?;

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

    // 启动后台扫描 + 聚合 albums。图片目录和视频目录都作为媒体根扫描；
    // 二者可以混放图片/视频，`media_roots()` 会去重。
    let pictures = crate::config::pictures_dir();
    let media_roots = crate::config::media_roots();
    crate::core::bootstrap::scan_and_aggregate(&pool, &media_roots).await?;

    // 回收站对账：扫描系统回收站，把"原路径在相册目录下、确实已被删"的图片补进 DB
    //（标 trashed）。这样被外部（文件管理器）删除、或历史 DB 行丢失的图片也能在
    // 回收站视图出现。必须在加载首屏 grid 前完成：补进来的都是 trashed 行，不会
    // 进 live grid，但会进 list_trashed_media。详见 trash::reconcile_trash。
    {
        let pool_for_trash = pool.clone();
        let pictures_for_trash = pictures.clone();
        tokio::task::spawn_blocking(move || -> crate::core::error::Result<()> {
            match crate::core::trash::reconcile_trash(&pool_for_trash, &pictures_for_trash) {
                Ok(stats) => tracing::info!(
                    "回收站对账完成：新增 {}、标记 {}、清理 {}、跳过 {}",
                    stats.inserted,
                    stats.marked,
                    stats.pruned,
                    stats.skipped
                ),
                Err(e) => tracing::warn!("回收站对账失败: {e}"),
            }
            Ok(())
        })
        .await
        .map_err(|e| {
            crate::core::error::AppError::Backend(format!("reconcile_trash join error: {e}"))
        })??;
    }

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
        notifier,
    );

    // 首屏先加载一页，剩余数据稍后分批追加，避免大图库启动时一次性构造所有 GTK 对象。
    let items = db::list_media_page(&pool, 0, INITIAL_MEDIA_PAGE_SIZE)?;
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    append_media_items(&list, items);
    load_remaining_media_pages(pool.clone(), list.clone(), INITIAL_MEDIA_PAGE_SIZE);

    // change_rx 交给 activate 处的消费者：那里能拿到 MainWindow，TrashChanged 时
    // 可以刷新可见的回收站页面（相册列表的 Upserted/Removed 也由它应用到 media_list）。
    Ok((list, thumbnail_loader, pool, change_rx))
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
