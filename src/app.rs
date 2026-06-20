//! AdwApplication lifecycle management
use libadwaita as adw;
use libadwaita::prelude::*;
use crate::ui::MainWindow;

pub fn build_app() -> adw::Application {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer")
        .build();

    app.connect_activate(|app| {
        let window = MainWindow::new(app);
        window.populate_sidebar();
        window.present();
    });

    app
}