# Album Crossfade Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Make Photos ↔ album detail and album ↔ album browsing transitions use the same `Gtk.Stack` crossfade as Year/Month/Day.

**Architecture:** Keep one outer `Adw.NavigationView` root page for viewer/search/trash navigation. Put `PhotosPage` and the currently active `AlbumDetailPage` inside an inner `Gtk.Stack` owned by that root page, configured with `transition-type: crossfade` and duration `200`. Sidebar selection swaps the inner stack child; viewer pages continue to push onto the outer navigation view.

**Tech Stack:** Rust, GTK4, libadwaita, Blueprint, existing GTK integration tests.

## Global Constraints

- Edit `data/ui/*.blp` templates, not generated `.ui` output.
- Keep `src/core/` independent from GTK UI concerns.
- Preserve existing viewer/search/trash navigation and keyboard behavior.
- Reuse the Photos mode selector transition model: one `Gtk.Stack`, `crossfade`, `200ms`.
- Update UI naming documentation when adding or renaming template widgets.

---

### Task 1: Add the browsing stack to the window shell

**Files:**
- Modify: `data/ui/window.blp`
- Modify: `src/ui/window.rs`
- Test: `src/ui/window/tests.rs` or a focused source-structure test under `tests/`

**Interfaces:**
- Produces `MainWindow::browsing_stack()`, `MainWindow::show_photos_browsing_page(PhotosPage)`, and `MainWindow::show_album_browsing_page(AlbumDetailPage)` for navigation code.
- The outer `nav_view` remains the target passed to viewer/search pages.

- [ ] **Step 1: Write the failing structural test**

Assert that the window template contains a `Gtk.Stack browsing_stack` with
`transition-type: crossfade` and `transition-duration: 200`, and that the
generated `MainWindow` exposes the template child. Use the repository's
existing source-structure test style if a GTK display is not available.

- [ ] **Step 2: Run the focused test and verify it fails**

Run `cargo test --test ui_window_source_structure -- --nocapture`.
Expected: FAIL because `browsing_stack` is not present.

- [ ] **Step 3: Add the root browsing page and stack in Blueprint**

Change the `content` child in `data/ui/window.blp` from a bare
`Adw.NavigationView` to an `Adw.NavigationView` containing a root
`Adw.NavigationPage browsing_root_page`. Give that page a child
`Gtk.Stack browsing_stack` with `transition-type: crossfade`,
`transition-duration: 200`, and a stable Photos child placeholder that can be
replaced by Rust after resources load.

- [ ] **Step 4: Add the window-side stack API**

Add a `TemplateChild<gtk::Stack>` and a retained `RefCell<Option<PhotosPage>>`
or equivalent root-page ownership in `MainWindow`. Implement helpers that
remove the current dynamic album child, add the requested browsing widget with
a stable name, and call `set_visible_child`. Ensure replacing an album does
not leave the old page parented or visible.

- [ ] **Step 5: Run the focused test and verify it passes**

Run `cargo test --test ui_window_source_structure -- --nocapture`.
Expected: PASS.

- [ ] **Step 6: Commit the shell change**

Run `git add data/ui/window.blp src/ui/window.rs tests src/ui/window/tests.rs && git commit -m "feat: add crossfade browsing stack"`.

### Task 2: Initialize Photos inside the browsing stack

**Files:**
- Modify: `src/app.rs`
- Modify: `src/ui/window.rs`
- Modify: `src/ui/photos_page.rs` only if its host-page assumptions require a narrow adjustment
- Test: `src/ui/window/tests.rs` or `tests/ui_window_source_structure.rs`

**Interfaces:**
- Consumes the stack API from Task 1.
- Produces a root browsing state with Photos visible and outer navigation stack
  containing only the root page before any viewer/search/trash push.

- [ ] **Step 1: Write the failing regression test**

Build a test window with a Photos page, install it through the new browsing
API, and assert the outer `NavigationView` has one root page while the inner
stack's visible child is Photos. Assert the Photos page is not pushed directly
onto the outer navigation view.

- [ ] **Step 2: Run it and verify it fails**

Run the focused window UI test. Expected: FAIL because app initialization still
calls `nav.push(&photos)`.

- [ ] **Step 3: Replace the startup push**

In `src/app.rs`, configure Photos with the outer navigation view as its viewer
target, then call the new `MainWindow` browsing initializer instead of pushing
Photos directly. Ensure the root browsing page is pushed exactly once.

- [ ] **Step 4: Update keyboard/page lookup for the inner browsing child**

Keep viewer handling based on `nav_view.visible_page()`. When the outer visible
page is the browsing root, resolve the inner stack child as Photos or Album
Detail for browsing scope and keyboard dispatch. Back from the browsing root
must remain a no-op; Back from viewer/search/trash continues to pop outer nav.

- [ ] **Step 5: Run focused tests**

Run `cargo test --test ui_window_source_structure -- --nocapture` and the
window navigation tests. Expected: PASS with no direct Photos push regression.

- [ ] **Step 6: Commit startup integration**

Run `git add src/app.rs src/ui/window.rs src/ui/photos_page.rs tests && git commit -m "feat: host Photos in browsing stack"`.

### Task 3: Switch album opening from outer push to inner crossfade

**Files:**
- Modify: `src/ui/window/navigation.rs`
- Modify: `src/ui/window.rs` if helper visibility or active-page lookup is needed
- Modify: `src/ui/album_detail_page.rs` only for host visibility checks
- Test: `src/ui/window/navigation/tests.rs` and/or `tests/sidebar_navigation.rs`

**Interfaces:**
- Consumes `show_album_browsing_page` and `show_photos_browsing_page`.
- Keeps `AlbumDetailPage::set_nav_target(&outer_nav)` unchanged for viewer and
  search pushes.

- [ ] **Step 1: Write the failing album-switch test**

Exercise album selection with two album pages and assert the outer navigation
stack count stays at the browsing root count while the inner visible child
changes from Photos to Album A and then Album B. Assert the active page identity
changes and no second outer album page is pushed.

- [ ] **Step 2: Run it and verify it fails**

Run the focused navigation test. Expected: FAIL because `open_album` calls
`pop_to_photos_root` followed by `nav_view.push(&page)`.

- [ ] **Step 3: Replace the push path**

In `open_album`, retain repository paging, page construction, DB actor setup,
and backfill exactly as-is. Replace only the outer `pop_to_photos_root` /
`nav_view.push` pair with the MainWindow helper that installs the detail page
in the inner stack and makes it visible. Keep `page.set_nav_target(nav_view)`.

- [ ] **Step 4: Make Photos/sidebar return select the Photos child**

Change browsing-root return paths so selecting Photos removes/replaces the
active album child and selects the Photos child. Do not pop the outer root page.
When viewer/search/trash is visible, first pop those outer pages, then restore
the Photos browsing child as appropriate.

- [ ] **Step 5: Preserve album detail viewer activation**

Update `AlbumDetailPage`'s visible check to recognize that its page is the
inner stack's visible child rather than requiring it to equal
`nav.visible_page()`. Keep its viewer push and duplicate-open guard intact.

- [ ] **Step 6: Run focused tests**

Run `cargo test --test sidebar_navigation -- --nocapture` and the relevant
window navigation tests. Expected: PASS; outer stack count remains stable
across album switches.

- [ ] **Step 7: Commit album switching**

Run `git add src/ui/window/navigation.rs src/ui/window.rs src/ui/album_detail_page.rs tests && git commit -m "feat: crossfade between album browsing pages"`.

### Task 4: Update documentation and verify the complete behavior

**Files:**
- Modify: `docs/modules/albums-trash.md`
- Modify: `docs/modules/browsing.md`
- Modify: `docs/ui-naming-reference/index.html` if the new template IDs are not already represented
- Test: focused UI tests plus project checks

- [ ] **Step 1: Document the navigation contract**

State that Photos and the active AlbumDetailPage are peer children of the
window browsing stack, that it uses `crossfade`/`200ms`, and that outer
NavigationView remains for viewer/search/trash.

- [ ] **Step 2: Run formatting and focused tests**

Run `cargo fmt --check`, `cargo test --test ui_window_source_structure`,
`cargo test --test sidebar_navigation`, and the relevant album navigation/UI
tests. Expected: PASS.

- [ ] **Step 3: Run broader verification**

Run `cargo test`. If GTK display constraints prevent a UI test, report the
exact failing test and warning while confirming all non-display checks.

- [ ] **Step 4: Inspect the diff and commit documentation**

Run `git diff HEAD~4..HEAD --check` and inspect `git status --short`. Commit
remaining docs/test updates with `git commit -m "docs: document browsing crossfade navigation"`.
