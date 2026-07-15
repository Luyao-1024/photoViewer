# UX Test Suite — Full-App-Shell Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the standalone Trash UX flow onto a full `MainWindow`+sidebar shell and add a shared full-shell fixture, so page-to-page navigation is exercised — and add a mechanism guard for the `selecting_programmatically` path.

**Architecture:** Add `build_full_app_shell()` (a `MainWindow` + sidebar + nav + `PhotosPage` root, with seeded media and a db_actor) and an `open_trash_via_sidebar()` helper to `tests/ux_click_flows.rs`. Rewrite `trash_page_clicks_selection_cancel_restore_and_delete` to run on the shell and assert the Trash page stays visible after restore/delete. Add one crate-internal mechanism guard test in `src/ui/window/navigation/tests.rs`.

**Tech Stack:** Rust, GTK4, libadwaita, the existing `tests/ux_click_flows.rs` helpers (`click_button`, `activate_virtual_grid_slot`, `wait_until`), `#[gtk::test]`.

## Global Constraints

- Tests run against the live local display (no local `xvfb`): `cargo test --all` directly, matching CI's intent. The `#[gtk::test]` cases need a display.
- Before pushing, run the exact CI set: `cargo fmt --all --check`; `cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments`; `cargo test --all`.
- Do not edit generated `.ui`; this plan only touches Rust test files.
- Each `adw::Application` in a test needs a **unique** application id (GApplication is single-instance per id per process). The fixture uses a monotonic counter.
- Keep the monolithic single `#[test]` runner (`ux_click_flow_suite_...`) — sub-flows share one gtk/tokio init.

## Scope

Covers spec stages 1 (fixture) and 2 (trash migration) plus the `selecting_programmatically` guard. Stages 3–5 (migrate the other 8 flows; retire the old fixture; update `docs/testing.md`) repeat the Task 2 pattern and will be planned after this lands and the fixture API is validated.

Reference spec: `docs/superpowers/specs/2026-07-16-ux-test-suite-design.md`.

---

### Task 1: `build_full_app_shell()` fixture + `open_trash_via_sidebar()` helper + smoke test

**Files:**
- Modify: `tests/ux_click_flows.rs` (add struct + two functions near `build_photos_page_with_nav`; add a smoke sub-flow; wire it into the suite runner).

**Interfaces:**
- Produces:
  - `struct AppShell { _app, _tmp, pool, loader, media_list, db_actor, items, window, photos }`
  - `fn build_full_app_shell() -> AppShell`
  - `fn open_trash_via_sidebar(window: &MainWindow) -> TrashPage`
- Consumes existing: `MainWindow::new`, `populate_sidebar`, `set_resources`, `set_db_actor`, `populate_album_rows`, `nav_view`, `show_photos_browsing_page`, `connect_sidebar`, `PhotosPage::new/set_nav_target/set_db_pool/set_db_actor`, `seed_media`, `window.imp().trash_list`, `wait_until`.

- [ ] **Step 1: Add the smoke sub-flow call to the suite runner**

In `ux_click_flow_suite_including_album_sidebar_multi_select_deletes_real_albums` (the single `#[test]`), add a call right after the `trash_page_clicks_selection_cancel_restore_and_delete();` line:

```rust
    trash_page_clicks_selection_cancel_restore_and_delete();
    full_app_shell_renders_photos_and_opens_trash_via_sidebar();
}
```

- [ ] **Step 2: Write the smoke sub-flow (failing — fixture not defined yet)**

Add near the other sub-flows (e.g., just before `trash_page_clicks_selection_cancel_restore_and_delete`):

```rust
fn full_app_shell_renders_photos_and_opens_trash_via_sidebar() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();
    assert!(
        nav.visible_page().is_some(),
        "full app shell should have a visible browsing root"
    );
    assert_eq!(
        shell.window.imp().trash_list.get().observe_children().n_items(),
        1,
        "sidebar should contain one Trash row"
    );
    let trash = open_trash_via_sidebar(&shell.window);
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "selecting the Trash sidebar row should show the TrashPage"
    );
    let _ = trash;
}
```

- [ ] **Step 3: Run the suite to verify it fails**

Run: `cargo test --test ux_click_flows 2>&1 | tail -20`
Expected: compile error — `build_full_app_shell` / `open_trash_via_sidebar` not defined.

- [ ] **Step 4: Add the `AppShell` struct and `build_full_app_shell()`**

Add next to `struct PhotosFixture` (top area of the file). Use a monotonic counter for a unique application id.

```rust
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

struct AppShell {
    _app: adw::Application,
    _tmp: tempfile::TempDir,
    pool: db::DbPool,
    loader: Arc<ThumbnailLoader>,
    media_list: gtk::gio::ListStore,
    db_actor: photo_viewer::core::db_actor::DbActorHandle,
    items: Vec<MediaItem>,
    window: MainWindow,
    photos: PhotosPage,
}

/// Monotonic counter so every full-shell fixture registers a unique
/// `adw::Application` id (GApplication is single-instance per id per process).
static FULL_SHELL_SEQ: AtomicU64 = AtomicU64::new(0);

fn build_full_app_shell() -> AppShell {
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("shell.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let items = seed_media(&pool, tmp.path());
    albums::refresh(&pool).unwrap();

    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in &items {
        media_list.append(&glib::BoxedAnyObject::new(item.clone()));
    }
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(pool.clone(), event_sender);

    let seq = FULL_SHELL_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
    let app = adw::Application::builder()
        .application_id(format!(
            "io.github.luyao_1024.photoviewer.UxFullShell{seq}"
        ))
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();

    let window = MainWindow::new(&app);
    window.populate_sidebar();
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());
    window.set_db_actor(db_actor.clone());
    window.populate_album_rows();

    let nav = window.nav_view();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    photos.set_db_actor(db_actor.clone());
    window.show_photos_browsing_page(&photos);
    window.connect_sidebar(&nav);

    AppShell {
        _app: app,
        _tmp: tmp,
        pool,
        loader,
        media_list,
        db_actor,
        items,
        window,
        photos,
    }
}

/// Open the Trash page the way a user does: select the Trash sidebar row.
/// Returns the `TrashPage` that `show_trash_page` built from the window's
/// real pool/loader/media_list/db_actor.
fn open_trash_via_sidebar(window: &MainWindow) -> TrashPage {
    let trash_list = window.imp().trash_list.get();
    let trash_row = trash_list.row_at_index(0).expect("Trash row exists");
    trash_list.select_row(Some(&trash_row));
    let nav = window.nav_view();
    assert!(
        wait_until(Duration::from_secs(2), || {
            nav.visible_page().and_downcast::<TrashPage>().is_some()
        }),
        "selecting the Trash sidebar row should show TrashPage"
    );
    nav.visible_page()
        .and_downcast::<TrashPage>()
        .expect("TrashPage is visible")
}
```

- [ ] **Step 5: Run the suite to verify it passes**

Run: `cargo test --test ux_click_flows 2>&1 | tail -20`
Expected: PASS (`test ux_click_flow_suite_... ... ok`).

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -5`
Expected: no diff, no warnings. (If fmt wants changes, run `cargo fmt --all` and re-check.)

- [ ] **Step 7: Commit**

```bash
git add tests/ux_click_flows.rs
git commit -m "test(ux): add full-app-shell fixture for UX flows

build_full_app_shell() wires a MainWindow + sidebar + nav + PhotosPage root
with seeded media and a db_actor, so UX flows can run against the real
navigation shell instead of a standalone PhotosPage. Adds an
open_trash_via_sidebar() helper and a smoke sub-flow."
```

---

### Task 2: Migrate the Trash flow to the full shell + "page stays" assertions

**Files:**
- Modify: `tests/ux_click_flows.rs` — rewrite `trash_page_clicks_selection_cancel_restore_and_delete`.

**Interfaces:**
- Consumes: `build_full_app_shell`, `open_trash_via_sidebar` (Task 1), `click_button`, `activate_virtual_grid_slot`, `wait_until`, `db::list_trashed_media`.

**Why this task is the highest value:** this is the flow class that the original bug (commit `8053a4b`) escaped through. After migration it opens Trash via the sidebar (so the navigation stack and sidebar state are real) and asserts the Trash page stays visible after restore and after delete.

- [ ] **Step 1: Rewrite the trash sub-flow on the full shell**

Replace the entire body of `trash_page_clicks_selection_cancel_restore_and_delete` with:

```rust
fn trash_page_clicks_selection_cancel_restore_and_delete() {
    let shell = build_full_app_shell();
    // Move both seeded files into the trash backend so the restore/delete
    // buttons perform real operations and the grid genuinely reloads.
    let uri0 = shell.items[0].uri.clone();
    let uri1 = shell.items[1].uri.clone();
    photo_viewer::core::trash::move_to_trash(&uri0).unwrap();
    common::db::mark_trashed(&shell.pool, shell.items[0].id).unwrap();
    photo_viewer::core::trash::move_to_trash(&uri1).unwrap();
    common::db::mark_trashed(&shell.pool, shell.items[1].id).unwrap();

    let nav = shell.window.nav_view();
    let trash = open_trash_via_sidebar(&shell.window);
    let grid = trash.imp().grid.borrow().as_ref().cloned().unwrap();
    assert!(
        wait_until(Duration::from_secs(2), || grid.logical_media_count() == 2),
        "TrashPage should render the two trashed items"
    );

    // Select one tile, then Cancel: action bar hides, page stays.
    assert!(
        wait_until(Duration::from_secs(2), || grid.first_ready_media_slot().is_some()),
        "Trash should load an interactive media slot"
    );
    let first_slot = grid.first_ready_media_slot().expect("ready media slot");
    activate_virtual_grid_slot(&grid, first_slot);
    assert!(
        trash.imp().action_bar.get().is_revealed(),
        "selecting a Trash tile should reveal the action bar"
    );
    click_button(&trash.imp().cancel_btn.get());
    assert!(
        !trash.imp().action_bar.get().is_revealed(),
        "clicking Trash cancel should clear selection and hide actions"
    );

    // Restore one item: selection clears, grid reloads, AND the page must stay.
    activate_virtual_grid_slot(&grid, first_slot);
    click_button(&trash.imp().restore_btn.get());
    assert!(
        wait_until(Duration::from_secs(2), || {
            !trash.imp().action_bar.get().is_revealed()
        }),
        "clicking Restore should clear the current Trash selection"
    );
    assert!(
        wait_until(Duration::from_secs(2), || grid.logical_media_count() == 1),
        "TrashPage should reload to the one remaining item after Restore"
    );
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "TrashPage must stay visible after restoring one item"
    );

    // Delete the remaining item permanently: row leaves the DB, page stays.
    assert!(
        wait_until(Duration::from_secs(2), || grid.first_ready_media_slot().is_some()),
        "Trash should load the remaining media slot"
    );
    let remaining_slot = grid.first_ready_media_slot().expect("ready media slot");
    activate_virtual_grid_slot(&grid, remaining_slot);
    click_button(&trash.imp().delete_btn.get());
    assert!(
        wait_until(Duration::from_secs(2), || {
            db::list_trashed_media(&shell.pool)
                .map(|items| items.is_empty())
                .unwrap_or(false)
        }),
        "clicking Delete Permanently should empty the trashed rows"
    );
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "TrashPage must stay visible after deleting the last item"
    );

    // Clean up the host trash so the test leaves nothing behind.
    let _ = photo_viewer::core::trash::delete_permanently(&uri0);
    let _ = photo_viewer::core::trash::delete_permanently(&uri1);
}
```

- [ ] **Step 2: Run the suite**

Run: `cargo test --test ux_click_flows 2>&1 | tail -25`
Expected: PASS. If `move_to_trash` fails with a gio "not supported on internal mount" error (the test tempdir is on tmpfs), the tempdir is on a filesystem gio cannot trash to — in that case, switch the two `move_to_trash` calls to a real-scratch path (see Troubleshooting below) and re-run.

- [ ] **Step 3: fmt + clippy + full suite**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -5 && cargo test --all 2>&1 | grep -E "test result:|FAILED|panicked" | tail -20`
Expected: clean fmt, no warnings, all suites `ok` with 0 failed.

- [ ] **Step 4: Commit**

```bash
git add tests/ux_click_flows.rs
git commit -m "test(ux): run the Trash flow on the full app shell

Open Trash via the sidebar (real navigation stack) instead of a standalone
TrashPage, and assert the Trash page stays visible after restore and after
delete-permanently — the user-facing invariant behind the 'restore exits the
page' regression (8053a4b)."
```

**Troubleshooting (gio trash on tmpfs):** if `move_to_trash` errors, create the two seeded files under a real mount instead. Replace the `move_to_trash`/`mark_trashed` block with writing the files to `$HOME`-rooted paths and inserting those. Prefer keeping `seed_media` as-is and only relocating the two files if the error actually occurs; do not pre-emptively complicate the fixture.

---

### Task 3: Mechanism guard — programmatic sidebar selection does not navigate

**Files:**
- Modify: `src/ui/window/navigation/tests.rs` — add one `#[gtk::test]`.

**Interfaces:**
- Consumes: `MainWindow`, `PhotosPage`, `TrashPage`, `keyboard_media_list`, `keyboard_thumbnail_loader`, the `selecting_programmatically` imp field.

**Why:** the `selecting_programmatically` guard stops the sidebar `row-selected` handlers from re-entering navigation when `sync_sidebar_selection_for_browsing_page` / `show_photos_browsing_page` re-select rows during a refresh. This is the sibling guard to the focus-fallback one and has no UX coverage.

- [ ] **Step 1: Write the failing test**

Append to `src/ui/window/navigation/tests.rs`:

```rust
#[gtk::test]
fn programmatic_sidebar_selection_does_not_navigate() {
    // `selecting_programmatically` is set while the window re-selects sidebar
    // rows during a refresh; those selections must not navigate. (Sibling guard
    // to focus_driven_sidebar_selection_does_not_pop_pushed_page.)
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.ProgrammaticSidebarNoNav")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let media_list = keyboard_media_list();
    let (_tmp, loader) = keyboard_thumbnail_loader();
    let pool = crate::core::db::init_pool(&_tmp.path().join("programmatic-sidebar.db")).unwrap();
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());
    window.populate_sidebar();
    let root = PhotosPage::new(media_list.clone(), loader);
    root.set_nav_target(&nav);
    window.show_photos_browsing_page(&root);
    window.connect_sidebar(&nav);

    let trash = TrashPage::with_media_list(pool, keyboard_thumbnail_loader().1, media_list);
    nav.push(&trash);
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "TrashPage should be pushed on top of the browsing root"
    );

    // A programmatic (refresh-driven) Photos selection must NOT pop the Trash page.
    let sidebar = window.imp().sidebar_list.get();
    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    window.imp().selecting_programmatically.set(true);
    sidebar.unselect_all();
    sidebar.select_row(Some(&photos_row));
    while glib::MainContext::default().iteration(false) {}
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "a programmatic Photos row selection must not pop the Trash page"
    );

    // Once the programmatic guard is cleared, the selection is ambient again.
    window.imp().selecting_programmatically.set(false);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --lib programmatic_sidebar_selection_does_not_navigate 2>&1 | tail -15`
Expected: PASS (the guard already exists in production code; this test locks it in).

- [ ] **Step 3: fmt + clippy + full suite**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -5 && cargo test --all 2>&1 | grep -E "test result:|FAILED|panicked" | tail -20`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/ui/window/navigation/tests.rs
git commit -m "test(window): lock the programmatic sidebar-selection guard

A sidebar row re-selected during a refresh (selecting_programmatically) must
not navigate. Regression guard sibling to the focus-fallback one."
```

---

## Follow-on (spec stages 3–5) — not in this plan

After Tasks 1–3 land and `build_full_app_shell()` is validated, the remaining migrations repeat the Task 2 pattern (swap `build_photos_page_with_nav()` → `build_full_app_shell()`; operate on `shell.window.nav_view()` and the visible PhotosPage; open albums via sidebar). They will be planned as a follow-up:

- Stage 3 (low change): mode selector, thumbnail→viewer, search→viewer, viewer chrome, batch toolbar.
- Stage 4 (medium change): album pages→viewer, album picker copy/move, album browser reorder.
- Stage 5 (cleanup): retire `build_photos_page_with_nav()`; update `docs/testing.md` to state UX flows run on the full app shell.

## Self-Review (completed)

- **Spec coverage:** Stage 1 (fixture) → Task 1. Stage 2 (trash migration + page-stays) → Task 2. Mechanism guards → Task 3 + the already-landed `focus_driven_sidebar_selection_does_not_pop_pushed_page` (which already includes the genuine-click counter-assertion, so no separate `genuine_click_navigates_*` test is needed). Stages 3–5 are explicitly deferred to a follow-up plan.
- **Placeholders:** none — every code step shows the full code; the only conditional is the tmpfs troubleshooting branch, which gives a concrete remedy.
- **Type consistency:** `AppShell` field names (`window`, `photos`, `pool`, `items`, `db_actor`) match what Task 2 consumes; `open_trash_via_sidebar(&MainWindow) -> TrashPage` matches its call sites.
