# DB Actor Refresh Hub Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route all SQLite mutations through a single DB actor and drive UI refresh through precise runtime events.

**Architecture:** `DbActor` serializes DB mutations and emits `DomainEvent` values. `UiRefreshHub` lives on the GTK main thread and dispatches those events to page subscribers. Existing repository reads remain in place for paging/search/viewer navigation while mutation paths migrate incrementally.

**Tech Stack:** Rust, GTK4/libadwaita, tokio mpsc/oneshot, rusqlite/r2d2_sqlite, existing `DomainEvent` and `MediaRepository` types.

---

## File Structure

- Create `src/core/db_actor.rs`: command protocol, actor handle, actor loop, command execution tests.
- Create `src/ui/refresh_hub.rs`: GTK-thread subscription registry and dispatch tests.
- Modify `src/core/events.rs`: add precise event variants and `MediaFields` flags.
- Modify `src/core/mod.rs`: export `db_actor`.
- Modify `src/ui/mod.rs`: export `refresh_hub`.
- Modify `src/app.rs`: create actor and hub, dispatch actor events through hub, keep legacy `apply_to_media_list` bridge during migration.
- Modify `src/core/bootstrap.rs`: send startup scan/prune DB mutations through actor after scanner extraction.
- Modify `src/core/notify_watcher.rs`: send upsert/delete events through actor instead of directly mutating DB.
- Modify `src/ui/photos_page.rs`: migrate favorite/trash actions to actor-backed calls and hub events.
- Modify `src/ui/viewer_page.rs`: migrate favorite/trash/rename to actor-backed calls and hub events.
- Modify `src/ui/trash_page.rs`: migrate restore/delete/empty to actor-backed calls.
- Modify `src/ui/window.rs`, `src/ui/album_detail_page.rs`, `src/ui/album_picker.rs`: migrate album cover/ignore/delete/add-to-album refresh decisions to events.
- Update `docs/modules/storage.md`, `docs/modules/browsing.md`, `docs/modules/albums-trash.md`, and `docs/modules/viewer.md`.

## Task 1: Add DB Actor Skeleton

**Files:**
- Create: `src/core/db_actor.rs`
- Modify: `src/core/mod.rs`

- [x] Add `DbActorHandle`, `DbCommand`, `DbCommandResult`, and `start_db_actor`.
- [x] Add tests proving command responses and event forwarding work.
- [x] Export `db_actor` from `core::mod`.
- [x] Run `cargo test db_actor`.

## Task 2: Add UI Refresh Hub

**Files:**
- Create: `src/ui/refresh_hub.rs`
- Modify: `src/ui/mod.rs`

- [x] Add `UiRefreshHub`, `UiRefreshScope`, and `SubscriptionToken`.
- [x] Add subscribe/unsubscribe/dispatch tests using fake events.
- [x] Export `refresh_hub` from `ui::mod`.
- [x] Run `cargo test refresh_hub`.

## Task 3: Extend Domain Events

**Files:**
- Modify: `src/core/events.rs`
- Modify: event match sites in `src/app.rs` and `src/ui/apply_to_media_list.rs`

- [x] Extend `MediaFields` with thumbnail and attributes flags.
- [x] Add `MediaMovedToTrash`, `MediaRestored`, `AlbumsChanged`, and `AlbumCoverChanged`.
- [x] Update exhaustive matches to ignore new events until subscribers migrate.
- [x] Run focused event tests through `cargo test -q apply_to_media_list`.

## Task 4: Wire Actor And Hub At Startup

**Files:**
- Modify: `src/app.rs`

- [x] Create `DbActorHandle` after DB initialization.
- [x] Create one `UiRefreshHub` per window.
- [x] Forward actor `DomainEvent` receiver into `UiRefreshHub::dispatch`.
- [x] Register a legacy Photos/sidebar bridge that applies current events to `media_list` and refreshes sidebar, preserving current behavior while migration continues.
- [x] Run focused startup tests and `cargo test -q app::tests::startup_preload_reconcile_runs_before_initial_live_page_query`.

## Task 5: Migrate Scanner, Watcher, And Startup Prune

**Files:**
- Modify: `src/core/bootstrap.rs`
- Modify: `src/core/notify_watcher.rs`
- Modify: `src/core/backend/local.rs`

- [x] Change scanner callback to submit `UpsertMediaBatch` to `DbActor`.
- [x] Change watcher create/modify/delete/rename DB writes to actor commands.
- [x] Change startup missing-file prune to actor command.
- [x] Preserve background-thread execution for filesystem work.
- [x] Run watcher-focused tests: `cargo test -q notify_watcher`, `cargo test -q --test notify_watcher_callback`, `cargo test -q --test notify_watcher_notifier`.
- [x] Run scanner/startup tests: `cargo test -q --test bootstrap`, `cargo test -q --test local_scan`, and focused startup prune/app tests.

## Task 6: Migrate Photos And Viewer Core Actions

**Files:**
- Modify: `src/ui/photos_page.rs`
- Modify: `src/ui/viewer_page.rs`

- [x] Pass `DbActorHandle` into Photos and Viewer.
- [ ] Migrate favorite, move-to-trash, and rename mutations.
  - Done: Photos favorite, Photos move-to-trash, Viewer favorite.
  - Remaining: Viewer trash/rename and editor save follow-up refresh.
- [ ] Register page subscribers to patch media items by id.
- [x] Remove page-local `albums::refresh` calls for migrated Photos favorite/trash actions.
- [x] Run `cargo test -q photos_page`, `cargo test -q viewer_page`, and `cargo test -q apply_to_media_list`.

## Task 7: Migrate Trash And Album Actions

**Files:**
- Modify: `src/ui/trash_page.rs`
- Modify: `src/ui/window.rs`
- Modify: `src/ui/album_detail_page.rs`
- Modify: `src/ui/album_picker.rs`

- [ ] Route restore/delete/empty trash DB mutations through actor.
- [ ] Route album cover, ignore album, delete album, and add-to-album DB commits through actor.
- [ ] Register TrashPage and AlbumDetail subscribers for precise id patches.
- [ ] Keep empty-trash and broad trash-reconcile fallback reloads.
- [ ] Run `cargo test trash_page album_detail_page window`.

## Task 8: Remove Legacy Mutation Paths And Update Docs

**Files:**
- Modify docs and any remaining UI direct mutation call sites.

- [ ] Search for UI-layer `albums::refresh`, `repo.set_favorite`, `repo.move_to_trash`, `db::delete_*`, and replace remaining mutation paths.
- [ ] Update module docs with actor/hub invariants.
- [ ] Run `cargo fmt`.
- [ ] Run targeted tests and `cargo test`.

## Self-Review Notes

- The plan intentionally keeps repository reads out of actor scope in this pass.
- Empty trash and broad trash reconcile are allowed to reload a full visible trash projection.
- Precision is by `MediaId`; URI is compatibility metadata only.
