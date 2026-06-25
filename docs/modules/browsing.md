# Browsing Module

## Scope

Browsing covers the Photos page, Year/Month/Day grouping, thumbnail grid presentation, and the bottom mode selector.

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

`PhotosPage` owns three `MediaGrid` instances for Year, Month, and Day views. All views are backed by the same `gio::ListStore`, so changes to the media collection should propagate without rebuilding unrelated UI state.

`MediaGrid::spec_for_mode` owns per-view tile sizing. Section headers are separate GTK labels because the thumbnail grid cannot span a full-width header row by itself.

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
