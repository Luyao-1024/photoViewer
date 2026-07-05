# DB Actor And Refresh Hub Design

## Purpose

The existing repository/event architecture improved identity and paging, but DB
mutations still happen from many UI pages and background workers. Each caller
then decides which visible model, sidebar row, album projection, or trash page
to refresh. That makes correctness depend on every mutation site remembering
all downstream UI projections.

This design makes mutation ownership explicit:

- All SQLite mutations go through one `DbActor`.
- `DbActor` emits precise `DomainEvent` values after successful mutations.
- A GTK-main-thread `UiRefreshHub` distributes events to registered UI
  subscribers.
- UI pages patch their local projections by `MediaId`, not by list index or
  broad page rebuilds.

SQLite reads for paging, search, viewer neighbour lookup, thumbnail prewarm, and
library stats remain repository reads in this phase. This keeps long read paths
off the mutation queue while consolidating write ordering and refresh ownership.

## Non-Goals

- Do not move metadata extraction, directory walking, thumbnail decoding, or
  file copy/trash/delete work onto the DB actor.
- Do not rewrite visual layouts.
- Do not make events durable. SQLite remains the source of truth; events are
  runtime patch/invalidation hints.
- Do not force full library materialization for precision. Precision applies to
  known affected ids/items within bounded UI windows.

## Thread Model

`DbActor` runs as a single background async task backed by an unbounded command
channel. Each command performs synchronous rusqlite/r2d2 work inside the actor
task's blocking context or a dedicated blocking loop. The actor may perform
small reads needed to build response payloads and events, but it must not do
slow filesystem work.

Slow work stays outside the actor:

- Scanner walks roots and extracts `NewMediaItem` values, then sends
  `UpsertMediaBatch`.
- Watcher classifies notify events, then sends path-level DB commands.
- Trash, restore, permanent delete, rename, copy/move, and prune stat passes run
  in blocking workers, then submit DB commit/rollback commands.
- Thumbnail workers generate thumbnails off-thread and only submit DB marker or
  stats-dirty commands.

`UiRefreshHub` lives on the GTK main thread. It receives `DomainEvent` values
from the actor and invokes registered callbacks. Callbacks may mutate GTK
models, but must not run DB queries directly.

## Components

### `core::db_actor`

Owns:

- `DbActorHandle`, cloneable command sender.
- `DbCommand`, the mutation protocol.
- `DbCommandResult`, responses for callers that need values.
- `start_db_actor(pool, event_sender)`.

Initial commands:

- `UpsertMediaBatch { source, items }`
- `DeleteLiveByPath { source, path }`
- `PruneMissingLiveRows { roots, excluded_roots }`
- `SetFavorite { ids, is_favorite }`
- `MarkTrashed { ids }`
- `RollbackTrashed { ids }`
- `RestoreRows { ids }`
- `DeleteRows { ids }`
- `UpdateMediaLocation { id, path, folder_path }`
- `SetAlbumCover { folder_path, cover_uri }`
- `IgnoreAlbum { folder_path }`
- `RefreshAlbums { source }`
- `ReconcileTrash { pictures_root }`

Commands emit precise events after DB success. If a command also refreshes the
albums materialized view, the event stream includes `AlbumsChanged`.

### `core::events`

Keeps the existing `DomainEvent` vocabulary but extends payload precision.
Consumers can still treat events as dirty hints when local precision is not yet
implemented.

Required event shape:

- `MediaUpserted { source, items }`
- `MediaUpdated { source, items, fields }`
- `MediaRemoved { source, ids, uris }`
- `MediaMovedToTrash { source, items }`
- `MediaRestored { source, items }`
- `TrashChanged { source }`
- `AlbumsChanged { source, affected_folders, affected_virtual, live_count_delta }`
- `AlbumCoverChanged { folder_path, cover_uri }`
- `ThumbnailStatsDirty`
- `LiveCountDirty`

`MediaFields` expands to include `thumbnail` and `attributes` flags. Events
always include ids where rows are removed or changed.

### `ui::refresh_hub`

GTK-main-thread dispatcher:

```rust
pub struct UiRefreshHub;
pub struct SubscriptionToken(u64);

impl UiRefreshHub {
    pub fn new() -> Self;
    pub fn subscribe(
        &self,
        scope: UiRefreshScope,
        callback: Rc<dyn Fn(&DomainEvent)>,
    ) -> SubscriptionToken;
    pub fn unsubscribe(&self, token: SubscriptionToken);
    pub fn dispatch(&self, event: DomainEvent);
}
```

Scopes describe ownership and aid debugging:

- `Photos`
- `Sidebar`
- `TrashPage(u64)`
- `AlbumDetail(String)`
- `Viewer(u64)`

Closed pages must unsubscribe. Long-lived `MainWindow`/`PhotosPage` subscribers
live for the window lifetime.

## UI Projection Rules

Photos:

- Insert/update/remove by `MediaId`.
- Preserve sort order and `ui_media_list_cap`.
- Do not call `albums::refresh` or sidebar refresh from page actions.

Viewer:

- Subscribe to current item and filmstrip window ids.
- Current item update refreshes title/details/buttons.
- Current item remove/trash navigates to next item or pops.
- Rename/favorite updates arrive via `MediaUpdated`.

Trash:

- Restore/permanent delete patch matching ids.
- Empty trash may clear/reload the whole view.
- External trash reconcile may fall back to `TrashPage::refresh()` when the
  precise added/removed set is broad.

Albums and sidebar:

- Phase 1: `AlbumsChanged` triggers sidebar snapshot reload.
- Later: affected folders/virtual albums patch individual rows.
- `AlbumCoverChanged` patches the matching row cover.

Album detail:

- Folder albums patch if the affected item's folder matches.
- Favorites and type virtual albums insert/remove based on updated item
  membership.
- Large albums keep bounded windows; events outside the visible window can mark
  the page dirty instead of forcing full materialization.

## Migration Strategy

1. Add `DbActor` and `UiRefreshHub` with tests.
2. Route scanner, watcher, and startup prune mutations through `DbActor`.
3. Route Photos and Viewer favorite/trash/rename through `DbActor`.
4. Route TrashPage restore/delete/empty through `DbActor`.
5. Route album cover, ignore album, delete album, and album picker mutations
   through `DbActor`.
6. Remove UI-layer direct DB mutation calls and ad hoc refresh calls.

At the end of migration, UI modules may still use repository reads for initial
pages and projections, but all DB mutation calls must go through
`DbActorHandle`.

## Success Criteria

- A code search for UI-layer direct calls to mutation DB/repository methods
  finds none except inside `DbActor` or file-operation orchestration helpers.
- Every mutation emits one or more `DomainEvent` values.
- Photos, Viewer, Trash, AlbumDetail, and Sidebar react through
  `UiRefreshHub` subscriptions.
- Single-image favorite/rename/trash operations patch only affected items.
- Startup remains non-blocking; scan, prune, trash reconcile, and thumbnail
  prewarm stay in background workers.
