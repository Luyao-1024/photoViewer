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

## Scanning And Watching

The local backend scans filesystem paths and inserts/updates media rows. The watcher handles incremental changes after startup. Changes to scanner behavior should consider:

- New supported file extensions.
- Metadata extraction failures.
- Duplicate paths.
- Deletions and trash transitions.
- UI change notification timing.

## Thumbnails

`ThumbnailLoader` owns an mpsc queue feeding blocking workers. Requests return textures through `oneshot::Sender`.

Cache keys include path and mtime, hashed with blake3, so file modifications invalidate prior thumbnails. Disk cache is bucketed by requested size and an in-memory LRU avoids unnecessary decoding.
