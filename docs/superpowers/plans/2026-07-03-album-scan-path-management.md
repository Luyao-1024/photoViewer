# Album Scan Path Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add album scan path management so users can add custom scan folders and exclude existing album folders from future scanning without deleting files.

**Architecture:** Persist scan include/exclude folders in `settings.json`, merge them with the existing default media roots in `config::media_roots()`, and make scanner traversal prune excluded directories. The UI exposes the preference from the existing Settings dialog and asks for restart after path changes because startup scan and watcher roots are created during initialization.

**Tech Stack:** Rust, GTK4/libadwaita, serde_json settings, walkdir scanning, existing cargo integration tests.

---

### Task 1: Persist Scan Path Preferences

**Files:**
- Modify: `src/core/prefs.rs`

- [ ] **Step 1: Write failing unit tests**

Add tests in `src/core/prefs.rs` for `read_custom_scan_roots_at`, `write_custom_scan_roots_at`, `read_excluded_scan_roots_at`, and `write_excluded_scan_roots_at`. Assert defaults are empty, relative paths are ignored, duplicates collapse, and unrelated keys are preserved.

- [ ] **Step 2: Run RED**

Run: `cargo test core::prefs::tests::scan_path_preferences --lib`

Expected: compile failure because the scan path helpers do not exist.

- [ ] **Step 3: Implement minimal persistence**

Add `custom_scan_roots` and `excluded_scan_roots` JSON array keys, path-list read/write helpers, and public getters/setters.

- [ ] **Step 4: Run GREEN**

Run: `cargo test core::prefs::tests::scan_path_preferences --lib`

Expected: PASS.

### Task 2: Merge And Filter Media Roots

**Files:**
- Modify: `src/config.rs`
- Modify: `tests/locale_pictures_dir.rs`

- [ ] **Step 1: Write failing integration tests**

Add tests proving custom roots append after defaults, duplicates collapse, and excluded roots remove exact child roots from `media_roots_from_parts`.

- [ ] **Step 2: Run RED**

Run: `cargo test --test locale_pictures_dir media_roots_include_custom_and_filter_excluded -- --test-threads=1`

Expected: compile failure because `media_roots_from_parts` does not exist.

- [ ] **Step 3: Implement root resolver**

Add `media_roots_from_parts(defaults, custom, excluded)` and update `media_roots()` to call it with `prefs::custom_scan_roots()` and `prefs::excluded_scan_roots()`.

- [ ] **Step 4: Run GREEN**

Run: `cargo test --test locale_pictures_dir -- --test-threads=1`

Expected: PASS.

### Task 3: Prune Excluded Directories During Scans

**Files:**
- Modify: `src/core/backend/local.rs`
- Modify: `tests/local_scan.rs`

- [ ] **Step 1: Write failing scanner test**

Add a test that scans a root containing `keep/` and `skip/`, passes `skip/` as excluded, and asserts only `keep` media are returned.

- [ ] **Step 2: Run RED**

Run: `cargo test --test local_scan scan_dir_with_exclusions_prunes_excluded_directories`

Expected: compile failure because the excluded scan method does not exist.

- [ ] **Step 3: Implement scanner exclusion APIs**

Add `scan_dir_with_exclusions`, `scan_and_upsert_dir_with_exclusions`, and `scan_and_upsert_dir_notify_with_exclusions`, pruning `WalkDir` entries below excluded folders.

- [ ] **Step 4: Wire startup scans**

Update `core::bootstrap` to pass `prefs::excluded_scan_roots()` into startup scans.

- [ ] **Step 5: Run GREEN**

Run: `cargo test --test local_scan scan_dir_with_exclusions_prunes_excluded_directories`

Expected: PASS.

### Task 4: Settings UI

**Files:**
- Modify: `src/ui/window.rs`
- Modify: `src/core/i18n.rs` or translation resource used by current `tr()` keys
- Modify: `docs/modules/storage.md`
- Modify: `docs/modules/albums-trash.md`

- [ ] **Step 1: Add path management rows**

In `build_settings_page`, add an Album Management group with rows listing custom scan roots and excluded scan roots, folder picker buttons, and per-path remove buttons.

- [ ] **Step 2: Persist changes**

Connect add/remove actions to `prefs::set_custom_scan_roots` and `prefs::set_excluded_scan_roots`, then show the existing restart-required dialog.

- [ ] **Step 3: Update docs**

Document that scan path preferences affect future scan/watch behavior only and never delete files.

- [ ] **Step 4: Verify**

Run: `cargo test --test locale_pictures_dir -- --test-threads=1`, `cargo test --test local_scan`, and `cargo test --lib core::prefs`.

Expected: PASS.
