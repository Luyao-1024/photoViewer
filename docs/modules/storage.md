# Storage Module

## Scope

Storage covers the SQLite schema, media rows, filesystem scanning, metadata extraction, live filesystem watching, thumbnails, and preferences.

## Key Files

| File | Role |
|---|---|
| `src/core/db.rs` | SQLite pool, current-schema initialization, pragmas |
| `src/core/schema.sql` | Embedded schema |
| `src/core/media.rs` | `MediaItem`, media kind helpers, and insert/update model |
| `src/core/backend/local.rs` | Local filesystem scanner |
| `src/core/backend/scan_worker.rs` | Background scan worker |
| `src/core/metadata.rs` | EXIF metadata extraction |
| `src/core/notify_watcher.rs` | Incremental filesystem watcher |
| `src/core/media_change_notifier.rs` | Change notification plumbing |
| `src/core/thumbnails.rs` | Thumbnail loader public API, shared state, in-memory cache, and request entry points |
| `src/core/thumbnails/queue.rs` | Thumbnail worker queue, priority pop, background pull, and in-flight tracking cleanup |
| `src/core/thumbnails/decode.rs` | Thumbnail image decode, fallback placeholders, atomic cache writes, and brightness sampling |
| `src/core/thumbnails/cache.rs` | Thumbnail cache keys, cache paths, and synchronous cache decoding |
| `src/core/thumbnails/jpeg_turbo.rs` | JPEG signature checks and libjpeg-turbo scaled decode FFI |
| `src/core/thumbnails/video.rs` | Video thumbnail extraction, ffmpegthumbnailer/GStreamer fallback, and play badge overlay |
| `src/core/cache.rs` | Cache utilities |
| `src/core/prefs.rs` | User preferences |
| `src/core/runtime_config.rs` | Runtime sizing, loading, and worker strategy config |

Storage/core unit tests live in child test modules instead of inline source
blocks. Production source files declare `#[cfg(test)] mod tests;`, with test
bodies in paths such as `src/core/metadata/tests.rs`,
`src/core/trash/tests.rs`, `src/core/thumbnails/tests.rs`, and
`src/core/runtime_config/tests.rs`.

## Database

SQLite uses an r2d2 connection pool with WAL, foreign-key, and a 10-second
busy timeout applied through the pool init hook. SQLite still permits only one
writer at a time even in WAL mode; the timeout lets the filesystem watcher,
startup scan, thumbnail workers, and foreground mutations wait through short
writer contention instead of reporting a spurious `database is locked` error.
`schema.sql` is embedded with `include_str!` and creates the current schema for
new databases. `init_pool` also runs transactional, versioned migrations using
SQLite `PRAGMA user_version`; the version-1 migration upgrades historical
unversioned libraries in place and preserves media, favorites, album order,
and custom covers. A database newer than the running application is rejected
without modification.

UI-facing database access should go through `core::repository::MediaRepository`.
`core::db` remains the low-level SQL module, but widgets and pages
should not grow new direct calls to it. Repository methods return task-oriented
snapshots and mutations (`MediaPage`, `MediaMutation`, `FavoriteSummary`) keyed
by stable `MediaId` values, so future SQL optimizations can stay behind this
boundary.

Viewer inline rename also goes through `MediaRepository::rename_media_file`.
That path renames the filesystem entry first, preserves the original extension
instead of accepting a new suffix from UI text, then calls
`db::update_media_location` so `uri`, `path`, `folder_path`, and `file_mtime`
stay in sync for images and videos.

Runtime change notifications use `core::events::DomainEvent` as the shared
vocabulary and receiver payload. `MediaChangeNotifier` remains as the
scanner/watcher producer facade, but its channel emits domain events directly.
UI projections such as the bounded `gio::ListStore` consume those domain
events through explicit adapters rather than through a second event vocabulary.
The authoritative `DomainEventSender` channel is bounded and lossless: DB and
filesystem worker threads accept backpressure rather than allowing an
unbounded UI event backlog.

All production SQLite writes go through the single `core::db_actor::DbActor`
connection owner. `DbActorHandle` accepts a priority queue: user-interactive
mutations run first, followed by trash operations, filesystem watcher changes,
startup scan work, derived album refreshes, and thumbnail bookkeeping. Commands
with the same priority are FIFO. A running SQL transaction is not interrupted;
priority is applied at the next queue selection.

Every actor envelope also carries the reusable `OperationTrace` context. A
plain `execute` / `execute_blocking` creates a `database` chain automatically;
a higher-level cross-thread operation can use `execute_in_trace` /
`execute_blocking_in_trace` to retain its original `operation_id`. The actor
records command name, item count, and priority-queue wait; its RAII stage closes
automatically when execution returns. It separately logs any command, enqueue,
or response failure at the boundary. It also emits
the independent `database` projection of a higher-level operation, so `-T
database` sees **every** actor command even when the caller is `scan`,
`thumbnail`, or `mutation` (selecting both views intentionally shows the same
actor time in both operation contexts). This keeps DB contention diagnosis
generic across scanning, watcher work, thumbnails, edits, and user mutations;
see `diagnostics.md` for selective `-T database` capture.

Filesystem work may happen outside the actor, but its database commit must be a
`DbCommand` (for example `UpdateMediaLocation`, `CommitMovedToTrash`, or
`RestoreTrashed`). The actor emits `DomainEvent` values; UI pages should
subscribe through `ui::refresh_hub` or the current legacy hub bridge instead of
manually deciding which views to refresh. Test-only low-level helpers may still
write directly to an isolated pool to set up fixtures.

Derived refresh work belongs in `core::refresh::RefreshCoordinator`. Album
refreshes are single-flight with pending replay. Thumbnail/library statistics
are read through `MediaRepository::library_stats()` so widgets consume a
projection (`LibraryStats`) instead of calculating progress from
`ThumbnailLoader` internals. The DB projection treats stale
`thumbnail_generated_at < file_mtime` rows as not generated.

Live photos and trashed photos are separated with `trashed_at IS NULL` query/index behavior. Keep this distinction intact when changing media queries.

The live media page query sorts by `COALESCE(taken_at, file_mtime) DESC, id DESC`
and must use the partial expression index `idx_media_live_sort`. Without that
index, large libraries can fall back to a full live-row scan plus a temporary
sort for every virtual page request.

Album detail and virtual album queries use the same canonical sort order after
filtering by `folder_path`, `is_favorite`, `media_kind`, or `media_subkind`.
Keep the filtered sort indexes (`idx_media_folder_sort`,
`idx_media_favorite_sort`, `idx_media_kind_sort`, `idx_media_subkind_sort`) in
place so switching between albums does not build temporary sort tables over
large media collections.

Viewer previous/next and prefetch use `MediaRepository::neighbor_item()` and must stay
behind DB-level neighbour queries for the same projections: live media,
search/search-kind results, folder albums, favorites, image/video type albums,
motion photos, and trash. Trash uses its own `trashed_at DESC, id DESC` order.
Avoid implementing viewer navigation by calling `page(query, 0, u32::MAX)` for
these projections; that materializes large result sets and makes repeated
left/right navigation scale with the whole album, search result, or trash table
instead of one adjacent row. The interactive path performs a `(sort key, id)`
index seek and does not compute `ROW_NUMBER`, a global count, or a full-table
offset.

## Media Model

`MediaItem` values are wrapped in `glib::BoxedAnyObject` when surfaced to GTK model stores. Core code should stay independent from widget ownership even though UI adapters use GLib object wrappers.

`media_items.media_kind` is the persisted primary media discriminator (`image` / `video`), derived from MIME at insert/update time. `media_items.media_subkind` is the secondary classification (`standard`, `motion_photo`), and `media_items.media_attributes` is JSON for subkind-specific details plus additive media attributes. `media_items.media_type_flags` materializes the queryable logical-album categories (motion photo, animated, HDR), so category reads never scan JSON. Dynamic photos remain `media_kind='image'` and set `media_subkind='motion_photo'`; their embedded video offsets/lengths live under the JSON `motion_photo` object. General attributes such as animated images and HDR media also live as top-level JSON booleans (`animated`, `hdr`) and are converted to the corresponding bit flags at ingestion. GIF files are scanned with `animated: true`. A Motion Photo Container with a `GainMap` item is HDR and must persist `hdr: true`; the checked-in `hdr_gain_map_motion_photo.jpg` fixture guards this. Keep media extension/MIME rules centralized in `src/core/media.rs` so scanner, watcher, metadata, thumbnails, and DB writes agree.

## Preferences

User preferences are stored as JSON in `settings.json` under `config_dir()`. The file is a preserved-key object: writing one preference must keep unrelated keys intact. Writes use a synced sibling temporary file plus atomic rename; malformed existing JSON is copied to a timestamped `.corrupt-*` backup before replacement. Current keys include `liquid_glass`, `liquid_glass_transparency` (clamped `0.0..=1.0`, default `0.0`; `0.0` is opaque and `1.0` is transparent), `video_default_muted` (default `true`), and `video_volume` (clamped `0.0..=1.0`, default `1.0`). Disabling `video_default_muted` also recovers `video_volume` to `1.0` when an earlier muted stream left a stale `0.0`, so "start unmuted" does not still produce silence; existing config files with `video_default_muted=false` and `video_volume=0.0` are treated the same way on read.

Scan path preferences also live in `settings.json`. `custom_scan_roots` is an array of absolute directories added after the default Pictures/Videos roots. `excluded_scan_roots` is an array of absolute directories skipped by startup scans and runtime filesystem watching. These settings affect indexing only: excluding a folder must not delete files from disk, and must not call trash/delete operations. The Settings dialog, scan path rows, restart prompts after scan/runtime changes, storage usage rows, and the combined cache-data cleanup dialog live in `src/ui/window/settings.rs`. That one destructive action deletes thumbnail files, resets media-library database content without touching original files or preferences, then automatically restarts the application.

Runtime sizing and loading strategy are stored separately in `runtime.json` under `config_dir()`. Missing or malformed files fall back to centralized defaults in `src/core/runtime_config.rs`; numeric values are clamped to at least `1`. Current runtime keys include `initial_media_page_size`, `virtual_media_page_size`, `ui_media_list_cap`, `max_rendered_grid_items`, `grid_render_absolute_cap`, `grid_render_expand_step`, `grid_reprioritize_debounce_ms`, `thumbnail_worker_count`, `thumbnail_speed_tier`, `thumbnail_queue_capacity`, `thumbnail_mem_cache_cap`, `thumbnail_disk_cache_bytes`, `thumbnail_prewarm_poll_ms`, `thumbnail_idle_wait_ms`, `notify_trash_debounce_ms`, and `notify_file_settle_ms`. `thumbnail_mem_cache_cap` defaults to 512 foreground textures; prewarm has a separate 64-entry cache and cannot evict recently browsed thumbnails. Photos always uses `VirtualMediaGrid`; `photos_grid_backend` is no longer a supported key, and a value left by an older build is ignored. The Settings page exposes the thumbnail generation speed as a horizontal radio selector with four tiers: slow = 1 worker, normal = 2 workers (the default), fast = 4 workers, fastest = CPU physical-core count. The selected tier is persisted as the `thumbnail_speed_tier` string (`slow`/`normal`/`fast`/`fastest`, unambiguous) alongside the derived `thumbnail_worker_count` (still read by the worker pool at startup); on read the tier string wins, falling back to `from_worker_count` for configs written by older versions that only stored the number. The worker pool reads `thumbnail_worker_count` when the app starts, so changing the tier takes effect after restart; after a successful change, Settings asks whether to restart immediately.

Settings also shows thumbnail-cache and database sizes. Thumbnail-cache size is
computed by recursively walking the cache directory, so the Settings UI must
display a pending subtitle first and update the row from a background worker
after the scan finishes.

## Metadata Extraction (`metadata.rs`)

`extract()` reads dimensions (via `image::image_dimensions`, falling back to gdk-pixbuf) and EXIF (DateTimeOriginal + readable fields) via kamadak-exif.

**Motion photo structure is parsed by `motion_photo.rs`, not by UI code.** The scanner detects supported dynamic JPEG formats and persists the result in `media_subkind` / `media_attributes`. Current strategies are:
- Google/Xiaomi-style `GCamera:MicroVideoOffset`, where the XMP offset is the embedded MP4 length counted backward from EOF.
- Google Motion Photo `Container:Directory`, where appended items list a primary JPEG, optional GainMap JPEG, and `video/mp4` MotionPhoto item lengths in order.

Invalid or out-of-bounds motion metadata is ignored and the file remains a standard image. Viewer playback extracts only the persisted video byte range to a temporary MP4 and falls back to still-image display on failure.

**Video metadata comes from `ffprobe`.** For `video/*` items, `extract()` shells out to `ffprobe -print_format json -show_format -show_streams` (parsed with `serde_json::Value`, no `serde` derive) and fills `width`/`height`/`taken_at` (so videos sort and group by time like photos) plus a `VideoSummary` (duration, codec + profile, fps, bitrate, container, make/model). `video_duration_secs` is persisted on `media_items` for browsing badges; richer fields such as codec, fps, bitrate, container, and camera make/model are not persisted and are re-fetched at view time by the details panel, exactly like the EXIF camera summary. If `ffprobe` is missing or fails, the video branch returns only `mime_type` — non-fatal; the panel still shows name/type/size and the browsing badge falls back to an unknown duration label.
`ffprobe` and `ffmpegthumbnailer` run through the shared bounded subprocess
helper: each gets a 15-second timeout, a 4 MiB stdout/stderr cap, and an owned
process group that is terminated on timeout so helper descendants cannot leak.

**HEIC/HEIF needs a dedicated EXIF path.** kamadak-exif's `read_from_container` *can* parse the ISOBMFF container, but it caps the Exif item at `MAX_EXIF_SIZE = 65535` bytes. Camera phones (iPhone, many Androids) embed a high-resolution JPEG thumbnail inside the Exif item, pushing it to several hundred KB, so kamadak-exif rejects those files with "Exif data too large" and EXIF silently comes back empty. Both `metadata::read_exif` and `orientation::read_exif` therefore route `image/heic` through a shared in-tree ISOBMFF parser (`extract_heic_exif_tiff` and helpers) that locates the `Exif` item via `meta`/`iinf`/`iloc`, gathers its bytes (construction methods 0 and 1), strips the 4-byte `tiff_header_offset` prefix, and hands the raw TIFF block to `exif::Reader::read_raw` (no size cap). Do not "simplify" this back to `read_from_container` for HEIC — it reintroduces empty-EXIF for real phone photos and causes thumbnail generation failures ("Exif data too large"). The regression is guarded by `oversized_heic_exif_item_is_recovered` in `metadata.rs`.

**Display EXIF reads are lenient about truncated IFDs.** Every read-only EXIF path — the viewer's orientation read (`orientation::read_orientation_exif_lenient`) and the metadata/details path (`metadata::exif_from` / `read_exif` / `read_heic_exif_from_bytes`) — goes through a shared `lenient_exif_reader()` + `distill_partial_exif()` pair that opts into kamadak-exif's `continue_on_error` mode and salvages the intact primary IFD via `Error::distill_partial_result`. Phone-app re-encodes (WeChat, WhatsApp, …) often leave a half-written tail/thumbnail IFD whose dangling pointer makes kamadak-exif's default strict reader reject the *whole* file with `InvalidFormat("Truncated IFD count")`; without the lenient path the viewer failed to open such images and the details panel/scanner silently dropped their `taken_at`/camera fields. The lenient recovery is a display-only concern: `orientation::write_orientation` keeps a *strict* `read_exif` on purpose so a rotation click never silently rewrites a half-parseable EXIF block. The contract is guarded by `read_orientation_recovers_from_truncated_secondary_ifd` (`orientation`) and `exif_from_recovers_datetime_from_truncated_secondary_ifd` (`metadata`).

## Scanning And Watching

The local backend scans filesystem paths and inserts/updates media rows. Startup scans the de-duplicated media roots from `config::media_roots()` (`XDG_PICTURES_DIR`/`~/Pictures` or `~/图片`, plus `XDG_VIDEOS_DIR`/`~/Videos` or `~/视频`, plus `prefs::custom_scan_roots()`, minus roots that sit under `prefs::excluded_scan_roots()`). Images and videos can live under any included root. The scanner also prunes excluded directories during recursive walks, so excluding a child album folder still works when its parent remains an included root. The watcher receives the same excluded roots and ignores non-trash events under them after startup. Changes to scanner behavior should consider:

- New supported image/video file extensions.
- Metadata extraction failures.
- Duplicate paths.
- Deletions and trash transitions.
- UI change notification timing.

A root must be a readable directory immediately before scanning and again
before missing-file reconciliation. Unavailable/offline roots are skipped and
their existing DB rows are retained; absence of a mount is not evidence that
every file on it was deleted.

Startup must not wait for the full filesystem scan before showing Photos.
`app::initialize` loads only the first configured live DB rows
(`initial_media_page_size`, default 500) for the initial grid, using
`MediaRepository::items` so startup does not also block on a full live-media
count. Full-library counts, thumbnail stats, date section counts, and sidebar
album projections are refreshed afterward from background workers. The Photos
grid pages directly from the database as the user scrolls: its scrollbar uses
virtual top/bottom spacers derived from the full live-media count once that
background metadata has loaded, and scrolling near a global position swaps in a
configured DB window (`virtual_media_page_size`, default 500) around that
offset. While a requested page is still loading, the grid shows a skeleton
placeholder window at the target offset; rapid drag retargets use a generation
counter so stale DB page results are ignored. The GTK-facing `media_list` is
capped by `apply_to_media_list::ui_media_list_cap()` (from `runtime.json`,
default 1500 newest rows) for live scanner/watch merges; the database remains
the full source of truth for scans, search, album counts, thumbnail prewarm,
and virtual paging. Startup scan batches are ignored by the GTK model once that
cap is filled, preventing repeated visible grid rebuilds while the database scan
continues. The startup scan uses `LocalBackend::scan_and_upsert_dir_notify`,
preserving the `(uri, file_mtime, file_size)` unchanged-file short-circuit while
sending each actually upserted live `MediaItem` through
`MediaChangeNotifier::upserted` so the GTK consumer receives
`DomainEvent::MediaUpserted` and can merge it into the shared `media_list`
without letting UI memory grow with the full library.

For Photos, that same startup list is only an instant-first-paint seed and
domain-event signal. `VirtualMediaGrid`
gets `LiveAll` totals and per-mode section counts through `MediaRepository`,
then reads viewport-adjacent ranges with `MediaRepository::items` on blocking
workers. A stale result must be discarded when its layout/range generation no
longer matches the active grid. This does not widen the GTK-facing startup
projection and does not add UI SQL paths outside `MediaRepository`.

After the first page is shown, startup background work must complete these
storage tasks in order, off the GTK thread:

1. Start runtime filesystem watchers for media roots and existing trash roots.
2. Scan every configured media root with excluded directories pruned, upserting
   new or changed live media and batching `DomainEvent::MediaUpserted` events to
   the UI.
3. Reconcile indexed live rows against disk for the scanned roots: if a row is
   `trashed_at IS NULL`, is not under an excluded root, and its `path` no longer
   exists, delete that DB row and emit `DomainEvent::MediaRemoved`. This covers
   files deleted outside the app while it was closed. The filesystem stat pass
   runs in the background startup worker and produces a missing-row plan; only
   the short guarded batch delete is sent to the DB actor. The root/excluded
   scope and `trashed_at IS NULL` guard remain authoritative.
4. Refresh album projections/sidebar data after scan and prune have converged.
5. Reconcile known trash roots into the DB and emit `TrashChanged` so a
   visible Trash view refreshes.
6. Start thumbnail prewarm after scan and trash reconciliation; viewport
   thumbnail requests still take priority over background work.

Do not move the missing-file reconciliation into the synchronous first-page
startup path. It can touch many rows and filesystem paths, so the foreground
contract is eventual convergence via domain events, not blocking startup until
all stale rows have been pruned.

**Watcher must not hard-delete trashed rows.** When the app moves a photo to trash, `gio::File::trash()` relocates the file out of the watched directory, so the watcher sees the original path disappear. `db::delete_media_by_path` therefore filters with `AND trashed_at IS NULL`: a row the app has flagged via `mark_trashed` is preserved even though its original path is gone, so `list_trashed_media` keeps returning it for the Trash page. Removing that clause reintroduces "trash page shows nothing after deleting to trash."

**Trash flow must mark the DB row before moving the file.** Both deletion entry points go through `trash::move_to_trash_marked`, which runs `db::mark_trashed` *first* and then `gio::File::trash()`. This ordering is what makes the `AND trashed_at IS NULL` guard effective: gio's move is slow (writes `.trashinfo` + rename) and fires the watcher's Remove event before a separate `mark_trashed` would commit, so "move then mark" lets the watcher delete the still-un-trashed row — seen as "deleted several photos but Trash only shows one." If the move fails, `move_to_trash_marked` rolls back with `db::unmark_trashed`. Do not inline a move-then-mark sequence elsewhere.

**Re-indexing a present file clears `trashed_at` (external restore).** Restoring a photo from the system trash via the file manager makes it reappear at its original path; the app must notice. `LocalBackend::upsert` sets `trashed_at=NULL` on every existing-row update — a trashed row whose file is present was restored, so it becomes live again and the `DomainEvent::MediaUpserted` event re-adds it to the Photos grid. To keep the startup scan from short-circuiting such a row, `db::is_media_unchanged` also filters `AND trashed_at IS NULL`, so a restored file is re-upserted (not skipped) even when its mtime/size are unchanged. The Trash view itself is rebuilt fresh on each navigation, so a restored item disappears from it on next open.

**Startup reconciles known trash roots into the DB (`trash::reconcile_trash`), bidirectionally.** The Trash view is a DB projection (`trashed_at IS NOT NULL`), not a live mirror of only `~/.local/share/Trash`. At startup, after the pictures scan (so externally-restored files are already live again), `reconcile_trash` makes the DB match the system trash roots and the app-owned fallback trash root:
- **Add:** for each `info/*.trashinfo` whose decoded `Path=` is under the pictures dir and no longer present, insert a trashed row (metadata from the `Trash/files` copy via `LocalBackend::process_file_at`, recorded under the **original** uri/path) or mark an existing live row trashed. Files from outside the pictures library are ignored.
- **Skip non-media:** trash entries whose original path has no supported image/video extension are ignored before metadata extraction. Host trash roots may contain `.txt`, documents, and other files unrelated to the app, and those should not emit default warning logs.
- **Prune:** for each DB trashed row, if the original path is gone AND no known trash root has a matching entry, delete the row — it was emptied/permanently-deleted externally. Rows whose original file is present (restored) are never pruned here; the scan already turned them live.

It is idempotent and runs before the first grid page loads, so added rows land in `list_trashed_media`, not the live grid, and pruned rows disappear from the Trash view.

**Trash roots are also watched live (`notify_watcher`).** In addition to media roots, the watcher installs inotify on the system trash roots and app trash root. Events whose path is under a trash root are NOT treated as media upsert/delete — they set a dirty flag, and after the configured quiet period (`notify_trash_debounce_ms`, default ~400ms; gio's "empty trash" bursts many events) the watcher re-runs `reconcile_trash` and emits `DomainEvent::TrashChanged`. The UI consumer (`app.rs`) calls `MainWindow::refresh_visible_trash_page()` on that event, so an open Trash view reflects external restore/empty/delete without a page switch. External restore is also caught by the media-root watcher (file reappears → upsert clears `trashed_at`); the trash watcher's `TrashChanged` then makes the visible Trash view drop it.
Normal media events use the same quiet burst: changes are coalesced by path,
file settling happens once, metadata upserts are submitted in one batch, and
deletions use one transaction. Trash reconciliation is also two-phase: trash
root traversal/metadata extraction runs outside the DB actor and the actor only
commits the prepared row changes.

## Thumbnails

`ThumbnailLoader` owns a priority queue feeding blocking workers. Worker count, queue capacity, memory LRU size, disk cache size, and background prewarm wait intervals come from `runtime.json`.

Cache keys include path and mtime, hashed with blake3, so file modifications invalidate prior thumbnails. Disk cache is bucketed by requested size and a small in-memory LRU avoids unnecessary decoding near the current viewport. Keep the memory LRU conservative because Large textures are several MB each; disk cache, not RAM, is the durable thumbnail cache. Opaque thumbnails are cached as JPEG; thumbnails with transparency are cached as lossless WebP so transparent PNG screenshots do not gain white edges. The disk hash includes a thumbnail-cache version prefix, so format changes invalidate older cached files automatically.
Thumbnail cache files must be published atomically: write to a temporary sibling
path, then rename into the final `.jpg`/`.webp` path only after encoding
completes. Readers treat empty or undecodable cache files as corrupt, remove
them, and regenerate instead of logging the same failure repeatedly. Background
prewarm jobs must register their cache keys in `ThumbnailLoader`'s in-flight
map just like visible tile requests; otherwise multiple workers can race on the
same final cache path and readers can observe a partially-created file.

After startup scan and trash reconciliation, `ThumbnailLoader` enters background prewarm mode without waiting for full-library DB pagination: idle workers pull live media rows whose `thumbnail_generated_at` is missing or older than `file_mtime`, generate the Medium thumbnail, and immediately mark the row generated. This DB marker is part of the pull loop's convergence condition; do not defer it behind a large batch threshold, or small libraries will repeatedly regenerate the same thumbnails before they are considered complete. The prewarm pull loop must not hold `background_pull.offset` while running the DB query; mode switching resets that offset from the GTK thread when the target thumbnail size changes, so holding the mutex across SQL can block Year/Month/Day switching behind a background prewarm read.

The prewarm pull offset follows the user's browsing position, not always the newest rows. Because the scrollbar can jump to any region instantly, the Photos grid calls `ThumbnailLoader::redirect_prewarm_to_offset(prewarm_offset)` when a virtual DB page lands, moving the prewarm pull start to the current full-library live-media offset so off-screen tiles around the current position warm next (ahead of irrelevant newest-first batches). `prewarm_offset` is interpreted in the unfiltered live-media DESC order, not in the filtered "needs thumbnail" result set; the DB query anchors on that live offset and then finds cold rows at or after the anchor, so already-generated thumbnails before the viewport do not push prewarm away from the user's position. Visible tiles still take `TIER_BOOST` (highest priority); this redirect only decides where the lower-priority off-screen prewarm work happens. It does not change the pull model, DESC order, tier, or throttle.

Virtual GridView cells first consult the same in-memory thumbnail LRU and never
perform synchronous disk decoding during a bind. A cache miss stays as a
fixed-size loading tile while the existing visible-priority request runs. The
factory binds a thumbnail result only if its MediaId, cache key, physical slot,
and layout generation still match, so an old worker result cannot paint a
recycled Year, Month, or Day cell after a fast drag or mode switch.

**Decode failures use a shared unavailable thumbnail.** If an image cannot be decoded, or if both video frame extraction paths fail, the thumbnail worker returns and caches a themed unavailable placeholder instead of dropping the request. Video extraction failures that recover through this placeholder are `debug`-level diagnostics, not default warning/info log lines. The placeholder must not look like a playable video frame: normal extracted video thumbnails get the bottom-left play badge, while unavailable thumbnails use a slashed media glyph.

JPEG thumbnail generation uses the libjpeg-turbo scaled-decode fast path only
after both the file extension and file signature identify the source as JPEG.
Files such as GIF content with a stale `.jpg` suffix must fall through directly
to gdk-pixbuf instead of logging recoverable turbojpeg fallback warnings.

**Video thumbnails use `ffmpegthumbnailer` with a GStreamer fallback.** Most phone/camera video is limited-range (TV) YUV (16–235); the original `videoconvert → RGB` pipeline passed that through unexpanded, producing washed-out, low-saturation thumbnails (verified: black floor stuck at Y≈18, white ceiling at ≈227 instead of 0/255). `extract_video_frame` now prefers `ffmpegthumbnailer` (libav-based; correctly expands limited→full range and applies rotation), decoding its PNG output and returning the unmodified video frame as a `Pixbuf`. If `ffmpegthumbnailer` is missing or fails, it falls back to `extract_video_frame_gst` — the previous `uridecodebin → videoflip(auto) → videoconvert → appsink` pipeline, but with the output caps pinned to `colorimetry=sRGB` so the fallback also expands to full-range RGB. Only if both fail does it create the synthetic unavailable placeholder in memory; failed placeholders are not written to the thumbnail cache. `videoflip video-direction=auto` (GStreamer path) and ffmpegthumbnailer both auto-apply rotation from all sources (MP4 tkhd matrix, codec SEI, tags). Successfully extracted frames are cached as JPEG.
The per-video ffmpegthumbnailer fallback, GStreamer extraction, cache-hit, and generated-thumbnail progress messages are debug diagnostics only; default logs should retain warnings for unrecoverable decode/cache problems, not every recovered thumbnail path.
