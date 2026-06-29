# Database And Refresh Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework database access and UI refresh around repository, domain events, refresh coordination, and stable media identity.

**Architecture:** SQLite remains the durable source of truth, but UI code stops calling `core::db` directly. Repository APIs own DB access, domain events describe successful mutations, refresh coordinators own derived projection updates, and GTK-facing models represent visible windows keyed by stable `MediaId`.

**Tech Stack:** Rust, GTK4/libadwaita, rusqlite/r2d2_sqlite, Tokio/GIO blocking tasks, existing test harness.

---

## File Structure

- Create `src/core/identity.rs`: typed `MediaId` newtype around `i64`.
- Create `src/core/repository.rs`: repository read/write APIs, `MediaQuery`, `MediaPage`, `FavoriteSummary`, `MediaMutation`.
- Create `src/core/events.rs`: `DomainEvent`, `ChangeSource`, `MediaFields`, event channel wrapper.
- Create `src/core/refresh.rs`: single-flight refresh coordinator and projection invalidation types.
- Create `src/ui/models/mod.rs`: UI model module entry point.
- Create `src/ui/models/media_window_model.rs`: Photos visible-window projection and id-based selection helpers.
- Modify `src/core/mod.rs`: export new modules.
- Modify `src/ui/mod.rs`: export `ui::models`.
- Modify `src/app.rs`: wire repository/event bus/refresh coordinator; remove startup per-batch album refresh.
- Modify `src/core/bootstrap.rs`: emit domain events or bridge existing notifier output into domain events.
- Modify `src/core/backend/local.rs`: use repository batch upsert once repository is ready.
- Modify `src/core/thumbnails.rs` and `src/core/thumbnail_prewarm.rs`: emit thumbnail stats dirty events or route stats through projection.
- Modify `src/ui/photos_page.rs`: use `MediaWindowModel`, id-based selection, repository commands.
- Modify `src/ui/media_grid.rs`: render a provided visible window and emit `MediaId`, not long-lived indexes.
- Modify `src/ui/viewer_page.rs`: accept query context plus `MediaId`; keep first migration window-bounded if needed.
- Modify `src/ui/window.rs`, `src/ui/trash_page.rs`, `src/ui/album_detail_page.rs`: replace direct refresh/query calls with repository/projection calls where touched.
- Modify docs: `docs/modules/storage.md`, `docs/modules/browsing.md`, `docs/modules/albums-trash.md`, `docs/modules/viewer.md`.
- Add tests: `tests/repository.rs`, `tests/refresh_coordinator.rs`, `tests/media_window_model.rs`.

## Task 1: Add Stable Media Identity

**Files:**
- Create: `src/core/identity.rs`
- Modify: `src/core/mod.rs`
- Test: `cargo test identity`

- [ ] **Step 1: Add the type**

Create `src/core/identity.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaId(i64);

impl MediaId {
    pub fn new(value: i64) -> Self {
        Self(value)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl From<i64> for MediaId {
    fn from(value: i64) -> Self {
        Self::new(value)
    }
}

impl From<MediaId> for i64 {
    fn from(value: MediaId) -> Self {
        value.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_id_round_trips_i64() {
        let id = MediaId::from(42_i64);
        assert_eq!(id.get(), 42);
        assert_eq!(i64::from(id), 42);
    }
}
```

- [ ] **Step 2: Export it**

Modify `src/core/mod.rs`:

```rust
pub mod identity;
pub use identity::MediaId;
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test media_id_round_trips_i64
```

Expected: test passes.

- [ ] **Step 4: Commit**

```bash
git add src/core/identity.rs src/core/mod.rs
git commit -m "feat: add stable media id type"
```

## Task 2: Add Repository Read Model

**Files:**
- Create: `src/core/repository.rs`
- Modify: `src/core/mod.rs`
- Test: `tests/repository.rs`

- [ ] **Step 1: Write repository pagination tests**

Create `tests/repository.rs`:

```rust
mod common;

use chrono::{TimeZone, Utc};
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::repository::{MediaQuery, MediaRepository};

fn item(id_name: &str, ts: i64) -> NewMediaItem {
    let path = std::path::PathBuf::from(format!("/tmp/{id_name}.jpg"));
    NewMediaItem {
        uri: format!("file:///tmp/{id_name}.jpg"),
        path: path.clone(),
        folder_path: std::path::PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc.timestamp_opt(ts, 0).unwrap(),
        file_size: 1,
        blake3_hash: String::new(),
    }
}

#[test]
fn repository_live_page_returns_total_and_ordered_rows() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo.db")).unwrap();
    photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("older", 10), item("newer", 20), item("middle", 15)],
    )
    .unwrap();

    let repo = MediaRepository::new(pool);
    let page = repo.page(MediaQuery::LiveAll, 0, 2).unwrap();

    assert_eq!(page.total, 3);
    assert_eq!(page.start, 0);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].uri, "file:///tmp/newer.jpg");
    assert_eq!(page.items[1].uri, "file:///tmp/middle.jpg");
}
```

- [ ] **Step 2: Run test and confirm it fails**

Run:

```bash
cargo test --test repository repository_live_page_returns_total_and_ordered_rows
```

Expected: fail because `core::repository` does not exist.

- [ ] **Step 3: Implement read model**

Create `src/core/repository.rs`:

```rust
use crate::core::db::{self, DbPool};
use crate::core::error::Result;
use crate::core::media::MediaItem;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaQuery {
    LiveAll,
    AlbumFolder(PathBuf),
    Favorites,
    Images,
    Videos,
    Trash,
}

#[derive(Debug, Clone)]
pub struct MediaPage {
    pub query: MediaQuery,
    pub start: u32,
    pub total: u32,
    pub items: Vec<MediaItem>,
}

#[derive(Clone)]
pub struct MediaRepository {
    pool: DbPool,
}

impl MediaRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    pub fn count(&self, query: MediaQuery) -> Result<u32> {
        match query {
            MediaQuery::LiveAll => Ok(db::count_live_media(&self.pool)? as u32),
            MediaQuery::Trash => Ok(db::list_trashed_media(&self.pool)?.len() as u32),
            MediaQuery::AlbumFolder(path) => Ok(db::list_media_by_folder(&self.pool, &path)?.len() as u32),
            MediaQuery::Favorites => Ok(db::list_favorite_media_ids(&self.pool)?.len() as u32),
            MediaQuery::Images | MediaQuery::Videos => {
                let kind = match query {
                    MediaQuery::Images => "image",
                    MediaQuery::Videos => "video",
                    _ => unreachable!(),
                };
                count_by_media_kind(&self.pool, kind)
            }
        }
    }

    pub fn page(&self, query: MediaQuery, start: u32, limit: u32) -> Result<MediaPage> {
        let total = self.count(query.clone())?;
        let items = match &query {
            MediaQuery::LiveAll => db::list_media_page(&self.pool, start, limit)?,
            MediaQuery::Trash => page_vec(db::list_trashed_media(&self.pool)?, start, limit),
            MediaQuery::AlbumFolder(path) => page_vec(db::list_media_by_folder(&self.pool, path)?, start, limit),
            MediaQuery::Favorites => page_by_ids(&self.pool, db::list_favorite_media_ids(&self.pool)?, start, limit)?,
            MediaQuery::Images => page_by_kind(&self.pool, "image", start, limit)?,
            MediaQuery::Videos => page_by_kind(&self.pool, "video", start, limit)?,
        };
        Ok(MediaPage {
            query,
            start,
            total,
            items,
        })
    }
}

fn page_vec(items: Vec<MediaItem>, start: u32, limit: u32) -> Vec<MediaItem> {
    items.into_iter().skip(start as usize).take(limit as usize).collect()
}

fn page_by_ids(pool: &DbPool, ids: Vec<i64>, start: u32, limit: u32) -> Result<Vec<MediaItem>> {
    let mut out = Vec::new();
    for id in ids.into_iter().skip(start as usize).take(limit as usize) {
        out.push(db::get_media_item(pool, id)?);
    }
    Ok(out)
}

fn count_by_media_kind(pool: &DbPool, media_kind: &str) -> Result<u32> {
    Ok(page_by_kind(pool, media_kind, 0, u32::MAX)?.len() as u32)
}

fn page_by_kind(pool: &DbPool, media_kind: &str, start: u32, limit: u32) -> Result<Vec<MediaItem>> {
    let items = db::list_media_page(pool, 0, u32::MAX)?;
    Ok(items
        .into_iter()
        .filter(|item| {
            (media_kind == "image" && item.is_image()) || (media_kind == "video" && item.is_video())
        })
        .skip(start as usize)
        .take(limit as usize)
        .collect())
}
```

This first implementation may reuse existing DB helpers even if some queries are not optimal. Later tasks move SQL down behind repository-specific helpers.

- [ ] **Step 4: Export module**

Modify `src/core/mod.rs`:

```rust
pub mod repository;
pub use repository::{MediaPage, MediaQuery, MediaRepository};
```

- [ ] **Step 5: Verify**

Run:

```bash
cargo test --test repository
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/core/repository.rs src/core/mod.rs tests/repository.rs
git commit -m "feat: add media repository read model"
```

## Task 3: Add Repository Favorite Summary And Mutations

**Files:**
- Modify: `src/core/repository.rs`
- Test: `tests/repository.rs`

- [ ] **Step 1: Add failing favorite summary test**

Append to `tests/repository.rs`:

```rust
#[test]
fn repository_favorite_summary_batches_ids() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-favs.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("a", 10), item("b", 20)],
    )
    .unwrap();
    photo_viewer::core::db::set_media_favorite(&pool, inserted[0].id, true).unwrap();

    let repo = MediaRepository::new(pool);
    let summary = repo.favorite_state(&[inserted[0].id.into(), inserted[1].id.into()]).unwrap();

    assert!(summary.has_favorite);
    assert!(summary.has_unfavorite);
}
```

- [ ] **Step 2: Run test and confirm failure**

Run:

```bash
cargo test --test repository repository_favorite_summary_batches_ids
```

Expected: fail because `favorite_state` and `FavoriteSummary` do not exist.

- [ ] **Step 3: Implement summary and mutation types**

Modify `src/core/repository.rs`:

```rust
use crate::core::identity::MediaId;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FavoriteSummary {
    pub has_favorite: bool,
    pub has_unfavorite: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MediaMutation {
    pub changed_ids: Vec<MediaId>,
    pub changed_items: Vec<MediaItem>,
    pub removed_uris: Vec<String>,
}

impl MediaRepository {
    pub fn favorite_state(&self, ids: &[MediaId]) -> Result<FavoriteSummary> {
        let mut summary = FavoriteSummary::default();
        for id in ids {
            match db::is_media_favorite(&self.pool, id.get()) {
                Ok(true) => summary.has_favorite = true,
                Ok(false) => summary.has_unfavorite = true,
                Err(_) => summary.has_unfavorite = true,
            }
            if summary.has_favorite && summary.has_unfavorite {
                break;
            }
        }
        Ok(summary)
    }

    pub fn set_favorite(&self, ids: &[MediaId], is_favorite: bool) -> Result<MediaMutation> {
        let mut mutation = MediaMutation::default();
        for id in ids {
            db::set_media_favorite(&self.pool, id.get(), is_favorite)?;
            mutation.changed_ids.push(*id);
            mutation.changed_items.push(db::get_media_item(&self.pool, id.get())?);
        }
        Ok(mutation)
    }
}
```

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test repository
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/repository.rs tests/repository.rs
git commit -m "feat: add repository favorite mutations"
```

## Task 4: Add Domain Events

**Files:**
- Create: `src/core/events.rs`
- Modify: `src/core/mod.rs`
- Test: `cargo test domain_event_sender_sends_events`

- [ ] **Step 1: Add events module**

Create `src/core/events.rs`:

```rust
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSource {
    StartupScan,
    FilesystemWatcher,
    UserInteractive,
    TrashReconcile,
    ThumbnailWorker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaFields {
    pub favorite: bool,
    pub metadata: bool,
    pub location: bool,
    pub trash: bool,
}

impl MediaFields {
    pub const FAVORITE: Self = Self {
        favorite: true,
        metadata: false,
        location: false,
        trash: false,
    };
}

#[derive(Debug, Clone)]
pub enum DomainEvent {
    MediaUpserted { source: ChangeSource, items: Vec<MediaItem> },
    MediaRemoved { source: ChangeSource, ids: Vec<MediaId>, uris: Vec<String> },
    MediaUpdated { source: ChangeSource, items: Vec<MediaItem>, fields: MediaFields },
    TrashChanged { source: ChangeSource },
    AlbumsDirty { source: ChangeSource },
    ThumbnailStatsDirty,
    LiveCountDirty,
}

#[derive(Clone)]
pub struct DomainEventSender {
    tx: mpsc::UnboundedSender<DomainEvent>,
}

impl DomainEventSender {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<DomainEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    pub fn send(&self, event: DomainEvent) {
        if let Err(err) = self.tx.send(event) {
            tracing::warn!("DomainEventSender send failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_event_sender_sends_events() {
        let (sender, mut rx) = DomainEventSender::new();
        sender.send(DomainEvent::LiveCountDirty);
        assert!(matches!(rx.try_recv(), Ok(DomainEvent::LiveCountDirty)));
    }
}
```

- [ ] **Step 2: Export module**

Modify `src/core/mod.rs`:

```rust
pub mod events;
pub use events::{ChangeSource, DomainEvent, DomainEventSender, MediaFields};
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test domain_event_sender_sends_events
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/core/events.rs src/core/mod.rs
git commit -m "feat: add domain event channel"
```

## Task 5: Add Refresh Coordinator

**Files:**
- Create: `src/core/refresh.rs`
- Modify: `src/core/mod.rs`
- Test: `tests/refresh_coordinator.rs`

- [ ] **Step 1: Write single-flight test**

Create `tests/refresh_coordinator.rs`:

```rust
use photo_viewer::core::refresh::RefreshCoordinator;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn album_refresh_is_single_flight_with_pending_replay() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_job = calls.clone();
    let coordinator = RefreshCoordinator::new_for_tests(move || {
        calls_for_job.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    assert!(coordinator.mark_albums_dirty());
    assert!(!coordinator.mark_albums_dirty());
    coordinator.finish_album_refresh_for_tests().unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
```

- [ ] **Step 2: Run test and confirm failure**

Run:

```bash
cargo test --test refresh_coordinator
```

Expected: fail because `RefreshCoordinator` does not exist.

- [ ] **Step 3: Implement coordinator state machine**

Create `src/core/refresh.rs`:

```rust
use crate::core::error::Result;
use std::cell::Cell;
use std::rc::Rc;

type AlbumRefreshJob = Rc<dyn Fn() -> Result<()>>;

#[derive(Clone)]
pub struct RefreshCoordinator {
    album_refresh_running: Rc<Cell<bool>>,
    album_refresh_pending: Rc<Cell<bool>>,
    album_job: AlbumRefreshJob,
}

impl RefreshCoordinator {
    pub fn new_for_tests<F>(album_job: F) -> Self
    where
        F: Fn() -> Result<()> + 'static,
    {
        Self {
            album_refresh_running: Rc::new(Cell::new(false)),
            album_refresh_pending: Rc::new(Cell::new(false)),
            album_job: Rc::new(album_job),
        }
    }

    pub fn mark_albums_dirty(&self) -> bool {
        if self.album_refresh_running.get() {
            self.album_refresh_pending.set(true);
            return false;
        }
        self.album_refresh_running.set(true);
        let result = (self.album_job)();
        if let Err(err) = result {
            tracing::warn!("album refresh failed: {err}");
        }
        true
    }

    pub fn finish_album_refresh_for_tests(&self) -> Result<()> {
        self.album_refresh_running.set(false);
        if self.album_refresh_pending.replace(false) {
            self.mark_albums_dirty();
        }
        Ok(())
    }
}
```

This test-oriented synchronous version establishes the state machine. A later step wraps the job in `gio::spawn_blocking` for app use.

- [ ] **Step 4: Export module**

Modify `src/core/mod.rs`:

```rust
pub mod refresh;
pub use refresh::RefreshCoordinator;
```

- [ ] **Step 5: Verify**

Run:

```bash
cargo test --test refresh_coordinator
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/core/refresh.rs src/core/mod.rs tests/refresh_coordinator.rs
git commit -m "feat: add refresh coordinator state"
```

## Task 6: Remove Startup Per-Batch Album Refresh

**Files:**
- Modify: `src/app.rs`
- Modify: `src/core/bootstrap.rs`
- Test: `cargo test --test bootstrap`

- [ ] **Step 1: Add an explicit coordinator coalescing test**

Extend `tests/refresh_coordinator.rs` with a test that models startup scan batches by calling `mark_albums_dirty()` repeatedly while a refresh is running:

```rust
#[test]
fn startup_album_dirty_events_coalesce_while_refresh_runs() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_job = calls.clone();
    let coordinator = RefreshCoordinator::new_for_tests(move || {
        calls_for_job.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    assert!(coordinator.mark_albums_dirty());
    assert!(!coordinator.mark_albums_dirty());
    assert!(!coordinator.mark_albums_dirty());
    coordinator.finish_album_refresh_for_tests().unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
```

- [ ] **Step 2: Remove per-batch refresh branch**

In `src/app.rs`, replace the startup batch branch around the current per-event `albums::refresh` with a dirty mark:

```rust
if !is_startup_scan_batch {
    if let Some(window) = window_for_consumer.upgrade() {
        window.refresh_album_rows();
    }
} else {
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "startup scan batch applied; album refresh deferred"
    );
}
```

Keep the final `albums::refresh(&pool)` in `bootstrap.rs` until the async coordinator is wired. This removes the repeated rebuilds without changing final correctness.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --test bootstrap
cargo test --test albums
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/core/bootstrap.rs tests/refresh_coordinator.rs
git commit -m "fix: defer startup album refresh batches"
```

## Task 7: Add UI Models Module And Media Window Model

**Files:**
- Create: `src/ui/models/mod.rs`
- Create: `src/ui/models/media_window_model.rs`
- Modify: `src/ui/mod.rs`
- Test: `tests/media_window_model.rs`

- [ ] **Step 1: Write model test**

Create `tests/media_window_model.rs`:

```rust
mod common;

use chrono::{TimeZone, Utc};
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::repository::{MediaQuery, MediaRepository};
use photo_viewer::ui::models::media_window_model::MediaWindowModel;

fn item(name: &str, ts: i64) -> NewMediaItem {
    let path = std::path::PathBuf::from(format!("/tmp/{name}.jpg"));
    NewMediaItem {
        uri: format!("file:///tmp/{name}.jpg"),
        path: path.clone(),
        folder_path: std::path::PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc.timestamp_opt(ts, 0).unwrap(),
        file_size: 1,
        blake3_hash: String::new(),
    }
}

#[test]
fn media_window_selection_survives_window_replacement_by_id() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("window.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(
        &pool,
        &[item("a", 30), item("b", 20), item("c", 10)],
    )
    .unwrap();
    let selected = inserted[1].id.into();

    let repo = MediaRepository::new(pool);
    let mut model = MediaWindowModel::new(MediaQuery::LiveAll, 2);
    model.load_sync(&repo, 0).unwrap();
    model.select(selected);
    model.load_sync(&repo, 1).unwrap();

    assert!(model.is_selected(selected));
    assert_eq!(model.window_start(), 1);
}
```

- [ ] **Step 2: Run test and confirm failure**

Run:

```bash
cargo test --test media_window_model
```

Expected: fail because UI model module does not exist.

- [ ] **Step 3: Implement model**

Create `src/ui/models/mod.rs`:

```rust
pub mod media_window_model;
```

Create `src/ui/models/media_window_model.rs`:

```rust
use crate::core::error::Result;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::collections::HashSet;

pub struct MediaWindowModel {
    query: MediaQuery,
    page_size: u32,
    window_start: u32,
    total: u32,
    generation: u64,
    ids_in_window: Vec<MediaId>,
    selected: HashSet<MediaId>,
    store: gtk::gio::ListStore,
}

impl MediaWindowModel {
    pub fn new(query: MediaQuery, page_size: u32) -> Self {
        Self {
            query,
            page_size,
            window_start: 0,
            total: 0,
            generation: 0,
            ids_in_window: Vec::new(),
            selected: HashSet::new(),
            store: gtk::gio::ListStore::new::<glib::BoxedAnyObject>(),
        }
    }

    pub fn load_sync(&mut self, repo: &MediaRepository, start: u32) -> Result<()> {
        let page = repo.page(self.query.clone(), start, self.page_size)?;
        self.window_start = page.start;
        self.total = page.total;
        self.generation = self.generation.saturating_add(1);
        self.ids_in_window = page.items.iter().map(|item| MediaId::from(item.id)).collect();
        replace_store_items(&self.store, page.items);
        Ok(())
    }

    pub fn store(&self) -> gtk::gio::ListStore {
        self.store.clone()
    }

    pub fn window_start(&self) -> u32 {
        self.window_start
    }

    pub fn select(&mut self, id: MediaId) {
        self.selected.insert(id);
    }

    pub fn is_selected(&self, id: MediaId) -> bool {
        self.selected.contains(&id)
    }

    pub fn id_at_window_index(&self, index: u32) -> Option<MediaId> {
        self.ids_in_window.get(index as usize).copied()
    }
}

fn replace_store_items(store: &gtk::gio::ListStore, items: Vec<MediaItem>) {
    let additions: Vec<glib::BoxedAnyObject> =
        items.into_iter().map(glib::BoxedAnyObject::new).collect();
    store.splice(0, store.n_items(), &additions);
}
```

- [ ] **Step 4: Export module**

Modify `src/ui/mod.rs`:

```rust
pub mod models;
```

- [ ] **Step 5: Verify**

Run:

```bash
cargo test --test media_window_model
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/ui/models/mod.rs src/ui/models/media_window_model.rs src/ui/mod.rs tests/media_window_model.rs
git commit -m "feat: add media window model"
```

## Task 8: Change Grid Activation Boundary To MediaId

**Files:**
- Modify: `src/ui/media_grid.rs`
- Modify: `src/ui/photos_page.rs`
- Test: existing grid/UI tests

- [ ] **Step 1: Change callback type**

In `src/ui/media_grid.rs`, change activation callback from `Rc<dyn Fn(u32)>` to:

```rust
pub type ActivateCallback = Rc<dyn Fn(crate::core::identity::MediaId)>;
```

When building each tile, compute:

```rust
let media_id = crate::core::identity::MediaId::from(item.id);
```

Store per-section activation ids as `Vec<MediaId>` instead of global indexes. `displayed_items` may temporarily keep indexes for rendering tests, but activation must call:

```rust
on_act(media_id);
```

- [ ] **Step 2: Update PhotosPage callback**

In `src/ui/photos_page.rs`, change:

```rust
let on_activate: Rc<dyn Fn(u32)>
```

to:

```rust
let on_activate: Rc<dyn Fn(crate::core::identity::MediaId)>
```

and update `open_viewer` signature:

```rust
fn open_viewer(&self, media_id: crate::core::identity::MediaId)
```

Temporarily resolve `media_id` to the current store index with a helper until viewer migration is complete:

```rust
fn index_for_media_id(list: &gtk::gio::ListStore, media_id: MediaId) -> Option<u32> {
    for i in 0..list.n_items() {
        let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if obj.borrow::<MediaItem>().id == media_id.get() {
            return Some(i);
        }
    }
    None
}
```

This keeps behavior stable while removing activation's dependency on long-lived indexes.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --test ui_context_menu
cargo test --test ux_click_flows
cargo test --test e2e_browsing
```

Expected: pass, except pre-existing failures must be documented before continuing.

- [ ] **Step 4: Commit**

```bash
git add src/ui/media_grid.rs src/ui/photos_page.rs
git commit -m "refactor: activate media grid items by id"
```

## Task 9: Move Photos Selection To MediaId

**Files:**
- Modify: `src/ui/media_grid.rs`
- Modify: `src/ui/photos_page.rs`
- Test: `tests/media_window_model.rs`, `tests/ui_context_menu.rs`, `tests/ux_click_flows.rs`

- [ ] **Step 1: Replace selected index sets**

In `MediaGrid` implementation state, replace:

```rust
pub selected: RefCell<HashSet<u32>>,
```

with:

```rust
pub selected: RefCell<HashSet<crate::core::identity::MediaId>>,
```

Make `selected_indices()` become `selected_ids()` and return `Vec<MediaId>`.

- [ ] **Step 2: Update PhotosPage selected state**

In `PhotosPage` implementation state, replace:

```rust
pub selected_indices: RefCell<HashSet<u32>>,
```

with:

```rust
pub selected_ids: RefCell<HashSet<crate::core::identity::MediaId>>,
```

Rename helpers:

- `selected_indices_vec` -> `selected_ids_vec`
- `media_ids_for_indices` -> delete or replace with `selected_ids_vec`
- `favorite_state_for_indices` -> `favorite_state_for_ids`
- `open_album_picker_for_indices` -> `open_album_picker_for_ids`
- `delete_to_trash_for_indices` -> `delete_to_trash_for_ids`
- `set_favorite_for_indices` -> `set_favorite_for_ids`

- [ ] **Step 3: Keep a compatibility adapter only at UI boundaries**

Where `AlbumPickerDialog::present` still expects `Vec<i64>`, convert:

```rust
let ids: Vec<i64> = media_ids.into_iter().map(|id| id.get()).collect();
```

Do not convert back to indexes.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test media_window_model
cargo test --test ui_context_menu
cargo test --test ux_click_flows
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/ui/media_grid.rs src/ui/photos_page.rs tests/media_window_model.rs
git commit -m "refactor: store photo selection by media id"
```

## Task 10: Route Favorite And Trash Commands Through Repository

**Files:**
- Modify: `src/core/repository.rs`
- Modify: `src/ui/photos_page.rs`
- Test: `tests/repository.rs`, trash flow tests

- [ ] **Step 1: Add repository trash mutation test**

Append to `tests/repository.rs`:

```rust
#[test]
fn repository_set_favorite_returns_changed_items() {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("repo-set-fav.db")).unwrap();
    let inserted = photo_viewer::core::db::upsert_media_items_batch(&pool, &[item("a", 10)]).unwrap();

    let repo = MediaRepository::new(pool);
    let mutation = repo.set_favorite(&[inserted[0].id.into()], true).unwrap();

    assert_eq!(mutation.changed_ids, vec![inserted[0].id.into()]);
    assert_eq!(mutation.changed_items.len(), 1);
    assert!(mutation.changed_items[0].is_favorite);
}
```

- [ ] **Step 2: Add or complete `move_to_trash` repository method**

In `src/core/repository.rs`:

```rust
pub fn move_to_trash(&self, ids: &[MediaId]) -> Result<MediaMutation> {
    let mut mutation = MediaMutation::default();
    for id in ids {
        let item = db::get_media_item(&self.pool, id.get())?;
        crate::core::trash::move_to_trash_marked(&self.pool, item.id, &item.uri)?;
        mutation.changed_ids.push(*id);
        mutation.removed_uris.push(item.uri);
    }
    Ok(mutation)
}
```

- [ ] **Step 3: Update PhotosPage workers**

Replace direct calls to `db::set_media_favorite`, `db::get_media_item`, and `trash::move_to_trash_marked` in `PhotosPage` with `MediaRepository` methods inside `gio::spawn_blocking`.

Keep this explicit transitional UI patching until Task 11 introduces domain-event projection updates:

```rust
this.update_media_favorite_flags_by_ids(&mutation.changed_ids, is_favorite);
this.remove_media_by_ids(&raw_ids);
```

The event projection will replace manual patching later.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test repository
cargo test --test trash_flow
cargo test --test ux_click_flows
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/repository.rs src/ui/photos_page.rs tests/repository.rs
git commit -m "refactor: route photo commands through repository"
```

## Task 11: Introduce Domain Event Bridge In App

**Files:**
- Modify: `src/app.rs`
- Modify: `src/core/media_change_notifier.rs` or add bridge helpers in `src/core/events.rs`
- Test: existing notifier tests

- [ ] **Step 1: Add bridge helper**

In `src/core/events.rs`, add:

```rust
impl From<crate::core::media_change_notifier::MediaChangeSource> for ChangeSource {
    fn from(source: crate::core::media_change_notifier::MediaChangeSource) -> Self {
        match source {
            crate::core::media_change_notifier::MediaChangeSource::StartupScan => Self::StartupScan,
            crate::core::media_change_notifier::MediaChangeSource::UserInteractive => Self::UserInteractive,
        }
    }
}
```

If the existing enum has more variants, map each explicitly.

- [ ] **Step 2: App consumer converts media change events**

In `src/app.rs`, keep the existing `MediaChangeNotifier` receiver initially, but convert `UpsertedBatch`, `Upserted`, `Removed`, and `TrashChanged` into `DomainEvent` before applying UI changes. This establishes one event vocabulary without a full scanner rewrite.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test media_change_notifier
cargo test domain_event_sender_sends_events
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/core/events.rs src/core/media_change_notifier.rs
git commit -m "refactor: bridge media changes to domain events"
```

## Task 12: Wire Async Refresh Coordinator For Albums

**Files:**
- Modify: `src/core/refresh.rs`
- Modify: `src/app.rs`
- Modify: `src/ui/window.rs` only if a callback hook is needed
- Test: `tests/refresh_coordinator.rs`, album tests

- [ ] **Step 1: Add async app constructor**

Extend `RefreshCoordinator` with an app-facing constructor that accepts `DbPool` and a UI callback:

```rust
pub fn new(pool: DbPool, on_albums_refreshed: Rc<dyn Fn()>) -> Self
```

The app-facing `mark_albums_dirty_async` should:

- If running, set pending and return.
- Spawn `gio::spawn_blocking(move || albums::refresh(&pool))`.
- On completion, call `on_albums_refreshed`.
- If pending was set during the run, clear pending and run once more.

- [ ] **Step 2: Route album dirty events**

In `src/app.rs`, when processing domain events:

- `MediaUpserted` from startup scan: apply media list patch, mark albums dirty without immediate sidebar refresh.
- `MediaUpdated` favorite: mark albums dirty.
- `MediaRemoved`: mark albums dirty and apply the removal to visible media projections.
- `TrashChanged`: call `MainWindow::refresh_visible_trash_page()` and mark albums dirty.

- [ ] **Step 3: Remove direct startup album refresh branch**

Delete the per-batch `spawn_blocking(albums::refresh)` branch entirely. The only album refresh paths should be bootstrap final refresh until fully migrated, and coordinator-driven refresh.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test refresh_coordinator
cargo test --test albums
cargo test --test album_navigation
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/refresh.rs src/app.rs src/ui/window.rs tests/refresh_coordinator.rs
git commit -m "feat: coordinate album refreshes"
```

## Task 13: Move Virtual Page Loading Out Of MediaGrid

**Files:**
- Modify: `src/ui/models/media_window_model.rs`
- Modify: `src/ui/photos_page.rs`
- Modify: `src/ui/media_grid.rs`
- Test: `tests/media_window_model.rs`, browsing tests

- [ ] **Step 1: Add generation stale discard test**

Append to `tests/media_window_model.rs`:

```rust
#[test]
fn stale_generation_does_not_replace_newer_window() {
    let mut model = MediaWindowModel::new(MediaQuery::LiveAll, 2);
    let old = model.next_generation_for_tests();
    let new = model.next_generation_for_tests();
    assert!(!model.generation_is_current_for_tests(old));
    assert!(model.generation_is_current_for_tests(new));
}
```

- [ ] **Step 2: Add generation helpers**

In `MediaWindowModel`:

```rust
pub fn next_generation_for_tests(&mut self) -> u64 {
    self.generation = self.generation.saturating_add(1);
    self.generation
}

pub fn generation_is_current_for_tests(&self, generation: u64) -> bool {
    self.generation == generation
}
```

Use equivalent private helpers for production async page loads.

- [ ] **Step 3: Move virtual page state**

Move these responsibilities from `MediaGrid` into `PhotosPage`/`MediaWindowModel`:

- current window start
- total live count
- query in-flight flag
- pending target start/ratio
- stale generation checks
- DB page query via repository

Keep `MediaGrid` responsible for:

- rendering the provided store
- calculating scroll ratio and emitting a load-window request callback
- thumbnail reprioritization
- visual skeleton rendering if the model reports loading

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test media_window_model
cargo test --test e2e_browsing
cargo test --test ui_grid_canvas
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/ui/models/media_window_model.rs src/ui/photos_page.rs src/ui/media_grid.rs tests/media_window_model.rs
git commit -m "refactor: move virtual paging into window model"
```

## Task 14: Migrate Viewer Opening To Query Plus MediaId

**Files:**
- Modify: `src/ui/viewer_page.rs`
- Modify: `src/ui/photos_page.rs`
- Test: viewer and browsing tests

- [ ] **Step 1: Add viewer constructor**

Add a new constructor while keeping the old one temporarily:

```rust
pub fn new_for_query(
    query: crate::core::repository::MediaQuery,
    current_id: crate::core::identity::MediaId,
    initial_items: gtk::gio::ListStore,
) -> Self
```

Internally resolve `current_id` to the current window index for the first migration.

- [ ] **Step 2: Update PhotosPage open path**

When opening from grid activation, pass:

- current `MediaQuery`
- activated `MediaId`
- current window `ListStore`

Do not pass the original activation index across the page boundary.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --test e2e_viewer
cargo test --test e2e_browsing
cargo test --test ux_click_flows
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/ui/viewer_page.rs src/ui/photos_page.rs
git commit -m "refactor: open viewer by media id"
```

## Task 15: Replace UI Synchronous Stats Queries

**Files:**
- Modify: `src/core/refresh.rs`
- Modify: `src/core/thumbnails.rs`
- Modify: `src/core/thumbnail_prewarm.rs`
- Modify: `src/ui/media_grid.rs`
- Test: thumbnail tests and browsing tests

- [ ] **Step 1: Add stats projection state**

In `src/core/refresh.rs`, add:

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct LibraryStats {
    pub live_total: usize,
    pub thumbnails_generated: usize,
}
```

Expose a cached value and an update callback. The DB reload must happen in `gio::spawn_blocking`, not from `MediaGrid`.

- [ ] **Step 2: Emit dirty callbacks after thumbnail marks**

Add a callback hook on `ThumbnailLoader` that the app can set:

```rust
pub fn set_stats_dirty_callback(&self, callback: Arc<dyn Fn() + Send + Sync>)
```

After `mark_thumbnails_generated`, invoke the callback. The app callback sends or schedules `ThumbnailStatsDirty` on the main event path. This keeps worker threads independent from GTK objects while still removing direct stats polling from `MediaGrid`.

- [ ] **Step 3: Update MediaGrid stats label**

Replace:

```rust
let generated = loader.generated_count();
```

with an in-memory stats value passed from `PhotosPage` or `MediaWindowModel`.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test thumbnails
cargo test --test e2e_browsing
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/refresh.rs src/core/thumbnails.rs src/core/thumbnail_prewarm.rs src/ui/media_grid.rs
git commit -m "refactor: cache library stats outside grid"
```

## Task 16: Tighten Repository SQL Internals

**Files:**
- Modify: `src/core/db.rs`
- Modify: `src/core/repository.rs`
- Test: repository, media CRUD, local scan tests

- [ ] **Step 1: Add batch upsert behavior test**

In `tests/repository.rs`, add a test that repository `upsert_batch` returns materialized items in input order and clears `trashed_at` for restored rows.

- [ ] **Step 2: Replace N+1 upsert path**

In `src/core/db.rs`, add an internal helper using SQLite `RETURNING` if the bundled SQLite version supports it:

```sql
INSERT INTO media_items (...)
VALUES (...)
ON CONFLICT(uri) DO UPDATE SET ...
RETURNING id, uri, path, folder_path, mime_type, media_subkind,
          media_attributes, width, height, video_duration_secs, taken_at,
          file_mtime, file_size, blake3_hash, is_favorite, trashed_at
```

If `RETURNING` is not available in the build environment, keep the existing path and only move N+1 behind repository boundaries. Do not block the architecture migration on SQLite feature availability.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --test repository
cargo test --test media_crud
cargo test --test local_scan
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/core/db.rs src/core/repository.rs tests/repository.rs
git commit -m "perf: streamline repository upserts"
```

## Task 17: Cleanup Direct DB And Refresh Calls From UI

**Files:**
- Modify: `src/ui/photos_page.rs`
- Modify: `src/ui/viewer_page.rs`
- Modify: `src/ui/trash_page.rs`
- Modify: `src/ui/window.rs`
- Modify: `src/ui/album_detail_page.rs`
- Test: broad UI and domain tests

- [ ] **Step 1: Find direct calls**

Run:

```bash
rg -n "core::db|db::|albums::refresh|count_live_media|generated_count|list_media_page" src/ui src/app.rs
```

Expected remaining direct calls are either gone or isolated in app-level wiring with a documented migration reason.

- [ ] **Step 2: Replace UI calls**

For each remaining UI direct DB call:

- Move read/write to `MediaRepository`.
- Run it via `gio::spawn_blocking` from UI code.
- Return results through callbacks/events.
- Keep widget code focused on model mutation and presentation.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --test trash_flow
cargo test --test album_navigation
cargo test --test e2e_browsing
cargo test --test e2e_viewer
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/ui src/app.rs
git commit -m "refactor: remove direct ui database refreshes"
```

## Task 18: Update Module Documentation

**Files:**
- Modify: `docs/modules/storage.md`
- Modify: `docs/modules/browsing.md`
- Modify: `docs/modules/albums-trash.md`
- Modify: `docs/modules/viewer.md`

- [ ] **Step 1: Update storage docs**

Document:

- `core::repository` is the UI-facing DB boundary.
- `core::events` describes post-commit changes.
- `core::refresh` coordinates derived projection refreshes.
- `core::db` remains SQL plumbing.

- [ ] **Step 2: Update browsing docs**

Document:

- Photos grid uses a visible `MediaWindowModel`.
- Selection and activation use `MediaId`.
- Virtual indexes are local to a window only.

- [ ] **Step 3: Update albums/trash and viewer docs**

Document:

- Albums refresh through coordinator.
- Trash page is repository-backed and event-refreshed.
- Viewer opens by query plus `MediaId`.

- [ ] **Step 4: Verify docs diff**

Run:

```bash
git diff -- docs/modules/storage.md docs/modules/browsing.md docs/modules/albums-trash.md docs/modules/viewer.md
```

Expected: docs describe the new architecture and name any remaining transitional adapters with the task that removes them.

- [ ] **Step 5: Commit**

```bash
git add docs/modules/storage.md docs/modules/browsing.md docs/modules/albums-trash.md docs/modules/viewer.md
git commit -m "docs: document database refresh architecture"
```

## Task 19: Final Verification

**Files:**
- No planned edits.

- [ ] **Step 1: Run formatting**

```bash
cargo fmt
```

Expected: no formatting diff, or commit formatting-only changes if Rust files changed.

- [ ] **Step 2: Run focused suites**

```bash
cargo test --test repository
cargo test --test refresh_coordinator
cargo test --test media_window_model
cargo test --test bootstrap
cargo test --test trash_flow
cargo test --test album_navigation
cargo test --test e2e_browsing
cargo test --test e2e_viewer
```

Expected: pass.

- [ ] **Step 3: Run broad tests**

```bash
cargo test
```

Expected: pass, allowing only documented pre-existing GTK warnings.

- [ ] **Step 4: Run static review search**

```bash
rg -n "core::db|db::|albums::refresh|global_index|selected_indices|generated_count\\(" src/ui src/app.rs
```

Expected: no direct UI DB calls except explicitly documented transitional adapters; no long-lived selection by index.

- [ ] **Step 5: Commit final cleanup**

```bash
git status --short
git add .
git commit -m "chore: finalize db refresh architecture migration"
```

Only commit if there are real cleanup changes. Do not include unrelated user changes.
