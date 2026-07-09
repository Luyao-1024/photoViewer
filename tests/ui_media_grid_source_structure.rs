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
fn media_grid_virtual_paging_helpers_live_in_virtual_paging_module() {
    let module_path = Path::new("src/ui/media_grid/virtual_paging.rs");
    assert!(
        module_path.exists(),
        "virtual paging helpers should live in src/ui/media_grid/virtual_paging.rs"
    );

    let module = fs::read_to_string(module_path).expect("virtual_paging module readable");
    for marker in [
        "pub(super) fn virtual_offset_for_ratio",
        "pub(super) fn virtual_page_start_for_offset",
        "pub(super) fn virtual_spacer_height",
        "pub(super) fn build_virtual_placeholder_flow",
    ] {
        assert!(
            module.contains(marker),
            "virtual_paging.rs missing `{marker}`"
        );
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    assert!(root.contains("mod virtual_paging;"));
    assert!(
        !root.contains("fn virtual_offset_for_ratio("),
        "virtual_offset_for_ratio should move out of media_grid.rs"
    );
}

#[test]
fn media_grid_render_helpers_live_in_render_module() {
    let module_path = Path::new("src/ui/media_grid/render.rs");
    assert!(
        module_path.exists(),
        "tile render helpers should live in src/ui/media_grid/render.rs"
    );

    let module = fs::read_to_string(module_path).expect("render module readable");
    for marker in [
        "pub(super) fn prepare_reused_tile",
        "pub(super) fn sync_flow_child_visibility_for_tile",
        "pub(super) fn build_photo_picture",
    ] {
        assert!(module.contains(marker), "render.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    assert!(root.contains("mod render;"));
    assert!(
        !root.contains("fn build_photo_picture("),
        "build_photo_picture should move out of media_grid.rs"
    );
}

#[test]
fn media_grid_selection_helpers_live_in_selection_module() {
    let module_path = Path::new("src/ui/media_grid/selection.rs");
    assert!(
        module_path.exists(),
        "selection helpers should live in src/ui/media_grid/selection.rs"
    );
    let module = fs::read_to_string(module_path).expect("selection module readable");
    for marker in [
        "impl MediaGrid",
        "pub fn selected_ids",
        "pub fn select_all",
        "pub fn set_multi_select_mode",
        "fn apply_selection_mode",
        "fn sync_visible_selection",
        "fn toggle_selection",
    ] {
        assert!(module.contains(marker), "selection.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    assert!(root.contains("mod selection;"));
    for marker in [
        "fn apply_selection_mode(",
        "fn sync_visible_selection(",
        "fn toggle_selection(",
    ] {
        assert!(
            !root.contains(marker),
            "selection helper `{marker}` should move out of media_grid.rs"
        );
    }
}

#[test]
fn media_grid_update_helpers_live_in_updates_module() {
    let module_path = Path::new("src/ui/media_grid/updates.rs");
    assert!(
        module_path.exists(),
        "incremental update helpers should live in src/ui/media_grid/updates.rs"
    );
    let module = fs::read_to_string(module_path).expect("updates module readable");
    for marker in [
        "impl MediaGrid",
        "fn apply_incremental_removal",
        "fn apply_incremental_addition",
        "fn apply_progressive_render_addition",
        "fn apply_incremental_addition_inner",
        "fn defer_incremental_addition_until_thumbnail_ready",
        "fn insert_deferred_incremental_item",
        "fn remove_displayed_child",
        "fn refresh_section_header_labels",
    ] {
        assert!(module.contains(marker), "updates.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    assert!(root.contains("mod updates;"));
    for marker in [
        "fn apply_incremental_removal(",
        "fn apply_incremental_addition(",
        "fn apply_progressive_render_addition(",
        "fn apply_incremental_addition_inner(",
        "fn defer_incremental_addition_until_thumbnail_ready(",
        "fn insert_deferred_incremental_item(",
        "fn remove_displayed_child(",
        "fn refresh_section_header_labels(",
    ] {
        assert!(
            !root.contains(marker),
            "update helper `{marker}` should move out of media_grid.rs"
        );
    }
}

#[test]
fn media_grid_model_change_orchestration_lives_in_updates_module() {
    let module =
        fs::read_to_string("src/ui/media_grid/updates.rs").expect("updates module readable");
    for marker in [
        "pub(super) fn connect_model_changes",
        "fn handle_items_changed",
        "GRID_MODEL_TRACE model_changed",
        "action=incremental_removal",
        "action=incremental_addition",
        "action=schedule_rebuild",
    ] {
        assert!(module.contains(marker), "updates.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    for marker in [
        "connect_items_changed(move |list, position, removed, added|",
        "GRID_MODEL_TRACE model_changed",
        "fn handle_items_changed(",
    ] {
        assert!(
            !root.contains(marker),
            "model-change orchestration `{marker}` should move out of media_grid.rs"
        );
    }
}

#[test]
fn media_grid_viewport_orchestration_lives_in_viewport_module() {
    let module_path = Path::new("src/ui/media_grid/viewport.rs");
    assert!(
        module_path.exists(),
        "viewport orchestration should live in src/ui/media_grid/viewport.rs"
    );
    let module = fs::read_to_string(module_path).expect("viewport module readable");
    for marker in [
        "impl MediaGrid",
        "pub(super) fn connect_scroll_handlers",
        "fn collect_visible_cache_keys",
        "pub fn reprioritize_visible",
        "fn schedule_reprioritize",
    ] {
        assert!(module.contains(marker), "viewport.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    assert!(root.contains("mod viewport;"));
    for marker in [
        "fn collect_visible_cache_keys(",
        "fn schedule_reprioritize(",
        "connect_value_changed(move |adj|",
    ] {
        assert!(
            !root.contains(marker),
            "viewport orchestration `{marker}` should move out of media_grid.rs"
        );
    }
}

#[test]
fn media_grid_loading_helpers_live_in_loading_module() {
    let module_path = Path::new("src/ui/media_grid/loading.rs");
    assert!(
        module_path.exists(),
        "loading helpers should live in src/ui/media_grid/loading.rs"
    );
    let module = fs::read_to_string(module_path).expect("loading module readable");
    for marker in [
        "impl MediaGrid",
        "fn try_expand_render_limit",
        "fn try_load_virtual_page",
        "fn spawn_virtual_page_query",
        "fn start_pending_virtual_page_query",
        "fn schedule_progressive_render_fill",
        "fn invalidate_progressive_render_fill",
        "fn ensure_library_metadata_async",
        "fn invalidate_library_metadata",
        "fn start_stats_refresh",
        "fn schedule_rebuild",
        "fn rebuild_immediately",
    ] {
        assert!(module.contains(marker), "loading.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    assert!(root.contains("mod loading;"));
    for marker in [
        "fn try_expand_render_limit(",
        "fn try_load_virtual_page(",
        "fn spawn_virtual_page_query(",
        "fn start_pending_virtual_page_query(",
        "fn schedule_progressive_render_fill(",
        "fn invalidate_progressive_render_fill(",
        "fn ensure_library_metadata_async(",
        "fn invalidate_library_metadata(",
        "fn start_stats_refresh(",
        "fn schedule_rebuild(",
        "fn rebuild_immediately(",
    ] {
        assert!(
            !root.contains(marker),
            "loading helper `{marker}` should move out of media_grid.rs"
        );
    }
}

#[test]
fn media_grid_split_modules_use_explicit_imports() {
    for path in [
        "src/ui/media_grid/selection.rs",
        "src/ui/media_grid/updates.rs",
        "src/ui/media_grid/loading.rs",
        "src/ui/media_grid/viewport.rs",
    ] {
        assert_no_top_level_super_glob(path);
    }
}
