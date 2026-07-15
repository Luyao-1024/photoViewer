# UX Test Suite — Stages 3–5 (Full-Shell Migration + Dead-Code Removal) Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the remaining standalone UX flows onto the full app shell (`build_full_app_shell`), delete the orphaned `AlbumBrowserPage` dead code (and its two widget-only test flows), and retire the old `build_photos_page_with_nav` fixture so every UX flow runs against the real `MainWindow`+sidebar navigation.

**Architecture:** Every flow swaps `build_photos_page_with_nav()` → `build_full_app_shell()` and operates on `shell.window.nav_view()` / `shell.photos`. Low-change flows are mechanical handle swaps. Medium-change flows open pages the way a user does: albums via the sidebar row (real `open_album` path), the album picker via the Photos batch "Add to Album" button. The album-browser flows are **deleted** because `AlbumBrowserPage` is unreachable from production UI (orphaned); the valuable `AlbumDetailPage → viewer` half of the album flow is preserved by reworking it onto the real sidebar-open path. Stage 5 then deletes the now-unused fixture and updates docs.

**Tech Stack:** Rust, GTK4, libadwaita, `tests/ux_click_flows.rs` helpers (`click_button`, `activate_virtual_grid_slot`, `wait_until`, `find_descendant`, `visible_photos_grid`), `#[gtk::test]`/`#[test]`.

## Global Constraints

- Tests run against the live local display (no local `xvfb`): `cargo test --all` directly. `#[gtk::test]` and the UX `#[test]` need a display; the local Wayland session provides it. (CI still wraps with `xvfb-run`.)
- Before pushing to `main`, run the exact CI gate: `cargo fmt --all --check`; `cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments`; `cargo test --all`.
- Edit `data/ui/*.blp` templates (and the gresource manifest / `build.rs` list), never generated `.ui`.
- Each `adw::Application` in a test needs a **unique** application id (GApplication is single-instance per id per process). `build_full_app_shell` already uses the monotonic `FULL_SHELL_SEQ` counter — reuse it, do not introduce fixed ids.
- Keep the monolithic single `#[test]` runner (`ux_click_flow_suite_...`) — sub-flows share one gtk/tokio init.
- Nav-stack depth baseline: after `build_full_app_shell`, `shell.window.nav_view().navigation_stack().n_items() == 1` (the browsing root page holds the photos/album Stack; viewer/search/trash/picker push on top). Every depth assertion in this plan is computed against that baseline of 1.

## Scope (spec stages 3–5)

- **Stage 3 (low change):** migrate 5 flows — mode selector, thumbnail→viewer, search→viewer, viewer chrome, batch toolbar.
- **Stage 4 (medium change):** delete orphaned `AlbumBrowserPage`; delete `album_browser_reorder_...`; rework `album_pages_...` → open album via sidebar; migrate album picker to open via the batch button; migrate `sidebar_clicks_...` and `album_sidebar_multi_select_...` to the shared shell.
- **Stage 5 (cleanup):** delete `build_photos_page_with_nav` + `PhotosFixture`; decouple `seed_extra_album` from `PhotosFixture`; prune unused imports; update `docs/testing.md`.

Reference spec: `docs/superpowers/specs/2026-07-16-ux-test-suite-design.md`. Reference prior plan: `docs/superpowers/plans/2026-07-16-ux-test-suite.md` (stages 1–2 + guard, landed).

**Dead-code decision (approved):** `AlbumBrowserPage` is constructed only in `tests/ux_click_flows.rs` — no production code builds or opens it. Per the user, delete it now rather than carry coverage for an unreachable page.

---

### Task 1: Stage 3 — migrate the 5 low-change flows to the full shell

**Files:**
- Modify: `tests/ux_click_flows.rs` — rewrite the bodies of `mode_selector_click_switches_photos_view`, `thumbnail_activation_opens_one_viewer`, `search_result_activation_opens_one_viewer_while_pending`, `viewer_chrome_clicks_drive_visible_operations`, `photos_batch_toolbar_clicks_select_favorite_and_album`.

**Interfaces:**
- Consumes: `build_full_app_shell` (returns `AppShell { window, photos, pool, loader, media_list, db_actor, items, _tmp, _app }`), `visible_photos_grid`, `activate_virtual_grid_slot`, `click_button`, `wait_until`, `find_descendant`, `MediaId`, `SearchPage`, `ViewerPage`.
- Produces: nothing new — these keep their function names and call sites in the suite runner.

**Why:** the 5 standalone flows still run on a bare `PhotosPage` pushed onto a bare nav, so the `MainWindow`/sidebar/navigation-shell interactions are never exercised. Swapping to the shell makes the nav stack and sidebar state real.

- [ ] **Step 1: Rewrite the 5 flow bodies (TDD — run after each batch)**

The change for every flow is the same substitution: `build_photos_page_with_nav()` → `build_full_app_shell()`; `fixture.page` → `shell.photos`; `fixture.nav` → a local `let nav = shell.window.nav_view();`; `fixture.pool/loader/media_list/items/_tmp` → `shell.pool/loader/media_list/items/_tmp`. Replace each function body with the exact code below.

`mode_selector_click_switches_photos_view`:
```rust
fn mode_selector_click_switches_photos_view() {
    let shell = build_full_app_shell();
    let selector = find_descendant::<ModeSelector>(shell.photos.upcast_ref())
        .expect("PhotosPage should contain a ModeSelector");
    let stack = find_descendant::<gtk::Stack>(shell.photos.upcast_ref())
        .expect("PhotosPage should contain a GtkStack");

    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("day"),
        "PhotosPage starts on Day when media exists"
    );

    click_mode_selector_cell(&selector, 0);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("year"),
        "clicking Year switches the bound Photos view"
    );

    click_mode_selector_cell(&selector, 1);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("month"),
        "clicking Month switches the bound Photos view"
    );

    click_mode_selector_cell(&selector, 2);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("day"),
        "clicking Day returns to the dense photo grid"
    );
}
```

`thumbnail_activation_opens_one_viewer`:
```rust
fn thumbnail_activation_opens_one_viewer() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();
    let grid = visible_photos_grid(&shell.photos);

    let first_slot = grid
        .first_media_slot()
        .expect("Photos should expose a media slot");
    activate_virtual_grid_slot(&grid, first_slot);
    activate_virtual_grid_slot(&grid, first_slot);

    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "rapid repeated tile activation should push only one viewer page"
    );
    assert!(
        nav.visible_page()
            .and_downcast::<ViewerPage>()
            .is_some(),
        "thumbnail activation should open the viewer page"
    );
}
```

`search_result_activation_opens_one_viewer_while_pending`:
```rust
fn search_result_activation_opens_one_viewer_while_pending() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();
    let page = SearchPage::new(shell.pool.clone(), shell.loader.clone());
    page.set_nav_target(&nav);
    nav.push(&page);

    page.imp().search_entry.get().set_text("one");
    assert!(
        wait_until(Duration::from_secs(2), || {
            first_flowbox_child(page.upcast_ref()).is_some()
        }),
        "SearchPage should render a result tile for the fixture query"
    );

    let first_tile = first_flowbox_child(page.upcast_ref()).expect("search result tile exists");
    let flow = first_tile
        .parent()
        .and_then(|w| w.downcast::<gtk::FlowBox>().ok())
        .expect("search result tile should belong to a FlowBox");
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);
    assert!(
        !page.is_sensitive(),
        "SearchPage should ignore pointer input while viewer push is guarded"
    );
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);

    assert_eq!(
        nav.navigation_stack().n_items(),
        3,
        "rapid repeated Search result activation should push only one viewer page"
    );
    assert!(
        nav.visible_page()
            .and_downcast::<ViewerPage>()
            .is_some(),
        "search result activation should open the viewer page"
    );
}
```

`viewer_chrome_clicks_drive_visible_operations` — note: drop the leading `photo_viewer::ui::grid_css::install();` (the shell already installs it):
```rust
fn viewer_chrome_clicks_drive_visible_operations() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();
    let viewer = ViewerPage::new(shell.media_list.clone(), 0);
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(shell.pool.clone(), event_sender);
    viewer.set_edit_target(&nav, shell.pool.clone());
    viewer.set_db_actor(db_actor);
    viewer.set_thumbnail_loader(shell.loader.clone());
    viewer.show_at(0);

    let nav_events = Rc::new(RefCell::new(Vec::new()));
    let nav_events_for_cb = nav_events.clone();
    viewer.connect_navigation(move |delta| {
        nav_events_for_cb.borrow_mut().push(delta);
    });

    click_button(&viewer.imp().next_btn.get());
    click_button(&viewer.imp().prev_btn.get());
    assert_eq!(
        nav_events.borrow().as_slice(),
        &[1, -1],
        "viewer prev/next button clicks should emit navigation deltas"
    );

    click_button(&viewer.imp().details_btn.get());
    assert!(
        viewer.imp().details_split_view.get().shows_sidebar(),
        "clicking details should reveal the details sidebar"
    );
    viewer
        .imp()
        .name_row
        .get()
        .emit_by_name::<()>("activated", &[]);
    assert!(
        gtk::prelude::WidgetExt::is_visible(&viewer.imp().name_entry.get()),
        "clicking the details name row should reveal the inline rename entry"
    );
    assert_eq!(
        viewer.imp().name_entry.get().text().as_str(),
        "one",
        "inline rename should edit only the stem by default"
    );
    viewer.imp().name_entry.get().set_text("renamed.png");
    viewer
        .imp()
        .name_entry
        .get()
        .emit_by_name::<()>("activate", &[]);
    let renamed_path = shell._tmp.path().join("photos").join("renamed.jpg");
    assert!(
        wait_until(Duration::from_secs(2), || {
            renamed_path.exists()
                && db::get_media_item(&shell.pool, shell.items[0].id)
                    .map(|item| item.display_name() == "renamed.jpg")
                    .unwrap_or(false)
        }),
        "inline rename should preserve the original extension and update the DB item"
    );
    click_button(&viewer.imp().details_close_btn.get());
    assert!(
        !viewer.imp().details_split_view.get().shows_sidebar(),
        "clicking the details close button should hide the details sidebar"
    );

    let initial_zoom = viewer.imp().zoom_scale.get();
    click_button(&viewer.imp().zoom_in_btn.get());
    assert!(
        viewer.imp().zoom_scale.get() > initial_zoom,
        "clicking zoom-in should increase viewer zoom"
    );
    assert!(
        !viewer.imp().rotate_left_btn.get().is_visible()
            && !viewer.imp().rotate_right_btn.get().is_visible(),
        "viewer rotate buttons should hide once the image is enlarged"
    );
    click_button(&viewer.imp().zoom_reset_btn.get());
    assert_eq!(
        viewer.imp().zoom_scale.get(),
        initial_zoom,
        "clicking zoom reset should restore the initial zoom"
    );
    click_button(&viewer.imp().rotate_right_btn.get());
    assert_eq!(
        viewer.imp().viewer_rotation_degrees.get(),
        90,
        "clicking rotate-right should rotate the current viewer image clockwise"
    );
    click_button(&viewer.imp().zoom_in_btn.get());
    assert!(
        viewer.imp().zoom_scale.get() > initial_zoom,
        "clicking zoom-in should still enlarge after a viewer-only rotation"
    );
    click_button(&viewer.imp().rotate_left_btn.get());
    assert_eq!(
        viewer.imp().viewer_rotation_degrees.get(),
        0,
        "clicking rotate-left should rotate the current viewer image counter-clockwise"
    );

    click_button(&viewer.imp().favorite_btn.get());
    let favorite_id = shell.items[0].id;
    assert!(
        wait_until(Duration::from_secs(2), || db::is_media_favorite(
            &shell.pool,
            favorite_id
        )
        .unwrap_or(false)),
        "clicking the viewer favorite button should persist favorite state"
    );
}
```

`photos_batch_toolbar_clicks_select_favorite_and_album`:
```rust
fn photos_batch_toolbar_clicks_select_favorite_and_album() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();
    let grid = visible_photos_grid(&shell.photos);
    let first_id = MediaId::from(shell.items[0].id);

    grid.select_ids(&[first_id]);

    assert!(
        shell
            .photos
            .imp()
            .add_to_album_revealer
            .get()
            .reveals_child(),
        "selecting a tile should reveal the batch add-to-album action"
    );
    assert!(
        shell.photos.imp().favorite_revealer.get().reveals_child(),
        "selecting a tile should reveal the batch favorite action"
    );
    assert!(
        shell
            .photos
            .imp()
            .delete_to_trash_revealer
            .get()
            .reveals_child(),
        "selecting a tile should reveal the batch trash action"
    );

    click_button(&shell.photos.imp().select_all_btn.get());
    assert!(
        grid.is_all_displayed_selected(),
        "clicking Select All selects every rendered tile in the current mode"
    );
    click_button(&shell.photos.imp().select_all_btn.get());
    assert!(
        !shell.photos.imp().favorite_revealer.get().reveals_child(),
        "clicking the toggled Select All button clears selection and hides batch actions"
    );

    grid.select_ids(&[first_id]);
    click_button(&shell.photos.imp().favorite_btn.get());
    let favorite_id = shell.items[0].id;
    assert!(
        wait_until(Duration::from_secs(2), || {
            db::is_media_favorite(&shell.pool, favorite_id).unwrap_or(false)
        }),
        "clicking the batch favorite button should persist favorite state"
    );
    assert!(
        wait_until(Duration::from_secs(2), || {
            !shell.photos.imp().favorite_revealer.get().reveals_child()
        }),
        "favorite action should clear the previous selection before the next batch action"
    );

    grid.select_ids(&[first_id]);
    assert!(
        wait_until(Duration::from_secs(2), || {
            shell.photos.imp().add_to_album_btn.get().is_visible()
        }),
        "selecting a tile after favorite should expose the batch add-to-album action"
    );
    click_button(&shell.photos.imp().add_to_album_btn.get());
    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "clicking Add to Album should push the album picker page"
    );
}
```

- [ ] **Step 2: Run the suite**

Run: `cargo test --test ux_click_flows 2>&1 | tail -25`
Expected: PASS. If a depth assertion is off, confirm `shell.window.nav_view().n_items()` is 1 at baseline (it should be — the browsing root is a single page) and adjust only if a real discrepancy is shown.

- [ ] **Step 3: fmt + clippy + full suite**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -5 && cargo test --all 2>&1 | grep -E "test result:|FAILED|panicked" | tail -20`
Expected: clean fmt, no warnings, all suites `ok` with 0 failed.

- [ ] **Step 4: Commit**

```bash
git add tests/ux_click_flows.rs
git commit -m "test(ux): run the 5 low-change flows on the full app shell

mode/viewer/search/batch-toolbar flows swap build_photos_page_with_nav()
for build_full_app_shell() so the MainWindow + sidebar + real navigation
stack are exercised, not a standalone PhotosPage."
```

---

### Task 2: Stage 4a — delete orphaned `AlbumBrowserPage` + rework the album flow onto the sidebar path

**Files:**
- Delete: `src/ui/album_browser_page.rs`, `data/ui/album-browser-page.blp`.
- Modify: `src/ui/mod.rs` (drop `pub mod album_browser_page;` and the `pub use album_browser_page::AlbumBrowserPage;` re-export), `build.rs` (drop the `.blp` from the blueprint list), `data/resources.gresource.xml` (drop `<file>ui/album-browser-page.ui</file>`), `data/css/base.css` (drop the three `.album-browser-card-*` rules), `docs/modules/albums-trash.md` (drop the `AlbumBrowserPage` paragraph), `tests/ux_click_flows.rs` (drop the `AlbumBrowserPage` and now-unused `Album` imports; delete `album_browser_reorder_persists_full_album_order`; rework+rename `album_pages_clicks_open_album_and_viewer` → `album_sidebar_open_then_tile_opens_viewer`; update the suite runner).

**Interfaces:**
- Consumes: `build_full_app_shell`, `window.imp().album_list`, `window.imp().album_targets`, `window.browsing_stack()`, `AlbumDetailPage`, `VirtualMediaGrid`, `activate_virtual_grid_slot`, `wait_until`, `find_descendant`.

**Why:** `AlbumBrowserPage` has zero production construction sites — no sidebar entry, button, or nav opens it. Carrying widget-only coverage for an unreachable page adds no regression protection. The `AlbumDetailPage → viewer` half of the album flow is valuable and has a real path (`open_album` via the sidebar), so it is preserved by reworking it onto that path.

- [ ] **Step 1: Delete the dead production code + assets**

```bash
git rm src/ui/album_browser_page.rs data/ui/album-browser-page.blp
```

In `src/ui/mod.rs`, remove these two lines:
```rust
pub mod album_browser_page;
```
and
```rust
pub use album_browser_page::AlbumBrowserPage;
```

In `build.rs`, remove this line from the `blueprint_files` array:
```rust
        "data/ui/album-browser-page.blp",
```

In `data/resources.gresource.xml`, remove this line:
```xml
    <file>ui/album-browser-page.ui</file>
```

In `data/css/base.css`, remove the three rules `.album-browser-card-dragging`, `.album-browser-card-drop-before`, `.album-browser-card-drop-after` (and their brace bodies). Verify first with `rg -n "album-browser-card" src/ data/ui/` — the only remaining hits should be none.

In `docs/modules/albums-trash.md`, remove the paragraph that starts "`AlbumBrowserPage` uses the same persistent ordering for its full album grid." (the sentences through "notifies `MainWindow` to rebuild the sidebar rows."). Keep the preceding sidebar drag-to-reorder paragraph (`MainWindow::attach_album_dnd` … `reorder_album`) — that path still exists.

- [ ] **Step 2: Verify the production build still compiles**

Run: `cargo build 2>&1 | tail -20`
Expected: clean build. If any other `src/` file referenced `AlbumBrowserPage`, the compiler names it — fix by removing that reference (there should be none).

- [ ] **Step 3: Update the test imports**

In `tests/ux_click_flows.rs`:
- Remove `AlbumBrowserPage` from the `use photo_viewer::ui::{ ... }` list (line ~21).
- Remove `use photo_viewer::core::albums::Album;` (line ~14) — after this task no flow names `Album` directly. (If clippy in Step 7 reports it still used, keep it; otherwise it must go to satisfy `-D warnings`.)

- [ ] **Step 4: Delete `album_browser_reorder_persists_full_album_order` and rework the album flow**

Delete the entire `album_browser_reorder_persists_full_album_order` function.

Replace the entire `album_pages_clicks_open_album_and_viewer` function (rename + rewrite) with:
```rust
fn album_sidebar_open_then_tile_opens_viewer() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();

    // Open the first real folder album the way a user does: select its
    // sidebar row. `open_album` is deferred via an idle, so drain the main
    // context until the browsing stack swaps to the album detail page.
    let album_list = shell.window.imp().album_list.get();
    let album_idx = shell
        .window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .position(|album| !album.is_virtual)
        .expect("fixture should seed at least one real folder album");
    let album_row = album_list
        .row_at_index(album_idx as i32)
        .expect("real album row should be present");
    album_list.select_row(Some(&album_row));
    assert!(
        wait_until(Duration::from_secs(2), || {
            shell
                .window
                .browsing_stack()
                .visible_child_name()
                .as_deref()
                == Some("album")
        }),
        "selecting a real album row should open its AlbumDetailPage"
    );
    let detail = shell
        .window
        .browsing_stack()
        .visible_child()
        .and_downcast::<AlbumDetailPage>()
        .expect("album detail page should be visible");

    let detail_grid = find_descendant::<VirtualMediaGrid>(detail.upcast_ref())
        .expect("Album detail should use VirtualMediaGrid");
    assert!(
        wait_until(Duration::from_secs(2), || detail_grid.first_media_slot().is_some()),
        "AlbumDetailPage should load a media slot for the album"
    );
    let first_slot = detail_grid.first_media_slot().expect("album media slot");
    activate_virtual_grid_slot(&detail_grid, first_slot);
    assert!(
        !detail.is_sensitive(),
        "AlbumDetailPage should ignore pointer input while viewer push is guarded"
    );
    activate_virtual_grid_slot(&detail_grid, first_slot);
    assert!(
        nav.visible_page()
            .and_downcast::<ViewerPage>()
            .is_some(),
        "activating an AlbumDetail tile should open the viewer"
    );
    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "rapid repeated AlbumDetail tile activation should push only one viewer page"
    );
}
```

Rationale for the depth assertion changing 3 → 2: the old flow hand-pushed `AlbumDetailPage` onto the nav (root + detail + viewer = 3). The real path puts the album detail in the browsing Stack (not the nav), so only root + viewer = 2.

- [ ] **Step 5: Update the suite runner**

In `ux_click_flow_suite_including_album_sidebar_multi_select_deletes_real_albums`, replace:
```rust
    album_pages_clicks_open_album_and_viewer();
    album_browser_reorder_persists_full_album_order();
```
with:
```rust
    album_sidebar_open_then_tile_opens_viewer();
```

- [ ] **Step 6: Run the suite**

Run: `cargo test --test ux_click_flows 2>&1 | tail -25`
Expected: PASS. If the album detail grid never reports a `first_media_slot`, confirm `open_album` ran (the browsing stack reached `"album"`) and that the seeded "photos" folder album has items; the `wait_until` 2 s guard should suffice for the synchronous initial repo page load.

- [ ] **Step 7: fmt + clippy + full suite**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -5 && cargo test --all 2>&1 | grep -E "test result:|FAILED|panicked" | tail -20`
Expected: clean. If clippy flags `Album` as unused, remove the import (Step 3 already does); if it flags it still used, leave it.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(ui): remove orphaned AlbumBrowserPage dead code

AlbumBrowserPage had no production construction site — no sidebar entry,
button, or navigation opened it; only tests built it. Delete the page,
its Blueprint template, gresource/build registration, and its dead CSS.
Rework the album UX flow onto the real sidebar-open path (open_album)
and drop the album-browser reorder flow that tested only the dead page."
```

---

### Task 3: Stage 4b — migrate album picker, sidebar navigation, and album multi-select to the shell

**Files:**
- Modify: `tests/ux_click_flows.rs` — rewrite `album_picker_clicks_album_row_and_copy_move`, `sidebar_clicks_drive_top_level_navigation`, `album_sidebar_multi_select_deletes_real_albums`; change `seed_extra_album` to take `(pool, base)` instead of `&PhotosFixture`.

**Interfaces:**
- Consumes: `build_full_app_shell`, `visible_photos_grid`, `click_button`, `wait_until`, `find_descendant`, `find_button_with_css`, `MediaId`, `album_picker::push_action_page`, `albums::refresh`, `db::list_all_media`/`get_media_item`, `shell.db_actor`, `shell._tmp`.

**Why:** these three flows already built a full `MainWindow` but pulled resources from the standalone fixture, so `build_photos_page_with_nav` could not be retired. Moving them onto the shared shell is what enables Stage 5. The picker additionally switches from calling `AlbumPickerDialog::present` directly to driving the real Photos batch "Add to Album" button.

- [ ] **Step 1: Decouple `seed_extra_album` from `PhotosFixture`**

Replace the `seed_extra_album` function with:
```rust
fn seed_extra_album(pool: &db::DbPool, base: &std::path::Path) {
    let album_dir = base.join("second-album");
    std::fs::create_dir_all(&album_dir).unwrap();
    let album_path = album_dir.join("three.jpg");
    std::fs::write(&album_path, b"ux-flow-second-album").unwrap();
    let item = sample_item(200, album_path);
    common::db::insert_media_item(pool, &NewMediaItem::from(&item)).unwrap();
}
```

- [ ] **Step 2: Rewrite `album_picker_clicks_album_row_and_copy_move`**

Replace the entire function with:
```rust
fn album_picker_clicks_album_row_and_copy_move() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();
    let grid = visible_photos_grid(&shell.photos);
    let original_count = db::list_all_media(&shell.pool).unwrap().len();

    // Open the picker the way a user does: select a photo, then click the
    // batch "Add to Album" button, which presents AlbumPickerDialog onto the
    // window's outer nav.
    grid.select_ids(&[MediaId::from(shell.items[0].id)]);
    assert!(
        wait_until(Duration::from_secs(2), || shell
            .photos
            .imp()
            .add_to_album_btn
            .get()
            .is_visible()),
        "selecting a tile should expose the batch add-to-album action"
    );
    click_button(&shell.photos.imp().add_to_album_btn.get());

    let wrapper = nav
        .visible_page()
        .expect("AlbumPicker should push a wrapper page");
    let inner = find_descendant::<adw::NavigationView>(wrapper.upcast_ref())
        .expect("AlbumPicker wrapper should contain an inner NavigationView");
    let list_box = find_descendant::<gtk::ListBox>(wrapper.upcast_ref())
        .expect("AlbumPicker should contain an album ListBox");
    assert!(
        wait_until(Duration::from_secs(2), || {
            list_box.observe_children().n_items() > 0
        }),
        "AlbumPicker should populate album rows"
    );
    let first_album_row = list_box
        .row_at_index(0)
        .expect("AlbumPicker should render at least one album row");
    first_album_row.emit_by_name::<()>("activate", &[]);
    assert_eq!(
        inner.navigation_stack().n_items(),
        2,
        "activating an album row should push the Copy/Move action page"
    );

    let copy_btn = find_button_with_css(wrapper.upcast_ref(), "glass-toolbar-suggested")
        .expect("Copy button should be present on the AlbumPicker action page");
    click_button(&copy_btn);
    assert!(
        wait_until(Duration::from_secs(2), || {
            db::list_all_media(&shell.pool)
                .map(|items| items.len() > original_count)
                .unwrap_or(false)
        }),
        "clicking Copy should create a copied media row"
    );
    assert!(
        wait_until(Duration::from_secs(2), || {
            inner.navigation_stack().n_items() == 1
        }),
        "AlbumPicker should return to the album list after Copy"
    );

    let move_target = shell._tmp.path().join("move-target");
    std::fs::create_dir_all(&move_target).unwrap();
    album_picker::push_action_page(
        &inner,
        shell.pool.clone(),
        shell.db_actor.clone(),
        vec![shell.items[1].id],
        move_target.clone(),
        &nav,
    );
    let move_btn = find_button_with_css(wrapper.upcast_ref(), "glass-toolbar-danger")
        .expect("Move button should be present on the AlbumPicker action page");
    click_button(&move_btn);
    assert!(
        wait_until(Duration::from_secs(2), || {
            db::get_media_item(&shell.pool, shell.items[1].id)
                .map(|item| item.folder_path == move_target)
                .unwrap_or(false)
        }),
        "clicking Move should update the media item's album folder"
    );
}
```

Notes: the standalone `gtk::Window` wrapper is dropped — the nav already lives in the `MainWindow`. Copy/Move operate on files under `shell._tmp` (tmpfs); file copy/rename work on tmpfs (only gio *trash* does not, and this flow does not trash).

- [ ] **Step 3: Rewrite `sidebar_clicks_drive_top_level_navigation`**

Replace the entire function with:
```rust
fn sidebar_clicks_drive_top_level_navigation() {
    let shell = build_full_app_shell();
    let window = &shell.window;
    let nav = window.nav_view();

    let sidebar = window.imp().sidebar_list.get();
    let trash_list = window.imp().trash_list.get();
    assert_eq!(
        sidebar.observe_children().n_items(),
        2,
        "top sidebar list should contain Photos and Albums header",
    );
    assert_eq!(
        trash_list.observe_children().n_items(),
        1,
        "trash list should contain one stable Trash row",
    );
    let header = sidebar.row_at_index(1).expect("Albums header row exists");
    assert!(visible_flag(window.imp().album_scroll.get().upcast_ref()));
    release_click_on_widget(header.upcast_ref());
    assert!(
        !visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "clicking the Albums header should collapse the album scroll region"
    );
    release_click_on_widget(header.upcast_ref());
    assert!(
        visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "clicking the Albums header again should expand the album scroll region"
    );

    let trash_row = trash_list.row_at_index(0).expect("Trash row exists");
    trash_list.select_row(Some(&trash_row));
    while glib::MainContext::default().iteration(false) {}
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "selecting the Trash sidebar row should show TrashPage"
    );

    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    sidebar.select_row(Some(&photos_row));
    assert!(
        window.browsing_stack().visible_child_name().as_deref() == Some("photos"),
        "selecting the Photos sidebar row should return to PhotosPage"
    );

    click_button(&window.imp().settings_button.get());
    assert!(
        nav.has_css_class("settings-background-blur"),
        "clicking the settings button should present settings chrome over the content nav"
    );
}
```

- [ ] **Step 4: Rewrite `album_sidebar_multi_select_deletes_real_albums`**

Replace the entire function with:
```rust
fn album_sidebar_multi_select_deletes_real_albums() {
    let shell = build_full_app_shell();
    let window = &shell.window;
    // The base shell seeds one real folder album; add a second so multi-select
    // has two real albums to select (the assertion expects exactly two).
    seed_extra_album(&shell.pool, shell._tmp.path());
    albums::refresh(&shell.pool).unwrap();
    window.populate_album_rows();

    window.enter_album_selection_mode();
    assert_eq!(
        window.imp().album_list.get().selection_mode(),
        gtk::SelectionMode::Multiple,
        "album selection mode should switch the album list to multiple selection",
    );
    assert!(
        window.imp().album_selection_bar.get().is_revealed(),
        "album selection mode should reveal the batch action bar",
    );
    assert!(
        !window.imp().album_selection_delete_btn.get().is_sensitive(),
        "delete selected should stay disabled until real albums are selected",
    );

    let real_rows: Vec<gtk::ListBoxRow> = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .enumerate()
        .filter(|(_, album)| !album.is_virtual)
        .take(2)
        .filter_map(|(idx, _)| window.imp().album_list.get().row_at_index(idx as i32))
        .collect();
    assert_eq!(
        real_rows.len(),
        2,
        "fixture should include two real album rows"
    );
    for row in &real_rows {
        window.imp().album_list.get().select_row(Some(row));
    }
    assert_eq!(window.selected_album_delete_count(), 2);
    assert!(
        window.imp().album_selection_delete_btn.get().is_sensitive(),
        "selecting real albums should enable the delete selected action",
    );
}
```

Note: `connect_sidebar` is no longer called here — `build_full_app_shell` already wired it, and the album selection handler is part of that wiring.

- [ ] **Step 5: Run the suite**

Run: `cargo test --test ux_click_flows 2>&1 | tail -25`
Expected: PASS. If the AlbumPicker rows never populate, add `shell.window.present();` at the start of `album_picker_clicks_album_row_and_copy_move` (the picker's async row load runs on the main context and may need a realized toplevel); the existing shell flows do not require this, so only add if the wait times out.

- [ ] **Step 6: fmt + clippy + full suite**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -5 && cargo test --all 2>&1 | grep -E "test result:|FAILED|panicked" | tail -20`
Expected: clean. (`PhotosFixture`/`build_photos_page_with_nav` are now unused but still defined — Stage 5 removes them; clippy will not flag an unused `fn`/`struct` defined in the test crate unless called, so this is fine for this task.)

- [ ] **Step 7: Commit**

```bash
git add tests/ux_click_flows.rs
git commit -m "test(ux): run picker/sidebar/multi-select flows on the full shell

album_picker now opens via the real Photos batch Add-to-Album button;
sidebar_clicks and album_sidebar_multi_select drop their hand-rolled
MainWindow in favor of build_full_app_shell. seed_extra_album is decoupled
from PhotosFixture. All three now exercise the real navigation shell."
```

---

### Task 4: Stage 5 — retire `build_photos_page_with_nav`, prune imports, update docs

**Files:**
- Modify: `tests/ux_click_flows.rs` (delete `build_photos_page_with_nav` + `struct PhotosFixture`; prune any imports clippy now flags as unused), `docs/testing.md` (state UX flows run on the full app shell).

**Interfaces:** none new.

**Why:** after Tasks 1–3 nothing calls `build_photos_page_with_nav()` or constructs `PhotosFixture`. Deleting them removes the standalone-fixture escape hatch so future flows must use the real shell.

- [ ] **Step 1: Confirm zero callers**

Run: `rg -n "build_photos_page_with_nav|PhotosFixture" tests/ src/`
Expected: only the definitions in `tests/ux_click_flows.rs` (the `fn` and the `struct`). If any caller remains, stop and re-migrate it first.

- [ ] **Step 2: Delete the fixture**

In `tests/ux_click_flows.rs`, delete the entire `struct PhotosFixture { ... }` block and the entire `fn build_photos_page_with_nav() -> PhotosFixture { ... }` function.

- [ ] **Step 3: Prune now-unused imports**

Run: `cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -20`
Remove every import clippy reports as unused (likely candidates: none new beyond `Album`/`AlbumBrowserPage` already handled — but verify). Run `cargo fmt --all` after edits.

- [ ] **Step 4: Update `docs/testing.md`**

Near the existing UX mention (around line 14 and line 42), add a short note that the UX click flows run against the full `MainWindow`+sidebar shell via `build_full_app_shell()` (not a standalone `PhotosPage`), so page-to-page navigation and sidebar/state interactions are covered. Keep it to one or two sentences matching the surrounding style.

- [ ] **Step 5: fmt + clippy + full suite**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments 2>&1 | tail -5 && cargo test --all 2>&1 | grep -E "test result:|FAILED|panicked" | tail -20`
Expected: clean fmt, no warnings, all suites `ok` with 0 failed.

- [ ] **Step 6: Commit**

```bash
git add tests/ux_click_flows.rs docs/testing.md
git commit -m "test(ux): retire the standalone PhotosPage UX fixture

build_photos_page_with_nav and PhotosFixture are gone now that every UX
flow runs on build_full_app_shell. docs/testing.md notes the shell-backed
coverage model."
```

---

## Self-Review (completed)

- **Spec coverage:** Stage 3 (5 low-change flows) → Task 1. Stage 4: dead-code deletion + album-browser flow removal + album-flow rework → Task 2; album picker via real button + sidebar/multi-select migration → Task 3. Stage 5 (retire fixture + docs) → Task 4. All 8 spec-listed flows are accounted for: 5 migrated, 1 reworked onto the real path, 1 migrated (picker), 1 deleted (browser reorder, because the page is dead).
- **Dead-code decision:** applied — `AlbumBrowserPage` source/template/resources/CSS deleted; the two widget-only flows removed/reworked. Doc paragraph removed. No `ui-naming-reference` entry existed for it (verified).
- **Nav-depth assertions:** every assertion is recomputed against the shell baseline of 1 (root page holds the browsing Stack): thumbnail 2, search 3, batch/picker 2, album→viewer 2. The only assertion that changed value is album→viewer (3 → 2), and the rationale is documented inline.
- **Placeholder scan:** none — every step shows full code or an exact deletion list.
- **Type consistency:** `seed_extra_album(pool, base)` signature in Task 3 matches its sole caller (`album_sidebar_multi_select`); `AppShell` fields (`window`, `photos`, `pool`, `loader`, `media_list`, `db_actor`, `items`, `_tmp`) match every flow's usage.
- **Global constraints:** unique app ids via `FULL_SHELL_SEQ` (no fixed ids reintroduced); single `#[test]` runner preserved; `.blp`/gresource/`build.rs` edited, not `.ui`.
