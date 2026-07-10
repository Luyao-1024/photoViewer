# Test Module Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce oversized source files by moving large inline `#[cfg(test)] mod tests` blocks into focused module-owned test files, without changing runtime behavior.

**Architecture:** Keep production APIs and module boundaries stable. Each task moves tests to the module that owns the behavior under test, adds source-structure guardrails so root files do not accumulate unrelated inline tests again, and runs the smallest targeted Cargo test first.

**Tech Stack:** Rust, GTK4, Libadwaita, Cargo unit tests, Cargo integration tests.

## Global Constraints

- This is a test-ownership migration only; do not refactor production behavior while moving tests.
- Keep `src/core/` independent from GTK UI concerns.
- Do not edit generated `data/ui/*.ui` output.
- Do not revert user changes or unrelated worktree changes.
- Preserve all existing test names unless a name has to change because it conflicts after moving.
- Prefer `#[cfg(test)] mod tests;` plus sibling `tests.rs` files for files that are still single Rust modules.
- Prefer putting tests in existing focused modules when the tested function already lives there.
- Keep integration tests under `tests/` when they assert cross-module source structure or user-visible template behavior.
- Run the smallest useful verification first, then broaden to related module tests.

---

### Task 1: Add Test Ownership Guardrails

**Files:**
- Create: `tests/inline_test_ownership.rs`
- Test: `tests/inline_test_ownership.rs`

**Interfaces:**
- Consumes: existing source files and module layout.
- Produces: a source-structure integration test that fails while oversized root files still own tests that belong to focused modules.

- [ ] **Step 1: Write the failing ownership test**

Create `tests/inline_test_ownership.rs` with:

```rust
use std::fs;
use std::path::Path;

fn source(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("{path} should be readable: {err}"))
}

fn assert_test_moved(root_path: &str, target_path: &str, test_name: &str) {
    let root = source(root_path);
    assert!(
        !root.contains(&format!("fn {test_name}(")),
        "{test_name} should move out of {root_path}"
    );

    assert!(Path::new(target_path).exists(), "{target_path} should exist");
    let target = source(target_path);
    assert!(
        target.contains(&format!("fn {test_name}(")),
        "{target_path} should contain {test_name}"
    );
}

#[test]
fn viewer_page_tests_live_with_viewer_modules() {
    for (target, tests) in [
        (
            "src/ui/viewer/filmstrip.rs",
            &[
                "initial_window_centred_on_current_in_middle_of_album",
                "extend_at_window_cap_slides_right_without_growing_live_items",
                "filmstrip_thumbnail_width_is_clamped_to_reasonable_aspect_ratio",
                "thumb_centering_retries_until_allocation_is_ready",
            ][..],
        ),
        (
            "src/ui/viewer/crop.rs",
            &[
                "crop_overlay_contain_rect_centers_letterboxed_image",
                "crop_overlay_drag_move_clamps_to_image_bounds",
                "crop_overlay_drag_corner_resizes_rect",
            ][..],
        ),
        (
            "src/ui/viewer/stage.rs",
            &[
                "video_stage_click_toggles_above_builtin_controls",
                "viewer_preview_uses_medium_thumbnail",
                "animated_image_adds_half_second_pause_before_looping",
                "video_stage_reveals_only_for_current_prepared_stream",
            ][..],
        ),
        (
            "src/ui/viewer/navigation.rs",
            &[
                "next_index_after_deleted_item_stays_in_bounds",
                "find_media_index_by_id_uses_item_identity",
                "current_media_item_stays_anchored_when_startup_scan_inserts_before_it",
            ][..],
        ),
        (
            "src/ui/viewer/fullscreen_window.rs",
            &[
                "fullscreen_preview_opens_separate_window_without_changing_viewer_layout",
                "fullscreen_preview_reuses_existing_window_handle",
                "fullscreen_preview_close_disconnects_paintable_sync_handler",
            ][..],
        ),
        (
            "src/ui/viewer/transform.rs",
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
            "src/ui/media_grid/selection.rs",
            &["section_flowbox_selection_mode_tracks_multi_select"][..],
        ),
        (
            "src/ui/media_grid/updates.rs",
            &[
                "grid_removes_backing_store_item_without_replacing_section_flow",
                "grid_inserts_same_section_item_without_replacing_existing_tiles",
                "grid_defers_uncached_incremental_insert_until_thumbnail_ready",
                "progressive_render_addition_appends_without_rebuilding_existing_tiles",
            ][..],
        ),
        (
            "src/ui/media_grid/loading.rs",
            &[
                "inactive_grid_defers_initial_tile_build_until_activated",
                "day_grid_defers_stats_until_background_metadata_refresh",
                "metadata_invalidation_during_load_requests_followup_refresh",
                "completed_day_grid_stats_are_hidden",
            ][..],
        ),
        (
            "src/ui/media_grid/virtual_paging.rs",
            &[
                "virtual_scroll_ratio_maps_to_full_library_offset",
                "virtual_spacer_height_scales_with_unloaded_items",
                "coalesced_virtual_page_target_keeps_latest_drag_target",
            ][..],
        ),
        (
            "src/ui/media_grid/viewport.rs",
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
            "src/ui/window/sidebar.rs",
            &[
                "sidebar_album_snapshot_updates_stable_rows_in_place",
                "sidebar_album_snapshot_removes_missing_row_without_replacing_survivors",
                "sidebar_media_type_snapshot_removes_missing_row_without_replacing_survivors",
            ][..],
        ),
        (
            "src/ui/window/navigation.rs",
            &[
                "keyboard_scope_is_viewer_when_viewer_page_is_visible",
                "ctrl_f_opens_search_from_photos_page",
                "ctrl_f_reuses_visible_search_page",
            ][..],
        ),
        (
            "src/ui/window/albums.rs",
            &[
                "album_delete_pruning_removes_deleted_folder_rows_except_remaining_live_uris",
                "album_initial_load_limit_caps_large_albums_to_render_window",
                "album_backfill_limit_caps_large_albums_to_ui_window",
            ][..],
        ),
        (
            "src/ui/window/settings.rs",
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
```

- [ ] **Step 2: Run the ownership test to verify RED**

Run:

```bash
cargo test --test inline_test_ownership
```

Expected: FAIL with messages naming tests that still live in root files.

---

### Task 2: Move Viewer Tests To Viewer Modules

**Files:**
- Modify: `src/ui/viewer_page.rs`
- Modify: `src/ui/viewer/filmstrip.rs`
- Modify: `src/ui/viewer/crop.rs`
- Modify: `src/ui/viewer/stage.rs`
- Modify: `src/ui/viewer/navigation.rs`
- Modify: `src/ui/viewer/fullscreen_window.rs`
- Modify: `src/ui/viewer/details.rs`
- Modify: `src/ui/viewer/editor.rs`
- Modify: `src/ui/viewer/transform.rs`
- Test: `tests/inline_test_ownership.rs`
- Test: `tests/ui_viewer_source_structure.rs`

**Interfaces:**
- Consumes: existing private viewer helper functions already in focused modules.
- Produces: `src/ui/viewer_page.rs` with only viewer-shell tests that need the full composite widget root.

- [ ] **Step 1: Read viewer test ownership**

Run:

```bash
rg -n "^\\s*fn [a-zA-Z0-9_]+\\(" src/ui/viewer_page.rs src/ui/viewer/*.rs
sed -n '1,120p' tests/ui_viewer_source_structure.rs
```

Confirm the target module for each test before moving it.

- [ ] **Step 2: Move pure filmstrip tests**

Move these tests from `src/ui/viewer_page.rs` into `#[cfg(test)] mod tests` inside `src/ui/viewer/filmstrip.rs`:

```text
initial_window_centred_on_current_in_middle_of_album
initial_window_clips_at_album_start
initial_window_clips_at_album_end
initial_window_is_empty_for_empty_album
extend_left_grows_window_without_changing_end
extend_right_grows_window_without_changing_start
right_growth_uses_append_update_instead_of_rebuilding_strip
left_growth_is_classified_as_prepend_update
sliding_window_still_rebuilds_because_existing_indices_change
extend_left_returns_none_at_album_start
extend_right_returns_none_at_album_end
extend_at_window_cap_slides_left_without_growing_live_items
extend_at_window_cap_slides_right_without_growing_live_items
extend_left_clamps_to_zero_not_negative
extend_right_clamps_to_n_items
current_near_right_edge_triggers_right_thumb_extend
current_near_left_edge_triggers_left_thumb_extend
current_in_middle_does_not_extend_thumb_window
current_edge_extend_respects_album_edges_and_window_cap
current_edge_extend_continues_at_window_cap_by_sliding_window
initial_window_total_item_count_matches_docstring
residual_centres_first_thumbnail_without_layout_padding
residual_is_suppressed_when_content_does_not_exceed_viewport
visual_transform_uses_css_offset_when_adjustment_has_no_scroll_range
visual_transform_uses_only_residual_when_adjustment_can_scroll
animated_scroll_value_eases_between_current_and_target
positioning_centres_current_when_content_is_narrower_than_viewport
filmstrip_thumbnail_width_is_clamped_to_reasonable_aspect_ratio
filmstrip_placeholder_width_uses_media_dimensions_when_available
item_geometry_uses_sequence_when_current_allocation_x_is_stale
item_geometry_includes_filmstrip_edge_inset
item_geometry_rejects_transient_tiny_allocations_after_rebuild
item_geometry_accepts_small_loaded_thumbnail_allocations
scroll_value_centres_middle_thumbnail_without_residual
residual_centres_last_thumbnail_without_layout_padding
thumb_centering_retries_until_allocation_is_ready
thumb_strip_template_starts_without_layout_spacers
```

Use `super::*` inside the test module only. Keep helper imports local to that test module.

- [ ] **Step 3: Move crop, stage, navigation, fullscreen, and transform tests**

Move these groups:

```text
src/ui/viewer/crop.rs:
crop_overlay_contain_rect_centers_letterboxed_image
crop_overlay_drag_move_clamps_to_image_bounds
crop_overlay_drag_corner_resizes_rect

src/ui/viewer/stage.rs:
video_stage_click_toggles_above_builtin_controls
video_stage_click_leaves_builtin_controls_alone
viewer_preview_uses_medium_thumbnail
animated_image_adds_half_second_pause_before_looping
viewer_plays_misnamed_gif_even_when_db_row_is_stale
viewer_starts_frame_timer_for_animated_gif
video_stage_reveals_only_for_current_prepared_stream
video_audio_preferences_are_applied_to_media_stream
stop_video_playback_retires_stream_until_next_idle
show_at_keeps_video_stream_when_startup_scan_re_anchors_same_item
show_at_rebuilds_video_stream_after_optimistic_navigation_to_different_video

src/ui/viewer/navigation.rs:
viewer_keyboard_action_navigates_and_closes
next_index_after_deleted_item_stays_in_bounds
find_media_index_by_id_uses_item_identity
current_media_item_stays_anchored_when_startup_scan_inserts_before_it

src/ui/viewer/fullscreen_window.rs:
fullscreen_preview_opens_separate_window_without_changing_viewer_layout
fullscreen_preview_reuses_existing_window_handle
fullscreen_preview_close_disconnects_paintable_sync_handler

src/ui/viewer/transform.rs:
zoom_step_clamps_to_viewer_limits
image_overlay_has_no_touch_zoom_or_pan_gestures
zoom_pan_is_clamped_and_resets_at_identity
zoom_controls_live_in_top_right_with_reset_out_rotate_fullscreen_increase_order
reset_zoom_restores_identity_state
```

If a test still needs the full `ViewerPage` template, keep it in `viewer_page.rs` and do not weaken the ownership guard; instead move the guard's target to the module that actually owns the setup helper used by that test.

- [ ] **Step 4: Keep root-only viewer integration tests in `viewer_page.rs`**

Leave tests in `src/ui/viewer_page.rs` when they assert coordination across details panel, editor panel, initial pop guard, or `show_at` wiring that spans several viewer modules:

```text
video_error_background_exists_in_viewer_overlay
video_error_background_hides_default_video_error_surface
editing_hides_overlay_navigation_buttons
escape_closes_details_panel_without_navigation_pop
close_details_button_keeps_viewer_page_visible
navigation_pop_closes_details_before_leaving_viewer
details_panel_temporarily_disables_navigation_pop
initial_open_guard_ignores_navigation_pop_action
initial_open_guard_ignores_keyboard_cancel
```

- [ ] **Step 5: Run viewer verification**

Run:

```bash
cargo test --test inline_test_ownership viewer_page_tests_live_with_viewer_modules
cargo test ui::viewer_page
cargo test --test ui_viewer_source_structure
cargo test --test ui_viewer_toolbar
```

Expected: all commands PASS.

---

### Task 3: Move MediaGrid Tests To MediaGrid Modules

**Files:**
- Modify: `src/ui/media_grid.rs`
- Modify: `src/ui/media_grid/selection.rs`
- Modify: `src/ui/media_grid/updates.rs`
- Modify: `src/ui/media_grid/loading.rs`
- Modify: `src/ui/media_grid/render.rs`
- Modify: `src/ui/media_grid/virtual_paging.rs`
- Modify: `src/ui/media_grid/viewport.rs`
- Test: `tests/inline_test_ownership.rs`
- Test: `tests/ui_media_grid_source_structure.rs`

**Interfaces:**
- Consumes: existing `MediaGrid` private helpers and focused grid modules.
- Produces: `src/ui/media_grid.rs` with widget-shell tests only.

- [ ] **Step 1: Read grid test ownership**

Run:

```bash
rg -n "^\\s*fn [a-zA-Z0-9_]+\\(" src/ui/media_grid.rs src/ui/media_grid/*.rs
sed -n '1,260p' tests/ui_media_grid_source_structure.rs
```

Confirm each test's target module matches the function under test.

- [ ] **Step 2: Move selection and update tests**

Move these tests:

```text
src/ui/media_grid/selection.rs:
section_flowbox_selection_mode_tracks_multi_select

src/ui/media_grid/updates.rs:
grid_rebuilds_when_backing_store_removes_item
grid_removes_backing_store_item_without_replacing_section_flow
grid_inserts_same_section_item_without_replacing_existing_tiles
grid_defers_uncached_incremental_insert_until_thumbnail_ready
grid_inserts_deferred_item_after_thumbnail_failure
rebuild_reuses_loaded_tiles_when_a_new_section_is_added
progressive_render_addition_appends_without_rebuilding_existing_tiles
```

Move any helper used only by these tests, such as `tile_count`, `first_section_flow`, `flow_child_at`, `first_square_tile`, and `square_tiles`, into the target module's `#[cfg(test)] mod tests`.

- [ ] **Step 3: Move render, loading, virtual paging, and viewport tests**

Move these tests:

```text
src/ui/media_grid/render.rs:
mem_cached_thumbnail_tile_is_built_without_loading_class
uncached_thumbnail_tile_stays_hidden_until_result_arrives
tile_duration_formats_minutes_and_hours

src/ui/media_grid/loading.rs:
album_grid_uses_progressive_first_render_seed
inactive_grid_defers_initial_tile_build_until_activated
inactive_full_library_grid_uses_progressive_seed_on_first_activation
active_empty_day_grid_rebuilds_when_first_scan_items_arrive
day_grid_defers_stats_until_background_metadata_refresh
metadata_invalidation_during_load_requests_followup_refresh
day_grid_stats_are_above_first_section_header
pending_thumbnail_stats_survive_metadata_invalidation_rebuild
album_grid_skips_global_library_stats
completed_day_grid_stats_are_hidden
library_stats_text_clamps_generated_to_total

src/ui/media_grid/virtual_paging.rs:
virtual_placeholder_flow_renders_loading_tiles_immediately
virtual_scroll_ratio_maps_to_full_library_offset
virtual_scroll_prefetches_before_window_edge
virtual_scroll_absolute_end_targets_last_page
virtual_spacer_height_scales_with_unloaded_items
virtual_loading_window_counts_placeholders_for_target_page
scroll_ratio_tracks_latest_drag_value_while_page_is_loading
programmatic_scroll_restore_does_not_request_virtual_page
coalesced_virtual_page_target_keeps_latest_drag_target

src/ui/media_grid/viewport.rs:
thumbnail_request_window_includes_viewport_and_one_page_overscan
```

- [ ] **Step 4: Keep root-only grid shell tests in `media_grid.rs`**

Leave tests in `src/ui/media_grid.rs` when they validate root-only construction state, source logging boundaries, or top-level helpers:

```text
thumbnail_request_mtime_uses_indexed_file_mtime_without_stat
progressive_render_progress_logs_stay_debug
high_frequency_thumbnail_trace_spans_stay_debug
```

- [ ] **Step 5: Run MediaGrid verification**

Run:

```bash
cargo test --test inline_test_ownership media_grid_tests_live_with_grid_modules
cargo test ui::media_grid
cargo test --test ui_media_grid_source_structure
```

Expected: all commands PASS.

---

### Task 4: Move MainWindow Tests To Window Modules

**Files:**
- Modify: `src/ui/window.rs`
- Modify: `src/ui/window/sidebar.rs`
- Modify: `src/ui/window/navigation.rs`
- Modify: `src/ui/window/albums.rs`
- Modify: `src/ui/window/settings.rs`
- Test: `tests/inline_test_ownership.rs`
- Test: `tests/ui_window_source_structure.rs`

**Interfaces:**
- Consumes: existing `MainWindow` extension modules.
- Produces: `src/ui/window.rs` with root shell tests only.

- [ ] **Step 1: Read window test ownership**

Run:

```bash
rg -n "^\\s*fn [a-zA-Z0-9_]+\\(" src/ui/window.rs src/ui/window/*.rs
sed -n '1,260p' tests/ui_window_source_structure.rs
```

Use the current source-structure tests as the module ownership map.

- [ ] **Step 2: Move sidebar and album tests**

Move these tests:

```text
src/ui/window/sidebar.rs:
sidebar_album_snapshot_updates_stable_rows_in_place
sidebar_album_snapshot_removes_missing_row_without_replacing_survivors
sidebar_media_type_snapshot_removes_missing_row_without_replacing_survivors
sidebar_trace_logs_stay_debug
sidebar_album_row_summary_logs_stay_debug

src/ui/window/albums.rs:
album_delete_pruning_removes_deleted_folder_rows_except_remaining_live_uris
album_delete_pruning_preserves_unknown_remaining_live_folder_rows
album_initial_load_limit_caps_large_albums_to_render_window
album_backfill_limit_caps_large_albums_to_ui_window
album_switch_trace_points_cover_selection_to_backfill
high_frequency_album_progress_logs_stay_debug
```

- [ ] **Step 3: Move navigation and settings tests**

Move these tests:

```text
src/ui/window/navigation.rs:
keyboard_scope_is_viewer_when_viewer_page_is_visible
keyboard_scope_is_browsing_when_photos_page_is_visible
ctrl_f_opens_search_from_photos_page
ctrl_a_selects_visible_photos_grid_items
escape_clears_photos_grid_selection_before_navigation_back
ctrl_f_opens_search_from_trash_page
settings_modal_blocks_global_search_and_closes_on_escape
glass_menu_modal_escape_does_not_pop_visible_page
ctrl_f_reuses_visible_search_page
viewer_right_key_navigates_when_focus_is_on_header_button

src/ui/window/settings.rs:
restart_spec_uses_current_executable_and_preserves_args
restart_spec_requires_current_process_exit_after_spawn
settings_page_exposes_video_default_mute_without_volume_control
settings_page_exposes_liquid_glass_transparency_slider
settings_page_exposes_theme_selector
settings_page_exposes_thumbnail_generation_speed_selector
settings_page_exposes_scan_path_management
settings_page_exposes_trash_backend_controls
settings_storage_rows_defer_size_calculation
settings_dialog_uses_bounded_scroll_child
```

Move shared helpers such as `collect_labels`, `collect_scales`, `collect_check_buttons`, and `collect_preference_titles` into `src/ui/window/settings.rs` tests unless another module still uses them.

- [ ] **Step 4: Keep root-only MainWindow tests in `window.rs`**

Leave tests in `src/ui/window.rs` when they assert root wiring that spans multiple window modules:

```text
shared_media_projection_refresh_emits_pure_addition_for_new_item
main_window_installs_single_keyboard_router
navigation_view_has_no_touch_swipe_controller
```

- [ ] **Step 5: Run window verification**

Run:

```bash
cargo test --test inline_test_ownership window_tests_live_with_window_modules
cargo test ui::window
cargo test --test ui_window_source_structure
cargo test --test sidebar_navigation
```

Expected: all commands PASS.

---

### Task 5: Move Single-File Module Tests To Sibling Test Files

**Files:**
- Modify: `src/ui/grid_css.rs`
- Create: `src/ui/grid_css/tests.rs`
- Modify: `src/core/thumbnails.rs`
- Create: `src/core/thumbnails/tests.rs`
- Modify: `src/core/metadata.rs`
- Create: `src/core/metadata/tests.rs`
- Modify: `src/core/trash.rs`
- Create: `src/core/trash/tests.rs`
- Test: `tests/inline_test_ownership.rs`

**Interfaces:**
- Consumes: each module's private helpers through child test modules.
- Produces: root files that delegate large tests using `#[cfg(test)] mod tests;`.

- [ ] **Step 1: Add external test-module declarations**

Replace each inline `#[cfg(test)] mod tests { ... }` block with a module declaration:

```rust
#[cfg(test)]
mod tests;
```

Apply this pattern in:

```text
src/ui/grid_css.rs
src/core/thumbnails.rs
src/core/metadata.rs
src/core/trash.rs
```

- [ ] **Step 2: Move each inline block body into its sibling file**

Create these files and move the body that was previously inside `mod tests { ... }` into each file:

```text
src/ui/grid_css/tests.rs
src/core/thumbnails/tests.rs
src/core/metadata/tests.rs
src/core/trash/tests.rs
```

Each new file should start with:

```rust
use super::*;
```

Then keep the existing imports, helpers, and tests from the moved block below it. If the moved block already had `use super::*;`, do not duplicate it.

- [ ] **Step 3: Run single-module verification**

Run:

```bash
cargo test ui::grid_css
cargo test thumbnails
cargo test metadata
cargo test trash
cargo test --test inline_test_ownership single_file_modules_delegate_large_test_blocks
```

Expected: all commands PASS.

---

### Task 6: Document Test Ownership And Run Final Verification

**Files:**
- Modify: `docs/testing.md`
- Modify: `docs/modules/viewer.md`
- Modify: `docs/modules/browsing.md`
- Modify: `docs/modules/storage.md`
- Modify: `docs/modules/ui-liquid-glass.md`
- Test: targeted Cargo commands from earlier tasks

**Interfaces:**
- Consumes: completed test moves from Tasks 2-5.
- Produces: documentation that explains where new tests should live.

- [ ] **Step 1: Update test ownership docs**

Add a short section to `docs/testing.md`:

```markdown
## Test Ownership

Large widget and core modules keep production files readable by placing tests
next to the behavior they exercise. When a focused submodule owns the behavior,
put its unit tests in that submodule's `#[cfg(test)] mod tests`. When a
single-file module would otherwise carry a large inline test block, declare
`#[cfg(test)] mod tests;` and place the body in a sibling `tests.rs` file under
the same module directory. Keep cross-module source-structure checks in
`tests/*_source_structure.rs` or `tests/inline_test_ownership.rs`.
```

- [ ] **Step 2: Update module docs**

Add one sentence to the relevant Key Files or testing notes in each module doc:

```text
docs/modules/viewer.md: viewer submodule tests live with `src/ui/viewer/*.rs`; `viewer_page.rs` keeps only root coordination tests.
docs/modules/browsing.md: MediaGrid submodule tests live with `src/ui/media_grid/*.rs`; `media_grid.rs` keeps only root widget-shell tests.
docs/modules/storage.md: large core single-file tests may live in sibling `tests.rs` modules, such as `src/core/metadata/tests.rs`, `src/core/trash/tests.rs`, and `src/core/thumbnails/tests.rs`.
docs/modules/ui-liquid-glass.md: `grid_css` CSS behavior tests live in `src/ui/grid_css/tests.rs`, while provider/source-file integration checks stay under `tests/`.
```

- [ ] **Step 3: Run final targeted verification**

Run:

```bash
cargo test --test inline_test_ownership
cargo test ui::viewer_page
cargo test ui::media_grid
cargo test ui::window
cargo test ui::grid_css
cargo test thumbnails
cargo test metadata
cargo test trash
cargo test --test ui_viewer_source_structure
cargo test --test ui_media_grid_source_structure
cargo test --test ui_window_source_structure
```

Expected: all commands PASS.

- [ ] **Step 4: Check code size trend with cloc**

Run:

```bash
/usr/bin/cloc src/ui/viewer_page.rs src/ui/media_grid.rs src/ui/window.rs src/ui/grid_css.rs src/core/thumbnails.rs src/core/metadata.rs src/core/trash.rs --by-file
```

Expected: `viewer_page.rs`, `media_grid.rs`, `window.rs`, `grid_css.rs`, `thumbnails.rs`, `metadata.rs`, and `trash.rs` all have fewer total lines than before migration; moved tests appear in focused module files.

---

## Self-Review

- Spec coverage: covers the initial test migration objective for oversized root files and single-file modules.
- Placeholder scan: no incomplete-marker instructions are present.
- Type consistency: all paths and test names are copied from the current repository scan.
- Scope check: production refactors such as splitting `settings.rs` or `metadata.rs` remain out of scope for this plan.
