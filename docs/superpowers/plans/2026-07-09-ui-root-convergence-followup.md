# UI Root Convergence Follow-Up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Continue the architecture convergence work by turning the remaining large UI root files into state/API shells with focused implementation modules.

**Architecture:** Preserve public widget APIs and runtime behavior while moving cohesive implementation blocks out of `src/ui/media_grid.rs`, `src/ui/viewer_page.rs`, and `src/ui/window.rs`. Each task starts by extending the source-structure tests, verifies the new test fails, then moves code mechanically into focused sibling modules and updates module documentation.

**Tech Stack:** Rust, GTK4, Libadwaita, Blueprint templates, Cargo integration tests.

## Global Constraints

- Edit `data/ui/*.blp` templates, not generated `.ui` output.
- Keep `src/core/` independent from GTK UI concerns; do not introduce UI widget ownership in `src/core/`.
- Do not revert user changes or unrelated worktree changes.
- Preserve public widget APIs unless a task explicitly changes tests and docs for that API.
- Update the relevant `docs/modules/*.md` file when module ownership changes.
- Use `rg` for source discovery.
- Follow TDD for structure changes: update/add the focused source-structure test first, run it and confirm it fails, then move code.
- Run the smallest useful verification first, then broaden to targeted behavior tests when shared UI behavior is touched.
- Keep the current browsing contract: virtual page retargets do not rebuild a skeleton window before the DB page lands.

---

### Task 1: Move MediaGrid Model Change And Viewport Orchestration

**Files:**
- Create: `src/ui/media_grid/viewport.rs`
- Modify: `src/ui/media_grid.rs`
- Modify: `src/ui/media_grid/updates.rs`
- Modify: `tests/ui_media_grid_source_structure.rs`
- Docs: `docs/modules/browsing.md`

**Interfaces:**
- Consumes: existing `MediaGrid`, `gio::ListStore`, `runtime_config::grid_reprioritize_debounce_ms()`, `tile_intersects_request_window`, `apply_incremental_removal`, `apply_incremental_addition`, `schedule_rebuild`, `try_load_virtual_page`, and `try_expand_render_limit`.
- Produces:
  - `MediaGrid::connect_model_changes(&self, media_list: &gio::ListStore)` in `updates.rs`.
  - `MediaGrid::handle_items_changed(&self, list: &gio::ListStore, position: u32, removed: u32, added: u32)` in `updates.rs`.
  - `MediaGrid::connect_scroll_handlers(&self)` in `viewport.rs`.
  - `MediaGrid::reprioritize_visible(&self)` in `viewport.rs`.

- [ ] **Step 1: Read the current MediaGrid boundary**

Read:

```bash
sed -n '1,180p' docs/modules/browsing.md
sed -n '450,930p' src/ui/media_grid.rs
sed -n '1,620p' src/ui/media_grid/updates.rs
sed -n '1,220p' tests/ui_media_grid_source_structure.rs
```

Confirm `src/ui/media_grid.rs` still owns the `connect_items_changed` closure, scroll `value-changed` closure, `collect_visible_cache_keys`, `reprioritize_visible`, and `schedule_reprioritize`.

- [ ] **Step 2: Write the failing source-structure test**

Extend `tests/ui_media_grid_source_structure.rs` with:

```rust
#[test]
fn media_grid_model_change_orchestration_lives_in_updates_module() {
    let module = fs::read_to_string("src/ui/media_grid/updates.rs")
        .expect("updates module readable");
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
    assert!(module_path.exists(), "viewport orchestration should live in src/ui/media_grid/viewport.rs");
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
```

- [ ] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_media_grid_source_structure
```

Expected: FAIL because `src/ui/media_grid/viewport.rs` does not exist and `updates.rs` does not yet contain `connect_model_changes`.

- [ ] **Step 4: Move model-change orchestration into `updates.rs`**

In `src/ui/media_grid/updates.rs`, add these methods on `impl MediaGrid`:

```rust
pub(super) fn connect_model_changes(&self, media_list: &gio::ListStore) {
    let weak = self.downgrade();
    media_list.connect_items_changed(move |list, position, removed, added| {
        let Some(this) = weak.upgrade() else {
            return;
        };
        this.handle_items_changed(list, position, removed, added);
    });
}

fn handle_items_changed(&self, list: &gio::ListStore, position: u32, removed: u32, added: u32) {
    // Move the existing closure body from MediaGrid::new_with_options here unchanged.
}
```

When moving the body, preserve all `GRID_MODEL_TRACE` log messages, the `applying_virtual_page` early return, the `was_empty` branch, and the inactive `dirty_model` branch.

In `src/ui/media_grid.rs`, replace the whole `media_list.connect_items_changed` block in `new_with_options` with:

```rust
obj.connect_model_changes(&media_list);
```

- [ ] **Step 5: Create `viewport.rs` and move viewport orchestration**

Create `src/ui/media_grid/viewport.rs` with:

```rust
use super::*;

impl MediaGrid {
    pub(super) fn connect_scroll_handlers(&self) {
        let weak_scroll = self.downgrade();
        self.imp()
            .scroller
            .get()
            .vadjustment()
            .connect_value_changed(move |adj| {
                if let Some(this) = weak_scroll.upgrade() {
                    this.schedule_reprioritize();
                    if !this.imp().restoring_scroll.get() {
                        this.try_load_virtual_page(adj);
                        this.try_expand_render_limit(adj);
                    }
                }
            });

        let weak_map = self.downgrade();
        self.connect_map(move |_| {
            let weak_map = weak_map.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(this) = weak_map.upgrade() {
                    this.reprioritize_visible();
                }
            });
        });
    }
}
```

Move these existing methods from `src/ui/media_grid.rs` into the same `impl MediaGrid` in `viewport.rs`:

- `collect_visible_cache_keys`
- `reprioritize_visible`
- `schedule_reprioritize`

In `src/ui/media_grid.rs`, add:

```rust
mod viewport;
```

Replace the scroll and map wiring in `new_with_options` with:

```rust
obj.connect_scroll_handlers();
```

Keep `tile_intersects_request_window` in `media_grid.rs` only if tests still import it from the root test module; otherwise move it to `viewport.rs` and update the unit tests to import it from `super::viewport`.

- [ ] **Step 6: Update browsing docs**

In `docs/modules/browsing.md`, update the Key Files table so it includes:

```markdown
| `src/ui/media_grid/viewport.rs` | Scroll-position viewport scan, visible thumbnail reprioritization, and scroll-triggered loading hooks |
```

Also change the `src/ui/media_grid.rs` role to:

```markdown
| `src/ui/media_grid.rs` | MediaGrid widget state, constructors, public API shell, sizing specs, and shared grouping helpers |
```

- [ ] **Step 7: Verify MediaGrid extraction**

Run:

```bash
cargo fmt
cargo test --test ui_media_grid_source_structure
cargo test ui::media_grid
```

Expected: structure tests pass; `ui::media_grid` behavior tests pass with only known GTK CSS warnings.

---

### Task 2: Move Viewer Crop Overlay Into `viewer/crop.rs`

**Files:**
- Create: `src/ui/viewer/crop.rs`
- Modify: `src/ui/viewer_page.rs`
- Modify: `tests/ui_viewer_source_structure.rs`
- Docs: `docs/modules/viewer.md`

**Interfaces:**
- Consumes: `ViewerPage`, `EditorPanel`, `CropOverlayUpdate`, `gtk::DrawingArea`, and current crop state fields in `imp::ViewerPage`.
- Produces:
  - `pub(super) struct CropDragState`
  - `pub(super) fn set_crop_overlay(&self, update: CropOverlayUpdate)`
  - `pub(super) fn setup_crop_overlay(&self)`
  - Pure helper functions for crop geometry in `viewer/crop.rs`.

- [ ] **Step 1: Read the current crop boundary**

Read:

```bash
sed -n '720,910p' src/ui/viewer_page.rs
sed -n '1938,2118p' src/ui/viewer_page.rs
sed -n '1,180p' tests/ui_viewer_source_structure.rs
```

Confirm `viewer_page.rs` still owns `ImageRect`, `CropDragMode`, `CropDragState`, `set_crop_overlay`, `setup_crop_overlay`, `draw_crop_overlay`, `begin_crop_drag`, `update_crop_drag`, `compute_contained_image_rect`, `crop_rect_to_widget`, `crop_handle_points`, `hit_crop_drag_mode`, `drag_rect`, and `resize_from_edges`.

- [ ] **Step 2: Write the failing source-structure test**

Extend `tests/ui_viewer_source_structure.rs` inside `viewer_focused_modules_exist` with a new module entry:

```rust
(
    "src/ui/viewer/crop.rs",
    &[
        "impl ViewerPage",
        "pub(super) struct CropDragState",
        "pub(super) fn set_crop_overlay",
        "pub(super) fn setup_crop_overlay",
        "fn draw_crop_overlay",
        "fn begin_crop_drag",
        "fn update_crop_drag",
        "pub(super) fn compute_contained_image_rect",
        "pub(super) fn crop_rect_to_widget",
        "pub(super) fn hit_crop_drag_mode",
        "pub(super) fn drag_rect",
        "pub(super) fn resize_from_edges",
    ][..],
),
```

Add root absence checks:

```rust
for marker in [
    "fn set_crop_overlay(",
    "fn setup_crop_overlay(",
    "fn draw_crop_overlay(",
    "fn begin_crop_drag(",
    "fn update_crop_drag(",
    "fn compute_contained_image_rect(",
    "fn crop_rect_to_widget(",
    "fn crop_handle_points(",
    "fn hit_crop_drag_mode(",
    "fn drag_rect(",
    "fn resize_from_edges(",
] {
    assert!(
        !root.contains(marker),
        "crop helper `{marker}` should live in viewer/crop.rs"
    );
}
```

Add `"crop"` to the module declaration check:

```rust
for module in ["navigation", "filmstrip", "details", "stage", "fullscreen", "crop"] {
    assert!(root.contains(&format!("mod {module};")), "viewer_page.rs should declare mod {module}");
}
```

- [ ] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_viewer_source_structure
```

Expected: FAIL because `src/ui/viewer/crop.rs` does not exist.

- [ ] **Step 4: Move crop types and constants**

Create `src/ui/viewer/crop.rs` and move these items out of `src/ui/viewer_page.rs`:

- `ImageRect`
- `CropDragMode`
- `CropDragState`
- `CROP_HANDLE_RADIUS`
- `CROP_MIN_SOURCE_SIZE`

Make `CropDragState` visible to the parent module because `imp::ViewerPage` stores it:

```rust
pub(super) struct CropDragState {
    mode: CropDragMode,
    rect: (u32, u32, u32, u32),
}
```

In `src/ui/viewer_page.rs`, declare the module and import the state type:

```rust
#[path = "viewer/crop.rs"]
mod crop;
use crop::CropDragState;
```

- [ ] **Step 5: Move crop widget methods and pure helpers**

Move these methods into `impl ViewerPage` in `src/ui/viewer/crop.rs`:

- `set_crop_overlay`
- `setup_crop_overlay`
- `draw_crop_overlay`
- `begin_crop_drag`
- `update_crop_drag`

Move these pure helpers into `src/ui/viewer/crop.rs`:

- `compute_contained_image_rect`
- `crop_rect_to_widget`
- `crop_handle_points`
- `hit_crop_drag_mode`
- `drag_rect`
- `resize_from_edges`

Expose pure helpers with `pub(super)` when they are used by root unit tests:

```rust
pub(super) fn compute_contained_image_rect(
    widget_width: f64,
    widget_height: f64,
    image_dimensions: (u32, u32),
) -> Option<ImageRect> {
    // Move the existing implementation unchanged.
}
```

Update `#[cfg(test)]` imports in `src/ui/viewer_page.rs` so existing tests can still call the helpers:

```rust
#[cfg(test)]
use crop::{
    compute_contained_image_rect, crop_rect_to_widget, drag_rect, hit_crop_drag_mode,
    resize_from_edges,
};
```

- [ ] **Step 6: Update viewer docs**

In `docs/modules/viewer.md`, add:

```markdown
| `src/ui/viewer/crop.rs` | Editor crop overlay drawing, hit-testing, drag/resize geometry, and overlay-to-source coordinate conversion |
```

- [ ] **Step 7: Verify crop extraction**

Run:

```bash
cargo fmt
cargo test --test ui_viewer_source_structure
cargo test ui::viewer_page
```

Expected: structure tests pass; `ui::viewer_page` tests pass with only known GTK/GstPlay warnings.

---

### Task 3: Move Viewer Editor Panel Wiring Into `viewer/editor.rs`

**Files:**
- Create: `src/ui/viewer/editor.rs`
- Modify: `src/ui/viewer_page.rs`
- Modify: `tests/ui_viewer_source_structure.rs`
- Docs: `docs/modules/viewer.md`

**Interfaces:**
- Consumes: `ViewerPage`, `EditorPanel`, `SaveResultKind`, `ToastKind`, `CropOverlayUpdate`, `current_media_item`, `set_details_revealed`, `set_motion_play_button_for_item`, `set_spinner_visible`, `set_overlay_navigation_visible`, `set_zoom_controls_visible`, and `reset_viewer_transform`.
- Produces:
  - `pub(super) fn setup_edit_button(&self)`
  - `pub(super) fn start_editing(&self)`
  - `pub(super) fn stop_editing(&self)`
  - `pub(super) fn setup_editor_callbacks(&self)`
  - `fn present_save_result_dialog(&self, heading: &str, body: &str)`

- [ ] **Step 1: Read the current editor boundary**

Read:

```bash
sed -n '586,748p' src/ui/viewer_page.rs
sed -n '2118,2145p' src/ui/viewer_page.rs
sed -n '1,220p' docs/modules/viewer.md
```

Confirm `viewer_page.rs` still owns `setup_edit_button`, `start_editing`, `stop_editing`, `setup_editor_callbacks`, `present_save_result_dialog`, and `save_result_closes_editor`.

- [ ] **Step 2: Write the failing source-structure test**

Extend `tests/ui_viewer_source_structure.rs` with:

```rust
(
    "src/ui/viewer/editor.rs",
    &[
        "impl ViewerPage",
        "pub(super) fn setup_edit_button",
        "pub(super) fn start_editing",
        "pub(super) fn stop_editing",
        "pub(super) fn setup_editor_callbacks",
        "fn present_save_result_dialog",
        "fn save_result_closes_editor",
    ][..],
),
```

Add `"editor"` to the module declaration check and add root absence checks:

```rust
for marker in [
    "fn setup_edit_button(",
    "fn start_editing(",
    "fn stop_editing(",
    "fn setup_editor_callbacks(",
    "fn present_save_result_dialog(",
    "fn save_result_closes_editor(",
] {
    assert!(
        !root.contains(marker),
        "editor helper `{marker}` should live in viewer/editor.rs"
    );
}
```

- [ ] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_viewer_source_structure
```

Expected: FAIL because `src/ui/viewer/editor.rs` does not exist.

- [ ] **Step 4: Move editor methods**

Create `src/ui/viewer/editor.rs` with explicit imports for the moved code:

```rust
use super::ViewerPage;
use crate::core::i18n::tr;
use crate::ui::editor_panel::{CropOverlayUpdate, SaveResultKind, ToastKind};
use crate::ui::toasts;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};
```

Move these methods from `src/ui/viewer_page.rs` into `impl ViewerPage`:

- `setup_edit_button`
- `start_editing`
- `stop_editing`
- `setup_editor_callbacks`
- `present_save_result_dialog`

Move `save_result_closes_editor` into `viewer/editor.rs` unchanged.

In `src/ui/viewer_page.rs`, add:

```rust
#[path = "viewer/editor.rs"]
mod editor;
```

- [ ] **Step 5: Update visibility for cross-module calls**

If the compiler reports private-method access, change only these existing method signatures in their owning modules and keep each method body unchanged:

```rust
pub(super) fn reset_viewer_transform(&self)
pub(super) fn set_zoom_controls_visible(&self, visible: bool)
pub(super) fn set_overlay_navigation_visible(&self, visible: bool)
```

Do not make editor helpers `pub` outside `src/ui/viewer_page.rs`; they should remain `pub(super)` or private inside `viewer/editor.rs`.

- [ ] **Step 6: Update viewer docs**

In `docs/modules/viewer.md`, add:

```markdown
| `src/ui/viewer/editor.rs` | Editor side-panel lifecycle, editor callback wiring, save-result dialogs, and editor navigation lock state |
```

- [ ] **Step 7: Verify editor extraction**

Run:

```bash
cargo fmt
cargo test --test ui_viewer_source_structure
cargo test ui::viewer_page
cargo test --test ui_viewer_toolbar
```

Expected: structure tests pass; viewer page and toolbar tests pass.

---

### Task 4: Move Viewer Toolbar Actions, Transform, And Fullscreen Preview Window

**Files:**
- Create: `src/ui/viewer/actions.rs`
- Create: `src/ui/viewer/transform.rs`
- Create: `src/ui/viewer/fullscreen_window.rs`
- Modify: `src/ui/viewer_page.rs`
- Modify: `src/ui/viewer/fullscreen.rs` only if `viewer_overlay_button` is re-exported or reused
- Modify: `tests/ui_viewer_source_structure.rs`
- Docs: `docs/modules/viewer.md`

**Interfaces:**
- Consumes: `ViewerPage`, `DbActorHandle`, `DbCommand`, `MediaId`, `MediaRepository`, `viewer_overlay_button`, current transform CSS provider state, and current `fullscreen_preview_window` state.
- Produces:
  - `viewer/actions.rs`: favorite/delete toolbar wiring and state sync.
  - `viewer/transform.rs`: image zoom/rotation controls and pure transform helpers.
  - `viewer/fullscreen_window.rs`: fullscreen preview window construction and preview-local controls.

- [ ] **Step 1: Read current action and transform boundaries**

Read:

```bash
sed -n '895,1445p' src/ui/viewer_page.rs
sed -n '1445,1570p' src/ui/viewer_page.rs
sed -n '1938,1970p' src/ui/viewer_page.rs
sed -n '1,120p' src/ui/viewer/fullscreen.rs
```

Confirm `viewer_page.rs` still owns `setup_delete_button`, `remove_deleted_item`, `setup_favorite_button`, `refresh_favorite_button`, `sync_favorite_state`, `setup_fullscreen_button`, `open_fullscreen_preview_window`, `update_fullscreen_button`, `setup_zoom_controls`, `step_viewer_zoom`, `reset_viewer_zoom`, `reset_viewer_transform`, `rotate_viewer_image`, `set_viewer_zoom`, `update_zoom_transform`, `set_zoom_controls_visible`, `update_zoom_buttons`, `step_zoom`, and `clamp_zoom_pan`.

- [ ] **Step 2: Write the failing source-structure test**

Extend `tests/ui_viewer_source_structure.rs` with these module entries:

```rust
(
    "src/ui/viewer/actions.rs",
    &[
        "impl ViewerPage",
        "pub(super) fn setup_delete_button",
        "fn remove_deleted_item",
        "pub(super) fn setup_favorite_button",
        "pub(super) fn sync_favorite_state",
        "fn refresh_favorite_button",
    ][..],
),
(
    "src/ui/viewer/transform.rs",
    &[
        "impl ViewerPage",
        "pub(super) fn setup_zoom_controls",
        "pub(super) fn reset_viewer_transform",
        "pub(super) fn set_zoom_controls_visible",
        "fn update_zoom_transform",
        "pub(super) fn step_zoom",
        "pub(super) fn clamp_zoom_pan",
    ][..],
),
(
    "src/ui/viewer/fullscreen_window.rs",
    &[
        "impl ViewerPage",
        "pub(super) fn setup_fullscreen_button",
        "fn open_fullscreen_preview_window",
        "fn update_fullscreen_button",
    ][..],
),
```

Add `"actions"`, `"transform"`, and `"fullscreen_window"` to the module declaration check. Add root absence checks for all method names listed in Step 1.

- [ ] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_viewer_source_structure
```

Expected: FAIL because the three new viewer modules do not exist.

- [ ] **Step 4: Move delete and favorite actions into `actions.rs`**

Create `src/ui/viewer/actions.rs` with explicit imports:

```rust
use super::ViewerPage;
use crate::core::db_actor::DbCommand;
use crate::core::i18n::{tr, trf};
use crate::core::identity::MediaId;
use crate::core::repository::MediaRepository;
use crate::ui::toasts;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};
```

Move these methods from `src/ui/viewer_page.rs` into `impl ViewerPage` in `actions.rs`:

- `setup_delete_button`
- `remove_deleted_item`
- `setup_favorite_button`
- `refresh_favorite_button`
- `sync_favorite_state`

Keep `remove_deleted_item` private to `actions.rs` unless another module calls it directly.

- [ ] **Step 5: Move transform controls into `transform.rs`**

Create `src/ui/viewer/transform.rs` and move:

- `setup_zoom_controls`
- `step_viewer_zoom`
- `reset_viewer_zoom`
- `reset_viewer_transform`
- `rotate_viewer_image`
- `set_viewer_zoom`
- `set_viewer_zoom_for_tests`
- `update_zoom_transform`
- `set_zoom_controls_visible`
- `update_zoom_buttons`
- `step_zoom`
- `clamp_zoom_pan`

Use these imports:

```rust
use super::ViewerPage;
use gtk4 as gtk;
use gtk4::prelude::*;
```

Expose helpers used by tests by changing their signatures and keeping the current bodies unchanged:

```rust
pub(super) fn step_zoom(current: f64, direction: i32) -> f64

pub(super) fn clamp_zoom_pan(
    scale: f64,
    pan_x: f64,
    pan_y: f64,
    width: f64,
    height: f64,
) -> (f64, f64)
```

Update `#[cfg(test)]` imports in `src/ui/viewer_page.rs`:

```rust
#[cfg(test)]
use transform::{clamp_zoom_pan, step_zoom};
```

- [ ] **Step 6: Move fullscreen preview window into `fullscreen_window.rs`**

Create `src/ui/viewer/fullscreen_window.rs` and move:

- `setup_fullscreen_button`
- `open_fullscreen_preview_window`
- `update_fullscreen_button`

Use `viewer_overlay_button` from `viewer/fullscreen.rs`:

```rust
use super::fullscreen::viewer_overlay_button;
use super::transform::step_zoom;
use super::ViewerPage;
```

Keep the existing behavior:

- Reuse an existing fullscreen window if already open.
- Do not mark the fullscreen window transient for the main window.
- Recreate only preview navigation and image transform controls.
- Disconnect `picture.paintable` notification on close.
- Close only the preview window on Escape.

- [ ] **Step 7: Update viewer docs**

In `docs/modules/viewer.md`, add:

```markdown
| `src/ui/viewer/actions.rs` | Viewer toolbar delete/favorite actions, favorite state sync, and post-delete navigation |
| `src/ui/viewer/transform.rs` | Image-stage zoom/rotation controls, CSS transform updates, and transform math helpers |
| `src/ui/viewer/fullscreen_window.rs` | Independent fullscreen preview window, preview navigation buttons, and preview-local transforms |
```

Update `src/ui/viewer/fullscreen.rs` role to:

```markdown
| `src/ui/viewer/fullscreen.rs` | Shared fullscreen/overlay button helper used by viewer chrome |
```

- [ ] **Step 8: Verify action/transform/fullscreen extraction**

Run:

```bash
cargo fmt
cargo test --test ui_viewer_source_structure
cargo test ui::viewer_page
cargo test --test ui_viewer_toolbar
```

Expected: structure tests pass; viewer page and toolbar tests pass.

---

### Task 5: Move Window Sidebar Snapshot And Navigation Orchestration

**Files:**
- Create: `src/ui/window/navigation.rs`
- Modify: `src/ui/window.rs`
- Modify: `src/ui/window/sidebar.rs`
- Modify: `tests/ui_window_source_structure.rs`
- Docs: `docs/modules/albums-trash.md`

**Interfaces:**
- Consumes: `MainWindow`, `SidebarAlbumSnapshot`, `SidebarTarget`, `Album`, `AlbumDetailPage`, `TrashPage`, `PhotosPage`, `SearchPage`, existing `albums.rs` helpers, and existing `sidebar.rs` row helper functions.
- Produces:
  - `window/sidebar.rs` owns sidebar row rebuild, sidebar snapshots, media-type rows, layout trace, photos-count label helpers, and collapse toggles.
  - `window/navigation.rs` owns sidebar signal wiring, album open scheduling, album page push, trash/search/viewer page navigation helpers, and visible page refresh helpers.

- [ ] **Step 1: Read current window sidebar/navigation boundary**

Read:

```bash
sed -n '314,930p' src/ui/window.rs
sed -n '930,1585p' src/ui/window.rs
sed -n '1,360p' src/ui/window/sidebar.rs
sed -n '1,180p' tests/ui_window_source_structure.rs
sed -n '1,180p' docs/modules/albums-trash.md
```

Confirm `window.rs` still owns `populate_album_rows`, `rebuild_album_rows`, `apply_album_rows`, `rebuild_media_type_rows`, `apply_media_type_rows`, `apply_sidebar_album_snapshot`, `install_sidebar_layout_trace`, `connect_sidebar_widget_trace`, `log_sidebar_layout_state_next_idle`, `log_sidebar_layout_state`, `reselect_active_album_row`, `toggle_albums_expanded`, `toggle_media_types_expanded`, `refresh_album_rows`, `update_photos_count_label`, `set_photos_count_label`, `update_photos_count_label_from_db`, `refresh_sidebar_snapshot_async`, `connect_sidebar`, `schedule_album_open_from_sidebar`, `open_album`, `refresh_visible_trash_page`, and `refresh_visible_album_detail_page`.

- [ ] **Step 2: Write the failing source-structure test**

Extend `tests/ui_window_source_structure.rs` with:

```rust
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
    assert!(module_path.exists(), "navigation flows should live in src/ui/window/navigation.rs");
    let module = fs::read_to_string(module_path).expect("navigation module readable");
    for marker in [
        "impl MainWindow",
        "pub fn connect_sidebar",
        "fn schedule_album_open_from_sidebar",
        "pub(crate) fn open_album",
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
        "fn refresh_visible_trash_page(",
        "fn refresh_visible_album_detail_page(",
    ] {
        assert!(
            !root.contains(marker),
            "navigation flow `{marker}` should move out of window.rs"
        );
    }
}
```

- [ ] **Step 3: Run the structure test to verify RED**

Run:

```bash
cargo test --test ui_window_source_structure
```

Expected: FAIL because `src/ui/window/navigation.rs` does not exist and stateful sidebar methods still live in `window.rs`.

- [ ] **Step 4: Move sidebar stateful flows into `sidebar.rs`**

Move these methods from `src/ui/window.rs` into `impl MainWindow` in `src/ui/window/sidebar.rs`:

- `populate_album_rows`
- `rebuild_album_rows`
- `apply_album_rows`
- `rebuild_media_type_rows`
- `apply_media_type_rows`
- `apply_sidebar_album_snapshot`
- `install_sidebar_layout_trace`
- `connect_sidebar_widget_trace`
- `log_sidebar_layout_state_next_idle`
- `log_sidebar_layout_state`
- `reselect_active_album_row`
- `toggle_albums_expanded`
- `toggle_media_types_expanded`
- `refresh_album_rows`
- `update_photos_count_label`
- `set_photos_count_label`
- `update_photos_count_label_from_db`
- `refresh_sidebar_snapshot_async`

Keep existing pure helper functions in `sidebar.rs` next to these methods. If import conflicts arise, prefer explicit imports at the top of `sidebar.rs` rather than `use super::*`.

- [ ] **Step 5: Move navigation flows into `navigation.rs`**

Create `src/ui/window/navigation.rs` and move these methods from `src/ui/window.rs`:

- `connect_sidebar`
- `schedule_album_open_from_sidebar`
- `open_album`
- `show_trash_page` if it is currently near `open_album`
- `refresh_visible_trash_page`
- `refresh_visible_album_detail_page`

Add the module declaration to `src/ui/window.rs`:

```rust
mod navigation;
```

Keep `show_settings_dialog` and `close_settings_dialog` in `window.rs` unless the compiler makes the navigation module cleaner by calling them through a `pub(super)` method in `settings.rs`. Do not move Settings logic back into `window.rs`.

- [ ] **Step 6: Update albums/trash docs**

Update `docs/modules/albums-trash.md` Key Files:

```markdown
| `src/ui/window.rs` | MainWindow state, resource injection, keyboard routing, and root sidebar shell |
| `src/ui/window/sidebar.rs` | Sidebar row widgets, album/media-type row diffing, sidebar snapshots, count labels, collapse state, and layout tracing |
| `src/ui/window/navigation.rs` | Sidebar selection signal wiring, Photos/album/trash/search navigation, album page push, and visible page refresh hooks |
```

- [ ] **Step 7: Verify window extraction**

Run:

```bash
cargo fmt
cargo test --test ui_window_source_structure
cargo test ui::window
cargo test --test sidebar_navigation
```

Expected: structure tests pass; window and sidebar navigation tests pass with only known GTK CSS warnings.

---

### Task 6: Tighten Split Module Boundaries And Final Verification

**Files:**
- Modify: `src/ui/media_grid/selection.rs`
- Modify: `src/ui/media_grid/updates.rs`
- Modify: `src/ui/media_grid/loading.rs`
- Modify: `src/ui/media_grid/viewport.rs`
- Modify: `src/ui/window/albums.rs`
- Modify: `src/ui/window/settings.rs`
- Modify: `src/ui/window/sidebar.rs`
- Modify: `src/ui/window/navigation.rs`
- Modify: `tests/ui_media_grid_source_structure.rs`
- Modify: `tests/ui_window_source_structure.rs`
- Modify: `docs/superpowers/plans/2026-07-09-architecture-convergence-cleanup.md`

**Interfaces:**
- Consumes: all split UI modules created by previous tasks.
- Produces: split modules with explicit imports instead of top-level `use super::*`; final structure tests that guard against accidental boundary drift.

- [ ] **Step 1: Write failing import-boundary tests**

Add this helper and test to `tests/ui_media_grid_source_structure.rs`:

```rust
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
```

Add the same helper shape to `tests/ui_window_source_structure.rs` and test:

```rust
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
```

- [ ] **Step 2: Run structure tests to verify RED**

Run:

```bash
cargo test --test ui_media_grid_source_structure --test ui_window_source_structure
```

Expected: FAIL while any listed split module starts with `use super::*;`.

- [ ] **Step 3: Replace `use super::*` in MediaGrid split modules**

For each listed `src/ui/media_grid/*.rs` module, replace top-level:

```rust
use super::*;
```

with explicit imports. Start with these groups and add only compiler-required names:

```rust
use super::{
    build_library_stats_label, build_media_uri_index, find_media_item_by_uri,
    media_item_at_displayed_index, thumbnail_request_mtime, tile_duration_label,
    tile_intersects_request_window, DisplayedItem, MediaGrid, ViewSpec,
};
use crate::core::media::MediaItem;
use crate::core::runtime_config;
use crate::core::section_model::{group_items, GroupBy, MediaSection, SectionKey};
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::square_tile::SquareTile;
use gtk4 as gtk;
use gtk4::{gio, glib};
use gtk4::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
```

Keep imports narrower than this starter list when a module needs fewer names.

- [ ] **Step 4: Replace `use super::*` in Window split modules**

For each listed `src/ui/window/*.rs` module, replace top-level:

```rust
use super::*;
```

with explicit imports. Start with these groups and add only compiler-required names:

```rust
use super::{MainWindow, SidebarAlbumSnapshot, SidebarTarget};
use crate::core::albums::{list_media_type_albums, list_with_favorites, Album};
use crate::core::db::DbPool;
use crate::core::i18n::{tr, trf};
use crate::core::media::MediaItem;
use crate::core::repository::{MediaMutation, MediaQuery};
use crate::core::thumbnails::ThumbnailLoader;
use gtk4 as gtk;
use gtk4::{gio, glib};
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
```

Keep imports narrower than this starter list when a module needs fewer names.

- [ ] **Step 5: Correct the stale previous-plan checkpoint**

In `docs/superpowers/plans/2026-07-09-architecture-convergence-cleanup.md`, replace the stale validation wording that says:

```markdown
Skeleton placeholder window renders during page load.
```

with:

```markdown
Virtual page retargets keep the existing tile window visible until the DB page lands; no synchronous skeleton rebuild occurs before landing.
```

This aligns the old plan with `docs/modules/browsing.md` and the current implementation contract.

- [ ] **Step 6: Run final structure verification**

Run:

```bash
cargo fmt
cargo test --test ui_media_grid_source_structure --test ui_viewer_source_structure --test ui_window_source_structure
```

Expected: all source-structure tests pass.

- [ ] **Step 7: Run targeted behavior verification**

Run:

```bash
cargo test ui::media_grid
cargo test ui::viewer_page
cargo test ui::window
cargo test --test sidebar_navigation
cargo test --test ui_viewer_toolbar
```

Expected: all targeted tests pass with only known GTK CSS warnings and known GstPlay teardown warnings described in `docs/testing.md`.

- [ ] **Step 8: Final architecture sanity check**

Run:

```bash
wc -l src/ui/media_grid.rs src/ui/media_grid/*.rs src/ui/viewer_page.rs src/ui/viewer/*.rs src/ui/window.rs src/ui/window/*.rs
rg -n "use super::\\*;" src/ui/media_grid src/ui/window
rg -n "fn (connect_items_changed|collect_visible_cache_keys|setup_crop_overlay|setup_edit_button|setup_delete_button|setup_zoom_controls|open_fullscreen_preview_window|apply_album_rows|connect_sidebar|open_album)" src/ui/media_grid.rs src/ui/viewer_page.rs src/ui/window.rs
```

Expected:

- `rg -n "use super::\\*;" src/ui/media_grid src/ui/window` prints no top-level split-module matches.
- Root files no longer contain the moved method definitions.
- The largest remaining root responsibilities are constructors, composite-template state, public setters/getters, keyboard routing, and `ViewerPage::show_at`.
