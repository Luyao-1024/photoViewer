# Albums And Trash Modules

## Scope

Albums expose folders as browsable collections. Trash integrates system trash behavior with restore and delete flows.

## Key Files

| File | Role |
|---|---|
| `src/core/albums.rs` | Album query/model helpers |
| `src/core/album_ops.rs` | Album operations |
| `src/core/trash.rs` | Trash operations |
| `src/ui/albums_page.rs` | Albums overview |
| `src/ui/album_detail_page.rs` | Album detail grid |
| `src/ui/trash_page.rs` | Trash UI and actions |
| `data/ui/albums-page.blp` | Albums template |
| `data/ui/album-detail-page.blp` | Album detail template |
| `data/ui/trash-page.blp` | Trash template |
| `tests/e3e_albums_trash.rs` | End-to-end albums/trash flow |
| `tests/trash_flow.rs` | Trash behavior |

## Albums

Albums are mostly folder-derived rather than a separate user-authored collection model. Keep album covers and counts derived from media rows so scanner/database state remains the source of truth.

The Albums page also has virtual logical albums:

- Favorites: filtered by `is_favorite`.
- Photos: filtered by `media_items.media_kind = 'image'`.
- Videos: filtered by `media_items.media_kind = 'video'`.

The Photos and Videos virtual albums are type-based only. Do not infer them from folder paths; images and videos may live under either picture or video roots.

## Trash

Trash views must distinguish reversible trash state from permanent delete. Database state and filesystem state need to remain consistent across restore/delete operations.

**Trashed files live in the HOST `~/.local/share/Trash`, not the sandbox `XDG_DATA_HOME/Trash`.** Under Flatpak the gvfs trash backend runs on the host, so `gio::File::trash()` moves files to `~/.local/share/Trash/files/` even though the sandbox sees a per-app `XDG_DATA_HOME`. `src/core/trash.rs` therefore searches both roots, scans every `.trashinfo` (gio collision suffixes can start at `.0`), and percent-decodes the `Path=` field (non-ASCII like `图片` is stored as `%E5%9B%BE%E7%89%87`). Thumbnail decoding, restore, and permanent-delete all depend on this resolution being correct.

**The Trash view is fully reconciled with the system trash at startup (`trash::reconcile_trash`), and kept live thereafter.** Bidirectional: it adds trashed rows for trash entries whose original path was under the pictures dir (inserting from the `Trash/files` copy under the original uri, or marking an existing live row), and prunes DB trashed rows whose file is no longer in the system trash (externally emptied). Restored files (original present) are left to the scan. The watcher also watches the trash dir: external restore/empty/delete is debounced → re-reconciled → `TrashChanged` → the visible Trash view refreshes without a page switch. See [`storage.md`](storage.md).

When touching trash flows, verify:

- Moving an item to trash hides it from live photo queries.
- The trashed row survives the filesystem watcher's removal event (the original file is gone, but the row must still appear in the trash view). See [`storage.md`](storage.md): `delete_media_by_path` filters `AND trashed_at IS NULL`.
- Restoring makes it visible again.
- Permanent delete removes the expected record/file state.
- Multi-select actions keep selection and empty states coherent.
