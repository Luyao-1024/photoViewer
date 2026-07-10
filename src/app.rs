//! AdwApplication lifecycle management
use crate::core::db::DbPool;
use crate::core::error::Result as CoreResult;
use crate::core::events::DomainEvent;
#[cfg(test)]
use crate::core::init_pool;
use crate::core::media::MediaItem;
use crate::core::runtime_config;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::apply_to_media_list::ui_media_list_cap;
use crate::ui::{theme, MainWindow, PhotosPage};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::APP_ID;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

static TOKIO: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

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

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| {
        theme::apply(crate::core::prefs::theme_preference());

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
                Ok((media_list, loader, pool, change_rx, db_actor, db_event_rx)) => {
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
                    photos.set_db_actor(db_actor.clone());
                    nav.push(&photos);

                    // Store DB pool + loader on the window so the sidebar can
                    // build album detail / trash pages on demand, then wire
                    // row-selected to push them onto nav_view.
                    window.set_resources(pool, loader, media_list.clone());
                    window.set_db_actor(db_actor.clone());
                    window.connect_sidebar(&nav);
                    // Heavy sidebar projections (album rows, per-album counts,
                    // and the true live-media total) are loaded after the
                    // Photos page is usable so startup is not gated by COUNT /
                    // GROUP BY queries.
                    window.refresh_sidebar_snapshot_async();

                    // Consumer: GTK 主线程独占 media_list 写权限，所以 spawn_local
                    // 排空 change_rx。Upserted/Removed → 同步到 media_list；
                    // TrashChanged → 刷新当前可见的回收站页面（文件管理器改了回收站
                    // 后无需切换页面即可看到）。
                    let window_for_consumer = window.downgrade();
                    let album_refresh = crate::core::refresh::RefreshCoordinator::new(
                        db_actor.clone(),
                        Rc::new({
                            let window = window.downgrade();
                            move || {
                                if let Some(window) = window.upgrade() {
                                    tracing::debug!(
                                        target: crate::core::log_targets::BROWSING,
                                        "SIDEBAR_TRACE album_refresh_callback_refresh_sidebar_snapshot"
                                    );
                                    window.refresh_sidebar_snapshot_async();
                                }
                            }
                        }),
                    );
                    let refresh_hub = crate::ui::refresh_hub::UiRefreshHub::new();
                    refresh_hub.subscribe(
                        crate::ui::refresh_hub::UiRefreshScope::Photos,
                        Rc::new({
                            let media_list = media_list.clone();
                            let window_for_consumer = window_for_consumer.clone();
                            let album_refresh = album_refresh.clone();
                            move |event| {
                                apply_domain_event_to_legacy_ui(
                                    event,
                                    &media_list,
                                    &window_for_consumer,
                                    &album_refresh,
                                );
                            }
                        }),
                    );

                    gtk::glib::MainContext::default().spawn_local({
                        let refresh_hub = refresh_hub.clone();
                        async move {
                            let mut rx = change_rx;
                            while let Some(event) = rx.recv().await {
                                refresh_hub.dispatch(&event);
                            }
                        }
                    });
                    gtk::glib::MainContext::default().spawn_local({
                        let refresh_hub = refresh_hub.clone();
                        async move {
                            let mut rx = db_event_rx;
                            while let Some(event) = rx.recv().await {
                                refresh_hub.dispatch(&event);
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

fn domain_event_label(event: &DomainEvent) -> String {
    match event {
        DomainEvent::MediaUpserted { source, items } => {
            format!("media_upserted({source:?}, {})", items.len())
        }
        DomainEvent::MediaRemoved { uris, .. } => format!("media_removed({})", uris.len()),
        DomainEvent::MediaUpdated { source, items, .. } => {
            format!("media_updated({source:?}, {})", items.len())
        }
        DomainEvent::MediaMovedToTrash { source, items } => {
            format!("media_moved_to_trash({source:?}, {})", items.len())
        }
        DomainEvent::MediaRestored { source, items } => {
            format!("media_restored({source:?}, {})", items.len())
        }
        DomainEvent::TrashChanged { .. } => "trash_changed".to_string(),
        DomainEvent::AlbumsChanged {
            source,
            affected_folders,
            affected_virtual,
            ..
        } => {
            format!(
                "albums_changed({source:?}, folders={}, virtual={})",
                affected_folders.len(),
                affected_virtual.len()
            )
        }
        DomainEvent::AlbumCoverChanged { .. } => "album_cover_changed".to_string(),
        DomainEvent::AlbumsDirty { source } => format!("albums_dirty({source:?})"),
        DomainEvent::ThumbnailStatsDirty => "thumbnail_stats_dirty".to_string(),
        DomainEvent::LiveCountDirty => "live_count_dirty".to_string(),
    }
}

fn apply_domain_event_to_legacy_ui(
    event: &DomainEvent,
    media_list: &gtk::gio::ListStore,
    window: &glib::WeakRef<MainWindow>,
    album_refresh: &crate::core::refresh::RefreshCoordinator,
) {
    let event_label = domain_event_label(event);
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "SIDEBAR_TRACE domain_event_received event={} media_list_len={}",
        event_label,
        media_list.n_items()
    );
    match event {
        DomainEvent::TrashChanged { .. } => {
            if let Some(window) = window.upgrade() {
                window.refresh_visible_trash_page();
            }
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE schedule_album_refresh reason=trash_changed"
            );
            album_refresh.mark_albums_dirty_async();
        }
        DomainEvent::AlbumsChanged { .. } | DomainEvent::AlbumCoverChanged { .. } => {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE schedule_album_refresh reason={}",
                event_label
            );
            album_refresh.mark_albums_dirty_async();
        }
        _ => {
            let is_startup_scan_batch = matches!(
                event,
                DomainEvent::MediaUpserted {
                    source: crate::core::events::ChangeSource::StartupScan,
                    ..
                }
            );
            let list_len_before = media_list.n_items();
            crate::ui::apply_to_media_list::apply_to_media_list(media_list, event);
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "UI_CHANGE_APPLY event={} list_len_before={} list_len_after={}",
                event_label,
                list_len_before,
                media_list.n_items()
            );
            if !is_startup_scan_batch {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE schedule_album_refresh reason={} after_media_list_apply",
                    event_label
                );
                album_refresh.mark_albums_dirty_async();
            } else {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "startup scan batch applied; album refresh deferred"
                );
            }
        }
    }

    if visible_album_detail_should_refresh(event) {
        if let Some(window) = window.upgrade() {
            window.refresh_visible_album_detail_page();
        }
    }
}

fn visible_album_detail_should_refresh(event: &DomainEvent) -> bool {
    matches!(
        event,
        DomainEvent::MediaUpserted { .. }
            | DomainEvent::MediaRemoved { .. }
            | DomainEvent::MediaMovedToTrash { .. }
            | DomainEvent::MediaRestored { .. }
            | DomainEvent::MediaUpdated { .. }
    )
}

async fn initialize() -> anyhow::Result<(
    gtk::gio::ListStore,
    Arc<ThumbnailLoader>,
    DbPool,
    tokio::sync::mpsc::UnboundedReceiver<crate::core::events::DomainEvent>,
    crate::core::db_actor::DbActorHandle,
    tokio::sync::mpsc::UnboundedReceiver<crate::core::events::DomainEvent>,
)> {
    let data_dir = crate::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    if let Err(err) =
        gtk::gio::spawn_blocking(crate::core::trash::ensure_startup_trash_backend).await
    {
        tracing::warn!("startup trash backend probe worker failed: {err:?}");
    }
    let db_path = data_dir.join("photos.db");
    let initial_media_page_size = runtime_config::initial_media_page_size();
    let pictures = crate::config::pictures_dir();
    let (pool, items, db_actor, db_event_rx) =
        initialize_db_once_with_retry(db_path.clone(), initial_media_page_size, pictures.clone())
            .await?;

    // 缩略图加载器单例（M2-T1）
    let thumbnail_loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        crate::config::cache_dir(),
    ));
    thumbnail_loader.set_db_actor(db_actor.clone());
    thumbnail_loader.spawn_workers(runtime_config::thumbnail_worker_count());

    let media_roots = crate::config::media_roots();

    // 启动文件监听（M5-T5+）：监听媒体根的后续变更并增量 upsert。
    // 通过 `MediaChangeNotifier` 把"哪个 MediaItem 变了"推给 GTK 主线程
    // 消费者；消费者按 uri 在共享的 `media_list` 上做 splice/append/remove。
    //
    // 同时监听系统回收站根：文件管理器对回收站的还原/清空/删除只动回收站目录，
    // 必须单独监听才能实时感知（见 notify_watcher 的防抖对账）。
    let (_notifier, change_rx) = crate::core::media_change_notifier::MediaChangeNotifier::new();
    let trash_roots = crate::core::trash::trash_roots();
    let excluded_scan_roots = crate::core::prefs::excluded_scan_roots();
    let mut watch_paths = media_roots.clone();
    watch_paths.extend(trash_roots.iter().filter(|r| r.exists()).cloned());
    let _watcher = crate::core::notify_watcher::start_watching(
        db_actor.clone(),
        watch_paths,
        trash_roots,
        excluded_scan_roots,
        pictures.clone(),
    );

    // 首屏只加载一页，让窗口尽快可操作；之后由照片网格按全库滚动比例
    // 从 DB 换入当前窗口，避免启动时把超大图库全部灌进 GTK 模型。
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    append_media_items(&list, items);
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "STARTUP_INITIAL_PAGE list_len={} cap={} page_size={}",
        list.n_items(),
        ui_media_list_cap(),
        initial_media_page_size
    );
    start_background_startup_work(
        pool.clone(),
        media_roots,
        pictures,
        db_actor.clone(),
        list.clone(),
        initial_media_page_size,
        thumbnail_loader.clone(),
    );

    // change_rx 交给 activate 处的消费者：那里能拿到 MainWindow，TrashChanged 时
    // 可以刷新可见的回收站页面（相册列表的 Upserted/Removed 也由它应用到 media_list）。
    Ok((
        list,
        thumbnail_loader,
        pool,
        change_rx,
        db_actor,
        db_event_rx,
    ))
}

async fn initialize_db_once_with_retry(
    path: PathBuf,
    page_size: u32,
    pictures: PathBuf,
) -> anyhow::Result<(
    DbPool,
    Vec<MediaItem>,
    crate::core::db_actor::DbActorHandle,
    tokio::sync::mpsc::UnboundedReceiver<crate::core::events::DomainEvent>,
)> {
    let first = match gtk::gio::spawn_blocking({
        let path = path.clone();
        let pictures = pictures.clone();
        move || -> CoreResult<_> {
            let pool = crate::core::init_pool(&path)?;
            let (sender, receiver) = crate::core::events::DomainEventSender::new();
            let db_actor = crate::core::db_actor::start_db_actor(pool.clone(), sender);
            db_actor.execute_blocking(crate::core::db_actor::DbCommand::ReconcileTrash {
                pictures_root: pictures,
            })?;
            let items = crate::core::repository::MediaRepository::new(pool.clone()).items(
                crate::core::repository::MediaQuery::LiveAll,
                0,
                page_size,
            )?;
            Ok((pool, items, db_actor, receiver))
        }
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
            let pictures = pictures.clone();
            move || -> CoreResult<_> {
                let pool = crate::core::init_pool(&path)?;
                let (sender, receiver) = crate::core::events::DomainEventSender::new();
                let db_actor = crate::core::db_actor::start_db_actor(pool.clone(), sender);
                db_actor.execute_blocking(crate::core::db_actor::DbCommand::ReconcileTrash {
                    pictures_root: pictures,
                })?;
                let items = crate::core::repository::MediaRepository::new(pool.clone()).items(
                    crate::core::repository::MediaQuery::LiveAll,
                    0,
                    page_size,
                )?;
                Ok((pool, items, db_actor, receiver))
            }
        })
        .await
        {
            Ok(r) => r,
            Err(e) => return Err(anyhow::anyhow!("spawn_blocking join error: {:?}", e)),
        };
        second.map_err(anyhow::Error::from)
    }
}

#[cfg(test)]
fn initialize_db_once_blocking_with_preload<F>(
    path: PathBuf,
    page_size: u32,
    preload: F,
) -> CoreResult<(DbPool, Vec<MediaItem>)>
where
    F: FnOnce(&DbPool) -> CoreResult<()>,
{
    let pool = crate::core::init_pool(&path)?;
    preload(&pool)?;
    let items = crate::core::repository::MediaRepository::new(pool.clone()).items(
        crate::core::repository::MediaQuery::LiveAll,
        0,
        page_size,
    )?;
    Ok((pool, items))
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

fn start_background_startup_work(
    pool: DbPool,
    media_roots: Vec<std::path::PathBuf>,
    pictures: std::path::PathBuf,
    db_actor: crate::core::db_actor::DbActorHandle,
    _list: gtk::gio::ListStore,
    _remaining_offset: u32,
    loader: Arc<ThumbnailLoader>,
) {
    glib::MainContext::default().spawn_local(async move {
        if let Err(e) = crate::core::bootstrap::scan_and_aggregate_with_actor(
            &pool,
            &media_roots,
            db_actor.clone(),
        )
        .await
        {
            tracing::error!("后台扫描失败: {}", e);
        }

        if let Err(e) = db_actor
            .execute(crate::core::db_actor::DbCommand::ReconcileTrash {
                pictures_root: pictures.clone(),
            })
            .await
        {
            tracing::warn!("回收站对账失败: {e}");
        }

        // 扫描 + 回收站对账完毕后即可启动后台缩略图预热。剩余 DB 分页可能在
        // 大图库里很慢；预热是拉模型且 worker 总是先处理视口队列，不需要等分页。
        crate::core::thumbnail_prewarm::start_background_prewarm(&loader);
    });
}

#[cfg(test)]
mod tests;
