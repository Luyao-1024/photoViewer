# Browsing Module

## Scope

Browsing covers the Photos page, Year/Month/Day grouping, mixed media thumbnail grid presentation, and the bottom mode selector.

## Key Files

| File | Role |
|---|---|
| `src/ui/photos_page.rs` | Photos root page, view stack, shared store wiring |
| `src/ui/media_grid.rs` | Grouped grid layout and tile sizing |
| `src/ui/mode_selector.rs` | Year/Month/Day segmented control behavior |
| `src/ui/photo_tile.rs` | Thumbnail tile widget |
| `src/ui/section_header.rs` | Date/group section headers |
| `src/core/section_model.rs` | Year/Month/Day grouping model |
| `data/ui/photos-page.blp` | Photos page template |
| `data/ui/media-grid.blp` | Grid template |
| `data/ui/mode-selector.blp` | Mode selector template |

## Behavior

`PhotosPage` owns three `MediaGrid` instances for Year, Month, and Day views. All views are backed by the same `gio::ListStore`, so changes to the media collection should propagate without rebuilding unrelated UI state. The list can contain both image and video `MediaItem`s; grouping still uses `taken_at` when present, falling back to file time.

`MediaGrid::spec_for_mode` owns per-view tile sizing. Section headers are separate GTK labels because the thumbnail grid cannot span a full-width header row by itself.

Media activation is debounced by `PhotosPage` while it pushes `ViewerPage` onto the shared `AdwNavigationView`. Rapid repeated clicks in Year/Month/Day views must open only one viewer page and must not leak a second click into viewer-level pop/navigation handling during the transition.

Multi-select selection state is owned by each section `GtkFlowBox` (`selection-mode = Multiple`); `toggle_selection` / `select_all` / `clear_selection` call `flow.select_child` / `unselect_child`, which drives the `flowboxchild:selected` state. The selected affordance is a translucent-white checkmark pinned to each tile's bottom-right (`SquareTile`'s `.thumb-checkmark` child), revealed by CSS on `flowboxchild:selected`; do not add a parallel selected-state mechanism. See [`ui-design.md`](ui-design.md) "Media Grids And Tiles".

## Mode Selector

The Year/Month/Day control is both navigation and the canonical Liquid Glass segmented control. Preserve its visual structure:

- One outer raised glass capsule.
- Equal-width internal segments.
- Active state shown through label contrast and a short bottom indicator.
- No per-segment active background block.

Reusable segmented classes are documented in [`ui-liquid-glass.md`](ui-liquid-glass.md).

## Layout Pitfalls

Do not add fixed bottom padding to the grid to reserve space for the floating selector. That creates dark empty bands and weakens the backdrop effect. The selector should float as overlay chrome above real content.

When changing grid sizing, verify Day view separately because it has the densest section/header behavior.
