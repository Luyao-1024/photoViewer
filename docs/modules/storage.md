# Storage Module

## Scope

Storage covers SQLite schema/migrations, media rows, filesystem scanning, metadata extraction, live filesystem watching, thumbnails, and preferences.

## Key Files

| File | Role |
|---|---|
| `src/core/db.rs` | SQLite pool, migrations, pragmas |
| `src/core/schema.sql` | Embedded schema |
| `src/core/media.rs` | `MediaItem` and insert/update model |
| `src/core/backend/local.rs` | Local filesystem scanner |
| `src/core/backend/scan_worker.rs` | Background scan worker |
| `src/core/metadata.rs` | EXIF metadata extraction |
| `src/core/notify_watcher.rs` | Incremental filesystem watcher |
| `src/core/media_change_notifier.rs` | Change notification plumbing |
| `src/core/thumbnails.rs` | Thumbnail queue/cache/loader |
| `src/core/cache.rs` | Cache utilities |
| `src/core/prefs.rs` | User preferences |

## Database

SQLite uses an r2d2 connection pool with WAL and foreign-key pragmas applied through the pool init hook. `schema.sql` is embedded with `include_str!`; migrations are expected to be idempotent.

Live photos and trashed photos are separated with `trashed_at IS NULL` query/index behavior. Keep this distinction intact when changing media queries.

## Media Model

`MediaItem` values are wrapped in `glib::BoxedAnyObject` when surfaced to GTK model stores. Core code should stay independent from widget ownership even though UI adapters use GLib object wrappers.

## Metadata Extraction (`metadata.rs`)

`extract()` reads dimensions (via `image::image_dimensions`, falling back to gdk-pixbuf) and EXIF (DateTimeOriginal + readable fields) via kamadak-exif.

**HEIC/HEIF needs a dedicated EXIF path.** kamadak-exif's `read_from_container` *can* parse the ISOBMFF container, but it caps the Exif item at `MAX_EXIF_SIZE = 65535` bytes. Camera phones (iPhone, many Androids) embed a high-resolution JPEG thumbnail inside the Exif item, pushing it to several hundred KB, so kamadak-exif rejects those files with "Exif data too large" and EXIF silently comes back empty. `read_exif` therefore routes `image/heic` through a small in-tree ISOBMFF parser (`extract_heic_exif_tiff` and helpers) that locates the `Exif` item via `meta`/`iinf`/`iloc`, gathers its bytes (construction methods 0 and 1), strips the 4-byte `tiff_header_offset` prefix, and hands the raw TIFF block to `exif::Reader::read_raw` (no size cap). Do not "simplify" this back to `read_from_container` for HEIC — it reintroduces empty-EXIF for real phone photos. The regression is guarded by `oversized_heic_exif_item_is_recovered` in `metadata.rs`.

## Scanning And Watching

The local backend scans filesystem paths and inserts/updates media rows. The watcher handles incremental changes after startup. Changes to scanner behavior should consider:

- New supported file extensions.
- Metadata extraction failures.
- Duplicate paths.
- Deletions and trash transitions.
- UI change notification timing.

**Watcher must not hard-delete trashed rows.** When the app moves a photo to trash, `gio::File::trash()` relocates the file out of the watched directory, so the watcher sees the original path disappear. `db::delete_media_by_path` therefore filters with `AND trashed_at IS NULL`: a row the app has flagged via `mark_trashed` is preserved even though its original path is gone, so `list_trashed_media` keeps returning it for the Trash page. Removing that clause reintroduces "trash page shows nothing after deleting to trash."

**Trash flow must mark the DB row before moving the file.** Both deletion entry points go through `trash::move_to_trash_marked`, which runs `db::mark_trashed` *first* and then `gio::File::trash()`. This ordering is what makes the `AND trashed_at IS NULL` guard effective: gio's move is slow (writes `.trashinfo` + rename) and fires the watcher's Remove event before a separate `mark_trashed` would commit, so "move then mark" lets the watcher delete the still-un-trashed row — seen as "deleted several photos but Trash only shows one." If the move fails, `move_to_trash_marked` rolls back with `db::unmark_trashed`. Do not inline a move-then-mark sequence elsewhere.

**Re-indexing a present file clears `trashed_at` (external restore).** Restoring a photo from the system trash via the file manager makes it reappear at its original path; the app must notice. `LocalBackend::upsert` sets `trashed_at=NULL` on every existing-row update — a trashed row whose file is present was restored, so it becomes live again and the `MediaChangeNotifier::Upserted` event re-adds it to the Photos grid. To keep the startup scan from short-circuiting such a row, `db::is_media_unchanged` also filters `AND trashed_at IS NULL`, so a restored file is re-upserted (not skipped) even when its mtime/size are unchanged. The Trash view itself is rebuilt fresh on each navigation, so a restored item disappears from it on next open.

**Startup reconciles the system trash into the DB (`trash::reconcile_trash`), bidirectionally.** The Trash view is a DB projection (`trashed_at IS NOT NULL`), not a live mirror of `~/.local/share/Trash`. At startup, after the pictures scan (so externally-restored files are already live again), `reconcile_trash` makes the DB match the system trash:
- **Add:** for each `info/*.trashinfo` whose decoded `Path=` is under the pictures dir and no longer present, insert a trashed row (metadata from the `Trash/files` copy via `LocalBackend::process_file_at`, recorded under the **original** uri/path) or mark an existing live row trashed. Files from outside the pictures library are ignored.
- **Prune:** for each DB trashed row, if the original path is gone AND the system trash no longer has a matching entry (`find_trash_entry` is `None`), delete the row — it was emptied/permanently-deleted externally. Rows whose original file is present (restored) are never pruned here; the scan already turned them live.

It is idempotent and runs before the first grid page loads, so added rows land in `list_trashed_media`, not the live grid, and pruned rows disappear from the Trash view.

**The system trash is also watched live (`notify_watcher`).** In addition to the pictures dir, the watcher installs inotify on the trash roots. Events whose path is under a trash root are NOT treated as pictures upsert/delete — they set a dirty flag, and after a ~400ms quiet period (debounce; gio's "empty trash" bursts many events) the watcher re-runs `reconcile_trash` and emits `MediaChangeEvent::TrashChanged`. The UI consumer (`app.rs`) calls `MainWindow::refresh_visible_trash_page()` on that event, so an open Trash view reflects external restore/empty/delete without a page switch. External restore is also caught by the pictures-dir watcher (file reappears → upsert clears `trashed_at`); the trash watcher's `TrashChanged` then makes the visible Trash view drop it.

## Thumbnails

`ThumbnailLoader` owns an mpsc queue feeding blocking workers. Requests return textures through `oneshot::Sender`.

Cache keys include path and mtime, hashed with blake3, so file modifications invalidate prior thumbnails. Disk cache is bucketed by requested size and an in-memory LRU avoids unnecessary decoding. Opaque thumbnails are cached as JPEG; thumbnails with transparency are cached as lossless WebP so transparent PNG screenshots do not gain white edges. The disk hash includes a thumbnail-cache version prefix, so format changes invalidate older cached files automatically.
