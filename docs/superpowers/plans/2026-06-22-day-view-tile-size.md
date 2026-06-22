# Day-View Tile Size Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce `MediaGrid`'s `GroupBy::Day` on-screen tile size from 360 px to 270 px, keeping `GroupBy::Year` (90 px) and `GroupBy::Month` (180 px) unchanged.

**Architecture:** A single source of truth — `spec_for_mode(mode: GroupBy) -> ViewSpec` in `src/ui/media_grid.rs:98-116` — maps each `GroupBy` variant to a `(pixel_size, thumb_size)` pair. `pixel_size` flows only to `SquareTile::set_target(spec.pixel_size)` inside `build_photo_picture` (`src/ui/media_grid.rs:377`), which queues a resize on the GTK widget. Editing one literal in that match arm is the entire code change.

**Tech Stack:** Rust 2021, GTK4 (`gtk4 = "0.8"` with `v4_8`), libadwaita, tokio, rusqlite. The codebase uses `cargo build`, `cargo run`, `cargo test`.

## Global Constraints

- **Single file touched**: `src/ui/media_grid.rs`. No other source or template file needs to change.
- **Disk thumbnail buckets unchanged**: `Small=256`, `Medium=512`, `Large=1024`. Only the on-screen `pixel_size` for `Day` is reduced.
- **Year and Month tile sizes unchanged**: 90 px and 180 px respectively. Do not touch those match arms.
- **`GroupBy` enum order unchanged**: `Year` (default), `Month`, `Day` in `src/core/section_model.rs`.
- **`SquareTile::default()` target value unchanged**: stays at `Cell::new(90)` in `src/ui/media_grid.rs:280`.
- **No new tests**: the change is a UI pixel constant; verification is via `cargo build` and a manual visual smoke test. Existing `cargo test` suite must still pass.

---

## File Structure

**Modified:**
- `src/ui/media_grid.rs` — change `pixel_size: 360` → `pixel_size: 270` (line 112), and the doc-comment line above the `spec_for_mode` function that documents the Day mode size (line 19).

**No new files.** No test files. No template files.

---

## Task 1: Apply the 360 → 270 pixel size change

**Files:**
- Modify: `src/ui/media_grid.rs:19` (doc-comment)
- Modify: `src/ui/media_grid.rs:111-114` (`pixel_size` literal in `GroupBy::Day` arm)

**Interfaces:**
- Consumes: `GroupBy::Day` variant (unchanged) from `crate::core::section_model::GroupBy`.
- Produces: `ViewSpec { pixel_size: 270, thumb_size: ThumbnailSize::Large }` for `GroupBy::Day`. All downstream consumers (`SquareTile::set_target`, `tracing::debug!` log) consume `spec.pixel_size` unchanged.

- [ ] **Step 1: Update the module-level doc comment for Day mode**

Open `src/ui/media_grid.rs` and locate the `## Sizing` doc block (lines 14–19). The third bullet documents the Day-mode size. Change it from `360×360 px` to `270×270 px`. The exact current line is:

```
//! - Day   → 360×360 px (thumbnail bucket Large / 1024)
```

After editing, the line must read:

```
//! - Day   → 270×270 px (thumbnail bucket Large / 1024)
```

- [ ] **Step 2: Update the `pixel_size` literal in `spec_for_mode`**

In the same file, locate the `GroupBy::Day` arm of `spec_for_mode` (lines 111–114):

```rust
GroupBy::Day => ViewSpec {
    pixel_size: 360,
    thumb_size: ThumbnailSize::Large,
},
```

Change `pixel_size: 360` to `pixel_size: 270`. The result must read:

```rust
GroupBy::Day => ViewSpec {
    pixel_size: 270,
    thumb_size: ThumbnailSize::Large,
},
```

Do NOT touch the `GroupBy::Year` (line 104) or `GroupBy::Month` (line 108) arms.

- [ ] **Step 3: Verify the file diff is exactly two changed lines**

Run from the project root:

```bash
git diff src/ui/media_grid.rs
```

Expected: the diff shows exactly two changed lines — the doc-comment line at line 19 and the `pixel_size:` line at line 112. No other lines should differ. If `SquareTile`'s default (`Cell::new(90)` at line 280) or any other line is shown as changed, revert and start over.

- [ ] **Step 4: Run `cargo build`**

Run from the project root:

```bash
cargo build 2>&1 | tail -20
```

Expected: build succeeds, exits 0, and the output contains no `error[Exxxx]` or `warning:` lines that originate from `media_grid.rs`. Other pre-existing warnings elsewhere in the codebase are fine; only warnings tied to our edit are blockers.

If `cargo build` fails, read the error, fix the literal, and re-run. The only realistic failure mode here is a typo in the literal (e.g. accidentally editing the wrong match arm).

- [ ] **Step 5: Run the existing test suite as a regression check**

Run from the project root:

```bash
cargo test 2>&1 | tail -30
```

Expected: the test suite finishes with `test result: ok` lines and exit 0. There is no test directly asserting the tile pixel size, but `section_group.rs` and other tests exercise `GroupBy` / `MediaGrid` adjacent paths; they must continue to pass.

If any test fails, treat it as a regression: revert with `git checkout -- src/ui/media_grid.rs`, re-read Task 1 steps, and redo.

- [ ] **Step 6: Commit the change**

```bash
git add src/ui/media_grid.rs
git commit -m "fix(ui): reduce Day-view tile size 360 -> 270 px

Per spec docs/superpowers/specs/2026-06-22-day-view-tile-size-design.md,
the 1:2:4 ratio across Year/Month/Day modes made the Day tile (360 px)
visually dominate the viewport on the default 1200 px window. Reducing
to 270 px gives a smoother 1:2:3 hierarchy while keeping the
ThumbnailSize::Large (1024 px) disk bucket intact — viewer zoom
quality is unchanged. Year (90 px) and Month (180 px) untouched."
```

Expected: commit lands cleanly on `main`. `git log --oneline -1` shows the new commit at HEAD.

---

## Task 2: Visual smoke test of the three view modes

**Files:**
- No file changes. This task is read-only verification on the running app.

**Interfaces:**
- Consumes: the running `photo-viewer` binary (`cargo run`).
- Produces: a verified observation that Day tiles are smaller, Year/Month unchanged, and click-through still loads the `Large` viewer.

- [ ] **Step 1: Launch the app and switch to the Day view**

Run from the project root:

```bash
cargo run
```

Expected: the main window opens to the Photos page (default `GroupBy::Year`). The sidebar shows "Photos / Albums / Trash". The window is at its default 1200 × 800 size.

Switch the Photos-page view stack to the **日 (Day)** tab. Expected: each tile in the grid is visibly smaller than before — at the default window width, you should see roughly 3 to 4 tiles per row (was ~2.5 to 3 before).

- [ ] **Step 2: Verify Year and Month views are unchanged**

Still in the app, switch to the **年 (Year)** tab. Expected: tiles are still small (~10 per row at default window width, same as before this change). Switch to **月 (Month)**. Expected: tiles are medium-sized (~5 per row, same as before).

If Year or Month tile sizes have visibly changed, you edited the wrong match arm — return to Task 1, revert, and reapply only the `GroupBy::Day` arm.

- [ ] **Step 3: Verify click-through into the single-photo viewer**

Still on the Day view, click any tile. Expected: the `ViewerPage` opens and shows the photo at full-window size using the `Large` (1024 px) thumbnail — visually identical to the pre-change viewer (no zoom detail lost because the disk bucket is unchanged).

If the viewer shows a visibly lower-detail image (blocky, soft) than before, that indicates `ThumbnailSize::Large` is somehow not being loaded — but the spec says this code path is untouched, so the most likely cause is confusion with another view. Re-verify by closing and re-opening the photo from the Day view.

- [ ] **Step 4: Verify regression in Trash and Album detail pages**

In the sidebar, click **Trash**. Expected: trash tiles render at 125 px (the legacy `PhotoTile` size in `src/ui/trash_page.rs:228, 271`) — visually identical to before this change. Open any album; album-detail tiles render at 250 px (`src/ui/album_detail_page.rs:105`) — also identical to before.

These pages use independent tile-size constants (`photo_tile.rs::DEFAULT_TILE_SIZE = 125`, `album_detail_page.rs::set_item(.., Medium, 250)`) and are not affected by our edit, but confirming them catches accidental cross-file changes.

- [ ] **Step 5: No commit (verification-only task)**

This task produces no file changes. Close the app with the window manager or `Ctrl+C` in the terminal. The plan is complete when all four observation steps above match expectations.