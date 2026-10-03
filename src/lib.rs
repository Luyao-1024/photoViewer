// Library root: exposes all public modules
pub mod app;
pub mod config;
pub mod core;
pub mod platform;
pub mod ui;

pub use core::error::AppError;

/// Resource path the app's own icons live under, added to the GTK icon theme
/// so `GtkImage` can resolve them by icon name. Looked up through the theme
/// rather than loaded as a bitmap on purpose: only an icon resolved that way
/// is symbolic, and symbolic is what lets CSS `color` recolor it, which is how
/// the favorite mark takes white in the grid, the header foreground in the
/// viewer, and translucent red when a photo is already favorited.
pub(crate) const ICON_RESOURCE_PATH: &str = "/io/github/luyao_1024/photoviewer/icons";

/// Register the compiled UI bundle once for both the application and tests.
pub fn ensure_resources_registered() {
    static REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    REGISTERED.get_or_init(|| {
        gtk4::gio::resources_register_include!("photo_viewer_resources.gresource")
            .expect("failed to register Photo Viewer resources");
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::IconTheme::for_display(&display).add_resource_path(crate::ICON_RESOURCE_PATH);
        }
    });
}
