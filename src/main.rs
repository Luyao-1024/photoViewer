use gtk4::prelude::ApplicationExtManual;

fn main() -> anyhow::Result<()> {
    // Keep file logging and the optional performance trace alive through exit.
    let _diagnostics = photo_viewer::core::diagnostics::init()?;
    let startup_trace = photo_viewer::core::telemetry::OperationTrace::start(
        photo_viewer::core::telemetry::TraceChain::Startup,
        "application_start",
    );

    // Register GResource (must be before any GTK operations)
    {
        let _stage = startup_trace.stage("register_resources");
        photo_viewer::ensure_resources_registered();
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
