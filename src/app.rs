//! AdwApplication lifecycle management
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use crate::core::{init_pool, LocalBackend};
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
                Ok(media_list) => {
                    let window: MainWindow = app_handle
                        .active_window()
                        .and_downcast::<MainWindow>()
                        .expect("MainWindow not found");
                    let nav = window.nav_view();
                    let photos = PhotosPage::new(media_list);
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

async fn initialize() -> anyhow::Result<gtk::gio::ListStore> {
    use crate::core::db;

    let data_dir = crate::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let pool = init_pool(&data_dir.join("photos.db"))?;

    // 启动扫描（这里只触发，更新在 Task 11）
    let backend = LocalBackend::new(pool.clone());
    let _ = backend; // 暂时未使用，M1-T11 接入

    // 加载已有数据 — 用 BoxedAnyObject 包装，让 MediaItem 可放入 gio::ListStore
    let items = db::list_all_media(&pool)?;
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in items {
        list.append(&glib::BoxedAnyObject::new(item));
    }
    Ok(list)
}
