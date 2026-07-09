# Codebase Refactor Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce oversized files by completing existing architectural migrations and extracting stable responsibilities without changing runtime behavior.

**Architecture:** Keep public widget APIs stable while moving implementation details into focused modules and resource files. Start with low-risk mechanical extractions, then migrate state ownership to existing abstractions such as `MediaWindowModel`, `DbActor`, and `UiRefreshHub`.

**Tech Stack:** Rust, GTK4, Libadwaita, Blueprint templates, Cargo integration tests.

## Global Constraints

- Edit `data/ui/*.blp` templates, not generated `.ui` output.
- Keep `src/core/` independent from GTK UI concerns.
- Do not revert user changes or unrelated worktree changes.
- Update module docs when changing contracts, UI invariants, or development workflow.
- Preserve existing public APIs until all call sites are migrated.
- Run the smallest useful verification first, then broaden when shared behavior is touched.

---

### Task 1: Move Grid CSS Sources Out Of Rust

**Files:**
- Create: `data/css/base.css`
- Create: `data/css/liquid.css`
- Create: `data/css/plain.css`
- Create: `data/css/a11y.css`
- Modify: `src/ui/grid_css.rs`
- Test: `tests/ui_grid_css_source_files.rs`

**Interfaces:**
- Consumes: `grid_css::install()`, `grid_css::reapply(bool)`, and existing `build_css` test helper behavior.
- Produces: the same runtime CSS string assembled from external source files through `include_str!`.

- [x] **Step 1: Write the failing test**

Add an integration test asserting the CSS source files exist and carry the expected block markers.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test --test ui_grid_css_source_files`

- [x] **Step 3: Move CSS literal bodies into `data/css/*.css`**

Replace `const BASE_CSS: &str = "...";` style literals in `src/ui/grid_css.rs` with `include_str!("../../data/css/base.css")`, and repeat for liquid/plain/a11y.

- [x] **Step 4: Run targeted CSS verification**

Run: `cargo test --test ui_grid_css_source_files && cargo test --test ui_grid_css_install && cargo test ui::grid_css`

### Task 2: Extract Thumbnail Loader Internals

**Files:**
- Create: `src/core/thumbnails/queue.rs`
- Create: `src/core/thumbnails/cache.rs`
- Create: `src/core/thumbnails/decode.rs`
- Create: `src/core/thumbnails/video.rs`
- Create: `src/core/thumbnails/jpeg_turbo.rs`
- Modify: `src/core/thumbnails.rs`
- Test: existing `cargo test thumbnails`

**Interfaces:**
- Consumes: public `ThumbnailLoader`, `ThumbnailSize`, `LoadedThumb`, `TIER_NORMAL`, `TIER_BOOST`.
- Produces: the same public API, with queue/cache/decode/video implementation in private modules.

- [x] **Step 1: Move pure cache helpers first**

Move `resolve_src`, `cache_key_str`, `cache_stem_for`, `existing_cache_path`, `load_pixbuf_sync`, and `load_pixbuf_sync_or_remove` into `cache.rs`. Atomic save helpers stay with image encoding until the decode/save split is done.

- [x] **Step 2: Move video extraction**

Move `extract_video_frame`, `extract_video_frame_ffmpeg`, `extract_video_frame_gst`, `ffmpeg_thumbnail_temp_path`, and play-icon overlay helpers into `video.rs`.

- [x] **Step 3: Move JPEG FFI**

Move the `extern "C"` declarations and `decode_jpeg_scaled` into `jpeg_turbo.rs`.

- [x] **Step 4: Verify behavior**

Run: `cargo test thumbnails`

### Task 3: Extract Grid Tile And Render Helpers

**Files:**
- Create: `src/ui/square_tile.rs`
- Create: `src/ui/media_grid/render.rs`
- Create: `src/ui/media_grid/virtual_paging.rs`
- Modify: `src/ui/media_grid.rs`
- Modify: `src/ui/mod.rs`
- Test: `cargo test ui::media_grid`

**Interfaces:**
- Consumes: existing `MediaGrid` constructors and `SquareTile` behavior.
- Produces: `crate::ui::square_tile::SquareTile` and private render/paging helpers.

- [x] **Step 1: Move `SquareTile` unchanged**

Extract the nested `square_tile` module to `src/ui/square_tile.rs` and re-export or update call sites.

- [x] **Step 2: Move pure virtual paging calculations**

Move virtual spacer and offset calculation helpers into `media_grid/virtual_paging.rs`.

- [x] **Step 3: Move tile construction helpers**

Move `build_photo_picture`, `prepare_reused_tile`, and flow-child visibility helpers into `media_grid/render.rs`.

- [x] **Step 4: Verify**

Run: `cargo test ui::media_grid && cargo test --test ui_grid_css_install`

### Task 4: Move Settings And Sidebar Out Of `window.rs`

**Files:**
- Create: `src/ui/window/settings.rs`
- Create: `src/ui/window/sidebar.rs`
- Create: `src/ui/window/albums.rs`
- Modify: `src/ui/window.rs`
- Test: `cargo test ui::window`

**Interfaces:**
- Consumes: `MainWindow` public API.
- Produces: private extension traits or helper modules called by `MainWindow`.

- [x] **Step 1: Extract settings page construction**

Move `build_settings_page`, trash settings, scan path settings, storage rows, and restart dialogs into `window/settings.rs`.

- [x] **Step 2: Extract sidebar row creation and refresh**

Move sidebar population, album/media-type row updates, and sidebar selection helpers into `window/sidebar.rs`.

- [x] **Step 3: Extract album DnD/context/delete flows**

Move album drag, context menu, and delete/ignore UI flows into `window/albums.rs`.

- [x] **Partial checkpoint:** Extracted settings/sidebar/albums helper modules and verified with `cargo test ui::window`, `cargo test --test sidebar_navigation`, and `cargo test --test ui_window_source_structure`.

- [x] **Step 4: Verify**

Run: `cargo test ui::window && cargo test --test sidebar_navigation`

### Task 5: Split Viewer Internals

**Files:**
- Create: `src/ui/viewer/navigation.rs`
- Create: `src/ui/viewer/filmstrip.rs`
- Create: `src/ui/viewer/details.rs`
- Create: `src/ui/viewer/stage.rs`
- Create: `src/ui/viewer/fullscreen.rs`
- Modify: `src/ui/viewer_page.rs`
- Test: `cargo test ui::viewer_page && cargo test --test ui_viewer_toolbar`

**Interfaces:**
- Consumes: `ViewerPage::new`, `ViewerPage::new_for_query`, `ViewerPage::show_at`, navigation callbacks, details/editor behavior.
- Produces: the same public `ViewerPage` API with private focused modules.

- [x] **Step 1: Extract pure filmstrip calculations**

Move tested calculation helpers and constants needed by them into `viewer/filmstrip.rs`.

- [x] **Step 2: Extract details rows**

Move EXIF/video row population and formatting helpers into `viewer/details.rs`.

- [x] **Partial checkpoint:** Moved details panel wiring, reveal/pop-guard handling, inline rename signal wiring, and EXIF/video row loading/population into `viewer/details.rs`. Verified with `cargo test --test ui_viewer_source_structure`, `cargo test ui::viewer_page`, and `cargo test --test ui_viewer_toolbar`.

- [x] **Step 3: Extract media stage**

Move image/video/animated/motion playback helpers into `viewer/stage.rs`.

- [x] **Partial checkpoint:** Moved media stage wiring, preview/original image loading, video stream lifecycle, video error background handling, animated GIF frame scheduling, and motion-photo extraction/playback into `viewer/stage.rs`. Verified with `cargo test --test ui_viewer_source_structure` and `cargo test ui::viewer_page`.

- [x] **Step 4: Extract navigation orchestration**

Move neighbour cache, deferred switch, and prefetch helpers into `viewer/navigation.rs`.

- [x] **Partial checkpoint:** Moved viewer navigation callbacks, prev/next wiring, navigation pop action, stable current-item lookup, neighbour cache, deferred thumb-ready switch, neighbour item prefetch, and neighbour page-cache warming into `viewer/navigation.rs`. Verified with `cargo test --test ui_viewer_source_structure` and `cargo test ui::viewer_page`.

- [x] **Step 5: Verify**

Run: `cargo test ui::viewer_page && cargo test --test ui_viewer_toolbar`

- [x] **Partial checkpoint:** Created viewer module boundaries for filmstrip/details/stage/navigation/fullscreen and verified with `cargo test ui::viewer_page`, `cargo test --test ui_viewer_toolbar`, and `cargo test --test ui_viewer_source_structure`.

- [x] **Partial checkpoint:** Removed duplicate root pure helpers for viewer filmstrip geometry, details formatting, navigation identity lookup, stage audio/click predicates, and fullscreen overlay button. Verified with `cargo test ui::viewer_page`, `cargo test --test ui_viewer_toolbar`, and `cargo test --test ui_viewer_source_structure`.
