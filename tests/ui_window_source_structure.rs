use std::{fs, path::Path};

fn assert_no_top_level_super_glob(path: &str) {
    let source = fs::read_to_string(path).expect("module readable");
    let has_top_level_super_glob = source
        .lines()
        .take(12)
        .any(|line| line.trim() == "use super::*;");
    assert!(
        !has_top_level_super_glob,
        "{path} should use explicit imports instead of top-level `use super::*`"
    );
}

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
fn window_sidebar_stateful_flows_live_in_sidebar_module() {
    let module = fs::read_to_string("src/ui/window/sidebar.rs").expect("sidebar module readable");
    for marker in [
        "impl MainWindow",
        "pub fn populate_album_rows",
        "fn rebuild_album_rows",
        "fn apply_album_rows",
        "fn rebuild_media_type_rows",
        "fn apply_media_type_rows",
        "fn apply_sidebar_album_snapshot",
        "fn install_sidebar_layout_trace",
        "fn log_sidebar_layout_state",
        "pub fn toggle_albums_expanded",
        "pub fn toggle_media_types_expanded",
        "pub fn refresh_album_rows",
        "pub fn refresh_sidebar_snapshot_async",
    ] {
        assert!(module.contains(marker), "sidebar.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/window.rs").expect("window root readable");
    for marker in [
        "fn rebuild_album_rows(",
        "fn apply_album_rows(",
        "fn rebuild_media_type_rows(",
        "fn apply_media_type_rows(",
        "fn apply_sidebar_album_snapshot(",
        "fn install_sidebar_layout_trace(",
        "fn log_sidebar_layout_state(",
        "fn update_photos_count_label(",
        "fn refresh_sidebar_snapshot_async(",
    ] {
        assert!(
            !root.contains(marker),
            "sidebar stateful flow `{marker}` should move out of window.rs"
        );
    }
}

#[test]
fn window_navigation_flows_live_in_navigation_module() {
    let module_path = Path::new("src/ui/window/navigation.rs");
    assert!(
        module_path.exists(),
        "navigation flows should live in src/ui/window/navigation.rs"
    );
    let module = fs::read_to_string(module_path).expect("navigation module readable");
    for marker in [
        "impl MainWindow",
        "pub fn connect_sidebar",
        "fn schedule_album_open_from_sidebar",
        "pub(crate) fn open_album",
        "fn open_search_page",
        "pub fn refresh_visible_trash_page",
        "pub fn refresh_visible_album_detail_page",
    ] {
        assert!(module.contains(marker), "navigation.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/window.rs").expect("window root readable");
    assert!(root.contains("mod navigation;"));
    for marker in [
        "fn connect_sidebar(",
        "fn schedule_album_open_from_sidebar(",
        "fn open_album(",
        "fn open_search_page(",
        "fn refresh_visible_trash_page(",
        "fn refresh_visible_album_detail_page(",
    ] {
        assert!(
            !root.contains(marker),
            "navigation flow `{marker}` should move out of window.rs"
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

#[test]
fn window_split_modules_use_explicit_imports() {
    for path in [
        "src/ui/window/albums.rs",
        "src/ui/window/settings.rs",
        "src/ui/window/sidebar.rs",
        "src/ui/window/navigation.rs",
    ] {
        assert_no_top_level_super_glob(path);
    }
}

#[test]
fn window_uses_crossfade_browsing_stack() {
    let template = fs::read_to_string("data/ui/window.blp").expect("window template readable");
    assert!(
        template.contains("Gtk.Stack browsing_stack"),
        "window should own a dedicated browsing stack"
    );
    assert!(
        template.contains("transition-type: crossfade;"),
        "browsing stack should use the Photos mode transition"
    );
    assert!(
        template.contains("transition-duration: 200;"),
        "browsing stack should use the Photos mode transition duration"
    );

    let window = fs::read_to_string("src/ui/window.rs").expect("window source readable");
    for marker in [
        "pub browsing_stack: TemplateChild<gtk::Stack>",
        "pub fn show_photos_browsing_page",
        "pub fn show_album_browsing_page",
    ] {
        assert!(window.contains(marker), "window source missing `{marker}`");
    }
}

#[test]
fn album_open_uses_browsing_stack_instead_of_outer_push() {
    let navigation =
        fs::read_to_string("src/ui/window/navigation.rs").expect("navigation source readable");
    assert!(
        navigation.contains("self.show_album_browsing_page(&page)"),
        "album opening should select the inner browsing child"
    );
    let open_start = navigation
        .find("pub(crate) fn open_album")
        .expect("open_album should exist");
    let trash_start = navigation
        .find("fn show_trash_page")
        .expect("show_trash_page should exist");
    assert!(
        !navigation[open_start..trash_start].contains("nav_view.push(&page);"),
        "album opening should not push a page onto the outer navigation view"
    );

    let app = fs::read_to_string("src/app.rs").expect("app source readable");
    assert!(
        app.contains("window.show_photos_browsing_page(&photos);"),
        "Photos should be installed in the browsing stack at startup"
    );
}

#[test]
fn trash_has_navigation_back_button_but_album_detail_does_not() {
    let album = fs::read_to_string("data/ui/album-detail-page.blp")
        .expect("album detail template readable");
    assert!(
        !album.contains("back_btn") && !album.contains("go-previous-symbolic"),
        "album detail should not add a dedicated back button"
    );

    let trash = fs::read_to_string("data/ui/trash-page.blp").expect("trash template readable");
    assert!(
        trash.contains("show-back-button: true;"),
        "TrashPage should expose the NavigationView back button"
    );
}

#[test]
fn photos_and_album_search_buttons_share_header_start_position() {
    let photos = fs::read_to_string("data/ui/photos-page.blp").expect("Photos template readable");
    let album = fs::read_to_string("data/ui/album-detail-page.blp")
        .expect("album detail template readable");
    assert!(
        photos.find("Gtk.Button search_btn") < photos.find("Gtk.Revealer select_all_revealer"),
        "Photos search button should be declared before hidden selection revealers"
    );
    assert!(
        album.contains("Gtk.Button search_btn"),
        "Album detail should keep the same search button"
    );
}
