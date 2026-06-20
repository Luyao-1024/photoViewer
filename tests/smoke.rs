// Smoke test: application can be built and exits immediately
#[test]
fn app_builds_without_panic() {
    // Initialize GTK in test mode (no display required)
    gtk4::init().expect("GTK init failed");
    let _app = photo_viewer::app::build_app();
    // Do not call run() — only verify build doesn't panic
}