# Hover Selection Hint — Design Spec

**Date:** 2026-06-21
**Status:** Approved (pending user spec review)
**Scope:** UI selection feedback in `MediaGrid` and `AlbumDetailPage`.

## Problem

In the current build, the visual "this photo is selected" hint (a 2 px
`@accent_color` outline around the tile + a 30 % accent-tinted background) is
driven by the GTK `:selected` pseudo-class on the FlowBox child. Combined with
`selection_mode = Single` + `activate_on_single_click = true`, this produces a
brief, transient flash on click — it never stays visible while the user is
browsing with the mouse.

The user wants the same outline + tint to be a **hover** hint: present while
the cursor is over a tile, gone when it moves away. Click still opens the
viewer; only the visual indication changes.

## Goals

1. The accent outline + tint appears on **mouse hover** over any tile in
   `MediaGrid` and `AlbumDetailPage`.
2. Moving the cursor off a tile removes the outline + tint immediately.
3. Click on a tile still opens the viewer (`child_activated` is preserved).
4. The `TrashPage` is **unaffected** — it keeps click-driven multi-select for
   batch restore / permanent-delete.

## Non-Goals

- Touch / pen support. GTK on Linux desktop is the target; `:hover` is the
  standard cursor-on-tile interaction.
- Keyboard-focus styling. GTK4's default focus ring is independent and stays
  as-is.
- Animations or transitions. Hover appears/disappears instantaneously, matching
  the previous click-driven flash in spirit.

## Approach

CSS `:hover` pseudo-class on the FlowBox child, paired with
`selection_mode = None`. The FlowBox's selection machinery is no longer needed:
`activate_on_single_click = true` already routes clicks to `child_activated`,
which opens the viewer. Removing the selection mode also stops GTK from
briefly applying `:selected` on click (the visual conflict that prompted this
change).

GTK4's `GtkFlowBoxChild` is a `GtkWidget` and supports the standard `:hover`
pseudo-class, so no event controllers or manual state toggling are required.

### CSS

Replace the existing `:selected` selectors with `:hover`:

```css
flowbox.thumb-grid > flowboxchild { padding: 0; }
flowbox.thumb-grid > flowboxchild:hover {
  background-color: alpha(@accent_color, 0.3);
}
flowbox.thumb-grid > flowboxchild:hover .tile {
  outline: 2px solid @accent_color;
  outline-offset: -1px;
}
```

The visual output is identical to the prior `:selected` style — only the
trigger changes.

## Code Changes

### 1. `src/ui/media_grid.rs`

- `GRID_CSS`: `:selected` → `:hover` (two selectors).
- `rebuild()` FlowBox builder: `selection_mode(gtk::SelectionMode::Single)` →
  `selection_mode(gtk::SelectionMode::None)`.
- Keep `flow.set_activate_on_single_click(true)`.
- Keep `flow.add_css_class("thumb-grid")`.
- The `child_activated` handler stays as-is (the viewer-open callback is
  unaffected).
- Move the CSS constant + `install_grid_css()` into a small shared helper so
  `AlbumDetailPage` can reuse it without duplication. Two options:
  - **(a)** Keep `GRID_CSS` and `install_grid_css()` in `media_grid.rs` as
    `pub(crate)`; `AlbumDetailPage::new` calls `media_grid::install_grid_css()`.
  - **(b)** Extract into a new `src/ui/grid_css.rs` module; both call
    `grid_css::install()`.

  Recommendation: **(b)** — single-purpose module, easier to extend later if
  a third page (e.g. search results) wants the same hover style.

### 2. `src/ui/album_detail_page.rs`

- After building the FlowBox, call `crate::ui::grid_css::install()` so the
  hover CSS is registered.
- FlowBox already uses no explicit selection-mode, so the default `Single`
  would still need to be changed to `None` to suppress the transient click
  flash.
- Confirm: `AlbumDetailPage::new` does not currently connect
  `child_activated` (per source — it appends `PhotoTile`s to the FlowBox but
  the open-viewer wiring lives elsewhere). Verify during implementation that
  hover-only behavior doesn't accidentally suppress an existing
  click-to-open path. If such a path exists via `child_activated`, it must be
  preserved.

### 3. New file `src/ui/grid_css.rs`

```rust
//! Hover CSS for thumbnail FlowBoxes (MediaGrid, AlbumDetailPage).
//! Idempotent: install at most once per process.

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

pub fn install() {
    CSS_INSTALLED.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(GRID_CSS);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}
```

### 4. `src/ui/mod.rs`

Add `pub mod grid_css;` and `pub use grid_css::install;` (optional re-export).

### 5. `PhotoTile` and `TrashPage`

Unchanged. `PhotoTile` doesn't paint the selection (it's the FlowBox child's
job). `TrashPage` keeps `selection_mode = Multiple` and click-driven
multi-select.

## Verification

1. **Build:** `cargo check` / `cargo build` clean.
2. **Lint:** `cargo clippy -- -D warnings` clean.
3. **Manual smoke test** (Dev Build):
   - Photos page: hover a tile → accent outline + tint appear; move mouse off
     → outline + tint disappear; click → viewer opens.
   - Albums → AlbumDetail: same hover behavior.
   - Trash: unchanged — click to select, multiple outlined, ActionBar reveals
     as before.
4. **Existing tests:** unit tests don't cover UI rendering. No new tests
   needed for a CSS-only change.
5. **README / CHANGELOG** (optional but recommended): one-line note under
   the next entry, e.g. "Photos / album grids now show selection outline on
   hover instead of click."

## Risk and Mitigations

- **Risk:** GTK4 `:hover` on FlowBoxChild may not fire on every theme.
  **Mitigation:** Manually verify with the system's default theme. If a
  theme override hides it, fall back to the `GtkEventControllerMotion` +
  custom `.hovered` class approach (Approach B from the brainstorm).
- **Risk:** `selection_mode = None` may break some other binding that relies
  on the FlowBox selection API. **Mitigation:** Source shows no other
  consumer of `selected_children()` in MediaGrid or AlbumDetailPage.
- **Risk:** Changing CSS can affect unrelated styling if the class name
  `thumb-grid` is reused elsewhere. **Mitigation:** `grep thumb-grid` over
  the source confirms it's only used in `media_grid.rs` (and will be added
  to `album_detail_page.rs`).

## Out of Scope (YAGNI)

- Persisting a "selected" state across mouse-moves (e.g. sticky selection
  after click).
- Hover-driven preview overlays.
- Custom focus styling for keyboard users.
- Theming the hover color via a GSetting.