# Database And Refresh Architecture Design

## Purpose

The current app already has several large-library optimizations: startup loads an initial DB page, scanning skips unchanged files, batch upsert reduces write cost, and grids use virtual paging. The remaining problem is architectural rather than tactical. Database access, refresh decisions, UI list identity, album refreshes, and statistics updates are spread across UI widgets, app startup, scanner callbacks, and page actions. This makes every later optimization fragile.

This design establishes a durable data and refresh architecture for future work:

- Database access goes through a focused repository layer.
- Domain changes are emitted as events.
- UI state is a projection of repository state, not a second source of truth.
- Photos browsing separates global media identity from visible windows.
- Albums, trash, thumbnail stats, and visible grids refresh through coordinators instead of ad hoc calls.

## Current Failure Modes

The design addresses these concrete classes of problems:

- `gio::ListStore` is used as both the current visible media window and the apparent global media list. Virtual paging replaces the whole store, so selection and viewer navigation rely on unstable indexes.
- UI code synchronously calls database functions for counts, favorite state, and visible stats. This can block the GTK main loop during scans or thumbnail prewarm.
- Startup scan batches can trigger repeated `albums::refresh` tasks, while the scanner also refreshes albums at the end.
- User actions call database functions directly from page code, then manually patch UI lists.
- Batch upsert still performs per-row lookup and per-row materialization inside the transaction.
- Refresh behavior is encoded at call sites, so later performance work tends to add local debounce rather than clarify ownership.

## Architecture

### Layers

The app should move toward four explicit layers.

1. `core::repository`
   Owns all SQLite access for media, albums, trash projections, favorites, and stats. UI code does not call `core::db` directly.

2. `core::events`
   Defines domain events emitted after successful repository writes or reconciliation jobs. Events describe what changed by stable identity, not by UI index.

3. `core::refresh`
   Owns coalescing and single-flight refresh jobs for derived projections such as albums, trash, thumbnail stats, and live media counts.

4. `ui::models`
   Converts repository snapshots and domain events into GTK-facing models. Widgets consume these models and issue commands; they do not query SQLite directly.

The existing `core::db` module can remain the low-level SQL module during migration, but it becomes private plumbing behind repository APIs.

### Stable Identity

All cross-layer operations use stable identity:

- Primary identity: `MediaId`, a small Rust newtype around the existing SQLite `media_items.id` `i64`.
- Secondary identity for filesystem reconciliation: `uri`.
- UI selection stores `MediaId`, not list indexes.
- Viewer navigation stores a query context plus current `MediaId`.
- Visible tile activation resolves from tile metadata to `MediaId`.

Indexes remain useful only inside a specific visible window. They must not cross widget, page, or async boundaries.

The first implementation may pass raw `i64` while introducing the boundary, but public APIs added by this work should use `MediaId` so accidental index/id mixups become type errors.

### Repository APIs

The repository should expose task-oriented methods rather than raw SQL helpers. Initial target shape:

```rust
pub struct MediaRepository {
    pool: DbPool,
}

pub struct MediaPage {
    pub query: MediaQuery,
    pub start: u32,
    pub total: u32,
    pub items: Vec<MediaItem>,
}

pub enum MediaQuery {
    LiveAll,
    AlbumFolder(PathBuf),
    Favorites,
    Images,
    Videos,
    Trash,
}
```

Core methods:

- `count(query) -> Result<u32>`
- `page(query, start, limit) -> Result<MediaPage>`
- `items_by_ids(ids) -> Result<Vec<MediaItem>>`
- `favorite_state(ids) -> Result<FavoriteSummary>`
- `set_favorite(ids, bool) -> Result<MediaMutation>`
- `move_to_trash(ids) -> Result<MediaMutation>`
- `upsert_batch(items) -> Result<MediaMutation>`
- `remove_by_uri(uri) -> Result<MediaMutation>`

`MediaMutation` contains changed ids and enough detail to emit events without re-querying at UI call sites.

### Domain Events

Events are emitted only after the database transaction or reconciliation job succeeds.

```rust
pub enum DomainEvent {
    MediaUpserted { source: ChangeSource, items: Vec<MediaItem> },
    MediaRemoved { source: ChangeSource, ids: Vec<i64>, uris: Vec<String> },
    MediaUpdated { source: ChangeSource, items: Vec<MediaItem>, fields: MediaFields },
    TrashChanged { source: ChangeSource },
    AlbumsDirty { source: ChangeSource },
    ThumbnailStatsDirty,
    LiveCountDirty,
}
```

Events are not a replacement for the database. They are invalidation and patch hints. If a consumer misses an event or receives a broad dirty event, it reloads from the repository.

### Threading Model

SQLite work runs off the GTK main thread through existing blocking-task patterns. Event delivery back to GTK remains on `glib::MainContext::default().spawn_local`, but the event handler only mutates in-memory GTK models or schedules background repository work. It must not run SQLite queries directly.

Repository methods are synchronous at the core layer because rusqlite is synchronous. UI-facing adapters wrap them in `gio::spawn_blocking` or a small async service so widgets receive futures/signals rather than raw blocking calls.

### Refresh Coordinator

`core::refresh::RefreshCoordinator` consumes domain events and schedules derived refresh jobs.

Rules:

- Album refresh is single-flight. If a refresh is running, new album dirty events mark one pending refresh.
- Startup scan does not refresh albums for every batch. It marks albums dirty and flushes once after startup scan completion, or after a coarse quiet window if interactive accuracy is needed.
- User-visible actions such as favorite/delete can request high-priority projection updates, but still go through the same coordinator.
- Thumbnail stats and live media counts are cached values updated through dirty events or periodic background polling, not synchronous UI queries.

This coordinator is the only place that decides debounce windows and refresh priority.

## Photos Browsing Model

### Problem With Current Model

The shared `gio::ListStore` currently backs Year, Month, Day, viewer, and selection operations. Virtual paging replaces that store with a DB window. This makes the store neither a complete global list nor a purely local view.

### New Model

Introduce a `MediaWindowModel` owned by `PhotosPage`:

```rust
pub struct MediaWindowModel {
    query: MediaQuery,
    total: u32,
    window_start: u32,
    items: gio::ListStore,
    ids_in_window: Vec<i64>,
    generation: u64,
}
```

Responsibilities:

- Load visible DB windows through `MediaRepository::page`.
- Expose a GTK `ListStore` for FlowBox rendering only.
- Map visible child activation to `media_id`.
- Maintain generation counters so stale DB results cannot replace newer windows.
- Apply event patches only when changed ids are in the current window; otherwise mark the window dirty.

The window model is not the app's global media collection. It is a projection of one query and one visible range.

### Viewer Navigation

Viewer should receive:

- `MediaQuery`
- current `media_id`
- current known window
- repository handle

When navigating left/right beyond the known window, viewer asks repository for the next page or neighbor by sort key. This avoids depending on a mutable `ListStore` that only contains the current grid window.

The first migration can keep viewer navigation inside the loaded window, but the public interface should no longer accept an unstable global index.

### Selection And Batch Actions

Selection stores `HashSet<i64>` media ids. Batch operations pass ids into repository command methods. After success, events update visible projections. This prevents operations from affecting the wrong media after a virtual page replacement.

## Albums And Trash

Albums are a derived projection. Direct calls to `albums::refresh` from UI pages should be removed over time.

Target behavior:

- Repository mutations emit `AlbumsDirty` when they can affect albums or virtual album counts.
- `RefreshCoordinator` runs `albums::refresh` single-flight.
- Sidebar album rows observe an `AlbumsProjection` or receive an `AlbumsRefreshed` signal after the coordinator completes.
- Trash reconciliation emits `TrashChanged`, `AlbumsDirty`, and live-count dirty events through the same event bus.

Trash page remains a DB projection, but it is loaded through repository APIs and refreshed by events.

## Thumbnail Stats

Thumbnail workers already own thumbnail generation. The UI should not poll SQLite directly every second from `MediaGrid`.

Target behavior:

- `ThumbnailLoader` or prewarm code emits `ThumbnailStatsDirty` after marking generated ids.
- A stats projection coalesces dirty events and reloads `generated_count` on a background thread.
- `MediaGrid` receives updated stats through a cheap in-memory value or signal.

## Migration Plan

This should be delivered incrementally.

### Phase 1: Repository Boundary And Read Model

- Add `core::repository` with `MediaRepository`, `MediaQuery`, `MediaPage`, and batch favorite-state APIs.
- Move UI synchronous DB reads behind repository calls executed off the GTK main thread.
- Keep existing `core::db` functions as implementation details.
- Add tests for query ordering, page metadata, and favorite summary.

### Phase 2: Event Bus And Refresh Coordinator

- Add `core::events` and replace `MediaChangeNotifier` with or wrap it in `DomainEvent`.
- Add `RefreshCoordinator` for albums, live count, and thumbnail stats invalidation.
- Remove startup per-batch album refresh from `app.rs`.
- Add tests for single-flight album refresh and pending refresh replay.

### Phase 3: Photos Window Model

- Add `ui::models::media_window_model`.
- Move virtual page loading and stale-generation handling out of `MediaGrid`.
- Change grid activation and selection to use `media_id`.
- Keep Year/Month/Day rendering behavior but make indexes local to the window.
- Add tests for selection survival across window replacement and stale page discard.

### Phase 4: Viewer And Batch Action Identity

- Change `ViewerPage::new` and `show_at` to accept query context plus `media_id`.
- Change batch add-to-album, favorite, and trash flows to pass ids only.
- Remove remaining UI logic that resolves long-lived actions by list index.

### Phase 5: Derived Projection Cleanup

- Move albums sidebar, trash page, and thumbnail stats onto repository/projection APIs.
- Remove direct `albums::refresh` calls from page widgets.
- Update module docs for storage, browsing, albums/trash, and viewer invariants.

## Testing Strategy

Tests should prove architectural invariants, not just individual SQL results.

- Repository tests use temporary SQLite DBs and assert ordering, pagination totals, favorite summaries, and batch mutation results.
- Refresh coordinator tests use fake jobs to assert single-flight behavior, pending dirty replay, and startup coalescing.
- Media window model tests assert that selection is keyed by id, stale page results are ignored, and replacing a visible window does not change selected ids.
- UI-level tests cover opening viewer from a paged grid, selecting then scrolling to another page, and batch actions affecting the intended media ids.
- Existing trash and album tests should keep guarding mark-before-trash and external reconciliation behavior.

## Non-Goals

- Do not replace SQLite.
- Do not rewrite thumbnail decoding or scanner metadata extraction as part of this architecture pass.
- Do not change visual design of grids, viewer chrome, or Liquid Glass controls.
- Do not introduce a general event-sourcing database. Events are runtime invalidation and projection signals; SQLite remains the durable source of truth.

## Success Criteria

- UI widgets no longer call `core::db` directly.
- Long-lived UI state uses `media_id` or `uri`, never virtual list indexes.
- Startup scan does not launch repeated album rebuilds per scan batch.
- Albums, trash, live counts, and thumbnail stats refresh through one coordinator.
- Photos virtual paging is represented as a query window projection, not as the global media list.
- A future optimization can improve SQL pagination, thumbnail stats, or album aggregation behind repository/projection boundaries without changing widget code.
