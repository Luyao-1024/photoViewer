use std::fs;
use std::path::Path;

fn source(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("{path} should be readable: {err}"))
}

fn rust_sources_under(path: &Path, sources: &mut Vec<String>) {
    for entry in
        fs::read_dir(path).unwrap_or_else(|err| panic!("{path:?} should be readable: {err}"))
    {
        let entry = entry.unwrap_or_else(|err| panic!("directory entry should be readable: {err}"));
        let entry_path = entry.path();
        if entry_path.is_dir() {
            rust_sources_under(&entry_path, sources);
        } else if entry_path.extension().is_some_and(|ext| ext == "rs") {
            sources.push(entry_path.display().to_string());
        }
    }
}

fn assert_test_moved(root_path: &str, target_path: &str, test_name: &str) {
    let root = source(root_path);
    assert!(
        !root.contains(&format!("fn {test_name}(")),
        "{test_name} should move out of {root_path}"
    );

    assert!(
        Path::new(target_path).exists(),
        "{target_path} should exist"
    );
    let target = source(target_path);
    assert!(
        target.contains(&format!("fn {test_name}(")),
        "{target_path} should contain {test_name}"
    );
}

#[test]
fn src_rust_files_do_not_use_inline_test_modules() {
    let mut sources = Vec::new();
    rust_sources_under(Path::new("src"), &mut sources);

    let offenders = sources
        .into_iter()
        .filter(|path| {
            source(path)
                .lines()
                .any(|line| line.trim_start().starts_with("mod tests {"))
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "inline test modules should move to child test files: {offenders:#?}"
    );
}

#[test]
fn viewer_page_tests_live_with_viewer_modules() {
    for (target, tests) in [
        (
            "src/ui/viewer/filmstrip/tests.rs",
            &[
                "initial_window_centred_on_current_in_middle_of_album",
                "extend_at_window_cap_slides_right_without_growing_live_items",
                "filmstrip_thumbnail_width_is_clamped_to_reasonable_aspect_ratio",
                "thumb_centering_retries_until_allocation_is_ready",
            ][..],
        ),
        (
            "src/ui/viewer/crop/tests.rs",
            &[
                "crop_overlay_contain_rect_centers_letterboxed_image",
                "crop_overlay_drag_move_clamps_to_image_bounds",
                "crop_overlay_drag_corner_resizes_rect",
            ][..],
        ),
        (
            "src/ui/viewer/stage/tests.rs",
            &[
                "video_stage_click_toggles_above_builtin_controls",
                "viewer_preview_uses_medium_thumbnail",
                "animated_image_adds_half_second_pause_before_looping",
                "video_stage_reveals_only_for_current_prepared_stream",
            ][..],
        ),
        (
            "src/ui/viewer/navigation/tests.rs",
            &[
                "next_index_after_deleted_item_stays_in_bounds",
                "find_media_index_by_id_uses_item_identity",
                "current_media_item_stays_anchored_when_startup_scan_inserts_before_it",
            ][..],
        ),
        (
            "src/ui/viewer/fullscreen_window/tests.rs",
            &[
                "fullscreen_preview_opens_separate_window_without_changing_viewer_layout",
                "fullscreen_preview_reuses_existing_window_handle",
                "fullscreen_preview_close_disconnects_paintable_sync_handler",
            ][..],
        ),
        (
            "src/ui/viewer/transform/tests.rs",
            &[
                "zoom_step_clamps_to_viewer_limits",
                "zoom_pan_is_clamped_and_resets_at_identity",
                "reset_zoom_restores_identity_state",
            ][..],
        ),
    ] {
        for test_name in tests {
            assert_test_moved("src/ui/viewer_page.rs", target, test_name);
        }
    }
}

#[test]
fn media_grid_tests_live_with_grid_modules() {
    for (target, tests) in [
        (
            "src/ui/media_grid/selection/tests.rs",
            &["section_flowbox_selection_mode_tracks_multi_select"][..],
        ),
        (
            "src/ui/media_grid/updates/tests.rs",
            &[
                "grid_removes_backing_store_item_without_replacing_section_flow",
                "grid_inserts_same_section_item_without_replacing_existing_tiles",
                "grid_defers_uncached_incremental_insert_until_thumbnail_ready",
                "progressive_render_addition_appends_without_rebuilding_existing_tiles",
            ][..],
        ),
        (
            "src/ui/media_grid/loading/tests.rs",
            &[
                "inactive_grid_defers_initial_tile_build_until_activated",
                "day_grid_defers_stats_until_background_metadata_refresh",
                "metadata_invalidation_during_load_requests_followup_refresh",
                "completed_day_grid_stats_are_hidden",
            ][..],
        ),
        (
            "src/ui/media_grid/virtual_paging/tests.rs",
            &[
                "virtual_scroll_ratio_maps_to_full_library_offset",
                "virtual_spacer_height_scales_with_unloaded_items",
                "coalesced_virtual_page_target_keeps_latest_drag_target",
            ][..],
        ),
        (
            "src/ui/media_grid/viewport/tests.rs",
            &["thumbnail_request_window_includes_viewport_and_one_page_overscan"][..],
        ),
    ] {
        for test_name in tests {
            assert_test_moved("src/ui/media_grid.rs", target, test_name);
        }
    }
}

#[test]
fn window_tests_live_with_window_modules() {
    for (target, tests) in [
        (
            "src/ui/window/sidebar/tests.rs",
            &[
                "sidebar_album_snapshot_updates_stable_rows_in_place",
                "sidebar_album_snapshot_removes_missing_row_without_replacing_survivors",
                "sidebar_media_type_snapshot_removes_missing_row_without_replacing_survivors",
            ][..],
        ),
        (
            "src/ui/window/navigation/tests.rs",
            &[
                "keyboard_scope_is_viewer_when_viewer_page_is_visible",
                "ctrl_f_opens_search_from_photos_page",
                "ctrl_f_reuses_visible_search_page",
            ][..],
        ),
        (
            "src/ui/window/albums/tests.rs",
            &[
                "album_delete_pruning_removes_deleted_folder_rows_except_remaining_live_uris",
                "album_initial_load_limit_caps_large_albums_to_render_window",
                "album_backfill_limit_caps_large_albums_to_ui_window",
            ][..],
        ),
        (
            "src/ui/window/settings/tests.rs",
            &[
                "restart_spec_uses_current_executable_and_preserves_args",
                "settings_page_exposes_liquid_glass_transparency_slider",
                "settings_page_exposes_scan_path_management",
                "settings_storage_rows_defer_size_calculation",
            ][..],
        ),
    ] {
        for test_name in tests {
            assert_test_moved("src/ui/window.rs", target, test_name);
        }
    }
}

#[test]
fn single_file_modules_delegate_large_test_blocks() {
    for (root, target, tests) in [
        (
            "src/ui/grid_css.rs",
            "src/ui/grid_css/tests.rs",
            &[
                "viewer_media_surface_uses_theme_adaptive_background",
                "liquid_mode_keeps_drama_and_shared_parts",
                "glass_transparency_hundred_keeps_interactive_edges_visible",
                "segmented_glass_style_is_exposed_as_reusable_css_classes",
            ][..],
        ),
        (
            "src/core/thumbnails.rs",
            "src/core/thumbnails/tests.rs",
            &[
                "loader_returns_existing_disk_cache_without_queueing",
                "redirect_prewarm_to_offset_uses_live_media_offset_after_generated_rows",
                "jpeg_thumbnail_via_turbojpeg_fast_path",
            ][..],
        ),
        (
            "src/core/metadata.rs",
            "src/core/metadata/tests.rs",
            &[
                "oversized_heic_exif_item_is_recovered",
                "heic_dims_read_from_ispe_without_decode",
                "video_metadata_extracted",
                "exif_summary_from_jpeg_with_camera_fields",
            ][..],
        ),
        (
            "src/core/trash.rs",
            "src/core/trash/tests.rs",
            &[
                "move_to_trash_marked_survives_watcher_remove_event",
                "migrate_trashed_entries_moves_metadata_between_roots",
                "reconcile_prunes_trashed_row_absent_from_system_trash",
            ][..],
        ),
    ] {
        for test_name in tests {
            assert_test_moved(root, target, test_name);
        }
    }
}
