//! AdwApplication lifecycle management
use libadwaita as adw;
use libadwaita::prelude::*;

pub fn build_app() -> adw::Application {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer")
        .build();

    app.connect_activate(|_app| {
        // M1 placeholder: just log
        tracing::info!("Photo Viewer activated");
    });

    app
}