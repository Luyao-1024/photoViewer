# Architecture Convergence Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Finish the architectural cleanup left after the first refactor migration by making the largest remaining roots into thin state/entry modules with focused implementation modules.

**Architecture:** Preserve public APIs and runtime behavior while moving implementation details behind existing private module boundaries. Each task starts by tightening source-structure tests so the intended ownership is executable, then moves code mechanically with the smallest targeted verification before broader tests.

**Tech Stack:** Rust, GTK4, Libadwaita, Blueprint templates, Cargo integration tests.

## Global Constraints

- Edit `data/ui/*.blp` templates, not generated `.ui` output.
- Keep `src/core/` independent from GTK UI concerns; do not introduce UI widget ownership in `src/core/`.
- Do not revert user changes or unrelated worktree changes.
- Preserve public widget APIs unless a task explicitly changes tests and docs for that API.
- Update the relevant `docs/modules/*.md` file when module ownership changes.
- Use `rg` for source discovery.
- Follow TDD for structure changes: update/add the focused source-structure test first, run it and confirm it fails, then move code.
- Run the smallest useful verification first, then broaden to `cargo test` when shared behavior is touched.

---

### Task 1: Complete Thumbnail Loader Split

**Files:**
- Create: `src/core/thumbnails/queue.rs`
- Create: `src/core/thumbnails/decode.rs`
- Modify: `src/core/thumbnails.rs`
- Modify: `src/core/thumbnails/cache.rs` only if helper visibility needs tightening
- Test: `tests/thumbnails_source_structure.rs`
- Docs: `docs/modules/storage.md`

**Interfaces:**
- Consumes: public `ThumbnailLoader`, `ThumbnailSize`, `LoadedThumb`, `TIER_NORMAL`, `TIER_BOOST`, and current private `PriItem`, `LoaderState`, `BackgroundPullState` behavior.
- Produces: the same public `ThumbnailLoader` API, with queue orchestration in `thumbnails/queue.rs` and image decode/save generation in `thumbnails/decode.rs`.

- [x] **Step 1: Read current thumbnail boundaries**

Read:

```bash
sed -n '1,220p' docs/modules/storage.md
sed -n '1,220p' src/core/thumbnails.rs
sed -n '720,1420p' src/core/thumbnails.rs
sed -n '1,220p' tests/thumbnails_source_structure.rs
```

Confirm current root still owns `worker_loop`, `next_request_or_pull`, `pull_batch_and_enqueue`, `generate`, `generate_via_pixbuf`, `save_pixbuf_as_webp`, `save_pixbuf_as_jpeg_atomic`, `ensure_opaque`, `scale_pixbuf_to_fit`, and `pixbuf_is_light`.

- [x] **Step 2: Write the failing structure test**

Update `tests/thumbnails_source_structure.rs` with two new tests:

```rust
#[test]
fn thumbnail_queue_helpers_live_in_queue_module() {
    let queue_path = Path::new("src/core/thumbnails/queue.rs");
    assert!(queue_path.exists(), "queue helpers should live in src/core/thumbnails/queue.rs");
    let queue = fs::read_to_string(queue_path).expect("queue module should be readable");
    for marker in [
        "pub(in crate::core::thumbnails) fn worker_loop",
        "pub(in crate::core::thumbnails) fn next_request_or_pull",
        "pub(in crate::core::thumbnails) fn pull_batch_and_enqueue",
        "pub(in crate::core::thumbnails) fn drop_in_flight",
    ] {
        assert!(queue.contains(marker), "queue.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/core/thumbnails.rs").expect("thumbnail root readable");
    assert!(root.contains("mod queue;"), "thumbnail root should declare the queue module");
    for marker in [
        "fn worker_loop(",
        "fn next_request_or_pull(",
        "fn pull_batch_and_enqueue(",
        "fn drop_in_flight(",
    ] {
        assert!(!root.contains(marker), "queue helper `{marker}` should move out of thumbnails.rs");
    }
}

#[test]
fn thumbnail_decode_helpers_live_in_decode_module() {
    let decode_path = Path::new("src/core/thumbnails/decode.rs");
    assert!(decode_path.exists(), "decode helpers should live in src/core/thumbnails/decode.rs");
    let decode = fs::read_to_string(decode_path).expect("decode module should be readable");
    for marker in [
        "pub(in crate::core::thumbnails) fn generate",
        "fn generate_unavailable_placeholder",
        "fn generate_via_pixbuf",
        "fn save_pixbuf_as_webp",
        "fn save_pixbuf_as_jpeg_atomic",
        "fn ensure_opaque",
        "fn scale_pixbuf_to_fit",
        "fn pixbuf_is_light",
    ] {
        assert!(decode.contains(marker), "decode.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/core/thumbnails.rs").expect("thumbnail root readable");
    assert!(root.contains("mod decode;"), "thumbnail root should declare the decode module");
    for marker in [
        "fn generate(",
        "fn generate_unavailable_placeholder(",
        "fn generate_via_pixbuf(",
        "fn save_pixbuf_as_webp(",
        "fn save_pixbuf_as_jpeg_atomic(",
        "fn ensure_opaque(",
        "fn scale_pixbuf_to_fit(",
        "fn pixbuf_is_light(",
    ] {
        assert!(!root.contains(marker), "decode helper `{marker}` should move out of thumbnails.rs");
    }
}
```

- [x] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test thumbnails_source_structure
```

Expected: FAIL because `src/core/thumbnails/queue.rs` and `src/core/thumbnails/decode.rs` do not exist.

- [x] **Step 4: Move queue orchestration**

Create `src/core/thumbnails/queue.rs` and move:

- `worker_loop`
- `next_request_or_pull`
- `pull_batch_and_enqueue`
- `drop_in_flight`

Adjust visibility of shared private types in `thumbnails.rs` to `pub(in crate::core::thumbnails)` only where required:

- `PriItem`
- `LoaderState`
- `BackgroundPullState`
- `LoaderInner`
- `StatsDirtyCallback`

Keep `ThumbnailLoader` public methods and constants in `thumbnails.rs`.

- [x] **Step 5: Verify queue extraction**

Run:

```bash
cargo fmt
cargo test --test thumbnails_source_structure
cargo test thumbnails
```

Expected: `thumbnail_queue_helpers_live_in_queue_module` passes; behavior tests still pass.

- [x] **Step 6: Move decode/generation helpers**

Create `src/core/thumbnails/decode.rs` and move:

- `generate`
- `generate_unavailable_placeholder`
- `generate_via_pixbuf`
- `pixbuf_has_transparency`
- `save_pixbuf_as_webp`
- `save_pixbuf_as_jpeg_atomic`
- `temporary_cache_path`
- `pixbuf_to_rgba_bytes`
- `ensure_opaque`
- `scale_pixbuf_to_fit`
- `pixbuf_is_light`

Expose only:

```rust
pub(in crate::core::thumbnails) fn generate(
    cache_dir: &Path,
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> anyhow::Result<LoadedThumb>
```

Keep JPEG-specific and video-specific code in `jpeg_turbo.rs` and `video.rs`.

- [x] **Step 7: Update storage docs**

Update `docs/modules/storage.md` Key Files so it includes:

- `src/core/thumbnails/queue.rs`: worker queue, priority pop, background pull, in-flight tracking
- `src/core/thumbnails/decode.rs`: image decode, fallback placeholders, atomic cache writes, brightness sampling

- [x] **Step 8: Verify thumbnail split**

Run:

```bash
cargo fmt
cargo test --test thumbnails_source_structure
cargo test thumbnails
```

Expected: all thumbnail structure and behavior tests pass.

---

### Task 2: Finish Moving Settings Helpers Out Of `window.rs`

**Files:**
- Modify: `src/ui/window.rs`
- Modify: `src/ui/window/settings.rs`
- Modify: `tests/ui_window_source_structure.rs`
- Docs: `docs/modules/albums-trash.md`
- Docs: `docs/modules/storage.md`

**Interfaces:**
- Consumes: `MainWindow::show_settings_dialog`, current private `ScanPathListKind`, `RestartSpec`, and settings helper methods.
- Produces: `window/settings.rs` owns settings dialog construction, scan path management, restart prompts, storage size helpers, and clear-cache/clear-db dialog helpers.

- [x] **Step 1: Read current settings boundaries**

Read:

```bash
sed -n '1500,2255p' src/ui/window.rs
sed -n '1,860p' src/ui/window/settings.rs
sed -n '1,140p' tests/ui_window_source_structure.rs
```

Confirm `window.rs` still owns `build_settings_dialog`, `build_scan_paths_group`, scan path mutation helpers, restart helpers, storage-size async helper, clear dialog/toast helpers, and size formatting.

- [x] **Step 2: Write the failing structure test**

Extend `window_settings_helpers_live_in_settings_module` in `tests/ui_window_source_structure.rs` with these required markers in `settings.rs`:

```rust
for marker in [
    "pub(super) fn build_settings_dialog",
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
```

Add root absence checks for the same function names.

- [x] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_window_source_structure
```

Expected: FAIL because these helpers still live in `src/ui/window.rs`.

- [x] **Step 4: Move settings dialog and scan path helpers**

Move from `window.rs` to `window/settings.rs`:

- `build_settings_dialog`
- `build_scan_paths_group`
- `add_scan_path_section`
- `add_scan_path_value_row`
- `choose_scan_folder`
- `append_scan_path`
- `remove_scan_path`
- `scan_paths`
- `set_scan_paths`

Keep `MainWindow::show_settings_dialog` in `window.rs` if desired, but it should call `self.build_settings_dialog(&host)` from the settings module.

- [x] **Step 5: Move restart and storage helpers**

Move from `window.rs` to `window/settings.rs`:

- `RestartSpec`
- `restart_spec_from`
- `current_restart_spec`
- `restart_application`
- `show_trash_operation_error_dialog` if it is only used by settings/albums dialog flows after checking call sites
- `update_storage_size_async`
- `show_clear_confirm_dialog`
- `show_clear_success_toast`
- `show_clear_error_toast`
- `format_size`

If a helper is also used by album/trash UI outside settings, keep the minimal wrapper in `window.rs` and move the implementation into `settings.rs` under a clearer name.

- [x] **Step 6: Update docs**

Update:

- `docs/modules/albums-trash.md`: `src/ui/window/settings.rs` owns settings dialog, scan path UI, trash backend settings, restart prompts, storage rows.
- `docs/modules/storage.md`: settings scan path and runtime/storage UI helpers live in `window/settings.rs`.

- [x] **Step 7: Verify settings extraction**

Run:

```bash
cargo fmt
cargo test --test ui_window_source_structure
cargo test ui::window
cargo test --test sidebar_navigation
```

Expected: all pass, with only known GTK parser warnings if GTK UI tests initialize CSS.

---

### Task 3: Move Stateful Viewer Filmstrip UI Into `viewer/filmstrip.rs`

**Files:**
- Modify: `src/ui/viewer_page.rs`
- Modify: `src/ui/viewer/filmstrip.rs`
- Modify: `tests/ui_viewer_source_structure.rs`
- Docs: `docs/modules/viewer.md`

**Interfaces:**
- Consumes: `ViewerPage::show_at`, thumbnail loader injection, current index/media list state, and existing pure filmstrip helpers.
- Produces: `viewer/filmstrip.rs` owns both pure filmstrip geometry and stateful filmstrip UI methods; `viewer_page.rs` calls only high-level filmstrip methods.

- [x] **Step 1: Read current filmstrip methods**

Read:

```bash
sed -n '1560,2395p' src/ui/viewer_page.rs
sed -n '1,280p' src/ui/viewer/filmstrip.rs
sed -n '1,170p' tests/ui_viewer_source_structure.rs
```

Confirm `viewer_page.rs` still owns `refresh_thumb_strip`, `load_initial_thumb_window`, `try_extend_thumb_window`, `try_extend_thumb_window_for_current`, `rebuild_thumb_strip`, `append_thumb_strip_items`, `prepend_thumb_strip_items`, `make_thumb_button`, `update_thumb_highlight`, `update_thumb_scroll_position`, `set_thumb_scroll_adjustment_value`, `animate_thumb_scroll_adjustment_to`, `apply_thumb_strip_transform`, `scroll_thumb_to_current`, `schedule_scroll_thumb_to_current`, `setup_thumb_strip_listener`, and `on_thumb_adj_changed`.

- [x] **Step 2: Write the failing structure test**

Extend `tests/ui_viewer_source_structure.rs` so `src/ui/viewer/filmstrip.rs` must contain:

```rust
"impl ViewerPage",
"pub(super) fn refresh_thumb_strip",
"pub(super) fn setup_thumb_strip_listener",
"fn load_initial_thumb_window",
"fn try_extend_thumb_window",
"fn rebuild_thumb_strip",
"fn make_thumb_button",
"fn update_thumb_scroll_position",
"fn animate_thumb_scroll_adjustment_to",
"fn scroll_thumb_to_current",
"fn schedule_scroll_thumb_to_current",
```

Add root absence markers for the same method names.

- [x] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_viewer_source_structure
```

Expected: FAIL because these stateful filmstrip methods still live in `viewer_page.rs`.

- [x] **Step 4: Move filmstrip UI methods**

Move the stateful methods listed in Step 1 into `src/ui/viewer/filmstrip.rs` under `impl ViewerPage`.

Use imports already established by other viewer submodules:

```rust
use super::ViewerPage;
use crate::core::media::MediaItem;
use crate::core::thumbnails::ThumbnailSize;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
```

Keep existing behavior exactly:

- Incremental append/prepend remains preferred over full rebuild.
- Current thumbnail highlight must not change layout.
- Centering retry and animated adjustment logic remains unchanged.
- Thumbnail mtime uses filesystem metadata fallback as before.

- [x] **Step 5: Fix visibility at module boundary**

Make only cross-module calls `pub(super)`:

- `refresh_thumb_strip`
- `setup_thumb_strip_listener`
- any helper still called by `viewer_page.rs` tests

Keep internal filmstrip helpers private inside `filmstrip.rs`.

- [x] **Step 6: Update viewer docs**

Update `docs/modules/viewer.md` Key Files role for `src/ui/viewer/filmstrip.rs` to explicitly include stateful filmstrip UI wiring, live window rebuild/extend, thumbnail button construction, and centering animation.

- [x] **Step 7: Verify filmstrip extraction**

Run:

```bash
cargo fmt
cargo test --test ui_viewer_source_structure
cargo test ui::viewer_page
cargo test --test ui_viewer_toolbar
```

Expected: all pass, preserving filmstrip geometry and viewer behavior.

---

### Task 4: Split MediaGrid Orchestration Further

**Files:**
- Create: `src/ui/media_grid/selection.rs`
- Create: `src/ui/media_grid/updates.rs`
- Create: `src/ui/media_grid/loading.rs`
- Modify: `src/ui/media_grid.rs`
- Modify: `tests/ui_media_grid_source_structure.rs`
- Docs: `docs/modules/browsing.md`

**Interfaces:**
- Consumes: current `MediaGrid` public API, `MediaGridCallbacks`, `DisplayedItem`, `GridMetadataSnapshot`, `ViewSpec`, and existing `render.rs` / `virtual_paging.rs`.
- Produces: `media_grid.rs` as widget state/API shell; `selection.rs` owns selection behavior; `updates.rs` owns incremental add/remove/deferred insert; `loading.rs` owns virtual page and progressive/background metadata loading orchestration.

- [x] **Step 1: Read current MediaGrid responsibility clusters**

Read:

```bash
sed -n '780,1460p' src/ui/media_grid.rs
sed -n '1460,1975p' src/ui/media_grid.rs
sed -n '1975,2995p' src/ui/media_grid.rs
sed -n '1,220p' tests/ui_media_grid_source_structure.rs
```

Identify these clusters:

- Selection: `selected_ids`, `displayed_indices`, `select_all`, `select_ids`, `set_multi_select_mode`, `apply_selection_mode`, `sync_visible_selection`, `clear_selection`, `ensure_context_selection`, `toggle_selection`.
- Incremental updates: `apply_incremental_removal`, `apply_incremental_addition`, `apply_progressive_render_addition`, `apply_incremental_addition_inner`, `defer_incremental_addition_until_thumbnail_ready`, `insert_deferred_incremental_item`, cached metadata increment/decrement, `remove_displayed_child`, `refresh_section_header_labels`.
- Loading orchestration: `try_expand_render_limit`, `try_load_virtual_page`, `spawn_virtual_page_query`, `start_pending_virtual_page_query`, `schedule_progressive_render_fill`, `invalidate_progressive_render_fill`, `ensure_library_metadata_async`, `invalidate_library_metadata`, `start_stats_refresh`, `schedule_rebuild`, `rebuild_immediately`.

- [x] **Step 2: Write the failing structure test**

Extend `tests/ui_media_grid_source_structure.rs` with module existence and root absence assertions:

```rust
#[test]
fn media_grid_selection_helpers_live_in_selection_module() {
    let module_path = Path::new("src/ui/media_grid/selection.rs");
    assert!(module_path.exists(), "selection helpers should live in src/ui/media_grid/selection.rs");
    let module = fs::read_to_string(module_path).expect("selection module readable");
    for marker in [
        "impl MediaGrid",
        "pub(super) fn selected_ids",
        "pub(super) fn select_all",
        "pub(super) fn set_multi_select_mode",
        "fn apply_selection_mode",
        "fn sync_visible_selection",
        "fn toggle_selection",
    ] {
        assert!(module.contains(marker), "selection.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/ui/media_grid.rs").expect("media_grid root readable");
    for marker in [
        "fn apply_selection_mode(",
        "fn sync_visible_selection(",
        "fn toggle_selection(",
    ] {
        assert!(!root.contains(marker), "selection helper `{marker}` should move out of media_grid.rs");
    }
}
```

Add equivalent tests for `updates.rs` and `loading.rs` using the method names from Step 1.

- [x] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_media_grid_source_structure
```

Expected: FAIL because the three modules do not exist.

- [x] **Step 4: Move selection helpers**

Create `src/ui/media_grid/selection.rs` and move the selection cluster from Step 1 into `impl MediaGrid`.

Keep public methods public if callers outside `media_grid.rs` already use them:

- `selected_ids`
- `displayed_indices`
- `select_all`
- `select_ids`
- `set_multi_select_mode`
- `is_multi_select_mode`
- `clear_selection`
- `is_all_displayed_selected`
- `connect_selection_changed`

Keep internal helpers private.

- [x] **Step 5: Verify selection extraction**

Run:

```bash
cargo fmt
cargo test --test ui_media_grid_source_structure
cargo test ui::media_grid
```

Expected: selection-related tests pass.

- [x] **Step 6: Move incremental update helpers**

Create `src/ui/media_grid/updates.rs` and move the incremental update cluster from Step 1 into `impl MediaGrid`.

Keep behavior unchanged:

- Contiguous removals still splice without rebuilding.
- Scattered removals preserve order.
- Deferred thumbnail-ready insertion still has failure fallback.
- Cached section metadata increments/decrements stay consistent.

- [x] **Step 7: Move loading orchestration helpers**

Create `src/ui/media_grid/loading.rs` and move the loading orchestration cluster from Step 1 into `impl MediaGrid`.

Keep behavior unchanged:

- Virtual page generation counter drops stale results.
- Skeleton placeholder window renders during page load.
- Progressive render fill invalidates correctly.
- Library metadata/stats refresh remains async and token guarded.

- [x] **Step 8: Update browsing docs**

Update `docs/modules/browsing.md` Key Files to include:

- `src/ui/media_grid/selection.rs`
- `src/ui/media_grid/updates.rs`
- `src/ui/media_grid/loading.rs`

Describe each module responsibility in one line.

- [x] **Step 9: Verify MediaGrid split**

Run:

```bash
cargo fmt
cargo test --test ui_media_grid_source_structure
cargo test ui::media_grid
cargo test --test ui_grid_css_install
```

Expected: all pass.

---

### Task 5: Final Architecture Verification And Documentation

**Files:**
- Modify: `docs/modules/storage.md`
- Modify: `docs/modules/browsing.md`
- Modify: `docs/modules/viewer.md`
- Modify: `docs/modules/albums-trash.md`
- Modify: `docs/superpowers/plans/2026-07-09-architecture-convergence-cleanup.md`

**Interfaces:**
- Consumes: all modules extracted in Tasks 1-4.
- Produces: updated architecture docs and a verified convergence checkpoint.

- [x] **Step 1: Re-run structure tests**

Run:

```bash
cargo test --test thumbnails_source_structure --test ui_window_source_structure --test ui_viewer_source_structure --test ui_media_grid_source_structure
```

Expected: all structure tests pass.

- [x] **Step 2: Re-run targeted behavior suites**

Run:

```bash
cargo test thumbnails
cargo test ui::window
cargo test ui::viewer_page
cargo test ui::media_grid
cargo test --test sidebar_navigation
cargo test --test ui_viewer_toolbar
cargo test --test ui_grid_css_install
```

Expected: all pass. Known GTK `backdrop-filter` parser warnings are acceptable.

- [x] **Step 3: Check largest Rust files**

Run:

```bash
find src -type f -name '*.rs' -print0 | xargs -0 wc -l | sort -nr | head -20
```

Expected: the largest files may still be substantial, but the top roots should no longer contain the moved helper markers guarded by source-structure tests.

- [x] **Step 4: Run full test suite**

Run:

```bash
cargo test
```

Expected: all tests pass, with ignored benchmark-style tests unchanged.

- [x] **Step 5: Update this plan**

Mark Tasks 1-5 complete only after the verification command in Step 4 exits 0.

Add a final checkpoint line:

```markdown
- [x] **Final checkpoint:** Completed architecture convergence cleanup and verified with source-structure tests, targeted module tests, and full `cargo test`.
```

## Self-Review

- Spec coverage: This plan covers the four gaps identified in the architecture review: thumbnail queue/decode split, incomplete settings migration, stateful viewer filmstrip migration, and MediaGrid responsibility split.
- Placeholder scan: No step uses `TBD`, `TODO`, or “similar to”; each task has explicit file paths, method names, and verification commands.
- Type consistency: Existing public APIs are preserved. New modules are private implementation modules; cross-module visibility should use `pub(super)` or `pub(in crate::core::thumbnails)` only where needed.
