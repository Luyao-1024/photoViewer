# Default Filesystem Watcher + PhotosPage Live Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the existing `notify_watcher` to `PhotosPage`'s `media_list` via a typed `MediaChangeNotifier` mpsc channel so newly added / removed / modified images under `~/Pictures` (and subfolders) appear in the main grid without restarting the app.

**Architecture:** Replace the fire-and-forget `on_change: Fn()` callback on `notify_watcher::start_watching` with a `MediaChangeNotifier` (a `tokio::sync::mpsc::UnboundedSender<MediaChangeEvent>` wrapper). The watcher produces `MediaChangeEvent::Upserted(MediaItem)` / `Removed { uri }` events; an `app::initialize` consumer loop on the GTK main thread drains the receiver and applies splice/append/remove diffs to the shared `gtk::gio::ListStore`. `LocalBackend::upsert_from_path` and `LocalBackend::upsert` are widened to return the materialized `MediaItem` so the watcher can forward it without a second DB round-trip.

**Tech Stack:** Rust, GTK4 (`gtk4` crate), Libadwaita, `notify = "6"`, `tokio` (multi-thread runtime, `sync::mpsc`), `r2d2_sqlite`, `tracing`.

## Global Constraints

- Per `CONTRIBUTING.md`: TDD discipline — write the failing test first, then implement to green, then `cargo fmt && cargo clippy --all-targets`.
- Bilingual comments (Chinese + English) match the rest of the codebase.
- `notify` is already declared at `Cargo.toml:24` as `notify = "6"`. No new dependencies.
- `tokio` is already declared with `rt-multi-thread, fs, macros, sync` features (`Cargo.toml:18`). No new features.
- The application already keeps a multi-thread tokio runtime entered for the process lifetime via `src/app.rs:17-31` (`install_tokio_runtime`). Consumers can `await` inside `glib::MainContext::spawn_local` without further setup.
- `gio::ListStore` mutations must happen on the GTK main thread. The notifier receiver is drained by a `spawn_local` future specifically to guarantee this.
- All new code must compile under existing `cargo build` (which runs `build.rs` blueprint compilation) with no new warnings.
- All `#[ignore]`-marked tests in `tests/` follow the existing convention: depend on inotify/fsevent, run manually with `cargo test -- --ignored`.
- Shared test fixtures live in `tests/common/mod.rs` (already provides `write_plain_jpeg`, `write_plain_png`, `tmp_dir`, `write_jpeg_with_exif`).

## File Structure

Files touched in this plan, in dependency order:

| File | Role |
|------|------|
| `src/core/media_change_notifier.rs` | **New.** `MediaChangeEvent` enum + `MediaChangeNotifier` struct (mpsc producer side). |
| `src/core/mod.rs` | Re-export `MediaChangeNotifier`, `MediaChangeEvent`. |
| `src/core/backend/local.rs` | `LocalBackend::upsert` and `upsert_from_path` widened to return `MediaItem`; new unit tests. |
| `src/core/notify_watcher.rs` | `start_watching` accepts `MediaChangeNotifier`; `handle_event` emits events; existing internal test adapted. |
| `src/ui/apply_to_media_list.rs` | **New.** Pure function `apply_to_media_list(list, event)`. Extracted from `app.rs` so it can be unit-tested headlessly. |
| `src/app.rs` | Construct `(notifier, rx)`; spawn consumer; wire `apply_to_media_list`; remove old `on_change` closure. |
| `src/ui/mod.rs` | Re-export `apply_to_media_list`. |
| `tests/notify_watcher.rs` | Adapt 4 unit + 1 `#[ignore]` test to new signatures. |
| `tests/notify_watcher_callback.rs` | Rewrite to assert via notifier receiver instead of `AtomicUsize`. |
| `tests/notify_watcher_notifier.rs` | **New.** `#[ignore]` integration tests for `Upserted` / `Removed` events. |
| `tests/apply_to_media_list.rs` | **New.** Headless unit tests for `apply_to_media_list`. |

---

## Task 1: Add `MediaChangeNotifier` module

**Files:**
- Create: `src/core/media_change_notifier.rs`
- Modify: `src/core/mod.rs` (add `pub mod media_change_notifier;` + re-exports)

**Interfaces (consumed by later tasks):**
- `pub enum MediaChangeEvent { Upserted(MediaItem), Removed { uri: String } }` — `Debug + Clone`
- `pub struct MediaChangeNotifier` — `Clone`
- `impl MediaChangeNotifier { pub fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<MediaChangeEvent>); pub fn upserted(&self, item: MediaItem); pub fn removed(&self, uri: String); }`

- [ ] **Step 1: Create `src/core/media_change_notifier.rs` with unit tests at the bottom**

```rust
//! Media change notification channel
//!
//! Decouples the filesystem watcher (producer) from the GTK main thread
//! consumer that mutates the shared `gio::ListStore`. The watcher holds
//! a `MediaChangeNotifier` clone; a `glib::MainContext::spawn_local` task
//! owns the receiver and applies splice/append/remove diffs.

use crate::core::media::MediaItem;
use tokio::sync::mpsc;

/// Media change event emitted by the watcher, consumed by the UI.
#[derive(Debug, Clone)]
pub enum MediaChangeEvent {
    /// A new item was inserted, or an existing item was updated.
    /// Consumers match by `uri`: existing → splice-replace; absent → append.
    Upserted(MediaItem),
    /// An item was removed. Consumer matches by `uri`.
    Removed { uri: String },
}

/// Producer side of the media-change channel.
///
/// Cheap to clone (wraps an `UnboundedSender`). Watcher keeps one clone
/// in its `spawn_blocking` thread.
#[derive(Clone)]
pub struct MediaChangeNotifier {
    tx: mpsc::UnboundedSender<MediaChangeEvent>,
}

impl MediaChangeNotifier {
    /// Create a paired notifier + receiver. The receiver is typically
    /// moved into a `glib::MainContext::spawn_local` task.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<MediaChangeEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Notify that `item` was inserted or updated. The current GTK-thread
    /// consumer will splice it into the shared list.
    pub fn upserted(&self, item: MediaItem) {
        if let Err(e) = self.tx.send(MediaChangeEvent::Upserted(item)) {
            tracing::warn!("MediaChangeNotifier::upserted send failed: {e}");
        }
    }

    /// Notify that the item with the given `uri` was removed.
    pub fn removed(&self, uri: String) {
        if let Err(e) = self.tx.send(MediaChangeEvent::Removed { uri }) {
            tracing::warn!("MediaChangeNotifier::removed send failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::media::MediaItem;
    use chrono::Utc;
    use std::path::PathBuf;

    fn sample_item(uri: &str) -> MediaItem {
        MediaItem {
            id: 1,
            uri: uri.into(),
            path: PathBuf::from(uri.trim_start_matches("file://")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            width: Some(64),
            height: Some(48),
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "h".into(),
            trashed_at: None,
        }
    }

    #[test]
    fn notifier_upserted_sends_event_to_receiver() {
        let (notifier, mut rx) = MediaChangeNotifier::new();
        let item = sample_item("file:///tmp/a.jpg");
        notifier.upserted(item.clone());

        match rx.try_recv() {
            Ok(MediaChangeEvent::Upserted(received)) => {
                assert_eq!(received.uri, item.uri);
            }
            other => panic!("expected Upserted, got {other:?}"),
        }
    }

    #[test]
    fn notifier_removed_sends_event_to_receiver() {
        let (notifier, mut rx) = MediaChangeNotifier::new();
        notifier.removed("file:///tmp/a.jpg".into());

        match rx.try_recv() {
            Ok(MediaChangeEvent::Removed { uri }) => assert_eq!(uri, "file:///tmp/a.jpg"),
            other => panic!("expected Removed, got {other:?}"),
        }
    }

    #[test]
    fn notifier_send_after_receiver_drop_does_not_panic() {
        let (notifier, rx) = MediaChangeNotifier::new();
        drop(rx);
        // Should not panic; only emits a tracing::warn.
        notifier.upserted(sample_item("file:///tmp/a.jpg"));
        notifier.removed("file:///tmp/a.jpg".into());
    }
}
```

- [ ] **Step 2: Run tests and verify they fail to compile (no module yet)**

```bash
cargo build --lib 2>&1 | head -5
```

Expected: error about missing `media_change_notifier` module.

- [ ] **Step 3: Register the module in `src/core/mod.rs`**

Add these two lines in alphabetical order within the `pub mod` block (after `media`):

```rust
pub mod media_change_notifier;
```

Add these two re-exports in the `pub use` block (after `media`):

```rust
pub use media_change_notifier::{MediaChangeEvent, MediaChangeNotifier};
```

The full `src/core/mod.rs` after this change:

```rust
pub mod album_ops;
pub mod albums;
pub mod backend;
pub mod bootstrap;
pub mod cache;
pub mod db;
pub mod i18n;
pub mod edit;
pub mod error;
pub mod media;
pub mod media_change_notifier;
pub mod metadata;
pub mod notify_watcher;
pub mod section_model;
pub mod thumbnails;
pub mod trash;

pub use album_ops::{add_to_album, AlbumOpMode};
pub use albums::{refresh as refresh_albums, Album};
pub use backend::local::LocalBackend;
pub use db::{init_pool, run_migrations, DbPool};
pub use edit::{
    CropRect, EditCategory, EditOperation, EditRegistry, EditState, ParamValue, Rotation,
};
pub use error::{AppError, Result};
pub use media::{MediaItem, NewMediaItem};
pub use media_change_notifier::{MediaChangeEvent, MediaChangeNotifier};
pub use metadata::{extract as extract_metadata, RawMetadata};
pub use section_model::{group_items, GroupBy, MediaSection, SectionKey};
pub use thumbnails::{ThumbnailLoader, ThumbnailRequest, ThumbnailSize};
pub use trash::{delete_permanently, move_to_trash, restore_from_trash};
```

- [ ] **Step 4: Run tests and verify they pass**

```bash
cargo test --lib core::media_change_notifier
```

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/core/media_change_notifier.rs src/core/mod.rs
git commit -m "feat(core): add MediaChangeNotifier channel for watcher -> UI"
```

---

## Task 2: Widen `LocalBackend::upsert` to return `MediaItem`

**Files:**
- Modify: `src/core/backend/local.rs:118-154` (change `upsert` return type)
- Modify: `src/core/backend/local.rs` (add unit test at bottom of `mod tests`)

**Interfaces (consumed by later tasks):**
- `pub fn upsert(&self, item: &NewMediaItem) -> Result<MediaItem>` — replaces existing `Result<i64>`.

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block in `src/core/backend/local.rs`:

```rust
    #[test]
    fn upsert_returns_inserted_media_item_with_populated_id() {
        use crate::core::media::NewMediaItem;
        use chrono::Utc;
        use std::path::PathBuf;

        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "x.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let new_item = NewMediaItem {
            uri: format!("file://{}", path.display()),
            path: path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/jpeg".into(),
            width: Some(64),
            height: Some(48),
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: std::fs::metadata(&path).unwrap().len(),
            blake3_hash: "placeholder".into(),
        };

        let returned = backend.upsert(&new_item).expect("upsert should succeed");
        assert!(returned.id > 0, "returned MediaItem must have a populated id");
        assert_eq!(returned.uri, new_item.uri);
        assert_eq!(returned.blake3_hash, "placeholder");
    }

    /// Test-only helper: write a 64x48 plain JPEG (mirrors
    /// `tests/common/mod.rs::write_plain_jpeg` without requiring that
    /// module to be in scope for the lib's own test binary).
    fn write_plain_jpeg_in(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        use image::{ImageBuffer, Rgb};
        let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| Rgb([128, 128, 128]));
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }
```

- [ ] **Step 2: Run the test and verify it fails to compile (return type still `i64`)**

```bash
cargo test --lib core::backend::local::tests::upsert_returns_inserted_media_item_with_populated_id 2>&1 | tail -20
```

Expected: compile error on `let returned = backend.upsert(&new_item).expect(...)` because `upsert` returns `i64`, not `MediaItem`.

- [ ] **Step 3: Change `upsert` to return `MediaItem`**

In `src/core/backend/local.rs`, replace the entire `upsert` method (lines 117–154) with:

```rust
    /// Insert or update (URI conflict → UPDATE). Returns the fully-materialized
    /// row so callers (notably `notify_watcher`) can forward it to the UI
    /// without a second DB round-trip.
    pub fn upsert(&self, item: &NewMediaItem) -> Result<MediaItem> {
        let mut conn = self.pool.get()?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM media_items WHERE uri = ?1",
                [&item.uri],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            conn.execute(
                "UPDATE media_items
                 SET path=?2, folder_path=?3, mime_type=?4, width=?5,
                     height=?6, taken_at=?7, file_mtime=?8, file_size=?9,
                     blake3_hash=?10, indexed_at=unixepoch()
                 WHERE id=?1",
                rusqlite::params![
                    id,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
                    item.width,
                    item.height,
                    item.taken_at.map(|t| t.timestamp()),
                    item.file_mtime.timestamp(),
                    item.file_size as i64,
                    item.blake3_hash,
                ],
            )?;
            drop(conn);
            Ok(db::get_media_item(&self.pool, id)?)
        } else {
            let id = db::insert_media_item(&self.pool, item)?;
            Ok(db::get_media_item(&self.pool, id)?)
        }
    }
```

Add `use crate::core::media::MediaItem;` at the top of the file (alongside the existing `use crate::core::media::NewMediaItem;`).

- [ ] **Step 4: Run the test and verify it passes**

```bash
cargo test --lib core::backend::local::tests
```

Expected: all existing tests + the new one pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/backend/local.rs
git commit -m "feat(core): LocalBackend::upsert returns materialized MediaItem"
```

---

## Task 3: Widen `LocalBackend::upsert_from_path` to return `Option<MediaItem>`

**Files:**
- Modify: `src/core/backend/local.rs:102-110` (change `upsert_from_path` return type)
- Modify: `src/core/backend/local.rs` (add 2 unit tests)

**Interfaces (consumed by later tasks):**
- `pub fn upsert_from_path(&self, path: &Path) -> Result<Option<MediaItem>>` — replaces `Result<()>`.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/core/backend/local.rs`:

```rust
    #[test]
    fn upsert_from_path_returns_inserted_media_item() {
        let dir = tempfile::tempdir().unwrap();
        let _path = write_plain_jpeg_in(dir.path(), "new.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let returned = backend
            .upsert_from_path(&dir.path().join("new.jpg"))
            .expect("upsert_from_path should succeed");
        let item = returned.expect("expected Some(MediaItem) for a valid jpeg");
        assert!(item.id > 0);
        assert!(item.path.ends_with("new.jpg"));
    }

    #[test]
    fn upsert_from_path_returns_none_for_directory_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let returned = backend
            .upsert_from_path(&dir.path().join("subdir"))
            .expect("directory path should not error");
        assert!(returned.is_none(), "directory path must yield None, not Some");
    }

    #[test]
    fn upsert_from_path_returns_updated_item_for_existing_uri() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "dup.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let first = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("first upsert must yield Some");
        // Re-write the file with different content to change blake3 hash.
        std::fs::write(&path, b"different content to change the hash").unwrap();
        let second = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("second upsert must yield Some");
        assert_eq!(first.id, second.id, "upsert must reuse the same id");
        assert_ne!(
            first.blake3_hash, second.blake3_hash,
            "second upsert must reflect new content"
        );
    }
```

- [ ] **Step 2: Run the tests and verify they fail to compile**

```bash
cargo test --lib core::backend::local::tests::upsert_from_path 2>&1 | tail -20
```

Expected: compile error — `Result<Option<MediaItem>>` is treated as `Result<()>` by current signature.

- [ ] **Step 3: Change `upsert_from_path` to return `Option<MediaItem>`**

Replace `src/core/backend/local.rs:102-110`:

```rust
    /// 从单个文件路径提取元数据并 upsert 到数据库。
    ///
    /// 专为 `notify_watcher` 等增量入口设计：
    ///   - 路径不是文件（目录事件、临时消失等）时返回 `Ok(None)`；
    ///   - 解析失败时返回错误，调用方负责记录日志；
    ///   - upsert 成功时返回 `Ok(Some(MediaItem))`，调用方可以直接转发给
    ///     `MediaChangeNotifier` 而无需再次查询 DB。
    pub fn upsert_from_path(&self, path: &Path) -> Result<Option<MediaItem>> {
        if !path.is_file() {
            return Ok(None);
        }
        let item = self
            .process_file(path)?
            .ok_or_else(|| AppError::Decode(format!("not an image: {}", path.display())))?;
        self.upsert(&item).map(Some)
    }
```

- [ ] **Step 4: Run the lib tests and verify they all pass**

```bash
cargo test --lib
```

Expected: all pass (including the 2 new `upsert_from_path_returns_*` tests and the prior `upsert` test, plus the existing `stream_file_hash_matches_blake3_hash_for_file_contents`).

- [ ] **Step 5: Verify the rest of the crate still compiles (no callers yet — `notify_watcher` will be updated in next task)**

```bash
cargo build
```

Expected: clean. The `notify_watcher` module will not yet compile because it still uses the old signature; the next task addresses that. If `cargo build` does fail on `notify_watcher.rs`, that's expected — proceed to Task 4 immediately.

- [ ] **Step 6: Commit**

```bash
git add src/core/backend/local.rs
git commit -m "feat(core): LocalBackend::upsert_from_path returns Option<MediaItem>"
```

---

## Task 4: Update `notify_watcher::start_watching` to accept `MediaChangeNotifier`

**Files:**
- Modify: `src/core/notify_watcher.rs` (full file: signature change, body update, internal test update)
- Modify: `tests/notify_watcher.rs` (4 unit tests + 1 `#[ignore]` test — adapt to new `LocalBackend::upsert_from_path` signature)

**Interfaces (consumed by later tasks):**
- `pub fn start_watching(pool: DbPool, paths: Vec<PathBuf>, notifier: MediaChangeNotifier) -> JoinHandle<()>` — replaces the `on_change: F` parameter.

- [ ] **Step 1: Adapt `tests/notify_watcher.rs` to the new `LocalBackend::upsert_from_path` signature**

The 4 unit tests in this file (lines 17–80) all call `backend.upsert_from_path(&path)` and assert `is_ok()`. With the new signature, the call still compiles (the `Result` is still `Result<...>`), and `is_ok()` still works, but the existing assertion `result.is_err()` in `upsert_from_path_skips_unsupported_extension` (line 79) also still works. So these tests do **not** need code changes for the `LocalBackend` signature — but they will fail to compile because `start_watching` (used by `watcher_picks_up_new_file` at line 90) still has the old `on_change: || {}` parameter. We'll address that in Step 4 below.

- [ ] **Step 2: Run the lib tests; confirm they're green (only integration tests are affected so far)**

```bash
cargo test --lib
```

Expected: pass.

- [ ] **Step 3: Rewrite the internal test in `src/core/notify_watcher.rs` (lines 142-194) to use `MediaChangeNotifier`**

Replace the entire `mod tests` block at the bottom of `src/core/notify_watcher.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db;
    use crate::core::media::NewMediaItem;
    use crate::core::media_change_notifier::MediaChangeEvent;
    use chrono::Utc;
    use notify::{event::RemoveKind, Event};

    #[test]
    fn remove_event_deletes_media_row_and_emits_removed_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.jpg");
        std::fs::write(&path, b"not actually decoded in this test").unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        let uri = format!("file://{}", path.display());
        db::insert_media_item(
            &pool,
            &NewMediaItem {
                uri: uri.clone(),
                path: path.clone(),
                folder_path: dir.path().to_path_buf(),
                mime_type: "image/jpeg".into(),
                width: None,
                height: None,
                taken_at: None,
                file_mtime: Utc::now(),
                file_size: 1,
                blake3_hash: "hash".into(),
            },
        )
        .unwrap();
        std::fs::remove_file(&path).unwrap();

        let backend = LocalBackend::new(pool.clone());
        let (notifier, mut rx) = crate::core::media_change_notifier::MediaChangeNotifier::new();
        handle_event(
            &backend,
            Ok(Event {
                kind: EventKind::Remove(RemoveKind::File),
                paths: vec![path],
                attrs: Default::default(),
            }),
            &notifier,
        );

        assert!(db::list_all_media(&pool).unwrap().is_empty());
        match rx.try_recv() {
            Ok(MediaChangeEvent::Removed { uri: received }) => assert_eq!(received, uri),
            other => panic!("expected Removed, got {other:?}"),
        }
    }
}
```

- [ ] **Step 4: Run the lib test and verify it fails to compile (`handle_event` signature still uses `on_change: F`)**

```bash
cargo test --lib core::notify_watcher 2>&1 | tail -20
```

Expected: compile error.

- [ ] **Step 5: Update `notify_watcher` to use `MediaChangeNotifier`**

Replace the entire body of `src/core/notify_watcher.rs` (lines 1–196) with the version below. The diff vs. the current file is:
- `on_change: F` parameter → `notifier: MediaChangeNotifier`
- `handle_event` takes `&MediaChangeNotifier` instead of `&F`
- Each event-success branch calls `notifier.upserted(item)` or `notifier.removed(uri)` instead of `on_change()`
- `albums::refresh` is still called inline for the upsert path (preserves the materialized-view contract)
- The internal test was updated in Step 3

```rust
//! 文件系统通知监听（增量更新）
//!
//! 启动一个阻塞线程，监听指定路径下的文件变化（创建 / 修改 / 删除 / 重命名）。
//! 当事件命中受支持的图片扩展名时，调用 [`LocalBackend::upsert_from_path`] 把最新
//! 的元数据写回 SQLite，并通过 [`MediaChangeNotifier`] 把"哪个 MediaItem 变了"
//! 推给 GTK 主线程的消费者（消费者负责把变更同步到 `media_list`）。
//!
//! 该模块与 [`crate::core::backend::scan_worker`] 互补：
//!   - `scan_worker` 在启动时做全量扫描；
//!   - `notify_watcher` 在运行期做增量更新。
use crate::core::albums;
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use crate::core::media_change_notifier::MediaChangeNotifier;
use notify::{event::EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// 启动后台文件监听，返回一个 `JoinHandle`。
///
/// 每次成功 upsert 之后，会通过 `notifier` 发出 `MediaChangeEvent::Upserted`
/// 事件；删除成功后发出 `MediaChangeEvent::Removed { uri }`。GTK 主线程
/// 的消费者负责把事件应用到 `media_list`。
///
/// 监听在独立的阻塞线程中运行（`spawn_blocking`），不会阻塞 tokio / GTK 主循环。
pub fn start_watching(
    pool: DbPool,
    paths: Vec<PathBuf>,
    notifier: MediaChangeNotifier,
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || run_watcher_loop(pool, paths, notifier))
}

fn run_watcher_loop(pool: DbPool, paths: Vec<PathBuf>, notifier: MediaChangeNotifier) {
    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("watcher 创建失败: {}", e);
            return;
        }
    };

    for path in &paths {
        if let Err(e) = watcher.watch(path, RecursiveMode::Recursive) {
            tracing::warn!("监听 {} 失败: {}", path.display(), e);
        } else {
            tracing::info!("notify watcher 已启动: {}", path.display());
        }
    }

    // 持有 watcher —— 离开作用域时它会被 drop，所有监听自动停止。
    let backend = LocalBackend::new(pool);
    for evt in rx {
        handle_event(&backend, evt, &notifier);
    }
    drop(watcher);
}

fn handle_event(
    backend: &LocalBackend,
    evt: Result<notify::Event, notify::Error>,
    notifier: &MediaChangeNotifier,
) {
    let evt = match evt {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("watcher 事件错误: {}", e);
            return;
        }
    };

    match evt.kind {
        EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Data(_)) => {
            for path in &evt.paths {
                if !is_supported_image(path) {
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(50));
                match backend.upsert_from_path(path) {
                    Ok(Some(item)) => {
                        tracing::debug!("增量 upsert 成功: {}", path.display());
                        // albums 物化视图同步刷新（与 on_change 时机一致）。
                        if let Err(e) = albums::refresh(backend.pool()) {
                            tracing::warn!("albums::refresh after upsert failed: {}", e);
                        }
                        notifier.upserted(item);
                    }
                    Ok(None) => {
                        // 非文件 / 已消失；不通知 UI。
                    }
                    Err(e) => tracing::warn!("upsert 失败 {}: {}", path.display(), e),
                }
            }
        }
        EventKind::Remove(_) => {
            for path in &evt.paths {
                if !is_supported_image(path) {
                    continue;
                }
                let uri = format!("file://{}", path.display());
                match backend.delete_path(path) {
                    Ok(changed) if changed > 0 => {
                        tracing::debug!("增量删除成功: {}", path.display());
                        if let Err(e) = albums::refresh(backend.pool()) {
                            tracing::warn!("albums::refresh after delete failed: {}", e);
                        }
                        notifier.removed(uri);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("增量删除失败 {}: {}", path.display(), e),
                }
            }
        }
        EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
            for path in &evt.paths {
                if !is_supported_image(path) {
                    continue;
                }
                if path.is_file() {
                    std::thread::sleep(Duration::from_millis(50));
                    match backend.upsert_from_path(path) {
                        Ok(Some(item)) => {
                            tracing::debug!("rename upsert 成功: {}", path.display());
                            if let Err(e) = albums::refresh(backend.pool()) {
                                tracing::warn!("albums::refresh after rename upsert failed: {}", e);
                            }
                            notifier.upserted(item);
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!("rename upsert 失败 {}: {}", path.display(), e),
                    }
                } else {
                    let uri = format!("file://{}", path.display());
                    match backend.delete_path(path) {
                        Ok(changed) if changed > 0 => {
                            if let Err(e) = albums::refresh(backend.pool()) {
                                tracing::warn!("albums::refresh after rename delete failed: {}", e);
                            }
                            notifier.removed(uri);
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("rename delete 失败 {}: {}", path.display(), e),
                    }
                }
            }
        }
        _ => {}
    }
}

fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg") | Some("jpeg") | Some("png") | Some("webp") | Some("heic") | Some("heif")
    )
}
```

Then add a `pub fn pool(&self) -> &DbPool` accessor to `LocalBackend` (since `handle_event` now needs it for `albums::refresh`):

In `src/core/backend/local.rs`, inside `impl LocalBackend`, add:

```rust
    /// 返回内部连接池的引用，供 `notify_watcher` 在事件处理中调用
    /// `albums::refresh(&pool)` 同步刷新物化视图。
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
```

`albums::refresh` is the only entry point that exists today (`src/core/albums.rs:77`); it rescans the affected items from DB on each call. That matches the existing watcher's behavior — the original `on_change` callback called exactly this. No new `albums::refresh_with` helper is introduced in this plan.

- [ ] **Step 6: Run the lib test and verify it passes**

```bash
cargo test --lib core::notify_watcher
```

Expected: 1 passed (`remove_event_deletes_media_row_and_emits_removed_event`).

- [ ] **Step 7: Verify the integration test file `tests/notify_watcher.rs` still compiles**

The `watcher_picks_up_new_file` test at lines 84-109 calls `start_watching(pool.clone(), vec![root.clone()], || {})`. The third argument has changed from `|| {}` to a `MediaChangeNotifier`. The unit tests at lines 17-80 only call `LocalBackend::upsert_from_path` and don't need updating for the `start_watching` signature.

Temporarily patch the `#[ignore]` test to use a `MediaChangeNotifier` so we can confirm the lib + integration test sources still compile together. Replace the body of `watcher_picks_up_new_file`:

```rust
#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_picks_up_new_file() {
    // 端到端：启动 watcher，丢一个文件进去，等待 upsert 出现在 DB 中。
    use photo_viewer::core::media_change_notifier::MediaChangeNotifier;
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let pool = db::init_pool(&dir.path().join("watch.db")).unwrap();
    let (notifier, _rx) = MediaChangeNotifier::new();
    let _watcher =
        photo_viewer::core::notify_watcher::start_watching(pool.clone(), vec![root.clone()], notifier);

    // 给 watcher 一点时间完成 setup
    std::thread::sleep(Duration::from_millis(300));

    write_plain_jpeg(&root, "watched.jpg");

    // 轮询 DB，最多 5 秒。
    let mut found = false;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        let items = db::list_all_media(&pool).unwrap();
        if items.iter().any(|m| m.path.ends_with("watched.jpg")) {
            found = true;
            break;
        }
    }
    assert!(found, "watcher 应当在 5s 内拾取新文件");
}
```

- [ ] **Step 8: Verify the integration test compiles (without running `#[ignore]`'d test)**

```bash
cargo test --test notify_watcher --no-run
```

Expected: compiles cleanly. Don't run the test (it's `#[ignore]`); just confirm compile.

- [ ] **Step 9: Commit**

```bash
git add src/core/notify_watcher.rs src/core/backend/local.rs tests/notify_watcher.rs
git commit -m "feat(core): notify_watcher uses MediaChangeNotifier for upsert/delete"
```

---

## Task 5: Add `apply_to_media_list` helper with headless tests

**Files:**
- Create: `src/ui/apply_to_media_list.rs`
- Modify: `src/ui/mod.rs` (add `pub mod apply_to_media_list;`)
- Create: `tests/apply_to_media_list.rs`

**Interfaces (consumed by later tasks):**
- `pub fn apply_to_media_list(list: &gtk::gio::ListStore, event: MediaChangeEvent)` — pure function, panic-free.

- [ ] **Step 1: Create `src/ui/apply_to_media_list.rs` with internal tests**

```rust
//! Apply a `MediaChangeEvent` to the shared `gio::ListStore`.
//!
//! Kept as a tiny free function in its own module so it can be tested
//! headlessly (no GTK window required). The list store is the single
//! data source backing the three `MediaGrid` instances on `PhotosPage`.

use crate::core::media::MediaItem;
use crate::core::media_change_notifier::MediaChangeEvent;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

/// Apply a `MediaChangeEvent` to `list`, preserving the position of
/// existing items. The function is panic-free: any unexpected type
/// mismatch in a list item is silently skipped.
pub fn apply_to_media_list(list: &gtk::gio::ListStore, event: MediaChangeEvent) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::media::MediaItem;
    use chrono::Utc;
    use std::path::PathBuf;

    fn item(id: i64, uri: &str) -> MediaItem {
        MediaItem {
            id,
            uri: uri.into(),
            path: PathBuf::from(uri.trim_start_matches("file://")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            width: Some(64),
            height: Some(48),
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "h".into(),
            trashed_at: None,
        }
    }

    fn list_with(items: Vec<MediaItem>) -> gtk::gio::ListStore {
        let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for it in items {
            list.append(&glib::BoxedAnyObject::new(it));
        }
        list
    }

    fn nth_uri(list: &gtk::gio::ListStore, idx: u32) -> String {
        list.item(idx)
            .and_downcast::<glib::BoxedAnyObject>()
            .unwrap()
            .borrow::<MediaItem>()
            .uri
            .clone()
    }

    #[test]
    fn upserted_appends_when_uri_absent() {
        let list = list_with(vec![item(1, "file:///tmp/a.jpg")]);
        apply_to_media_list(&list, MediaChangeEvent::Upserted(item(2, "file:///tmp/b.jpg")));
        assert_eq!(list.n_items(), 2);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/b.jpg");
    }

    #[test]
    fn upserted_replaces_in_place_when_uri_present() {
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
            item(3, "file:///tmp/c.jpg"),
        ]);
        let mut updated = item(2, "file:///tmp/b.jpg");
        updated.blake3_hash = "new-hash".into();
        apply_to_media_list(&list, MediaChangeEvent::Upserted(updated));
        assert_eq!(list.n_items(), 3, "upsert must not change list length");
        assert_eq!(nth_uri(&list, 1), "file:///tmp/b.jpg");
        // Sanity: the new blake3 hash actually took effect.
        let boxed = list.item(1).and_downcast::<glib::BoxedAnyObject>().unwrap();
        assert_eq!(boxed.borrow::<MediaItem>().blake3_hash, "new-hash");
    }

    #[test]
    fn removed_deletes_when_uri_present() {
        let list = list_with(vec![
            item(1, "file:///tmp/a.jpg"),
            item(2, "file:///tmp/b.jpg"),
        ]);
        apply_to_media_list(
            &list,
            MediaChangeEvent::Removed {
                uri: "file:///tmp/b.jpg".into(),
            },
        );
        assert_eq!(list.n_items(), 1);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
    }

    #[test]
    fn removed_is_noop_when_uri_absent() {
        let list = list_with(vec![item(1, "file:///tmp/a.jpg")]);
        apply_to_media_list(
            &list,
            MediaChangeEvent::Removed {
                uri: "file:///tmp/missing.jpg".into(),
            },
        );
        assert_eq!(list.n_items(), 1);
        assert_eq!(nth_uri(&list, 0), "file:///tmp/a.jpg");
    }
}
```

- [ ] **Step 2: Register the module in `src/ui/mod.rs`**

Add the line `pub mod apply_to_media_list;` to `src/ui/mod.rs` (in alphabetical order, after `album_picker` if present, or anywhere in the `pub mod` block). The re-export is not needed — the function is referenced by its full path in `app.rs`.

If `src/ui/mod.rs` does not currently export the function via `pub use`, no `pub use` is needed.

- [ ] **Step 3: Run the lib tests and verify they pass**

```bash
cargo test --lib ui::apply_to_media_list
```

Expected: 4 passed.

- [ ] **Step 4: Commit**

```bash
git add src/ui/apply_to_media_list.rs src/ui/mod.rs
git commit -m "feat(ui): apply_to_media_list helper with headless tests"
```

---

## Task 6: Wire `app::initialize` to consume the notifier

**Files:**
- Modify: `src/app.rs` (replace the `on_change` closure block at lines 113-138 with a notifier + consumer loop)

**Interfaces (consumed):** `MediaChangeNotifier::new()`, `start_watching(pool, paths, notifier)`, `apply_to_media_list(list, event)`.

- [ ] **Step 1: Replace the watcher block in `app.rs::initialize`**

Replace lines 113–138 of `src/app.rs` (the `let on_change = { ... }` closure and the `let _watcher = ...` call) with:

```rust
    // 启动文件监听（M5-T5+）：监听 ~/Pictures 的后续变更并增量 upsert。
    // 通过 `MediaChangeNotifier` 把"哪个 MediaItem 变了"推给 GTK 主线程
    // 消费者；消费者按 uri 在共享的 `media_list` 上做 splice/append/remove。
    let (notifier, change_rx) = crate::core::media_change_notifier::MediaChangeNotifier::new();
    let _watcher = crate::core::notify_watcher::start_watching(
        pool.clone(),
        vec![pictures],
        notifier,
    );

    // Consumer: GTK 主线程独占 `media_list` 的写权限，所以这里用 spawn_local
    // 把 mpsc 的 receiver 排到主循环。`install_tokio_runtime` 已让 tokio
    // reactor 在进程生命周期内驻留，所以 `rx.recv().await` 不需要额外配置。
    let list_for_consumer = list.clone();
    gtk::glib::MainContext::default().spawn_local(async move {
        let mut rx = change_rx;
        while let Some(event) = rx.recv().await {
            crate::ui::apply_to_media_list::apply_to_media_list(&list_for_consumer, event);
        }
    });
```

- [ ] **Step 2: Verify the crate compiles**

```bash
cargo build
```

Expected: clean, no warnings.

- [ ] **Step 3: Verify all lib tests still pass**

```bash
cargo test --lib
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): consume MediaChangeNotifier to refresh media_list on change"
```

---

## Task 7: Rewrite `tests/notify_watcher_callback.rs` to assert via notifier

**Files:**
- Modify: `tests/notify_watcher_callback.rs` (full rewrite)

- [ ] **Step 1: Replace the file with the notifier-based version**

```rust
//! Verify `start_watching` emits `MediaChangeEvent`s on the notifier.
//!
//! This test depends on `notify`'s inotify/fsevent behavior, so it's
//! `#[ignore]` by default. Run locally with
//! `cargo test --test notify_watcher_callback -- --ignored`.
mod common;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::media_change_notifier::{MediaChangeEvent, MediaChangeNotifier};
use photo_viewer::core::notify_watcher;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_upserted_after_successful_upsert() {
    let dir = tempdir().unwrap();
    let shots = dir.path().join("截图");
    std::fs::create_dir(&shots).unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let (notifier, mut rx) = MediaChangeNotifier::new();
    let _h = notify_watcher::start_watching(
        pool.clone(),
        vec![dir.path().to_path_buf()],
        notifier,
    );

    write_plain_png(&shots, "new.png");

    // Poll the receiver (50ms inotify event + 50ms write-sleep + DB write).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received = false;
    while Instant::now() < deadline {
        if let Ok(MediaChangeEvent::Upserted(item)) = rx.try_recv() {
            assert!(item.path.ends_with("new.png"));
            received = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(received, "watcher should have emitted an Upserted event");
}
```

- [ ] **Step 2: Verify the test compiles**

```bash
cargo test --test notify_watcher_callback --no-run
```

Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add tests/notify_watcher_callback.rs
git commit -m "test: rewrite notify_watcher_callback to use MediaChangeNotifier"
```

---

## Task 8: Add `tests/notify_watcher_notifier.rs` end-to-end event tests

**Files:**
- Create: `tests/notify_watcher_notifier.rs`

- [ ] **Step 1: Create the file**

```rust
//! End-to-end event emission tests for `notify_watcher` + `MediaChangeNotifier`.
//!
//! All tests in this file depend on `notify`'s inotify/fsevent behavior, so
//! they are `#[ignore]` by default. Run locally with
//! `cargo test --test notify_watcher_notifier -- --ignored`.
mod common;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::media_change_notifier::{MediaChangeEvent, MediaChangeNotifier};
use photo_viewer::core::notify_watcher;
use std::time::{Duration, Instant};
use tempfile::tempdir;

/// Spin up a watcher in `root` and return the receiver.
fn spawn_watcher(
    root: std::path::PathBuf,
) -> (
    photo_viewer::core::db::DbPool,
    tokio::sync::mpsc::UnboundedReceiver<MediaChangeEvent>,
    tokio::task::JoinHandle<()>,
) {
    let pool = db::init_pool(&root.join("test.db")).unwrap();
    let (notifier, rx) = MediaChangeNotifier::new();
    let h = notify_watcher::start_watching(pool.clone(), vec![root], notifier);
    // Give the watcher a moment to call `watcher.watch(...)`.
    std::thread::sleep(Duration::from_millis(300));
    (pool, rx, h)
}

/// Drain `rx` until we see an event whose uri matches `uri`, or the deadline
/// passes. Returns the event on success.
fn wait_for_uri(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<MediaChangeEvent>,
    uri: &str,
    timeout: Duration,
) -> Option<MediaChangeEvent> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(event) => {
                let matches = match &event {
                    MediaChangeEvent::Upserted(item) => item.uri == uri,
                    MediaChangeEvent::Removed { uri: u } => u == uri,
                };
                if matches {
                    return Some(event);
                }
                // Skip non-matching events.
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    None
}

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_upserted_for_new_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _h) = spawn_watcher(root.clone());

    let path = write_plain_jpeg(&root, "watched.jpg");
    let uri = format!("file://{}", path.display());

    let event = wait_for_uri(&mut rx, &uri, Duration::from_secs(5));
    assert!(
        matches!(event, Some(MediaChangeEvent::Upserted(_))),
        "expected Upserted for {uri}, got {event:?}"
    );
}

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_removed_for_deleted_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _h) = spawn_watcher(root.clone());

    let path = write_plain_jpeg(&root, "doomed.jpg");
    let uri = format!("file://{}", path.display());
    // Let the upsert settle before removing.
    assert!(
        wait_for_uri(&mut rx, &uri, Duration::from_secs(5)).is_some(),
        "expected upsert before delete"
    );

    std::fs::remove_file(&path).unwrap();
    let event = wait_for_uri(&mut rx, &uri, Duration::from_secs(5));
    assert!(
        matches!(event, Some(MediaChangeEvent::Removed { .. })),
        "expected Removed for {uri}, got {event:?}"
    );
}

#[test]
#[ignore = "depends on inotify/fsevent; may be flaky in CI sandboxes"]
fn watcher_emits_upserted_for_modified_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_pool, mut rx, _h) = spawn_watcher(root.clone());

    let path = write_plain_jpeg(&root, "modified.jpg");
    let uri = format!("file://{}", path.display());
    // First upsert.
    assert!(
        wait_for_uri(&mut rx, &uri, Duration::from_secs(5)).is_some(),
        "expected initial upsert"
    );

    // Re-write to trigger Modify(Data). We use std::fs::write instead of
    // the JPEG helper because the helper would produce a structurally
    // identical JPEG with the same hash, which might not be observed as
    // a Modify event on some backends.
    std::fs::write(&path, b"different bytes for modify event").unwrap();

    // We expect either another Upserted for the same uri, OR a Removed
    // + Upserted pair (depends on backend). Count Upserted events for
    // this uri within the window.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut upsert_count = 0;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(MediaChangeEvent::Upserted(item)) if item.uri == uri => upsert_count += 1,
            Ok(MediaChangeEvent::Removed { uri: u }) if u == uri => {
                // Removed counts toward "file changed" but is not strictly
                // what we assert on; allow it.
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    // After the initial upsert (counted in setup) + the post-modify
    // upsert, we expect at least 1 more Upserted event in the window.
    assert!(
        upsert_count >= 1,
        "expected at least one more Upserted after modify, got {upsert_count}"
    );
}
```

- [ ] **Step 2: Verify the test file compiles**

```bash
cargo test --test notify_watcher_notifier --no-run
```

Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add tests/notify_watcher_notifier.rs
git commit -m "test: end-to-end event tests for notify_watcher notifier"
```

---

## Task 9: Final verification

**Files:** none changed; runs commands only.

- [ ] **Step 1: Build cleanly**

```bash
cargo build
```

Expected: success, no warnings.

- [ ] **Step 2: Format and lint**

```bash
cargo fmt && cargo clippy --all-targets
```

Expected: no diff from `cargo fmt`, no clippy warnings.

- [ ] **Step 3: Run all unit tests**

```bash
cargo test --lib
```

Expected: all pass (including the 3 new `media_change_notifier` tests, 4 `apply_to_media_list` tests, the new `LocalBackend` tests, and the rewritten `notify_watcher` internal test).

- [ ] **Step 4: Run all non-ignored integration tests**

```bash
cargo test --tests
```

Expected: all pass (the existing 4 `tests/notify_watcher.rs` unit tests + the existing `LocalBackend`-related tests, with the 3 `#[ignore]` ones skipped).

- [ ] **Step 5: Compile-check the `#[ignore]` tests**

```bash
cargo test --tests -- --ignored --no-run
```

Expected: all compile. (Don't actually run them — they depend on inotify and may be slow / flaky in CI.)

- [ ] **Step 6: Manual smoke test**

```bash
cargo run
```

In another terminal:
```bash
cp /path/to/some/test.jpg ~/Pictures/
```

Expected: within ~1 second, the new tile appears in the Day view of the Photos page.

Then:
```bash
rm ~/ Pictures/test.jpg
```

Expected: within ~1 second, the tile disappears.

Then:
```bash
touch ~/Pictures/<some-existing-photo>.jpg
```

Expected: the tile is replaced in place (no scroll jump).

- [ ] **Step 7: Final commit (only if Step 2 produced a `cargo fmt` diff that wasn't auto-applied)**

```bash
git status
```

If `cargo fmt` produced changes that weren't committed, run:

```bash
cargo fmt
git add -u
git commit -m "style: cargo fmt"
```

---

## Out of Scope

The following are explicitly **not** part of this plan (matching the spec's Non-Goals):

- Multi-root directory configuration UI — `start_watching` continues to receive a single path.
- Fallback listening when `pictures_dir()` does not exist on disk.
- Replacing the bare `notify` watcher with `notify-debouncer-full`.
- Active push of Albums sidebar updates — Albums page still rebuilds on click.
- `ViewerPage` / `EditorPage` real-time refresh — they read the same shared list.
- `gtk::SortListModel` integration — new items are appended to preserve scroll position.

If a future task wants any of these, it should be a separate spec + plan.
