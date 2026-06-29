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

When the initial DB snapshot is empty, `PhotosPage` shows the empty-state child, but it must switch back to the Day grid as soon as the shared `media_list` receives items from background startup scanning. Do not leave the `ViewStack` pinned to the empty child after `items-changed` adds media.

Dynamic photos are still image items (`media_kind=image`, `media_subkind=motion_photo`). Grids and legacy photo tiles display the still JPEG thumbnail exactly like a normal photo. In Day view, dynamic photos show a playback glyph at the thumbnail's bottom-left; ordinary videos show their persisted duration at the bottom-left instead; favorited media shows a white heart at the top-right. Do not decode or extract embedded video from grid code; use persisted `MediaItem` fields only.

`MediaGrid::spec_for_mode` owns per-view tile sizing. Section headers are separate GTK labels because the thumbnail grid cannot span a full-width header row by itself.

For very large libraries, the GTK-facing model and each `MediaGrid` rebuild are
bounded while the database remains the full source of truth. Startup loads the
first configured live page (`initial_media_page_size`, default 500); after
that, `MediaGrid` treats the scroll position as a ratio across the full
live-media count and swaps in a configured DB page (`virtual_media_page_size`,
default 500) around that global offset before the user reaches the end of the
currently loaded window. While that DB page is loading, the grid immediately renders a
non-interactive skeleton FlowBox for the target window instead of leaving the
viewport inside blank spacer space. Top and bottom virtual spacer widgets
approximate the height of unloaded rows, so the scrollbar thumb represents the
full library rather than only the current page. Rapid drag retargets increment
a virtual-page generation counter; stale DB page results are discarded rather
than replacing a newer target window. Only one virtual DB page query should be
in flight per grid; additional drag targets are coalesced so the next query
loads the latest target rather than every intermediate position. Programmatic
scroll restoration after a virtual page rebuild must not request another DB
page, and the `ListStore` splice that applies a virtual page must be rebuilt
exactly once instead of also going through the generic removal rebuild path.
`apply_to_media_list::ui_media_list_cap()`
(configurable via `runtime.json`, default 1500) remains a safety cap for live
change merges, and `MediaGrid::max_rendered_grid_items()` (configurable via
`runtime.json`, default 800) caps tile widgets per rebuild. Runtime loading
and sizing keys live in `src/core/runtime_config.rs`; user-facing preferences
remain in `settings.json`. `PhotosPage` also
initializes only the visible Day grid as active; Year/Month grids defer their
FlowBox/tile construction until the user switches to them. Do not let GTK
model, hidden views, or FlowBox children grow with the full on-disk library;
doing so drives GB-level memory use and blocks the main thread before the app
is usable.

Browsing identity is migrating from list indexes to stable `MediaId` values.
`MediaGrid` activation and multi-select callbacks must pass media ids across
widget/page boundaries; indexes are local to the current visible window only.
The `ui::models::media_window_model::MediaWindowModel` is the intended owner of
visible-window state (`MediaQuery`, total count, window start, generation, and
the GTK `ListStore` projection). Batch actions, selection state, viewer
activation, and cross-async work should use `MediaId`; indexes are render-local
only.

Thumbnail requests are driven by a viewport scan, not by tile `map` signals:
`GtkFlowBox` can map most or all children in the current virtual page even when
they are far below the visible area. The scan requests and priority-boosts
tiles intersecting the viewport plus one viewport of overscan, which keeps
visible thumbnails ahead of off-screen work while still making near-scroll
content warm quickly.

The Day grid's library statistics label reads `MediaRepository::library_stats()`.
It should display the repository projection (`LibraryStats`) and not calculate
thumbnail progress from `ThumbnailLoader` internals; stale thumbnail markers
are filtered at the DB projection layer.

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
