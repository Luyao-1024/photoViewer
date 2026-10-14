# Changelog

## Unreleased

- Added the native SyncServer durable receive path (C1.2): snapshot bootstrap
  with an independent marker per record type, an ordered replayable delta inbox,
  a validated typed remote graph, and promotion that materializes resource
  obligations. `applied` stays behind `target` while any of them is
  outstanding, so a fully received but un-downloaded photo never reports itself
  as synchronized. A changed scope, namespace or database generation discards
  the previous completeness claim rather than inheriting it.

- Replaced the WebDAV synchronization engine with a native SyncServer client
  (C1.1): typed protocol DTOs, endpoint normalization, the health/capability
  probe, capability/scope/origin validation, and a pinned capability scope on
  the connection. Native jobs map a local root to an explicitly configured
  zone. Media synchronization itself (receive, leases, upload, publication) and
  the settings surface are not implemented yet; see
  `docs/modules/native-sync-plan.md` for the C1.2–C1.6 order.
- Removed the WebDAV sync feature outright: provider, planner, reconciler,
  backup archives, conflict resolution, run scheduler, settings group and the
  WebDAV-specific i18n/CSS/template chrome are gone. Cloud badges now reflect a
  confirmed native pair (`native_confirmed_generation` with no outstanding
  dirty bits).
- Schema 9: the sync tables were redesigned for the native client and the
  WebDAV-era columns were dropped. Upgrading a schema-8 library preserves
  media, albums, favorites and edits, and discards the WebDAV sync history that
  no longer has a consumer.
- Credentials: the platform keyring now stores one Bearer API key per native
  connection under a dedicated service name; SQLite keeps only the reference.
- Real-server acceptance: `tests/native_sync_probe.rs` drives the actual
  `syncserver` binary (probe, auth failures, origin checks, unknown-zone
  rejection); it skips with build instructions when the sibling checkout is
  absent.

## 0.9.0

- Renamed the application ID to `io.github.luyao_1024.photoviewer` based on the
  GitHub repository identity for Flathub/AppStream compatibility.
- Updated Flatpak, desktop, metainfo, icon installation names, development
  runner, visual check script, and tests to use the new app ID.
- Added Flathub-oriented AppStream metadata, including homepage, bugtracker,
  developer, and 0.9 release notes.
- Added `cargo-sources.json` and switched the Flatpak manifest to offline cargo
  source resolution.
- Removed the abandoned Huawei FUSE experiment and an unused editor helper
  module from the release tree.

## 0.5.0 (M5 complete)

- Empty states (AdwStatusPage) for all views
- Toast feedback via AdwToastOverlay
- LRU thumbnail cache cleanup (2GB limit)
- Application icon (symbolic + 64/128 PNG)
- Incremental file watcher (notify crate)
- System theme follow (AdwStyleManager)

## 0.4.0 (M4)

- EditorPage with live preview (30fps)
- 5 built-in EditOperations (Rotate/Crop/Brightness/Contrast/Saturation)
- Save Copy / Save Overwrite (mixed strategy + confirmation)
- Destructive rotation with 5s undo toast

## 0.3.0 (M3)

- AlbumsPage (folder-as-album grid)
- AlbumDetailPage (single album photos)
- TrashPage (multi-select + batch restore/delete)
- Sidebar routing (Photos/Albums/Trash)
- System trash integration with basename collision handling

## 0.2.0 (M2)

- ThumbnailLoader (worker pool + disk cache, 3 size buckets)
- ViewerPage (fullscreen + zoom/pan + keyboard nav + preloading)
- Real thumbnails replace gray placeholders

## 0.1.0 (M1)

- Project scaffold (Rust + GTK4 + Libadwaita)
- SQLite schema + migrations
- LocalBackend filesystem scanner
- EXIF DateTimeOriginal extraction
- PhotosPage with Year/Month/Day views (shared ListStore)
