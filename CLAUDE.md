# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Photo Viewer — a GNOME desktop photo manager (GTK4 + Libadwaita) in Rust, designed for smooth browsing of 10k–100k photos. M1–M5 complete (0.5.0). Docs/comments are bilingual (Chinese + English).

## Build / Run / Test

System deps (Fedora): `gtk4-devel libadwaita-devel gdk-pixbuf2-devel libheif-devel sqlite-devel`. Also requires `blueprint-compiler` on PATH — `build.rs` shells out to it to compile `data/ui/*.blp` → `.ui`, then bundles the `.ui` + icons into a GResource via `glib_build_tools`. **Edit the `.blp` files, not the generated `.ui`.**

```bash
cargo build            # also runs build.rs: blueprint compile + gresource bundle
cargo run
cargo test             # all integration tests live in tests/
cargo test --test <name>   # e.g. --test edit_ops, --test e2e_browsing
cargo fmt && cargo clippy --all-targets
```

`meson.build` is install-time only (desktop file + hicolor icons) — `cargo build` is the inner build.

### Flatpak visual testing

Liquid Glass / `backdrop-filter` visuals must be checked with the Flatpak GNOME 50 runtime, not the host `cargo run`: this machine's host GTK can be older than the Flatpak runtime and may ignore `backdrop-filter`.

For current-worktree visual checks, build locally first and run the debug binary inside the installed app sandbox:

```bash
cargo build
flatpak run \
  --filesystem=/home/luyao/workspace/photo_viewer/photoViewer \
  --filesystem=home \
  --command=sh org.gnome.PhotoViewer \
  -c 'exec /home/luyao/workspace/photo_viewer/photoViewer/target/debug/photo-viewer'
```

Do not use `flatpak-builder --force-clean` as a routine visual-test step in this workspace while the known `rofiles-fuse` unmount hang is present.

## Architecture

Three-layer split, intentionally decoupled (`src/core/` has zero GTK dependency except `core/edit` which returns `image::DynamicImage` and touches `glib` only for `ParamValue` variant conversion):

- **`core/`** — data layer. `db.rs` (r2d2 `SqliteConnectionManager` pool, `init_pool`/`run_migrations`, `schema.sql` embedded via `include_str!`); `media.rs` (`MediaItem`/`NewMediaItem`); `backend/` (`LocalBackend` filesystem scanner + `scan_worker` spawn); `metadata.rs` (EXIF `DateTimeOriginal` via kamadak-exif); `thumbnails.rs`; `albums.rs`; `trash.rs`; `section_model.rs` (Year/Month/Day grouping); `notify_watcher.rs` (incremental `notify` watcher); `edit/`.
- **`ui/`** — GTK widgets (custom `CompositeTemplate` subclasses, see below).
- **`platform/`** — XDG integration (`xdg.rs`).

### Runtime integration (read this before touching async)
`src/app.rs::build_app` builds a multi-thread tokio runtime and **`forget`s its `EnterGuard`** so the thread-local reactor stays entered for the process lifetime. GTK's main loop is not a tokio runtime, so without this `spawn_blocking` (used by thumbnail workers + scan worker) panics with "there is no reactor running". Async DB/scan init runs via `gtk::glib::MainContext::default().spawn_local`, then injects the resulting `DbPool` + `Arc<ThumbnailLoader>` into the `MainWindow` and pages.

### DB & state
SQLite with WAL/FK pragmas applied via the r2d2 `with_init` hook. `MediaItem`s are surfaced to GTK by wrapping each in `glib::BoxedAnyObject` and appending to a shared `gio::ListStore`. `trashed_at IS NULL` partial indexes separate live vs. trashed photos. Schema is idempotent `CREATE TABLE IF NOT EXISTS`; `schema_version` table tracks migration.

### Thumbnails
`ThumbnailLoader` owns an mpsc queue feeding N blocking workers (`spawn_workers`). Requests carry a `oneshot::Sender<Texture>`; the cache key is `path + mtime` blake3-hashed (mtime change invalidates). Disk cache is bucketed by `small|medium|large` size; an in-memory `LruCache<Texture>` avoids re-decoding.

### Edit operations
`EditOperation` trait (`apply(&DynamicImage, ParamValue) -> DynamicImage`) + `EditRegistry` (`new_with_v1` registers the 5 built-ins). `apply_all` runs the `EditState` pipeline rotation→brightness→contrast→saturation→crop, skipping no-op params. Add new ops by implementing the trait + registering. Destructive rotation has a 5s undo toast path (`destructive_rotate.rs`).

### UI navigation
`MainWindow` (window.blp template) = sidebar `ListBox` + `adw::NavigationView`. Sidebar rows 0/1/2 → Photos (root) / Albums / Trash, pushed lazily on `row-selected`. `PhotosPage` hosts three `MediaGrid` instances (Year/Month/Day) sharing one `ListStore`; clicking a tile pushes a `ViewerPage`, which can launch `EditorPage`. Pages receive the `NavigationView` and `DbPool` via injected setters (`set_nav_target` / `set_db_pool`).

### GTK widget pattern
UI objects use the `gtk::subclass`Relm-ish pattern: an `imp` module with `#[derive(CompositeTemplate)]` + `#[template(file = "../../data/ui/<name>.ui")]`, `#[template_child]` fields in `RefCell`, then a `glib::wrapper!`. Template paths are relative to `src/ui/`.

### Day-view grid sizing gotcha
The thumbnail grid uses per-section `GtkLabel` headers + `GtkFlowBox` of custom `SquareTile` widgets (headers are outside the grid because GridView/FlowBox can't span a full row). Critical GTK4 sizing pitfalls are recorded in the auto-memory `gtk4-gridview-thumbnail-sizing-pitfall.md` — read it before changing grid/tile sizing. Per-view tile targets live in `MediaGrid::spec_for_mode`. There is no working screenshot tool in this Wayland session; verify layout via `timeout_add_local_once` instrumentation + `eprintln!` in `measure`/`constructed`, or by reading the GTK C source.

## Conventions

- Per `CONTRIBUTING.md`: TDD (failing test first), then implement to green, then `cargo fmt` + `clippy`. Shared test fixtures live in `tests/common/mod.rs`.
- Design specs and milestone plans live under `docs/superpowers/{specs,plans}/` — consult these for milestone scope and prior design decisions.
