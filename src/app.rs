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

        let window = MainWindow::new(app);
        window.populate_sidebar();

        // 异步初始化 DB + 扫描
        let app_handle = app.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            match initialize().await {
                Ok((media_list, loader, pool)) => {
                    let window: MainWindow = app_handle
                        .active_window()
                        .and_downcast::<MainWindow>()
                        .expect("MainWindow not found");
                    let nav = window.nav_view();
                    let photos = PhotosPage::new(media_list.clone(), loader.clone());
                    // Inject the nav view so the PhotosPage can push a ViewerPage
                    // when a tile is clicked.
                    photos.set_nav_target(&nav);
                    // Inject the DB pool so ViewerPage can launch EditorPage
                    // (the editor needs the pool for M4-T4 save logic).
                    photos.set_db_pool(pool.clone());
                    nav.push(&photos);

                    // Store DB pool + loader on the window so the sidebar can
                    // construct AlbumsPage/TrashPage on demand, then wire
                    // row-selected to push them onto nav_view.
                    window.set_resources(pool, loader, media_list);
                    window.connect_sidebar(&nav);
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

async fn initialize() -> anyhow::Result<(gtk::gio::ListStore, Arc<ThumbnailLoader>, DbPool)> {
    use crate::core::db;

    let data_dir = crate::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let pool = init_pool(&data_dir.join("photos.db"))?;

    // 缩略图加载器单例（M2-T1）
    let thumbnail_loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        crate::config::cache_dir(),
    ));
    thumbnail_loader.spawn_workers(4);

    // 启动后台扫描 + 聚合 albums
    // 优先使用 XDG user-dirs.dirs，否则按 locale 回退（zh_CN -> ~/图片，否则 ~/Pictures）。
    let pictures = crate::config::pictures_dir();
    crate::core::bootstrap::scan_and_aggregate(&pool, std::slice::from_ref(&pictures)).await?;

    // 启动文件监听（M5-T5）：监听 ~/Pictures 的后续变更并增量 upsert。
    // 每次 upsert 成功后，在阻塞线程里调用 on_change；closure 内部把
    // `albums::refresh` 调度到 GTK 主线程（侧栏 Albums row 每次点击
    // 都会重建 AlbumsPage，所以"仅 refresh，下次点击重建"足够）。
    let on_change = {
        let pool = pool.clone();
        move || {
            let pool = pool.clone();
            glib::MainContext::default().spawn_local(async move {
                let result =
                    gtk::gio::spawn_blocking(move || crate::core::albums::refresh(&pool)).await;
                let result = result.unwrap_or_else(|e| {
                    Err(crate::core::error::AppError::Backend(format!(
                        "albums refresh join error: {e:?}"
                    )))
                });
                if let Err(e) = result {
                    tracing::error!("albums refresh failed: {}", e);
                }
            });
        }
    };
    // JoinHandle 故意丢弃——监听循环在进程生命周期内持续运行；
    // `RecommendedWatcher` 在循环退出时由 `drop` 自动释放底层 inotify 资源。
    let _watcher =
        crate::core::notify_watcher::start_watching(pool.clone(), vec![pictures], on_change);

    // 首屏先加载一页，剩余数据稍后分批追加，避免大图库启动时一次性构造所有 GTK 对象。
    let items = db::list_media_page(&pool, 0, INITIAL_MEDIA_PAGE_SIZE)?;
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    append_media_items(&list, items);
    load_remaining_media_pages(pool.clone(), list.clone(), INITIAL_MEDIA_PAGE_SIZE);
    Ok((list, thumbnail_loader, pool))
}

fn append_media_items(list: &gtk::gio::ListStore, items: Vec<MediaItem>) {
    if items.is_empty() {
        return;
    }

    let additions: Vec<glib::BoxedAnyObject> = items
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect();
    list.splice(list.n_items(), 0, &additions);
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
