# Hover Selection Hint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace click-driven `:selected` outline + tint on `MediaGrid` and `AlbumDetailPage` tiles with a `:hover`-driven visual hint. Click still opens the viewer. `TrashPage` is unchanged.

**Architecture:** Pure CSS swap (`:selected` → `:hover`) plus `selection_mode = None` on the two FlowBoxes. The current `activate_on_single_click = true` already routes clicks to `child_activated`, so the FlowBox selection machinery is unused. The hover CSS lives in a new `src/ui/grid_css.rs` module shared by both pages (idempotent `Once` install, same pattern as the existing `install_grid_css`).

**Tech Stack:** Rust, gtk4-rs 0.8, libadwaita, GTK4 CSS provider.

## Global Constraints

- CSS visual style must stay byte-identical to the previous `:selected` style (only the trigger changes).
- `selection_mode` must become `None` on `MediaGrid`'s per-section FlowBox AND on `AlbumDetailPage`'s FlowBox. The current default is `Single` on the latter (verify and override).
- `TrashPage` FlowBox MUST keep `selection_mode = Multiple` — no change there.
- `set_activate_on_single_click(true)` MUST stay on both FlowBoxes (preserves click → viewer path).
- `child_activated` handler MUST remain wired on `MediaGrid`'s FlowBox (preserves viewer-open callback).
- No new public API outside `src/ui/grid_css.rs::{install}`.
- All worktree state in `M` files from `git status` MUST be preserved (do not touch unrelated work-in-progress files).

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `src/ui/grid_css.rs` | Create | Single-source CSS for thumbnail FlowBox hover style + idempotent install. |
| `src/ui/mod.rs` | Modify | `pub mod grid_css;` (one line). |
| `src/ui/media_grid.rs` | Modify | Remove inline `GRID_CSS` / `install_grid_css`; call `grid_css::install()`; set FlowBox `selection_mode = None`. |
| `src/ui/album_detail_page.rs` | Modify | Add `grid_css::install()` call; set FlowBox `selection_mode = None` (override default `Single`). |

No new tests required: there are no unit tests for CSS rendering. Manual smoke test in Dev Build is the verification gate.

---

## Task 1: Create shared hover CSS module

**Files:**
- Create: `src/ui/grid_css.rs`
- Modify: `src/ui/mod.rs:1-27`

**Interfaces:**
- Produces: `pub fn grid_css::install()` — idempotent (uses `std::sync::Once`). No args, no return. Safe to call from any widget constructor.

- [ ] **Step 1: Create `src/ui/grid_css.rs`**

```rust
//! Hover CSS for thumbnail FlowBoxes (MediaGrid, AlbumDetailPage).
//!
//! Replaces the previous click-driven `:selected` outline + tint with a
//! `:hover` hint. Identical visual style — only the trigger changes.
//! `TrashPage` deliberately does NOT install this; it keeps click-driven
//! multi-select for batch restore / permanent-delete.
//!
//! Install is idempotent (process-wide `Once`), so multiple pages may call
//! `install()` without coordinating.

use gtk4 as gtk;

const GRID_CSS: &str = "
flowbox.thumb-grid > flowboxchild { padding: 0; }
flowbox.thumb-grid > flowboxchild:hover {
  background-color: alpha(@accent_color, 0.3);
}
flowbox.thumb-grid > flowboxchild:hover .tile {
  outline: 2px solid @accent_color;
  outline-offset: -1px;
}
";

static CSS_INSTALLED: std::sync::Once = std::sync::Once::new();

/// Register the thumbnail-grid hover CSS with the default display.
/// Idempotent: subsequent calls are no-ops.
pub fn install() {
    CSS_INSTALLED.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(GRID_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}
```

- [ ] **Step 2: Register the module in `src/ui/mod.rs`**

In `/home/luyao/workspace/photoViewer/src/ui/mod.rs`, add the new module declaration alphabetically (between `empty_states` and `grid_row`):

```rust
pub mod album_detail_page;
pub mod albums_page;
pub mod edit_panel;
pub mod editor_page;
pub mod empty_states;
pub mod grid_css;
pub mod grid_row;
pub mod media_grid;
```

(Insert the single line `pub mod grid_css;` only — do not modify the other lines.)

- [ ] **Step 3: Verify it compiles**

Run from `/home/luyao/workspace/photoViewer`:

```bash
cargo check --lib
```

Expected: `Finished ...` with no errors. Warnings about unused items are acceptable at this point (no caller yet).

- [ ] **Step 4: Commit**

```bash
git add src/ui/grid_css.rs src/ui/mod.rs
git commit -m "feat(ui): shared hover CSS module for thumbnail FlowBoxes"
```

---

## Task 2: Migrate `MediaGrid` to hover hint

**Files:**
- Modify: `src/ui/media_grid.rs:117-145` (remove `GRID_CSS` and `install_grid_css`)
- Modify: `src/ui/media_grid.rs:170` (replace inline install call with module call)
- Modify: `src/ui/media_grid.rs:236-243` (FlowBox `selection_mode`)

**Interfaces:**
- Consumes: `grid_css::install()` from Task 1.

- [ ] **Step 1: Remove the inline CSS constants and install helper**

In `/home/luyao/workspace/photoViewer/src/ui/media_grid.rs`, delete the entire block from line 117 (`/// CSS for the thumbnail FlowBoxes: ...`) through line 145 (closing `}` of `install_grid_css`).

The block to delete (exact content, including the doc comment and `static CSS_INSTALLED`):

```rust
/// CSS for the thumbnail FlowBoxes: remove the default FlowBoxChild padding so
/// tiles touch (the FlowBox `column/row spacing` is the thin separator), and
/// highlight the selected tile with an accent tint + outline.
const GRID_CSS: &str = "
flowbox.thumb-grid > flowboxchild { padding: 0; }
flowbox.thumb-grid > flowboxchild:selected {
  background-color: alpha(@accent_color, 0.3);
}
flowbox.thumb-grid > flowboxchild:selected .tile {
  outline: 2px solid @accent_color;
  outline-offset: -1px;
}
";

static CSS_INSTALLED: std::sync::Once = std::sync::Once::new();

fn install_grid_css() {
    CSS_INSTALLED.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(GRID_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}
```

- [ ] **Step 2: Replace the inline install call with the shared module call**

In `MediaGrid::new` (currently around line 170), find:

```rust
        install_grid_css();
        obj.rebuild(media_list, mode);
```

Replace with:

```rust
        crate::ui::grid_css::install();
        obj.rebuild(media_list, mode);
```

- [ ] **Step 3: Switch the FlowBox `selection_mode` to `None`**

In `MediaGrid::rebuild` (currently around line 236), find the FlowBox builder:

```rust
            let flow = gtk::FlowBox::builder()
                .orientation(gtk::Orientation::Horizontal)
                .homogeneous(true)
                .column_spacing(2)
                .row_spacing(2)
                .max_children_per_line(100)
                .selection_mode(gtk::SelectionMode::Single)
                .build();
```

Replace `.selection_mode(gtk::SelectionMode::Single)` with `.selection_mode(gtk::SelectionMode::None)`. The `set_activate_on_single_click(true)` line below it stays unchanged — it still routes clicks to `child_activated`.

- [ ] **Step 4: Update the module doc comment to reflect hover behavior**

In `/home/luyao/workspace/photoViewer/src/ui/media_grid.rs`, the module doc comment at the top has a `## Gap & selection` section (lines 29-35). Find:

```rust
//! ## Gap & selection
//!
//! The FlowBox `column-spacing` / `row-spacing` (2 px) is the thin separator
//! between tiles. Selection uses the FlowBox's `selection-mode = single` +
//! `activate-on-single-click`; the selected child is tinted and its tile gets
//! an accent outline (see `GRID_CSS`) — i.e. the separator doubles as the
//! selection hint.
```

Replace with:

```rust
//! ## Gap & hover hint
//!
//! The FlowBox `column-spacing` / `row-spacing` (2 px) is the thin separator
//! between tiles. The hover hint (accent tint + outline) lives in
//! `crate::ui::grid_css` and is driven by the FlowBoxChild `:hover`
//! pseudo-class; `selection-mode = None` because `activate-on-single-click`
//! already routes clicks to `child_activated`. The separator doubles as the
//! hover hint.
```

- [ ] **Step 5: Verify it compiles**

Run from `/home/luyao/workspace/photoViewer`:

```bash
cargo check --lib
```

Expected: `Finished ...` with no errors and no new warnings. The previously-unused warning from Task 1 should now be gone (the call site is present).

- [ ] **Step 6: Verify clippy is clean**

Run from `/home/luyao/workspace/photoViewer`:

```bash
cargo clippy --lib -- -D warnings
```

Expected: `Finished ...` with no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/ui/media_grid.rs
git commit -m "feat(ui): MediaGrid hover selection hint (CSS :hover, selection_mode=None)"
```

---

## Task 3: Migrate `AlbumDetailPage` to hover hint

**Files:**
- Modify: `src/ui/album_detail_page.rs:76-110` (in `AlbumDetailPage::new`)

**Interfaces:**
- Consumes: `grid_css::install()` from Task 1.

- [ ] **Step 1: Inspect the current FlowBox configuration in `AlbumDetailPage`**

Open `/home/luyao/workspace/photoViewer/src/ui/album_detail_page.rs` and find the FlowBox usage. In the current source it is the `flow_box` template child obtained via `obj.imp().flow_box.get();` (around line 79).

The FlowBox itself is declared in the `.blp` template. Run:

```bash
grep -nE 'FlowBox|flowbox|selection' /home/luyao/workspace/photoViewer/data/ui/album-detail-page.ui
```

Expected: a single `GtkFlowBox` element with no explicit `selection-mode` line. The default for `GtkFlowBox` in GTK4 is `GTK_SELECTION_SINGLE`, so we must override to `None` in Rust code.

- [ ] **Step 2: Override FlowBox `selection_mode` and install hover CSS**

In `/home/luyao/workspace/photoViewer/src/ui/album_detail_page.rs`, inside `AlbumDetailPage::new`, find the block that obtains the FlowBox:

```rust
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&album.name);
        let flow = obj.imp().flow_box.get();
```

Add the hover-CSS install + selection-mode override immediately after `let flow = ...`:

```rust
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&album.name);
        let flow = obj.imp().flow_box.get();

        // Hover hint: same style as MediaGrid (see grid_css::GRID_CSS).
        // `selection_mode = None` because the page's FlowBox default is
        // Single, which would briefly paint the `:selected` style on click
        // and conflict with the hover hint.
        crate::ui::grid_css::install();
        flow.set_selection_mode(gtk::SelectionMode::None);
```

Do NOT touch the rest of the function (the filter loop and empty-state handling).

- [ ] **Step 3: Verify it compiles**

Run from `/home/luyao/workspace/photoViewer`:

```bash
cargo check --lib
```

Expected: `Finished ...` with no errors.

- [ ] **Step 4: Verify clippy is clean**

Run from `/home/luyao/workspace/photoViewer`:

```bash
cargo clippy --lib -- -D warnings
```

Expected: `Finished ...` with no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/ui/album_detail_page.rs
git commit -m "feat(ui): AlbumDetailPage hover selection hint (CSS :hover, selection_mode=None)"
```

---

## Task 4: End-to-end verification

**Files:** none modified — verification only.

- [ ] **Step 1: Run full test suite**

Run from `/home/luyao/workspace/photoViewer`:

```bash
cargo test
```

Expected: all tests pass (same count as the pre-change baseline — this change should not affect any test).

- [ ] **Step 2: Run full clippy**

Run from `/home/luyao/workspace/photoViewer`:

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: clean, no warnings.

- [ ] **Step 3: Manual smoke test — Photos page**

Build and launch the Dev Build (use whichever command the project README documents — typically `cargo run` or `meson compile && meson run`). Then:

1. Click **Photos** in the sidebar. The grouped-by-year/month/day grid renders.
2. Hover the mouse over any thumbnail tile.
   - Expected: a 2 px accent-colored outline appears around the tile, and the cell background gets a 30 % accent tint.
3. Move the mouse off the tile.
   - Expected: the outline and tint disappear immediately.
4. Click the tile.
   - Expected: the viewer page opens (existing behavior preserved).
5. Return to the Photos page and verify no leftover outline persists after the viewer closes.

- [ ] **Step 4: Manual smoke test — Albums page**

1. Click **Albums** in the sidebar, then click any album to open `AlbumDetailPage`.
2. Hover a tile.
   - Expected: same accent outline + tint as on Photos.
3. Move off.
   - Expected: outline disappears.
4. Click the tile.
   - Expected: existing behavior (no `child_activated` handler is currently wired — clicking is a no-op, which is fine; the page just doesn't open a viewer yet).

- [ ] **Step 5: Manual smoke test — Trash page (regression check)**

1. Click **Trash** in the sidebar.
2. Click several trash items.
   - Expected: each click toggles selection (multi-select outline + tint), and the ActionBar reveals when at least one is selected — same as before. NO hover-only outline should replace the click selection.
3. Hover an unselected tile.
   - Expected: NO outline appears (TrashPage doesn't install the hover CSS; selection is click-only).

- [ ] **Step 6: Capture verification result**

If all three smoke tests pass, the change is verified. If any step fails, do NOT proceed to Step 7 — report the failing step back to the spec reviewer.

- [ ] **Step 7: Optional — update README/CHANGELOG**

If the project keeps a `CHANGELOG.md`, add a one-line entry under the next unreleased section:

```markdown
- Photos / album grids now show selection outline on hover instead of click.
```

If `README.md` mentions the selection hint, update the wording to say "hover" instead of "click".

Commit if changed:

```bash
git add CHANGELOG.md README.md   # only if either was actually modified
git commit -m "docs(ui): note hover selection hint in README/CHANGELOG"
```

---

## Self-Review

**Spec coverage:**
- Goal 1 (hover outline on `MediaGrid` and `AlbumDetailPage`) → Tasks 2 + 3.
- Goal 2 (mouse-off removes outline) → CSS `:hover` semantics; verified manually in Task 4.
- Goal 3 (click still opens viewer) → preserved `set_activate_on_single_click(true)` and `child_activated` handler in Task 2; verified manually in Task 4.
- Goal 4 (`TrashPage` unchanged) → TrashPage FlowBox untouched in any task; verified manually in Task 4 Step 5.
- Code-change item 1 (CSS `:selected` → `:hover`) → Task 1 + Task 2 (also applies via `grid_css::install()` to AlbumDetailPage in Task 3).
- Code-change item 2 (`selection_mode = None`) → Task 2 Step 3 + Task 3 Step 2.
- Code-change item 3 (extract to `grid_css.rs`) → Task 1.
- Code-change item 4 (mod.rs) → Task 1 Step 2.
- Code-change item 5 (`TrashPage` / `PhotoTile` untouched) → no task touches them.
- Verification section → Task 4.

No spec gaps.

**Placeholder scan:** No TBD / TODO / "implement later" / "add appropriate error handling" steps. All code shown verbatim.

**Type/signature consistency:**
- `grid_css::install()` — defined in Task 1 Step 1, called in Tasks 2 Step 2 and 3 Step 2. Same signature `pub fn install()`, no args, no return.
- `gtk::SelectionMode::None` — used in Task 2 Step 3 and Task 3 Step 2. Same enum, same variant.
- `crate::ui::grid_css::install()` — full path used consistently; matches `pub mod grid_css;` in Task 1 Step 2.