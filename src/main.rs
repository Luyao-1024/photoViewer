use gtk4::gio;
use gtk4::prelude::ApplicationExtManual;

fn main() -> anyhow::Result<()> {
    // Diagnostics first: file logging (`app.log`), Rust panic hook, GLib/GTK
    // log redirect, and the native signal handler. Runs before anything that
    // can panic or log, and owns the tracing subscriber init (stderr + file).
    // See `core::diagnostics`.
    photo_viewer::core::diagnostics::init()?;

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
