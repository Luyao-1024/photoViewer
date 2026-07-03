# Media Attribute Albums Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add non-empty media attribute virtual albums for motion photos, animated images, and HDR media.

**Architecture:** Store general attributes as top-level booleans in `media_attributes`, keep motion photos on `media_subkind`, and route all virtual media type albums through repository-backed DB queries. Sidebar visibility follows the returned album list.

**Tech Stack:** Rust, GTK/libadwaita, rusqlite, serde_json, cargo tests.

---

### Task 1: Media Attribute Model

**Files:**
- Modify: `src/core/media.rs`
- Test: `src/core/media.rs`

- [ ] Add constants for animated/HDR attributes and helpers on `MediaItem`.
- [ ] Add focused tests for `is_animated` and `is_hdr` reading top-level JSON booleans.

### Task 2: DB And Repository Attribute Queries

**Files:**
- Modify: `src/core/db.rs`
- Modify: `src/core/repository.rs`
- Test: `tests/repository.rs`

- [ ] Add count/list/neighbor DB helpers using `json_extract(media_attributes, '$.<attribute>') = 1`.
- [ ] Add `MediaQuery::Attribute(String)` and route count/page/items/neighbor through the new DB helpers.
- [ ] Add tests proving attribute queries return only matching live media and neighbor navigation stays inside the attribute set.

### Task 3: Sidebar Virtual Albums

**Files:**
- Modify: `src/core/albums.rs`
- Modify: `src/ui/album_detail_page.rs`
- Modify: `src/ui/window.rs`
- Test: `tests/albums.rs`
- Test: `tests/sidebar_navigation.rs`

- [ ] Add animated/HDR virtual album paths, display names, and album predicates.
- [ ] Make `list_media_type_albums` return only albums with `photo_count > 0`.
- [ ] Hide the media type header and scroll region when the list is empty.
- [ ] Route animated/HDR albums to `MediaQuery::Attribute`.

### Task 4: Scanning Support

**Files:**
- Modify: `src/core/media.rs`
- Modify: `src/core/backend/local.rs`
- Test: `tests/local_scan.rs`

- [ ] Add GIF MIME support.
- [ ] Persist `animated: true` for GIF scan results while preserving motion photo attributes.

### Task 5: Docs And Verification

**Files:**
- Modify: `docs/modules/storage.md`
- Modify: `docs/modules/browsing.md`

- [ ] Document media attribute JSON flags and media type sidebar empty-state behavior.
- [ ] Run focused tests, then `cargo test` if focused tests pass.
