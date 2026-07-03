# Albums And Trash Modules

## Scope

Albums expose folders as browsable collections. Trash integrates system trash behavior with restore and delete flows.

## Key Files

| File | Role |
|---|---|
| `src/core/albums.rs` | Album query/model helpers |
| `src/core/album_ops.rs` | Album operations |
| `src/core/trash.rs` | Trash operations |
| `src/ui/window.rs` | Sidebar: lists albums directly under the Albums group header |
| `src/ui/album_detail_page.rs` | Album detail grid + bounded album filtering helper |
| `src/ui/trash_page.rs` | Trash UI and actions |
| `data/ui/album-detail-page.blp` | Album detail template |
| `data/ui/trash-page.blp` | Trash template |
| `tests/album_navigation.rs` | Album detail + viewer push |
| `tests/album_order.rs` | Persistent sidebar album ordering (`set_album_order`) |
| `tests/e3e_albums_trash.rs` | End-to-end albums/trash flow |
| `tests/trash_flow.rs` | Trash behavior |

## Albums

Albums are mostly folder-derived rather than a separate user-authored collection model. Keep album counts derived from media rows so scanner/database state remains the source of truth.

Album rows are a derived projection over media rows. New refresh paths should
route through `core::refresh::RefreshCoordinator` so album rebuilds are
single-flight and repeated startup/watch events coalesce. UI pages should avoid
adding new direct `albums::refresh` calls; trash and favorite mutations should
go through `MediaRepository` so DB updates, filesystem side effects, and derived
album refreshes stay behind one core boundary.

Startup must not synchronously block on sidebar album projections. The window
initially shows the Photos row count from the already-loaded GTK model window,
then `MainWindow::refresh_sidebar_snapshot_async()` refreshes the true live
count, folder albums, virtual album counts, and media-type rows from a blocking
worker after the Photos page is usable.

Album covers are persisted separately from the `albums` materialized view in
`album_covers(folder_path, cover_uri)`. `albums::refresh` applies the same
priority everywhere: explicit user cover first, newest live media in that album
second, and no cover only for empty albums. Keep the override table independent
from `albums` so scan/refresh rebuilds do not erase user choices.

Albums are shown under a collapsible "Albums" group header. The group owns a
fixed-height scroll region in the sidebar: Photos, media-type categories,
Trash, and Settings remain stable while the album rows themselves scroll. All
virtual and folder albums are rendered directly in that scroll region; there is
no "More" row in the sidebar. Sidebar album rows show album cover thumbnails,
not symbolic folder/type icons. Covers load through `ThumbnailLoader` so row
construction does not decode media on the GTK main thread.

Below Albums, the sidebar has a matching collapsible "Media Types" group. It
uses the same sub-row visual treatment and opens the same `AlbumDetailPage`
virtual-album flow, but it is not part of album drag ordering or album deletion.
The only current media-type row is Dynamic Photos, backed by
`media_items.media_subkind = 'motion_photo'`.

Selecting an album row pushes its `AlbumDetailPage` immediately. The per-album
media list is built by `album_detail_page::filtered_items_for_album`. Virtual
albums (Favorites, Photos, Videos, and media-type rows such as Dynamic Photos)
load their membership from `MediaRepository` so they are not capped by the
startup GTK list window; real folder albums query the database by `folder_path`.
These album detail loads must stay behind `MediaRepository` and use SQL-level
filtering/counting; do not load the full live media table and filter in Rust
when switching albums. Opening an album synchronously loads only the initial
render window, then may backfill a bounded continuation window up to the UI
media-list cap. Do not backfill the full album into the GTK `ListStore`; viewer
navigation can resolve off-window neighbours through repository queries. A
favorite/trash change refreshes the visible virtual-album window and sidebar
counts without materializing the complete virtual album.

Right-clicking an album row opens the custom overlay `GlassContextMenu`, not a
`GtkPopover`, so the menu shares the same page-overlay glass rendering path as
the Year/Month/Day selector. "Manage Album" opens the album detail page. Real
folder albums also expose "Ignore Album" and "Delete Album". Ignore Album adds
the folder to `excluded_scan_roots`, immediately removes that folder's live
media rows from the database and visible GTK model, refreshes the derived album
list, and never touches the files on disk. Delete Album moves every media item
in that folder to the system trash and then refreshes the derived album list.
Virtual albums such as Favorites, Photos, and Videos are navigable but not
ignorable or deletable. Album multi-select is limited to deleting multiple real
folder albums through the same trash-backed operation.

Within an album detail page, right-clicking a media tile can set that item as
the album cover. The action writes `album_covers`, refreshes the sidebar, and
leaves media files untouched.

Album rows are **drag-to-reorder** (long-press + drag). The order is persisted in a standalone `album_order(folder_path, sort_order)` table — kept separate from the `albums` materialized view because that view is `DELETE`d and rebuilt on every `albums::refresh` (scan / add-to-album). `albums::set_album_order` writes the full top-to-bottom order (keyed by `folder_path`, so virtual albums reorder too); `albums::list_with_favorites` re-applies it via `apply_saved_order`, and albums with no saved order fall to the end in their default relative order. In the UI, `MainWindow::attach_album_dnd` wires a per-row `DragSource` (payload = `folder_path`) + `DropTarget` (above/below indicator) that call `MainWindow::reorder_album` to persist and rebuild.

`AlbumBrowserPage` uses the same persistent ordering for its full album grid.
Each album card is a drag source/drop target with the same `folder_path`
payload; dropping on the upper/lower half inserts before/after that card,
writes the complete order through `albums::set_album_order`, refreshes the
page, and notifies `MainWindow` to rebuild the sidebar rows.

The Albums page also has virtual logical albums:

- Favorites: filtered by `is_favorite`.
- Photos: filtered by `media_items.media_kind = 'image'`.
- Videos: filtered by `media_items.media_kind = 'video'`.

The Photos and Videos virtual albums are type-based only. Do not infer them from folder paths; images and videos may live under either picture or video roots.

Settings includes an Album Management section for scan paths. "Additional scan folders" adds custom roots to future scans. "Excluded scan folders" skips matching folders during startup scans and filesystem watching. Excluding a folder is not album deletion: it must not move files to trash, permanently delete files, or invoke `album_ops::delete_albums_to_trash`.

## Trash

Trash views must distinguish reversible trash state from permanent delete. Database state and filesystem state need to remain consistent across restore/delete operations.

**Trashed files live in the HOST `~/.local/share/Trash`, not the sandbox `XDG_DATA_HOME/Trash`.** Under Flatpak the gvfs trash backend runs on the host, so `gio::File::trash()` moves files to `~/.local/share/Trash/files/` even though the sandbox sees a per-app `XDG_DATA_HOME`. `src/core/trash.rs` therefore searches both roots, scans every `.trashinfo` (gio collision suffixes can start at `.0`), and percent-decodes the `Path=` field (non-ASCII like `图片` is stored as `%E5%9B%BE%E7%89%87`). Thumbnail decoding, restore, and permanent-delete all depend on this resolution being correct.

**The Trash view is fully reconciled with the system trash at startup (`trash::reconcile_trash`), and kept live thereafter.** Bidirectional: it adds trashed rows for trash entries whose original path was under the pictures dir (inserting from the `Trash/files` copy under the original uri, or marking an existing live row), and prunes DB trashed rows whose file is no longer in the system trash (externally emptied). Restored files (original present) are left to the scan. The watcher also watches the trash dir: external restore/empty/delete is debounced → re-reconciled → `TrashChanged` → the visible Trash view refreshes without a page switch. See [`storage.md`](storage.md).

Trash UI loads a bounded initial repository page capped by
`runtime_config::ui_media_list_cap()`; do not populate the GTK model with the
entire trash table during page construction. Viewer previous/next from Trash
uses a DB-level `trashed_at DESC, id DESC` neighbor query, so navigation should
not re-materialize the full trash page either. Empty Trash is the only flow that
intentionally walks the full trash set because it is applying a destructive
operation to every item.

When touching trash flows, verify:

- Moving an item to trash hides it from live photo queries.
- The trashed row survives the filesystem watcher's removal event (the original file is gone, but the row must still appear in the trash view). See [`storage.md`](storage.md): `delete_media_by_path` filters `AND trashed_at IS NULL`.
- Restoring makes it visible again.
- Permanent delete removes the expected record/file state.
- Multi-select actions keep selection and empty states coherent.
