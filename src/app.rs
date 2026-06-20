//! AdwApplication lifecycle management
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::sync::Arc;
use crate::core::init_pool;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::{MainWindow, PhotosPage};

pub fn build_app() -> adw::Application {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer")
        .build();

    app.connect_activate(|app| {
        let window = MainWindow::new(app);
        window.populate_sidebar();

        // 异步初始化 DB + 扫描
        let app_handle = app.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            match initialize().await {
                Ok((media_list, loader)) => {
                    let window: MainWindow = app_handle
                        .active_window()
                        .and_downcast::<MainWindow>()
                        .expect("MainWindow not found");
                    let nav = window.nav_view();
                    let photos = PhotosPage::new(media_list, loader);
                    // Inject the nav view so the PhotosPage can push a ViewerPage
                    // when a tile is clicked.
                    photos.set_nav_target(&nav);
                    nav.push(&photos);
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

async fn initialize() -> anyhow::Result<(gtk::gio::ListStore, Arc<ThumbnailLoader>)> {
    use crate::core::db;
    use crate::core::backend::scan_worker::spawn_scan;

    let data_dir = crate::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let pool = init_pool(&data_dir.join("photos.db"))?;

    // 缩略图加载器单例（M2-T1）
    let thumbnail_loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        crate::config::cache_dir(),
    ));
    thumbnail_loader.spawn_workers(4);

    // 启动后台扫描（M1 占位：扫描 ~/Pictures）
    // 从 $HOME 直接拼，不依赖 XDG 路径解析
    let home = std::env::var_os("HOME").expect("HOME not set");
    let pictures = std::path::PathBuf::from(home).join("Pictures");
    let paths = vec![pictures];
    let scan_handle = spawn_scan(pool.clone(), paths);

    // 同步等待扫描完成（M1 简单版；M5 可改为后台通知）
    let _ = scan_handle.await;

    // 加载所有数据 — 用 BoxedAnyObject 包装，让 MediaItem 可放入 gio::ListStore
    let items = db::list_all_media(&pool)?;
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in items {
        list.append(&glib::BoxedAnyObject::new(item));
    }
    Ok((list, thumbnail_loader))
}
