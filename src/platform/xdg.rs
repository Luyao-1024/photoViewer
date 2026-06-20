//! XDG Desktop Portal integration (V1 placeholder, full impl in M3)
use gtk4::glib;

pub async fn pick_folder() -> anyhow::Result<Option<std::path::PathBuf>> {
    // V1 placeholder: returns None
    Ok(None)
}

pub fn init() {
    // Future portal initialization reserved
    let _ = glib::user_config_dir();
}