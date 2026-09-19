// Library root: exposes all public modules
pub mod app;
pub mod config;
pub mod core;
pub mod platform;
pub mod ui;

pub use core::error::AppError;

/// Register the compiled UI bundle once for both the application and tests.
pub fn ensure_resources_registered() {
    static REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    REGISTERED.get_or_init(|| {
        gtk4::gio::resources_register_include!("photo_viewer_resources.gresource")
            .expect("failed to register Photo Viewer resources");
    });
}
