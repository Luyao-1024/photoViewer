# Browsing Module

## Scope

Browsing covers the Photos page, Year/Month/Day grouping, mixed media thumbnail grid presentation, and the bottom mode selector.

## Key Files

| File | Role |
|---|---|
| `src/ui/photos_page.rs` | Photos root page, view stack, shared store wiring |
| `src/ui/media_grid.rs` | Bounded FlowBox grid retained for search preview surfaces |
| `src/ui/media_grid/selection.rs` | Multi-select state, visible selection sync, context selection, and selection callbacks |
| `src/ui/media_grid/updates.rs` | Incremental add/remove handling, deferred thumbnail-ready insertions, metadata cache adjustments, and grid rebuild implementation |
| `src/ui/media_grid/viewport.rs` | Scroll-position viewport scan, visible thumbnail reprioritization, and scroll-triggered loading hooks |
| `src/ui/media_grid/loading.rs` | Virtual page loading, progressive render fill, async library metadata/stats refresh, and rebuild scheduling |
| `src/ui/media_grid/render.rs` | Tile construction, reused-tile preparation, and FlowBox child visibility sync |
| `src/ui/media_grid/virtual_paging.rs` | Virtual scroll offset, spacer, and placeholder window helpers |
| `src/ui/virtual_media_grid.rs` | Query-backed `GtkGridView`, lifecycle, range scheduling, and grid callbacks |
| `src/ui/virtual_media_grid/` | Pure layout index, virtual list model, range residency coordinator, factory, and focused tests |
| `src/ui/square_tile.rs` | Shared square thumbnail widget used by grids, albums, trash, and sidebar covers |
| `src/ui/mode_selector.rs` | Year/Month/Day segmented control behavior |
| `src/ui/photo_tile.rs` | Thumbnail tile widget |
| `src/ui/section_header.rs` | Date/group section headers |
| `src/core/section_model.rs` | Year/Month/Day grouping model |
| `data/ui/photos-page.blp` | Photos page template |
| `data/ui/media-grid.blp` | Bounded FlowBox grid template for search preview surfaces |
| `data/ui/virtual-media-grid.blp` | Direct `GtkScrolledWindow` → `GtkGridView` virtual-grid template |
| `data/ui/mode-selector.blp` | Mode selector template |

MediaGrid unit tests live with the submodule that owns the behavior. Production
source files declare `#[cfg(test)] mod tests;`, and test bodies live in child
test files such as `src/ui/media_grid/tests.rs`,
`src/ui/media_grid/loading/tests.rs`, and
`src/ui/media_grid/updates/tests.rs`.

## Behavior

The window keeps Photos and the currently active `AlbumDetailPage` as peer
children of `MainWindow`'s `browsing_stack`. That stack uses `crossfade` with a
200ms duration, so entering an album, switching albums, and returning to Photos
match the Year/Month/Day mode transition. `Adw.NavigationView` remains the
outer host for `ViewerPage`, `SearchPage`, and `TrashPage`; album changes do not
push additional outer navigation pages. The outer `TrashPage` keeps its
NavigationView back button so it can return to the browsing root.

`PhotosPage` owns three Year/Month/Day `VirtualMediaGrid` instances backed by
the same bounded `gio::ListStore`. Photos always uses the virtual
`GtkGridView`; there is no runtime backend selector or FlowBox fallback. A
legacy `photos_grid_backend` value left in `runtime.json` is ignored. Album
detail pages and Trash use a Day `VirtualMediaGrid` backed by their respective
queries. Search previews remain bounded `MediaGrid`/FlowBox surfaces, while a
search "More" result page uses a Year `VirtualMediaGrid` scoped to its term,
field, and media kind.

The GridView path exposes one logical slot for every full-library media item,
plus deterministic non-interactive filler slots that preserve section-row
boundaries. Its `gio::ListModel` only keeps ready `MediaItem`s for a bounded
viewport-adjacent range; all other media slots are light loading placeholders.
`VirtualGridLayoutIndex` maps section/media offsets to slots without materializing
widgets or a full `MediaItem` list. Database metadata and ranges are fetched off
the GTK thread, generation-checked, and coalesced while a scrollbar drag is in
flight. The lazy `gio::ListModel` keeps a stable GObject identity for each
queried slot until GTK releases it, and invalidates affected identities before
emitting replacement notifications. GTK recycles `SquareTile` cells through
`GtkSignalListItemFactory`; thumbnail results validate the current layout
generation, slot, MediaId, and cache key, then paint on an idle turn after the
factory bind stack has returned.

The Photos virtual grid groups image and video `MediaItem`s by `taken_at`,
falling back to file time. Year 90 px, Month 180 px, and Day 270 px remain
preferred tile targets for thumbnail quality; the number of columns comes only
from the persisted Photos Grid setting. `GtkGridView` fills each fixed column across the
real scroller viewport, and `SquareTile` declares height-for-width so each
thumbnail remains square rather than leaving a centered fixed-size gutter. The
virtual-grid tile reports the target as its natural width but accepts a smaller
minimum width, so a final cell that loses a few pixels to the scroller or CSS
box model remains square without GTK measurement warnings; legacy FlowBox
tiles retain their fixed minimum. The grid keeps an 8 px visual gutter by
combining 4 px list-item margins with 4 px outer padding, so the first and
last thumbnails retain room for their four-sided glass focus or selection
ring. `GtkGridView` itself must keep `border-spacing: 0`: GTK 4.22 subtracts
non-zero border spacing from an unallocated height-for-width probe and can
pass an invalid size to its list-item wrappers.
`VirtualGridViewportMetrics` mirrors GTK's integer
cell-width calculation on every resize, so tile side, row stride, date pill,
and visible-range calculations stay aligned. Resizing updates only viewport
metrics and never rebuilds the section-slot index. Changing the setting
intentionally reflows the three mode grids once. Updates never run
re-entrantly during GTK allocation. Hidden modes stay inactive and do not seed
or query ranges until selected.

The Day-view column count is a persisted Settings preference. Window resizing
never changes the Day column count. Day thumbnails keep the preferred 270 px
side; changing the preference raises the GridView/content minimum width and
requests a main-window width that fits the new number of columns, so the
setting does not silently shrink thumbnails. The same width request must be
applied before the main window is first presented, using the persisted
preference; waiting for a Settings interaction leaves the initial grid narrower
than its configured cells and produces GTK measurement warnings. Year and Month retain their
smaller-tile adaptive column count and may reflow when their natural width
thresholds are crossed. Changing the Day preference intentionally reflows the
Day grid once.

GTK 4.22 measures an installed height-for-width `GtkGridView` factory before
the scroller has a real width. Keep its own `border-spacing` at zero and model
the same visual gap with the list-item margins above; this preserves the
square height-for-width layout without a hard-coded cell-size workaround.

The Photos header includes a circular search button that pushes a dedicated
`SearchPage`. Search filters live media through `MediaRepository` using
`MediaQuery::Search` / `MediaQuery::SearchKind`; do not implement search by
filtering only the current GTK window, because large-library startup and
virtual paging keep only a bounded subset in memory. Search currently matches
file names and shooting dates (`taken_at`, falling back to `file_mtime`) with
date fragments such as `2026`, `2026-07`, or `2026-07-02`; Chinese `年/月/日`
input is normalized to the same date form. A segmented control above the search
entry lets the user restrict matching to file names only, dates only, or both
(the default). The selected field is stored as `SearchField` (`All`, `Name`,
`Date`) and passed through `MediaQuery` to the DB layer.

When the **Date** field is selected, the search entry provides automatic date
formatting: typing digits auto-inserts "/" separators (e.g., typing "20251001"
becomes "2025/10/01"). Date search supports multiple granularities:
- Year only: "2025" matches all media from 2025
- Year/Month: "2025/10" or "2025-10" matches October 2025
- Full date: "2025/10/01" or "2025-10-01" matches October 1, 2025
Both "/" and "-" separators are accepted; Chinese "年/月/日" characters are also
normalized. The DB layer converts all separators to "-" for matching against
SQLite's `strftime('%Y-%m-%d', ...)` output.

Search previews are rendered in separate image and video result sections, each
backed by its own bounded `ListStore` and a Year-mode `MediaGrid` so result
thumbnails use the compact overview size. Search preview grids are flat and
disable horizontal scrolling: they do not render per-year section headers, so
matches from different years wrap into the same preview area instead of
continuing in a single clipped row. Search result grids show a bounded preview
sized from the available page width and height: at least two Year-mode rows,
expanding to fill more of the result content area when the window is taller.
Preview grids disable internal scrolling; when a result set does not fit the
calculated preview capacity, the section shows a "More" button that pushes a
type-specific results page containing only those image or video matches. Empty
result sections stay hidden, so image and video areas grow from their result
counts instead of splitting the page 50/50. Opening the viewer from a result
section passes a kind-scoped search query so previous/next navigation remains
inside that section's result set.

When the initial DB snapshot is empty, `PhotosPage` shows the empty-state child, but it must switch back to the Day grid as soon as the shared `media_list` receives items from background startup scanning. Do not leave the `ViewStack` pinned to the empty child after `items-changed` adds media.

Dynamic photos are still image items (`media_kind=image`, `media_subkind=motion_photo`). Grids and legacy photo tiles display the still JPEG thumbnail exactly like a normal photo. In Day view, dynamic photos show a playback glyph at the thumbnail's bottom-left; ordinary videos show their persisted duration at the bottom-left instead; favorited media shows a white heart at the top-right. Do not decode or extract embedded video from grid code; use persisted `MediaItem` fields only.

The sidebar Media Types group contains only non-empty attribute virtual albums.
Motion photos are backed by `media_subkind='motion_photo'`; Animated and HDR are
backed by top-level `media_attributes` JSON booleans. If no media type album has
any live media, hide the whole Media Types group instead of showing an empty
header.

### Bounded FlowBox Grids (Search Previews)

`MediaGrid` remains the bounded FlowBox renderer for search preview surfaces;
it is not a Photos, album-detail, Trash, or search-detail renderer. `MediaGrid::spec_for_mode` owns its tile
sizing, and its date headers are separate GTK labels because a FlowBox cannot
span a header across a thumbnail row. Pure `ListStore` removals and insertions
should update the affected `GtkFlowBoxChild` in place where possible, preserving
already-visible tiles. Bulk construction may use only the thumbnail loader's
in-memory LRU; synchronous disk-cache decoding on the GTK thread is forbidden.
When a sparse inserted item lacks a cached thumbnail, wait for asynchronous
generation before adding its FlowBox child so it does not appear as a gray
placeholder.

For large Photos libraries, `VirtualMediaGrid` keeps the database as the full
source of truth. The shared `media_list` supplies instant-first-paint seed data;
afterward the virtual list model owns one lightweight slot per library item and
loads only viewport-adjacent ranges through `MediaRepository`. A layout or range
generation invalidates stale worker results, and the range coordinator coalesces
rapid scrollbar-drag targets so only the newest necessary request follows the
in-flight one. When a range lands, the model replaces ready slots in place and
evicts distant ready data; it never rebuilds a FlowBox page. Thumbnail prewarm
is redirected to the current viewport offset, while visible requests retain
their higher priority. See [`storage.md`](storage.md) "Thumbnails".

**FlowBox tile reuse must detach cleanly.** On search preview surfaces,
`detach_reusable_loaded_tiles` must call `set_child(None)` before clearing a
`FlowBoxChild`; GTK finalization can otherwise leave a reused tile parented and
blank. This is not part of the Photos renderer. `ui_media_list_cap` remains the
safety cap for the shared live projection, while `MediaGrid`'s render limits
apply only to its bounded FlowBox surfaces. Photos initializes only the visible
Day `VirtualMediaGrid`; Year and Month remain inactive until selected, so they
do not create metadata or range work while hidden.

Progressive first-page rendering remains an optimization for eligible bounded
`MediaGrid` surfaces such as search previews. It must not be reintroduced as a
Photos FlowBox fallback; Photos first paints its virtual seed and then lets GTK
recycle cells as authoritative ranges arrive.

Browsing identity uses stable `MediaId` values. Grid activation and multi-select
callbacks must pass media ids across widget/page boundaries; indexes are local
to a current visible window only.
The `ui::models::media_window_model::MediaWindowModel` is the intended owner of
visible-window state (`MediaQuery`, total count, window start, generation, and
the GTK `ListStore` projection). Batch actions, selection state, viewer
activation, and cross-async work should use `MediaId`; indexes are render-local
only.

Virtual-grid thumbnail requests are driven by the current visible range, not by
tile map signals. The range model keeps visible items and an overscan window
resident, prioritizes their thumbnail work, and uses the `MediaItem` metadata
already fetched from the database (including `file_mtime`); never add per-tile
filesystem metadata calls on the GTK thread. Factory binding may use only the
in-memory thumbnail cache synchronously; disk-cache reads and generation stay
off the GTK thread.

`VirtualMediaGrid` loads the full live count and per-mode section counts through
`MediaRepository` after its seed paint. Those counts define the layout index and
floating-date projection, so a bounded shared `media_list` can never truncate a
year, month, or day section's logical extent. Metadata queries must remain off
the GTK thread and stale results must be ignored by generation. A metadata
layout replacement must preserve the current logical media anchor and restore
its scroll position after GTK allocates the replacement model; metadata-only
updates such as batch favorite changes must not jump the Photos grid to top.
When the batch toolbar is visible, the favorite path first transfers focus to a
currently visible GridView item before hiding it. This prevents GTK from
briefly focusing the first GridView item. A single-item context-menu favorite
has no selection controls to hide and therefore leaves the right-clicked tile
focused. The custom GridView context menu keeps focus on that exact tile while
its non-focusable overlay is open, then returns to it before removal. It must
not use adjustment restoration or intercept normal wheel/touchpad scrolling as
a focus workaround.
Favorite-only mutations must not replace the authoritative virtual layout: they
do not change the live Photos query's ordering, sections, or count. Update the
resident model snapshots and realized Day-view heart badges in place, and
briefly suppress the matching shared-ListStore metadata reload; a full
`items_changed(0, old_slots, new_slots)` rebinds visible thumbnails and causes
a perceptible image flash.
The custom GridView context-menu layer temporarily owns keyboard focus so
Escape can dismiss it; it must return focus to its active GridView item before
removing the layer. The captured-offset watcher remains only as a fallback,
rather than letting GTK visibly fall back to the GridView's first item and
scroll the viewport to top.

Media activation is debounced while opening `ViewerPage` on the shared `AdwNavigationView`. Rapid repeated clicks in Year/Month/Day views must open only one viewer page: every viewer entry point (Photos, album details, search results) arms a short `viewer_open_pending` window that ignores duplicate activations during the push transition. There is no initial-open navigation-pop guard — a second click does not close the viewer (it cannot produce a pop), and an immediate back / Escape / swipe-back right after opening is intentional user input and is honored, so `can_pop` stays true from the first `show_at`.

Photos multi-select state is owned by each `VirtualMediaGrid` as a stable
`MediaId` set. A recycled factory cell derives its checkmark from that set when
it binds; selection must not depend on a realized cell or a FlowBox child.
Changing selection updates the CSS class only on realized factory cells whose
`MediaId` changed; it must not emit a `ListModel` replacement, because even one
replacement can recreate a GridView cell, reset its scroll adjustment, and
flash the page.
Context-menu entry enables multi-select before selecting its target, and
`clear_selection` updates every mode grid. See [`ui-design.md`](ui-design.md)
"Media Grids And Tiles".
Photos page "Select All" is intentionally capped at 2,000 live media items. For
large virtualized libraries it loads the first 2,000 ids from the database's
canonical live ordering, not from the current GTK seed or ready range.
`VirtualMediaGrid` may therefore hold selected ids outside its currently
realized factory cells.

Photo grid right-click actions use the custom overlay `GlassContextMenu` rather
than `GtkPopover`, so they render through the same page-overlay path as the
Year/Month/Day selector. Keep button-triggered popovers separate from this
right-click menu path.

While the Photos grid is scrolled, a compact glass date pill appears just left
of the scrollbar and tracks the thumb vertically, fading out ~700ms after
scrolling stops. It shows the date section at the current scroll position in the
active mode (Year/Month/Day). The date is resolved from the virtual layout's
section counts and top physical slot, NOT by reading realized tiles — so it
stays correct in unloaded ranges. It is hidden when library metadata has not
loaded, the library is empty, or there is a single section. The pill is `can-target: false`
(click-through) and reuses `.glass-raised`; it is Photos-page only (album detail
pages are a follow-up).

The virtual layout index resolves the date from the top physical slot rather
than realized tile widgets, so dragging through unloaded ranges and switching
to Year or Month remains immediate. The Photos renderer has no section-heading
widgets; deterministic filler slots give each section a clean row boundary while
the pill supplies the floating date context.

## Mode Selector

The Year/Month/Day control is both navigation and the canonical Liquid Glass segmented control. Preserve its visual structure:

- One outer raised glass capsule.
- Equal-width internal segments.
- Active state shown through label contrast and a short bottom indicator.
- No per-segment active background block.

Reusable segmented classes are documented in [`ui-liquid-glass.md`](ui-liquid-glass.md).

Mode switching is instrumented for Chrome/Perfetto traces from selector input
through stack notification, active-grid sync, virtual-grid activation, layout,
and range-loading phases. See [`diagnostics.md`](diagnostics.md) for the span
names and how to enable `PHOTOVIEWER_CHROME_TRACE`.

## Layout Pitfalls

Do not add fixed bottom padding to the grid to reserve space for the floating selector. That creates dark empty bands and weakens the backdrop effect. The selector should float as overlay chrome above real content.

When changing grid sizing, verify Day view separately because it has the densest section/header behavior.
