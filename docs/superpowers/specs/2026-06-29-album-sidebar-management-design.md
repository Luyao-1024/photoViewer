# Album Sidebar Management Design

## Summary

Change the main sidebar album section from a capped preview with a "More" row
into a fixed-height, independently scrollable album list. Every album remains
available directly in the sidebar through mouse-wheel scrolling. Add a
right-click album menu with management and deletion actions, plus multi-select
support for deleting multiple real folder albums.

## Goals

- Remove the sidebar "More" album row and the `AllAlbums` sidebar target.
- Show all virtual and folder albums in the sidebar album section.
- Keep Photos, the Albums header, Trash, and Settings stable while only album
  rows scroll.
- Preserve existing album row behavior: click opens `AlbumDetailPage`, counts
  stay live, drag-to-reorder persists order, and the active album can be
  reselected after refresh.
- Add a right-click menu on album rows:
  - Manage Album opens that album's detail page.
  - Delete Album deletes one real folder album by moving its media to system
    trash.
- Support selecting multiple real folder albums and deleting them in one
  operation.
- Keep virtual albums (`Favorites`, `Photos`, `Videos`) non-deletable.

## Non-Goals

- Do not add user-authored album collections. Albums remain derived from media
  rows and folder paths.
- Do not permanently delete media from album deletion.
- Do not delete filesystem directories.
- Do not redesign `AlbumBrowserPage` unless dead code removal is required after
  sidebar navigation no longer references it.

## Sidebar Layout

The sidebar should use separate stable regions:

- Top navigation row: Photos.
- Albums group header: non-selectable, still toggles expanded/collapsed state.
- Albums scroll region: a `Gtk.ScrolledWindow` containing an album `Gtk.ListBox`.
- Bottom navigation row: Trash.
- Footer: Settings button.

The album scroll region has a fixed or bounded height so a large album list
does not push Trash or Settings out of place. When collapsed, the whole album
scroll region is hidden. When expanded, mouse-wheel scrolling over the album
region reveals all album rows.

`MainWindow` should stop indexing album rows inside the same `Gtk.ListBox` as
Photos and Trash. Instead, keep separate target mappings for:

- Main sidebar rows: Photos, Albums header, Trash.
- Album list rows: `Album`.

This avoids fragile cross-list index coupling and makes the scroll region a
clear boundary.

## Album Row Interactions

Clicking or activating an album row opens `AlbumDetailPage` exactly as it does
today. The implementation should reuse `MainWindow::open_album`.

Drag-to-reorder remains available on album rows. The persisted order should be
derived from all album rows currently rendered in the album list, not from a
15-row prefix.

Right-clicking an album row opens a `gtk::Popover` using the existing
`glass-menu`, `glass-menu-list`, `glass-menu-item`, and danger/suggested menu
classes.

Menu actions:

- Manage Album: opens the album detail page.
- Delete Album: visible only for non-virtual albums. It prompts for
  confirmation, then moves every media item in that album folder to system
  trash and refreshes sidebar/page state.

Virtual album rows still allow Manage Album, but do not show Delete Album.

## Multi-Select Albums

Album multi-select is scoped to album management, not general navigation.
Entering selection mode should let the user select multiple album rows and
perform a batch delete. Only real folder albums are eligible for delete; virtual
albums are not selectable in album-delete selection mode, so the destructive
count always reflects real folder albums only.

Batch delete flow:

1. User enters multi-select from the album row context menu.
2. User selects one or more real folder albums.
3. User confirms Delete Selected Albums.
4. The app moves all media items belonging to those folders to system trash.
5. The app refreshes the media list, album sidebar, and any visible album page.

If a selected album is already empty by the time deletion runs, it should be
skipped.

## Deletion Semantics

Deleting an album means moving the album's media to the system trash through
the existing trash pipeline. It does not hard-delete files, delete directories,
or remove only `albums` rows. Because `albums` is a derived materialized view,
the album disappears after its media rows are marked trashed and album refresh
rebuilds the projection.

The core operation should live outside GTK UI code, preferably in
`src/core/album_ops.rs` or `src/core/repository.rs`, and should reuse
`MediaRepository::move_to_trash` or `trash::move_to_trash_marked` so DB and
filesystem behavior remains consistent with photo deletion.

The operation must reject virtual albums.

## Refresh And Navigation

After album deletion:

- Refresh the main media list so trashed items disappear from Photos.
- Refresh album rows so counts and removed albums update immediately.
- If the deleted album detail page is visible, return to Photos root or show an
  empty/currently refreshed state without leaving a stale page.
- If another visible album page is still valid, refresh its contents.

`refresh_album_rows` should no longer refresh `AlbumBrowserPage` as a sidebar
"More" destination, unless the page remains reachable elsewhere.

## Tests

Use test-first changes for behavior:

- `tests/sidebar_navigation.rs`
  - More than 15 albums render all album rows in the sidebar album list.
  - No `AllAlbums`/More target exists.
  - Trash remains a stable main sidebar row and is not pushed after album rows.
  - Collapse hides the album scroll region and expand shows it again.
- Album context menu test
  - Real album right-click menu contains Manage Album and Delete Album.
  - Virtual album right-click menu contains Manage Album and omits Delete Album.
  - Menu uses existing glass menu classes.
- Album deletion core/UI test
  - Deleting a real folder album marks all its media trashed through the existing
    trash/repository path.
  - The album disappears from `list_with_favorites` after refresh.
  - Virtual album deletion is rejected or unavailable.
- Multi-select album deletion test
  - Selecting multiple real albums and confirming delete trashes media from all
    selected folders.

Run the smallest relevant tests first, then broader verification:

- `cargo test --test sidebar_navigation`
- Targeted album deletion/context tests
- `cargo test`

## Documentation Updates

Update:

- `docs/modules/albums-trash.md` to describe full sidebar album scrolling,
  removed More row, right-click management, and trash-backed album deletion.
- `docs/modules/ui-design.md` to replace the old "More when count exceeds 15"
  sidebar rule with the fixed-height scroll region rule.
