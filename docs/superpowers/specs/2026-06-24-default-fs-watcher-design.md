# Default Filesystem Watcher + PhotosPage Live Refresh — Design Spec

**Date:** 2026-06-24
**Status:** Approved (pending user spec review)
**Scope:** Wire `notify_watcher` to `PhotosPage` so newly added/removed/modified
images in `~/Pictures` (or `~/图片` for zh_CN locale) appear in the main grid
without restarting the app. Replace the current fire-and-forget `on_change`
callback with a typed `MediaChangeNotifier` channel.

## Problem

The codebase already has a working `notify_watcher` (`src/core/notify_watcher.rs`)
that uses `notify = "6"` with `RecursiveMode::Recursive` on `crate::config::pictures_dir()`,
and `app.rs:138` already starts it on app activation. However, the user reports
that adding a new image under `~/Pictures` does not cause it to appear in
either the main Photos grid or the Albums sidebar.

Root cause (confirmed by reading `src/app.rs:117-138` and
`src/core/notify_watcher.rs:32-58`):

1. The `on_change` closure on the watcher side only calls
   `crate::core::albums::refresh(&pool)`. This refreshes the albums
   materialized view in the DB, but does **not** mutate the in-memory
   `gtk::gio::ListStore` (`media_list`) that backs the three `MediaGrid`
   instances on `PhotosPage`. So new files are written to the DB by the
   watcher, but the UI never re-reads them.
2. The Albums sidebar is rebuilt lazily on click (per
   `src/ui/album_detail_page::refresh_albums_page_in_nav`); a user who
   doesn't re-click Albums also won't see the new entry.

The watcher's `RecursiveMode::Recursive` is already correct — subfolders
are watched. The bug is purely the missing UI side of the signal.

## Goals

1. New images under `~/Pictures` (and any subdirectory) appear in the
   Photos main grid without restarting the app.
2. Removed images disappear from the grid.
3. Modified images (re-saved / `touch`) refresh in place — they keep
   their position so scroll is not lost.
4. The watcher's existing `albums::refresh` materialization continues to
   run; we do not regress it.
5. Replace the fire-and-forget `on_change: Fn()` callback with a typed
   `MediaChangeNotifier` channel so the watcher can communicate **which**
   item changed, not just "something changed".

## Non-Goals

- Multi-root directory configuration UI. `start_watching` already accepts
  `Vec<PathBuf>` and the call site in `app.rs` continues to pass
  `vec![pictures_dir()]` only.
- Fallback listening when `pictures_dir()` does not exist on disk.
- Replacing the bare `notify` watcher with `notify-debouncer-full` (the
  existing 50 ms sleep continues to be the de-facto debounce).
- Active push of Albums sidebar updates (Albums page rebuilds on click;
  that contract is unchanged).
- `ViewerPage` / `EditorPage` real-time refresh — they read the same
  shared `media_list` and pick up the new items automatically.
- Reordering the list on insert (no `gtk::SortListModel`); new items are
  appended to preserve scroll position.

## Approach

### New module: `src/core/media_change_notifier.rs`

A typed mpsc channel between the watcher (producer) and a GTK-thread
consumer (reader of `media_list`).

```rust
use crate::core::media::MediaItem;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum MediaChangeEvent {
    /// New or modified item. Consumers dedupe by `uri`:
    /// existing → splice-replace in place; absent → append.
    Upserted(MediaItem),
    /// Removed item. Consumer matches by `uri`.
    Removed { uri: String },
}

#[derive(Clone)]
pub struct MediaChangeNotifier {
    tx: mpsc::UnboundedSender<MediaChangeEvent>,
}

impl MediaChangeNotifier {
    /// Returns `(notifier, receiver)`. The receiver is held by a
    /// `glib::MainContext::spawn_local` task that drains it and applies
    /// diffs to `media_list`.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<MediaChangeEvent>);

    pub fn upserted(&self, item: MediaItem);   // logs warn on send-fail
    pub fn removed(&self, uri: String);        // logs warn on send-fail
}
```

`tokio::sync::mpsc::UnboundedSender` is `Send + Sync`; the watcher
(`spawn_blocking`) holds the sender, the GTK-thread consumer holds the
receiver. The application runtime already keeps a multi-thread tokio
runtime entered for the process lifetime (`src/app.rs:17-31`), so the
receiver can `await` inside `spawn_local` without additional setup.

### `LocalBackend` signature changes

```rust
// before
pub fn upsert_from_path(&self, path: &Path) -> Result<()>
// after
pub fn upsert_from_path(&self, path: &Path) -> Result<Option<MediaItem>>
//   - None: path is not a file (directory event, transient disappearance)
//   - Some(item): the upsert succeeded; `item.id` is populated

// before
pub fn upsert(&self, item: &NewMediaItem) -> Result<i64>
// after
pub fn upsert(&self, item: &NewMediaItem) -> Result<MediaItem>
//   Returns the fully-materialized row (id populated, all timestamps
//   canonical). Implementation: keep the same SELECT-then-INSERT/UPDATE
//   shape, but on the success branch call `db::get_media_item` (or
//   `RETURNING *` if we confirm SQLite ≥ 3.35 on the target system) so
//   the caller gets the row in one observable step. No second pool
//   round-trip from the caller's perspective.
```

`delete_path` stays as `Result<usize>` — the watcher constructs the uri
from the path with `format!("file://{}", path.display())`.

### `notify_watcher` signature change

```rust
// before
pub fn start_watching<F>(pool: DbPool, paths: Vec<PathBuf>, on_change: F) -> JoinHandle<()>
// after
pub fn start_watching(
    pool: DbPool,
    paths: Vec<PathBuf>,
    notifier: MediaChangeNotifier,
) -> JoinHandle<()>
```

Inside `handle_event`:
- `Create` / `Modify(Data)` success → `notifier.upserted(item)`
- `Remove` success (rows > 0) → `notifier.removed(uri)`
- `Modify(Name)` (rename) — file exists → `notifier.upserted(item)`;
  otherwise → `notifier.removed(uri)`.

`albums::refresh(&pool)` continues to be called inline after each
successful upsert/delete, preserving the existing materialized-view
contract.

### Consumer loop in `app.rs::initialize`

```rust
let (notifier, change_rx) = MediaChangeNotifier::new();
let _watcher = start_watching(pool.clone(), vec![pictures.clone()], notifier);

let list_for_consumer = list.clone();
glib::MainContext::default().spawn_local(async move {
    let mut rx = change_rx;
    while let Some(event) = rx.recv().await {
        apply_to_media_list(&list_for_consumer, event);
    }
});
```

`apply_to_media_list` (new free fn in `app.rs`):

```rust
fn apply_to_media_list(list: &gtk::gio::ListStore, event: MediaChangeEvent) {
    match event {
        MediaChangeEvent::Upserted(item) => {
            let uri = item.uri.clone();
            for i in 0..list.n_items() {
                if let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() {
                    if obj.borrow::<MediaItem>().uri == uri {
                        list.splice(i, 1, &[glib::BoxedAnyObject::new(item)]);
                        return;
                    }
                }
            }
            list.append(&glib::BoxedAnyObject::new(item));
        }
        MediaChangeEvent::Removed { uri } => {
            for i in 0..list.n_items() {
                if let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() {
                    if obj.borrow::<MediaItem>().uri == uri {
                        list.remove(i);
                        return;
                    }
                }
            }
        }
    }
}
```

Why append (not "insert at correct sort position"):
- The user said "保留滚动位置" (preserve scroll position). Appending
  to the end of a list does not move existing tiles.
- New items are visually discoverable as the most recent additions; the
  user can re-sort by switching to Year view and back if needed.
- Avoids the complexity of binary-search-by-taken-at and the corner
  cases of equal `taken_at` / `file_mtime`.

### Error handling

| Scenario | Behavior |
|----------|----------|
| `pictures_dir()` does not exist | `watcher.watch` returns Err, already logged via `tracing::warn!`; contract unchanged. |
| Watcher thread panics | `JoinHandle` is dropped (intentional); no recovery. Same as today. |
| `tx.send` after `rx` is dropped | `MediaChangeNotifier::upserted/removed` catches the error and `tracing::warn!`s; does not panic. |
| `apply_to_media_list` panic | Must never panic — `gio::ListStore` operations on the wrong thread crash the GTK loop. The implementation uses `and_downcast::<...>` and `.ok()` defensively throughout. |
| Duplicate `Upserted` for same `uri` | First splice-replaces; subsequent events on the same uri also splice-replace (idempotent). |
| `upsert_from_path` failure | `tracing::warn!` continues; no `MediaChangeEvent` emitted. (Avoids UI/data divergence.) |

### Data flow

```
disk event                    tokio runtime                 GTK main thread
──────────                    ────────────                  ───────────────
Create("a.jpg")                                              │
  │  notify::Event                                            │
  ▼                                                          │
handle_event                                                  │
  │  upsert_from_path() → Ok(Some(item))                      │
  │  notifier.upserted(item)                                  │
  │  albums::refresh()                                        │
  ▼                                                          │
tokio mpsc::UnboundedSender                                  │
  │                                                          │
  │ ─── MediaChangeEvent::Upserted(item) ─────────────────►  │
  │                                                          │
  │                                                  spawn_local consumer
  │                                                  rx.recv().await
  │                                                          │
  │                                                  apply_to_media_list
  │                                                          │ find uri in list
  │                                                  ListStore::splice OR append
  │                                                          │
  │                                                  MediaGrid re-flows (gtk::FlowBox)
```

### Relationship to other layers

- **`ThumbnailLoader` (unchanged):** New items have no disk thumbnail;
  `PhotoTile` requests them lazily on mount via
  `ThumbnailLoader::request(uri, size, reply)` (cache key
  `path + mtime` blake3-hashed; new files simply miss the cache and
  generate on first paint).
- **`ViewerPage` / `EditorPage` (unchanged):** Read the same
  `media_list`; new items are visible after pop-back.
- **`AlbumsPage` / `TrashPage` (unchanged):** Continue to rebuild on
  sidebar click. The albums materialized view is still refreshed by
  the watcher inline.

## Design Properties

### Threading

- Producer (watcher): `spawn_blocking` worker thread. Holds
  `MediaChangeNotifier` (clone of `tx`). Only touches DB + mpsc sender.
- Consumer: `glib::MainContext::spawn_local` future on the GTK main
  thread. Owns the `UnboundedReceiver` and the `gio::ListStore`. Only
  the main thread mutates the `ListStore`.
- The `tx` → `rx` bridge is the only synchronization point.

### Memory

Each `MediaChangeEvent::Upserted` carries one `MediaItem` clone
(~150 bytes). At the project's expected scale (10k–100k photos, watcher
events trickle in at user action speed), the unbounded channel will
never accumulate meaningfully. Dropping backpressure is acceptable
because consumers run on the same thread that drives UI rendering.

### Latency

`notify` → `apply_to_media_list`: end-to-end latency is dominated by
the 50 ms `thread::sleep` in `handle_event` (intentional, to wait for
file write completion on `Create`/`Modify(Data)`). The mpsc hop and
the `splice` are sub-millisecond on the GTK main thread.

## Testing

Per `CONTRIBUTING.md`: failing test first, then implementation, then
`cargo fmt && cargo clippy --all-targets`.

### Unit tests (no GTK, no real FS)

1. `src/core/media_change_notifier.rs::tests`
   - `notifier_upserted_sends_event_to_receiver`
   - `notifier_removed_sends_event_to_receiver`
   - `notifier_send_after_receiver_drop_does_not_panic` — critical
     robustness; watcher must not crash if the GTK-side consumer is
     torn down first.

2. `src/core/backend/local.rs::tests`
   - `upsert_from_path_returns_inserted_media_item` — assert
     `Some(item)` and `item.id > 0` for a fresh jpeg.
   - `upsert_from_path_returns_none_for_directory_path`.
   - `upsert_from_path_returns_updated_item_for_existing_uri` — insert
     then re-upsert; the second call returns the updated row with the
     same `id` and a different `blake3_hash`.

3. `src/core/notify_watcher.rs::tests` (existing internal test)
   - `remove_event_deletes_media_row_and_notifies_change` — update to
     use `MediaChangeNotifier` instead of `Arc<AtomicUsize>`. Assert
     `try_recv()` returns `Removed { uri }` with the expected uri.

### Integration tests

4. `tests/notify_watcher_notifier.rs` (new file)
   - `#[ignore]`'d tests, parallel to the existing
     `tests/notify_watcher.rs` `#[ignore]` tests. Inherit
     `tests/common/mod.rs` fixtures (`write_plain_jpeg`,
     `write_plain_png`).
   - `watcher_emits_upserted_for_new_file` — start watcher in tempdir,
     `write_plain_jpeg`, `try_recv` within 5 s, assert
     `Upserted(item)` with matching uri.
   - `watcher_emits_removed_for_deleted_file` — insert, then
     `std::fs::remove_file`, assert `Removed { uri }`.
   - `watcher_emits_upserted_for_modified_file` — insert, then
     `write_plain_jpeg` again (same name → `Modify(Data)` path), assert
     a second `Upserted` arrives.

5. `tests/notify_watcher_callback.rs` (rewrite)
   - Replace the existing `on_change_callback_fires_after_successful_upsert`
     with a version that constructs a `MediaChangeNotifier` and asserts
     on the receiver, not on an `AtomicUsize` counter.

### Regression coverage

All tests in `tests/notify_watcher.rs` (4 unit + 1 `#[ignore]`) must
continue to pass with the new signatures. `cargo test` must remain
green; `#[ignore]` tests must be runnable manually with
`cargo test -- --ignored` for sanity.

## Files Touched

| File | Change |
|------|--------|
| `src/core/media_change_notifier.rs` | **New** (~80 lines + tests). |
| `src/core/mod.rs` | Re-export `MediaChangeNotifier`, `MediaChangeEvent`. |
| `src/core/backend/local.rs` | `upsert_from_path` returns `Result<Option<MediaItem>>`; `upsert` returns `Result<MediaItem>`; 3 new unit tests. |
| `src/core/notify_watcher.rs` | `start_watching` takes `MediaChangeNotifier`; `handle_event` emits events; existing internal test adapted. |
| `src/app.rs` | Construct `(notifier, rx)`; spawn consumer; add `apply_to_media_list`; remove old `on_change` closure. |
| `tests/notify_watcher.rs` | Adapt 4 unit tests to new signatures (no behavior change). |
| `tests/notify_watcher_callback.rs` | **Rewrite** assertions to use the notifier receiver. |
| `tests/notify_watcher_notifier.rs` | **New** integration test file. |

## Verification

1. **Build:** `cargo build` clean, no new warnings.
2. **Unit tests:** `cargo test` green; new unit tests pass.
3. **Manual smoke test:**
   - `cargo run`, drop a jpeg into `~/Pictures/` → tile appears in
     Photos grid (Day view) within ~1 s.
   - Drop a jpeg into `~/Pictures/Subdir/` → tile appears (recursive
     watch is preserved).
   - Delete the file from disk → tile disappears from the grid.
   - `touch` an existing jpeg to bump mtime → tile is replaced
     in-place (no scroll jump).
4. **Album sidebar:** open Albums; the new file's date folder is
   present (because `albums::refresh` still runs).
5. **Lint:** `cargo fmt && cargo clippy --all-targets` clean.
6. **Regression:** `cargo test -- --ignored` (inotify-required tests
   pass locally).

## Risk

Medium-low. The refactor changes two public signatures
(`LocalBackend::upsert_from_path`, `notify_watcher::start_watching`)
and the call site in `app.rs`. There is exactly one call site for
each, so the blast radius is small. The main risk is regression in
the consumer loop on the GTK main thread — we mitigate by
- keeping `apply_to_media_list` panic-free via defensive `and_downcast`
  and `.ok()`,
- not introducing any new lock or mutex,
- not blocking the main thread on DB or FS.

The watcher's existing 50 ms `thread::sleep` and the lack of
`notify-debouncer-full` are pre-existing limitations, not introduced
by this change. They are listed under Non-Goals.
