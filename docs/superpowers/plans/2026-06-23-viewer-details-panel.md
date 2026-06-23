# Viewer Details Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a right-side viewer details panel with file metadata, shooting metadata, and always-visible image names.

**Architecture:** `core::metadata` extracts raw EXIF fields in addition to existing dimensions/date/MIME. `ViewerPage` keeps the current image name in the navigation title and uses a `Gtk.Revealer` overlay as a right-side details panel.

**Tech Stack:** Rust 2021, GTK4, Libadwaita, Blueprint, kamadak-exif, cargo integration tests.

---

### Task 1: Metadata EXIF Fields

**Files:**
- Modify: `src/core/metadata.rs`
- Modify: `tests/metadata_extract.rs`

- [ ] Add tests proving plain images have no EXIF fields and EXIF JPEGs expose `DateTimeOriginal`.
- [ ] Run `cargo test --test metadata_extract` and verify the new test fails before implementation.
- [ ] Add `ExifField { tag, value }` and populate `RawMetadata::exif_fields` from `kamadak-exif`.
- [ ] Re-run `cargo test --test metadata_extract` and verify it passes.

### Task 2: Viewer Details UI

**Files:**
- Modify: `data/ui/viewer-page.blp`
- Modify: `src/ui/viewer_page.rs`

- [ ] Add an info toggle button and right-aligned `Gtk.Revealer` details panel to the Blueprint.
- [ ] Add template children for the button, revealer, rows, and dynamic EXIF group.
- [ ] On `show_at`, set the page title to `MediaItem::display_name()`.
- [ ] Toggle the details panel from the info button.
- [ ] Populate file rows from `MediaItem` and EXIF rows from `metadata::extract`.

### Task 3: Verification

**Files:**
- Verify only; no new source files.

- [ ] Run `cargo fmt`.
- [ ] Run `cargo test --test metadata_extract`.
- [ ] Run `cargo test --test e2e_viewer`.
- [ ] Run `cargo build` to validate Blueprint compilation.
