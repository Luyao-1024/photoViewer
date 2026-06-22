use gtk4::gio;
use gtk4::prelude::ApplicationExtManual;

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Register GResource (must be before any GTK operations)
    gio::resources_register_include!("photo_viewer_resources.gresource")
        .expect("Failed to register resources");

    // Ensure XDG directories exist
    std::fs::create_dir_all(photo_viewer::config::data_dir())?;
    std::fs::create_dir_all(photo_viewer::config::cache_dir())?;

    let app = photo_viewer::app::build_app();
    let empty: Vec<String> = vec![];
    app.run_with_args(&empty);

    Ok(())
}
