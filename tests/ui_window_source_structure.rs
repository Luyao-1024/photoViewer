use std::{fs, path::Path};

#[test]
fn window_settings_helpers_live_in_settings_module() {
    let module_path = Path::new("src/ui/window/settings.rs");
    assert!(
        module_path.exists(),
        "settings helpers should live in src/ui/window/settings.rs"
    );
    let module = fs::read_to_string(module_path).expect("settings module readable");
    for marker in [
        "impl MainWindow",
        "pub(super) fn build_settings_page",
        "pub(super) fn build_settings_dialog",
        "pub(super) fn build_trash_settings_group",
        "pub(super) fn add_close_on_backdrop_click",
        "pub(super) fn update_trash_settings_state",
        "pub(super) fn show_settings_error_dialog",
        "pub(super) fn persist_locale",
        "fn build_scan_paths_group",
        "fn add_scan_path_section",
        "fn add_scan_path_value_row",
        "fn choose_scan_folder",
        "fn append_scan_path",
        "fn remove_scan_path",
        "fn restart_spec_from",
        "fn current_restart_spec",
        "fn restart_application",
        "fn update_storage_size_async",
        "fn show_clear_confirm_dialog",
        "fn show_clear_success_toast",
        "fn show_clear_error_toast",
        "fn format_size",
    ] {
        assert!(module.contains(marker), "settings.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/window.rs").expect("window root readable");
    assert!(
        !root.contains("fn build_settings_page("),
        "build_settings_page should move out of window.rs"
    );
    assert!(
        !root.contains("fn build_trash_settings_group("),
        "build_trash_settings_group should move out of window.rs"
    );
    for marker in [
        "fn build_settings_dialog(",
        "fn build_scan_paths_group(",
        "fn add_scan_path_section(",
        "fn add_scan_path_value_row(",
        "fn choose_scan_folder",
        "fn append_scan_path(",
        "fn remove_scan_path(",
        "fn restart_spec_from(",
        "fn current_restart_spec(",
        "fn restart_application(",
        "fn update_storage_size_async",
        "fn show_clear_confirm_dialog",
        "fn show_clear_success_toast(",
        "fn show_clear_error_toast(",
        "fn format_size(",
    ] {
        assert!(
            !root.contains(marker),
            "settings helper `{marker}` should move out of window.rs"
        );
    }
}

#[test]
fn window_sidebar_helpers_live_in_sidebar_module() {
    let module_path = Path::new("src/ui/window/sidebar.rs");
    assert!(
        module_path.exists(),
        "sidebar helpers should live in src/ui/window/sidebar.rs"
    );
    let module = fs::read_to_string(module_path).expect("sidebar module readable");
    for marker in [
        "pub(super) fn build_nav_row",
        "pub(super) fn build_albums_header_row",
        "pub(super) fn build_album_row",
        "pub(super) fn update_album_row_in_place",
        "pub(super) fn same_sidebar_album_identities",
        "pub(super) fn sidebar_album_summary",
        "pub(super) fn build_sidebar_album_cover",
        "pub(crate) fn refresh_albums_sidebar",
    ] {
        assert!(module.contains(marker), "sidebar.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/window.rs").expect("window root readable");
    for marker in [
        "fn build_nav_row(",
        "fn build_albums_header_row(",
        "fn build_album_row(",
        "fn update_album_row_in_place(",
    ] {
        assert!(
            !root.contains(marker),
            "sidebar row helper `{marker}` should move out of window.rs"
        );
    }
}

#[test]
fn window_album_flow_helpers_live_in_albums_module() {
    let module_path = Path::new("src/ui/window/albums.rs");
    assert!(
        module_path.exists(),
        "album flow helpers should live in src/ui/window/albums.rs"
    );
    let module = fs::read_to_string(module_path).expect("albums module readable");
    for marker in [
        "impl MainWindow",
        "pub(super) fn album_initial_load_limit",
        "pub(super) fn album_backfill_fetch_limit",
        "pub(super) fn build_album_context_menu_items",
        "pub(crate) fn refresh_after_album_operation",
        "pub(super) fn ignore_album_worker",
        "pub(super) fn delete_albums_to_trash_worker",
        "pub(super) struct AlbumDeleteUiResult",
        "pub(super) fn remove_deleted_album_media_from_media_list",
    ] {
        assert!(module.contains(marker), "albums.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/window.rs").expect("window root readable");
    for marker in [
        "fn attach_album_dnd(",
        "fn attach_album_context_menu(",
        "fn confirm_delete_album(",
        "fn confirm_ignore_album(",
        "fn delete_albums_to_trash_ui(",
        "fn prompt_album_trash_backend_fallback(",
        "fn build_album_context_menu_items(",
        "struct AlbumDeleteUiResult",
        "fn remove_deleted_album_media_from_media_list(",
    ] {
        assert!(
            !root.contains(marker),
            "album flow helper `{marker}` should move out of window.rs"
        );
    }
}
