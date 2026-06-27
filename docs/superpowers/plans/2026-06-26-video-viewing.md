# Video Viewing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add video discovery and playback to the mixed media library.

**Architecture:** Reuse `media_items.mime_type` as the discriminator and keep the DB schema unchanged. Add shared media helpers for image/video extension and MIME checks, scan picture and video roots, and switch the viewer between `GtkPicture` and `GtkVideo` based on the current item.

**Tech Stack:** Rust, GTK4/libadwaita, SQLite/rusqlite, notify, gdk-pixbuf, GTK media widgets.

---

### Task 1: Media Type And Roots

**Files:**
- Modify: `src/core/media.rs`
- Modify: `src/core/metadata.rs`
- Modify: `src/core/backend/local.rs`
- Modify: `src/core/notify_watcher.rs`
- Modify: `src/config.rs`
- Test: `tests/metadata_extract.rs`
- Test: `tests/local_scan.rs`
- Test: `tests/locale_pictures_dir.rs`

- [ ] Add `MediaKind`, `MediaItem::is_video`, `is_supported_media_path`, and `mime_from_extension`.
- [ ] Add config helpers returning picture and video roots without duplicates.
- [ ] Update scanner/watcher filtering to use supported media paths.
- [ ] Verify tests fail before implementation, then pass.

### Task 2: App Startup And Trash Scope

**Files:**
- Modify: `src/app.rs`
- Modify: `src/core/bootstrap.rs`
- Modify: `docs/modules/storage.md`

- [ ] Scan all media roots at startup.
- [ ] Watch all media roots plus trash roots.
- [ ] Keep trash reconciliation scoped to the picture root for the existing trash flow.
- [ ] Update storage docs for mixed media roots.

### Task 3: Viewer Video Mode

**Files:**
- Modify: `data/ui/viewer-page.blp`
- Modify: `src/ui/viewer_page.rs`
- Test: `tests/ui_viewer_toolbar.rs`

- [ ] Add `Gtk.Video`, `Gtk.Scale`, and media stream state to the viewer template/code.
- [ ] In `show_at`, route videos to `GtkMediaFile` playback and images to existing decode.
- [ ] Hide/disable the edit button for videos.
- [ ] Keep filmstrip navigation unchanged.

### Task 4: Verification

**Files:**
- Modify: `docs/modules/viewer.md`
- Modify: `docs/modules/browsing.md`
- Modify: `docs/modules/storage.md`

- [ ] Run focused tests for metadata, scanning, config, watcher, and viewer template.
- [ ] Run `cargo build`.
- [ ] Run `cargo test` if the focused suite is clean and time permits.
