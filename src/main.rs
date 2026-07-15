use gtk4::gio;
use gtk4::prelude::ApplicationExtManual;

fn main() -> anyhow::Result<()> {
    // Diagnostics first: file logging (`app.log`), Rust panic hook, GLib/GTK
    // log redirect, and the native signal handler. Runs before anything that
    // can panic or log, and owns the tracing subscriber init (stderr + file).
    // See `core::diagnostics`.
    //
    // `_chrome_flush_guard` is None unless PHOTOVIEWER_CHROME_TRACE or
    // PHOTOVIEWER_TRACE_CHAINS is set. Holding it until `main` returns
    // finalizes `<logs>/trace.json`.
    let _chrome_flush_guard = photo_viewer::core::diagnostics::init()?;
    let startup_trace = photo_viewer::core::telemetry::OperationTrace::start(
        photo_viewer::core::telemetry::TraceChain::Startup,
        "application_start",
    );

    // Register GResource (must be before any GTK operations)
    {
        let _stage = startup_trace.stage("register_resources");
        gio::resources_register_include!("photo_viewer_resources.gresource").unwrap_or_else(
            |error| {
                photo_viewer::core::telemetry::log_error(
                    &startup_trace,
                    "register_resources",
                    &error,
                );
                panic!("Failed to register resources: {error}");
            },
        );
    }

    // Ensure XDG directories exist
    for (stage, directory) in [
        ("data_directory", photo_viewer::config::data_dir()),
        ("cache_directory", photo_viewer::config::cache_dir()),
    ] {
        let _stage = startup_trace.stage(stage);
        if let Err(error) = std::fs::create_dir_all(directory) {
            photo_viewer::core::telemetry::log_error(&startup_trace, stage, &error);
            return Err(error.into());
        }
    }

    let app = photo_viewer::app::build_app_with_startup_trace(startup_trace);
    let empty: Vec<String> = vec![];
    app.run_with_args(&empty);

    Ok(())
}
