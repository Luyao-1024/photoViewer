# UX Test Suite Design — Migrate to the Full App Shell

Date: 2026-07-16
Status: Approved (brainstorm), pending implementation plan

## Problem

The Trash "restore exits the page" bug (commit `8053a4b`) reached the user even
though `tests/ux_click_flows.rs` already had a
`trash_page_clicks_selection_cancel_restore_and_delete` flow. Root cause of the
miss: that flow runs on a **standalone** `TrashPage` pushed onto a bare
`adw::NavigationView` — no `MainWindow`, no sidebar, no `connect_sidebar`. So the
navigation/state interaction that caused the bug (focus fallback → sidebar
auto-selects Photos → `row-selected` pops the Trash page) was never exercised.

Survey of the 11 UX sub-flows in `ux_click_flows.rs`:

- 2 use a full `MainWindow` + sidebar (`sidebar_clicks_drive_top_level_navigation`,
  `album_sidebar_multi_select_deletes_real_albums`).
- 9 use only `build_photos_page_with_nav()` — a standalone `PhotosPage` on a bare
  nav with no `MainWindow`/sidebar.

A second, independent limitation: some real-runtime interactions (GTK focus
fallback, real filesystem-watcher timing) **cannot be reproduced inside
`gtk::test`** even on the full shell. The Trash bug was like this — a full-shell
`gtk::test` passed both before and after the fix; the cause was only found via a
backtrace captured in the running Flatpak app.

## Goal

Migrate the existing standalone UX flows onto the **full app shell**
(`MainWindow` + sidebar + outer navigation + `PhotosPage` root), so page-to-page
navigation and sidebar/state interactions are exercised — and complement that
with **mechanism-level guard tests** for the interactions `gtk::test` cannot
reproduce.

Non-goals: adding brand-new user flows for breadth (settings, editor save, empty
states) is out of scope for this design; it can follow the same fixture later.

## Architecture

Two complementary test layers:

```
tests/ux_click_flows.rs            (keep the single #[test] runner; shared gtk/tokio init)
  ├─ build_full_app_shell()         NEW: full MainWindow + sidebar + nav + PhotosPage root
  ├─ build_photos_page_with_nav()   kept temporarily; retired in stage 5
  ├─ <9 migrated sub-flows>         standalone → full shell
  └─ shared helpers                 click_button / activate_virtual_grid_slot / wait_until …

src/ui/window/navigation/tests.rs  (crate-internal; can access imp())
  └─ mechanism guard tests          focus/selection/navigation coupling that gtk::test can't reproduce
```

Principles:

- **UX flow tests** drive real click/activation chains and cover flows a user can
  reach. After migration they all run on the full shell, so navigation stack and
  sidebar selection state match real use.
- **Mechanism guard tests** set flags / call internal APIs directly to verify the
  decision logic behind runtime-only interactions (the focus-fallback class).
  They live crate-internally because they need `imp()` access.
- Migrated sub-flows open pages the way a user does: via sidebar `select_row`
  (Trash/albums) or by pushing onto `window.nav_view()` (viewer/search), never a
  bare `nav.push` of a hand-built page.

## Full-shell fixture

`build_full_app_shell()` returns everything a flow needs:

- A registered `adw::Application` + `MainWindow`.
- `populate_sidebar()`; `set_resources(pool, loader, media_list)`;
  `set_db_actor(...)`; `albums::refresh`; `populate_album_rows()`;
  `connect_sidebar(&nav)`.
- A `PhotosPage` root shown via `show_photos_browsing_page(&root)`.
- Seeded media (at least two items in one album folder) so grids render.
- Returned handles: `window`, `nav` (= `window.nav_view()`), the visible
  `PhotosPage`, pool/loader/media_list/db_actor, items.

Helpers added next to it:

- `visible_photos_page(window) -> PhotosPage` (the browsing-root Photos page).
- `open_trash_via_sidebar(window)` — selects the Trash row, returns the now-visible
  `TrashPage` built by `show_trash_page` (real pool/loader/media_list/db_actor).

## Migration manifest

Core change for every flow: `build_photos_page_with_nav()` →
`build_full_app_shell()`; operate on the window's visible PhotosPage and
`window.nav_view()`; open Trash/albums/search via sidebar or the window's nav.

**Low change** (swap fixture + page/nav handles; logic unchanged):

- `mode_selector_click_switches_photos_view`
- `thumbnail_activation_opens_one_viewer`
- `search_result_activation_opens_one_viewer_while_pending`
- `viewer_chrome_clicks_drive_visible_operations`
- `photos_batch_toolbar_clicks_select_favorite_and_album`

**Medium change** (open via sidebar/real navigation instead of bare push):

- `album_pages_clicks_open_album_and_viewer`
- `album_picker_clicks_album_row_and_copy_move`
- `album_browser_reorder_persists_full_album_order`

**High change** (highest value — the bug class):

- `trash_page_clicks_selection_cancel_restore_and_delete`
  - Open Trash via `open_trash_via_sidebar(window)` instead of
    `TrashPage::with_media_list` + bare `nav.push`.
  - After restore and after delete-permanently, **assert the Trash page is still
    the visible page** (the user-facing invariant).
  - The focus-fallback itself is not reproducible in `gtk::test`; this UX test
    covers the *observable* behavior, and the mechanism guard covers the
    *decision logic*. The two are a pair.

## Mechanism guard tests

In `src/ui/window/navigation/tests.rs`:

| Guard | Regression it blocks | Status |
|---|---|---|
| `focus_driven_sidebar_selection_does_not_pop_pushed_page` | focus fallback → sidebar auto-select → pops a pushed page (the Trash bug) | done |
| `genuine_click_navigates_when_not_focus_traversal` | the guard's counter-test: a real (non-focus) Photos selection still navigates | to add |
| `programmatic_sidebar_selection_does_not_reenter_navigation` | `selecting_programmatically` guard: sidebar re-selection during refresh must not navigate | to add |
| `open_album_pops_viewer_pushed_above_root` | opening an album must pop a viewer pushed above the root | already exists |

The guard list grows as migration surfaces more fragile interactions; it is not
exhausted up front (YAGNI).

## Roadmap

Each stage is independently deliverable, runnable, and revertible. Each stage
ends with `cargo fmt --all --check`, `cargo clippy --all-targets -D warnings
-A clippy::type_complexity -A clippy::too_many_arguments`, and
`cargo test --all` green before the next.

- **Stage 0 (done):** Trash bug fix + focus-fallback guard test, pushed.
- **Stage 1:** Add `build_full_app_shell()` + helpers; prove the empty shell runs.
  No flow migration yet.
- **Stage 2 (highest value):** Migrate the Trash flow (high change) + add the
  "page stays after restore/delete" assertions.
- **Stage 3:** Migrate the 5 low-change flows (mode/viewer/search/batch).
- **Stage 4:** Migrate the 3 medium-change flows (album pages/picker/browser).
- **Stage 5:** Retire `build_photos_page_with_nav()` remnants; update
  `docs/testing.md` to state that UX flows run on the full app shell.

## Out of scope / future

- New-flow breadth (settings dialog, editor save round-trip, empty states,
  search "More" page, cross-page navigation like viewer-while-album-open) can
  reuse `build_full_app_shell()` later, in a separate design.
- A real-app (Flatpak) smoke test for the very top flows was considered and
  deferred — it needs scriptable GUI driving and is not required given the
  mechanism-guard complement.
