# Grid Scroll Date Indicator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** While the user scrolls the Photos media grid, show a compact glass date pill just left of the scrollbar that reports the date section at the current scroll position; it tracks the thumb vertically, appears only while scrolling, and fades out ~700 ms after scrolling stops.

**Architecture:** Keep GTK's native scrollbar. Add a `Gtk.Revealer`+`Gtk.Label` overlay child to `PhotosPage`'s existing `grid_overlay`. Drive it from the existing `MediaGrid::connect_view_changed` scroll hook. Resolve the date by projecting the scroll ratio × full-library live total through the already-loaded per-mode `section_counts` (new pure helper `section_model::section_for_global_offset`), so it stays correct even in unloaded virtual-paged regions. One pill serves all three Year/Month/Day grids (only the visible grid is ever on screen).

**Tech Stack:** Rust, gtk4-rs (`gtk`/`glib`/`gio`), libadwaita, Blueprint XML templates (`data/ui/*.blp`), JSON i18n (`i18n/en.json`, `i18n/zh-CN.json`).

## Global Constraints

- Edit `data/ui/*.blp` templates, not generated `.ui` output. `build.rs` compiles Blueprint.
- Keep `src/core/` independent of GTK. The new `section_for_global_offset` / `make_label_nocount` are pure Rust (no `gtk::`).
- Reuse the shared `.glass-raised` CSS class for the pill surface; any new selector must work in both Liquid Glass and plain translucent modes. Add only padding/radius/font via a new `.scroll-date-pill` class in `data/css/base.css` (mode-agnostic).
- i18n: every new string is added to BOTH `i18n/en.json` and `i18n/zh-CN.json`.
- The pill must not capture pointer events (`can-target: false`) so tiles and the scrollbar stay interactive.
- v1 scope is the Photos page only. Album detail pages and embedded preview grids are out of scope.
- Run the full CI mirror before committing/pushing: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments`, and `xvfb-run -a cargo test --all` (the `#[gtk::test]` cases need a display).

---

## File Structure

- `src/core/section_model.rs` — add two pure helpers: `section_for_global_offset` (offset → `SectionKey` via cumulative counts) and `make_label_nocount` (count-free date label). No GTK.
- `i18n/en.json`, `i18n/zh-CN.json` — add `section.label.{year,month,day,unknown}.nocount` keys.
- `src/core/section_model/tests.rs` — pure unit tests for the two helpers.
- `src/ui/media_grid.rs` — add `MediaGrid::scroll_fraction()` and `MediaGrid::current_scroll_section_key()` (read own `library_total_snapshot` + `section_count_snapshots` + vadjustment; delegate the projection to `section_model`).
- `src/ui/media_grid/tests.rs` — a `#[gtk::test]` that injects known counts/total, drives the vadjustment, and asserts the resolved section at top/middle/bottom.
- `data/ui/photos-page.blp` — add the `scroll_date_revealer` + `scroll_date_label` overlay child to `grid_overlay`.
- `data/css/base.css` — add `.scroll-date-pill` (padding/radius/font).
- `src/ui/photos_page.rs` — imp template children + hide-timer state; `update_scroll_date`, `schedule_scroll_date_update`, `arm_scroll_date_hide`; wire the existing `connect_view_changed` closure to also drive the pill.
- `docs/modules/browsing.md` — document the indicator and the counts-projection invariant.
- `docs/ui-naming-reference/index.html` — add the new pill widget id.

---

### Task 1: Pure date-resolution helpers in `section_model.rs`

**Files:**
- Modify: `src/core/section_model.rs` (add `section_for_global_offset`, `date_rank`, `make_label_nocount`)
- Modify: `i18n/en.json`, `i18n/zh-CN.json` (add four `.nocount` keys each)
- Test: `src/core/section_model/tests.rs`

**Interfaces:**
- Consumes: existing `SectionKey { year: Option<i32>, month: Option<u32>, day: Option<u32> }`, `HashMap`, `trf`.
- Produces:
  - `pub fn section_for_global_offset(counts: &HashMap<SectionKey, u32>, offset: u32) -> Option<SectionKey>`
  - `pub fn make_label_nocount(key: &SectionKey) -> String`

- [ ] **Step 1: Add the i18n keys (both locales)**

Append these entries into `i18n/zh-CN.json` immediately after the existing `"section.label.unknown"` line (keep valid JSON, match the surrounding 2-space indentation):

```json
    "section.label.year.nocount": "{year}年",
    "section.label.month.nocount": "{year}年{month}月",
    "section.label.day.nocount": "{year}年{month}月{day}日",
    "section.label.unknown.nocount": "未知日期",
```

Append these entries into `i18n/en.json` immediately after the existing `"section.label.unknown"` line:

```json
    "section.label.year.nocount": "{year}",
    "section.label.month.nocount": "{month}/{year}",
    "section.label.day.nocount": "{year}/{month}/{day}",
    "section.label.unknown.nocount": "Unknown date",
```

- [ ] **Step 2: Write the failing tests**

In `src/core/section_model/tests.rs`, add at the end of the file:

```rust
#[test]
fn section_for_global_offset_empty_returns_none() {
    let counts: HashMap<SectionKey, u32> = HashMap::new();
    assert!(section_for_global_offset(&counts, 0).is_none());
}

#[test]
fn section_for_global_offset_maps_boundaries_and_midpoints() {
    // Library ordered newest-first. Counts: 2026-07=3, 2026-06=2, 2025=4 (total 9).
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(
        SectionKey { year: Some(2026), month: Some(7), day: None },
        3,
    );
    counts.insert(
        SectionKey { year: Some(2026), month: Some(6), day: None },
        2,
    );
    counts.insert(SectionKey { year: Some(2025), month: None, day: None }, 4);

    let jul = SectionKey { year: Some(2026), month: Some(7), day: None };
    let jun = SectionKey { year: Some(2026), month: Some(6), day: None };
    let y2025 = SectionKey { year: Some(2025), month: None, day: None };

    assert_eq!(section_for_global_offset(&counts, 0), Some(jul.clone())); // first
    assert_eq!(section_for_global_offset(&counts, 2), Some(jul.clone())); // last of jul
    assert_eq!(section_for_global_offset(&counts, 3), Some(jun.clone())); // first of jun (boundary → next)
    assert_eq!(section_for_global_offset(&counts, 4), Some(jun.clone())); // last of jun
    assert_eq!(section_for_global_offset(&counts, 5), Some(y2025.clone())); // first of 2025
    assert_eq!(section_for_global_offset(&counts, 8), Some(y2025.clone())); // last overall
}

#[test]
fn section_for_global_offset_clamps_beyond_total_to_oldest() {
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(SectionKey { year: Some(2026), month: None, day: None }, 2);
    // Oldest (smallest year) when more than one section exists.
    counts.insert(SectionKey { year: Some(2020), month: None, day: None }, 1);
    // offset past total (3) → oldest section (2020), not None.
    assert_eq!(
        section_for_global_offset(&counts, 99),
        Some(SectionKey { year: Some(2020), month: None, day: None }),
    );
}

#[test]
fn section_for_global_offset_orders_newest_first_regardless_of_input_order() {
    // Insert oldest first; result must still treat newest as offset 0.
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(SectionKey { year: Some(2020), month: None, day: None }, 1);
    counts.insert(SectionKey { year: Some(2026), month: None, day: None }, 1);
    assert_eq!(
        section_for_global_offset(&counts, 0),
        Some(SectionKey { year: Some(2026), month: None, day: None }),
    );
    assert_eq!(
        section_for_global_offset(&counts, 1),
        Some(SectionKey { year: Some(2020), month: None, day: None }),
    );
}

#[test]
fn make_label_nocount_has_no_count_and_matches_mode() {
    let year = make_label_nocount(&SectionKey { year: Some(2026), month: None, day: None });
    let month = make_label_nocount(&SectionKey {
        year: Some(2026),
        month: Some(7),
        day: None,
    });
    let day = make_label_nocount(&SectionKey {
        year: Some(2026),
        month: Some(7),
        day: Some(12),
    });
    assert!(!year.contains("·"), "year label must not include the count separator: {year}");
    assert!(!month.contains("·"), "month label must not include the count separator: {month}");
    assert!(!day.contains("·"), "day label must not include the count separator: {day}");
    assert!(year.contains("2026"));
    assert!(month.contains("7"));
    assert!(day.contains("12"));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib section_model::tests::section_for_global_offset -- --nocapture` and `cargo test --lib section_model::tests::make_label_nocount -- --nocapture`
Expected: compile error — `section_for_global_offset` / `make_label_nocount` not found.

- [ ] **Step 4: Implement the helpers**

In `src/core/section_model.rs`, add the following. Place `date_rank` just above the existing `fn make_label` (it is a private helper); place the two public functions just below `apply_authoritative_counts` (after line 126):

```rust
/// Map a `SectionKey` to a comparable rank so sections can be ordered newest-first.
/// `None` components sink to the bottom (treated as the smallest value).
fn date_rank(key: &SectionKey) -> (i64, u32, u32) {
    let year = key.year.map(|y| y as i64).unwrap_or(i64::MIN);
    let month = key.month.unwrap_or(0);
    let day = key.day.unwrap_or(0);
    (year, month, day)
}

/// Resolve the date section a global media offset falls into, given the
/// full-library per-section `counts`. Sections are treated as newest-first
/// (matching the grid's descending `sort_datetime` order): the newest section
/// covers offset `[0, c0)`, the next covers `[c0, c0+c1)`, and so on.
///
/// `offset` past the total clamps to the oldest section. An empty map returns
/// `None`. Used by the scroll-date indicator so the date stays correct even in
/// virtual-paged regions whose tiles are not realized.
pub fn section_for_global_offset(
    counts: &HashMap<SectionKey, u32>,
    offset: u32,
) -> Option<SectionKey> {
    if counts.is_empty() {
        return None;
    }
    // Newest (largest date_rank) first.
    let mut ordered: Vec<(&SectionKey, u32)> =
        counts.iter().map(|(k, v)| (k, *v)).collect();
    ordered.sort_unstable_by(|a, b| date_rank(b.0).cmp(&date_rank(a.0)));

    let mut acc: u32 = 0;
    for (key, count) in &ordered {
        let upper = acc.saturating_add(*count);
        if offset < upper {
            return Some((*key).clone());
        }
        acc = upper;
    }
    // offset >= total: clamp to the oldest section.
    ordered.last().map(|(key, _)| (*key).clone())
}
```

And place `make_label_nocount` just below the existing private `fn make_label` (i.e. after its closing brace, before `#[cfg(test)] mod tests;`):

```rust
/// Like `make_label` but without the photo count (and without the weekday), for
/// the compact scroll-date pill. Mirrors the per-mode `section.label.*.nocount`
/// i18n keys.
pub fn make_label_nocount(key: &SectionKey) -> String {
    match (key.year, key.month, key.day) {
        (Some(y), Some(m), Some(d)) => trf(
            "section.label.day.nocount",
            &[
                ("year", &y.to_string()),
                ("month", &m.to_string()),
                ("day", &d.to_string()),
            ],
        ),
        (Some(y), Some(m), None) => trf(
            "section.label.month.nocount",
            &[("year", &y.to_string()), ("month", &m.to_string())],
        ),
        (Some(y), None, None) => {
            trf("section.label.year.nocount", &[("year", &y.to_string())])
        }
        _ => trf("section.label.unknown.nocount", &[]),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib section_model -- --nocapture`
Expected: PASS — all `section_for_global_offset_*` and `make_label_nocount` tests green.

- [ ] **Step 6: Commit**

```bash
git add src/core/section_model.rs src/core/section_model/tests.rs i18n/en.json i18n/zh-CN.json
git commit -m "feat(section-model): resolve date section from full-library offset

Add section_for_global_offset (newest-first cumulative-count projection) and a
count-free make_label_nocount, plus the section.label.*.nocount i18n keys. These
power the scroll-date indicator and stay correct in unloaded virtual-paged regions."
```

---

### Task 2: `MediaGrid::scroll_fraction` + `current_scroll_section_key`

**Files:**
- Modify: `src/ui/media_grid.rs` (add two public methods near `connect_view_changed`, ~line 804)
- Test: `src/ui/media_grid/loading/tests.rs` (this file already constructs grids, accesses `grid.imp()`, and pulls in `test_support::{noop_callbacks, sample_item}` — copy its preamble)

**Interfaces:**
- Consumes: `section_model::section_for_global_offset` (Task 1); existing private `scroll_ratio_from_adjustment_value`; imp fields `library_total_snapshot: Cell<Option<u32>>`, `section_count_snapshots: RefCell<HashMap<GroupBy, HashMap<SectionKey, u32>>>`, `mode`, `scroller`.
- Produces:
  - `pub fn scroll_fraction(&self) -> f64` — `value / (upper - page_size)`, clamped `[0,1]`.
  - `pub fn current_scroll_section_key(&self) -> Option<SectionKey>` — the date section at the current scroll position.

- [ ] **Step 1: Write the failing test**

Append to `src/ui/media_grid/loading/tests.rs`. This file already has the needed imports (`use super::super::test_support::*;` brings `noop_callbacks`/`sample_item`/`tile_count`; `use super::super::*;` and `use super::*;` bring `MediaGrid`, `GroupBy`, `SectionKey`, `HashMap`, `ThumbnailLoader`, `gio`, `glib`, `Arc`). Reuse the same construction preamble as the existing `active_empty_day_grid_rebuilds_when_first_scan_items_arrive` test (lines 111-127): a tempdir pool + `ThumbnailLoader` + empty `gio::ListStore`. Then:

```rust
#[gtk::test]
fn current_scroll_section_key_resolves_via_injected_counts() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let grid = MediaGrid::new(media_list, GroupBy::Month, loader, noop_callbacks(), false);

    // Empty/counts-less grid degrades safely.
    assert!(grid.current_scroll_section_key().is_none());
    assert_eq!(grid.scroll_fraction(), 0.0);

    // Inject full-library metadata as the background refresh would.
    let mut counts: HashMap<SectionKey, u32> = HashMap::new();
    counts.insert(SectionKey { year: Some(2026), month: Some(7), day: None }, 3);
    counts.insert(SectionKey { year: Some(2026), month: Some(6), day: None }, 2);
    counts.insert(SectionKey { year: Some(2025), month: None, day: None }, 4);
    grid.imp()
        .section_count_snapshots
        .borrow_mut()
        .insert(GroupBy::Month, counts);
    grid.imp().library_total_snapshot.set(Some(9));

    // Drive the scrolled window's adjustment. upper=1000, page=200 → travel=800.
    let adj = grid.imp().scroller.get().vadjustment();
    adj.set_upper(1000.0);
    adj.set_page_size(200.0);

    let jul = SectionKey { year: Some(2026), month: Some(7), day: None };
    let y2025 = SectionKey { year: Some(2025), month: None, day: None };

    // value=0 → ratio 0 → offset 0 → newest section (Jul).
    adj.set_value(0.0);
    assert_eq!(grid.scroll_fraction(), 0.0);
    assert_eq!(grid.current_scroll_section_key(), Some(jul));

    // value=800 → ratio 1.0 → offset clamps to oldest (2025).
    adj.set_value(800.0);
    assert_eq!(grid.scroll_fraction(), 1.0);
    assert_eq!(grid.current_scroll_section_key(), Some(y2025));
}
```

> The test exercises the exact imp fields the background refresh writes (`section_count_snapshots`, `library_total_snapshot`) — the same ones `loading.rs:407/412` set in production — so it validates the real data path, not a test-only seam.

- [ ] **Step 2: Run the test to verify it fails**

Run: `xvfb-run -a cargo test --lib media_grid::loading::tests::current_scroll_section_key_resolves_via_injected_counts -- --nocapture`
Expected: compile error — `scroll_fraction` / `current_scroll_section_key` not found on `MediaGrid`.

- [ ] **Step 3: Implement the two methods**

In `src/ui/media_grid.rs`, add immediately after the existing `pub fn connect_view_changed` method (after its closing brace, ~line 804, before `background_is_light_under`):

```rust
    /// The scrollbar thumb's travel fraction: `value / (upper - page_size)`,
    /// clamped to `[0, 1]`. `0.0` when the content does not overflow.
    pub fn scroll_fraction(&self) -> f64 {
        let adj = self.imp().scroller.get().vadjustment();
        scroll_ratio_from_adjustment_value(adj.value(), adj.upper(), adj.page_size())
    }

    /// The date section at the current scroll position, resolved from the
    /// already-loaded full-library `section_counts` so it stays correct even in
    /// virtual-paged regions whose tiles are not realized. `None` when library
    /// metadata has not loaded yet, the library is empty, or there is only one
    /// section (nothing meaningful to indicate).
    pub fn current_scroll_section_key(&self) -> Option<SectionKey> {
        let imp = self.imp();
        let total = imp.library_total_snapshot.get()?;
        let counts_by_mode = imp.section_count_snapshots.borrow();
        let counts = counts_by_mode.get(&imp.mode.get())?;
        if counts.len() <= 1 {
            return None;
        }
        let ratio = self.scroll_fraction();
        let offset = ((ratio * total as f64).round() as u32).min(total.saturating_sub(1));
        crate::core::section_model::section_for_global_offset(counts, offset)
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `xvfb-run -a cargo test --lib media_grid::loading::tests::current_scroll_section_key_resolves_via_injected_counts -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/media_grid.rs src/ui/media_grid/loading/tests.rs
git commit -m "feat(media-grid): expose scroll fraction and current date section

MediaGrid::scroll_fraction and current_scroll_section_key project the scroll
position through the loaded full-library section_counts, so the date is correct
even where virtual paging has not realized tiles."
```

---

### Task 3: Pill UI — Blueprint overlay child + CSS

**Files:**
- Modify: `data/ui/photos-page.blp` (add overlay child to `grid_overlay`)
- Modify: `data/css/base.css` (add `.scroll-date-pill`)

**Interfaces:**
- Produces: two new `PhotosPage` template children — `scroll_date_revealer: TemplateChild<gtk::Revealer>`, `scroll_date_label: TemplateChild<gtk::Label>` — wired in Task 4.

- [ ] **Step 1: Add the overlay child to `grid_overlay`**

In `data/ui/photos-page.blp`, inside the `Gtk.Overlay grid_overlay { … }` block, add a second `[overlay]` child right after the existing `$ModeSelector mode_selector { … }` block (i.e. before the closing brace of `grid_overlay`):

```blueprint
      [overlay]
      Gtk.Revealer scroll_date_revealer {
        transition-type: crossfade;
        transition-duration: 200;
        reveal-child: false;
        halign: end;
        valign: start;
        margin-end: 14;
        margin-top: 0;
        // Click-through: never steal pointer events from tiles or the scrollbar.
        can-target: false;

        Gtk.Label scroll_date_label {
          css-classes: ["glass-raised", "scroll-date-pill"];
        }
      }
```

- [ ] **Step 2: Add the `.scroll-date-pill` CSS**

In `data/css/base.css`, append (the `.glass-raised` surface — background/blur/border/shadow — is already defined in `liquid.css` and `plain.css`; this class only adds the pill geometry, so it is mode-agnostic and lives in `base.css`):

```css
/* Compact date pill shown beside the Photos grid scrollbar while scrolling.
   The surface material comes from the shared .glass-raised rule; this only
   adds pill geometry so it renders identically in Liquid Glass and plain modes. */
.scroll-date-pill {
  padding: 6px 12px;
  border-radius: 999px;
  font-weight: 600;
}
```

- [ ] **Step 3: Build to verify the template + CSS compile**

Run: `cargo build`
Expected: builds cleanly (Blueprint compiles via `build.rs`; a BLP typo fails here). Task 4 will reference the new template children, so a compile of the whole crate comes after Task 4 — this step just catches BLP/CSS syntax errors early.

- [ ] **Step 4: Commit**

```bash
git add data/ui/photos-page.blp data/css/base.css
git commit -m "feat(ui): add scroll-date pill overlay child and glass pill style

Revealer+Label overlay on PhotosPage grid_overlay (crossfade, click-through,
halign=end valign=start) reusing .glass-raised; .scroll-date-pill adds only
pill geometry so both themes render identically."
```

---

### Task 4: PhotosPage wiring — reveal, position, label, fade-out

**Files:**
- Modify: `src/ui/photos_page.rs` (imp fields, init, three methods, update the `connect_view_changed` closure)

**Interfaces:**
- Consumes: Task 2's `MediaGrid::scroll_fraction` / `current_scroll_section_key`; Task 1's `section_model::make_label_nocount`; Task 3's `scroll_date_revealer` / `scroll_date_label` template children.
- Produces: the live pill behavior. No new public API.

- [ ] **Step 1: Add imp state + template children**

`PhotosPage`'s imp struct uses `#[derive(gtk::CompositeTemplate)]` with a `#[template_child]` attribute on each templated field, and `klass.bind_template()` (already called in `class_init`) binds them by field name. So adding the attribute + field + a `::default()` init is all that's needed — no manual bind call, and the BLP child ids from Task 3 (`scroll_date_revealer` / `scroll_date_label`) must match the field names exactly.

In `src/ui/photos_page.rs`, in the imp struct `pub struct PhotosPage { … }` (annotated `#[derive(gtk::CompositeTemplate)]`, near the other `#[template_child]` fields — `grid_overlay` is at line 89), add the two templated children plus the two plain state fields:

```rust
        #[template_child]
        pub scroll_date_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub scroll_date_label: TemplateChild<gtk::Label>,
        /// One-shot hide timer: reset on every scroll event; fires ~700ms after
        /// the last scroll to fade the pill out.
        pub scroll_date_hide_timer: RefCell<Option<glib::SourceId>>,
        /// Coalescing flag for `schedule_scroll_date_update` (mirrors
        /// `contrast_update_pending`).
        pub scroll_date_update_pending: Cell<bool>,
```

In the constructed-field initializer (where each field gets its initial value — `grid_overlay: TemplateChild::default(),` is at line 140, `contrast_update_pending: Cell::new(false),` at line 136), add:

```rust
                scroll_date_revealer: TemplateChild::default(),
                scroll_date_label: TemplateChild::default(),
                scroll_date_hide_timer: RefCell::new(None),
                scroll_date_update_pending: Cell::new(false),
```

- [ ] **Step 2: Add the three methods**

In `src/ui/photos_page.rs`, in the `impl PhotosPage { … }` block (place them right after `schedule_mode_selector_contrast_update`, ~line 970), add:

```rust
    /// Refresh the scroll-date pill: resolve the current grid's date section,
    /// update the label, position the pill alongside the scrollbar thumb, and
    /// reveal it. Hides itself when there is nothing to show.
    fn update_scroll_date(&self) {
        let Some(grid) = self.current_grid() else {
            self.imp().scroll_date_revealer.set_reveal_child(false);
            return;
        };
        let Some(key) = grid.current_scroll_section_key() else {
            self.imp().scroll_date_revealer.set_reveal_child(false);
            return;
        };

        let imp = self.imp();
        imp.scroll_date_label
            .set_label(&crate::core::section_model::make_label_nocount(&key));

        // Track the thumb vertically. Only position once the overlay is
        // allocated; before that, heights are 0 and we just reveal at the top.
        let overlay = imp.grid_overlay.get();
        let revealer = imp.scroll_date_revealer.get();
        let overlay_h = overlay.height() as f32;
        let pill_h = revealer.height().max(1) as f32;
        if overlay_h > pill_h {
            let margin = 8.0_f32;
            let usable = (overlay_h - pill_h - 2.0 * margin).max(0.0);
            let top = margin + (grid.scroll_fraction() as f32) * usable;
            revealer.set_margin_top(top.round() as i32);
        }
        revealer.set_reveal_child(true);
    }

    /// Coalesce scroll-date updates (the resolution is cheap but we still avoid
    /// queuing more than one per idle tick during kinetic scrolling). Mirrors
    /// `schedule_mode_selector_contrast_update`.
    fn schedule_scroll_date_update(&self) {
        if self.imp().scroll_date_update_pending.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.update_scroll_date();
                this.imp().scroll_date_update_pending.set(false);
            }
        });
    }

    /// (Re)arm the one-shot hide timer so the pill fades out ~700ms after the
    /// last scroll event.
    fn arm_scroll_date_hide(&self) {
        let imp = self.imp();
        if let Some(old) = imp.scroll_date_hide_timer.borrow_mut().take() {
            old.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            if let Some(this) = weak.upgrade() {
                this.imp().scroll_date_revealer.set_reveal_child(false);
            }
        });
        *imp.scroll_date_hide_timer.borrow_mut() = Some(id);
    }
```

- [ ] **Step 3: Wire the scroll hook**

In `src/ui/photos_page.rs`, update the existing `connect_view_changed` closure (~line 361-368) from:

```rust
        for grid in [&year_grid, &month_grid, &day_grid] {
            let weak = obj.downgrade();
            grid.connect_view_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.schedule_mode_selector_contrast_update();
                }
            });
        }
```

to:

```rust
        for grid in [&year_grid, &month_grid, &day_grid] {
            let weak = obj.downgrade();
            grid.connect_view_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.schedule_mode_selector_contrast_update();
                    this.schedule_scroll_date_update();
                    this.arm_scroll_date_hide();
                }
            });
        }
```

- [ ] **Step 4: Build and verify the whole crate compiles**

Run: `cargo build`
Expected: clean build (catches the new template-child binding, method visibility, and `glib::SourceId` / `timeout_add_local_once` usage).

- [ ] **Step 5: Visual check via Flatpak (the repo's UI verification convention)**

Run: `./run-flatpak.sh`
Expected: open Photos, scroll the grid. The date pill appears left of the scrollbar, moves with the thumb, shows the correct Year/Month/Day date for the current mode, and fades out ~700ms after scrolling stops. Verify in all three modes and that clicking through the pill still activates the tile/scrollbar beneath.

- [ ] **Step 6: Commit**

```bash
git add src/ui/photos_page.rs
git commit -m "feat(photos): show a scroll-date pill beside the grid scrollbar

On scroll, resolve the current grid's date section and reveal a glass pill that
tracks the thumb; fade it out ~700ms after scrolling stops. Coalesced like the
mode-selector contrast update; click-through so it never blocks interaction."
```

---

### Task 5: Docs, naming reference, and full CI

**Files:**
- Modify: `docs/modules/browsing.md`
- Modify: `docs/ui-naming-reference/index.html`

- [ ] **Step 1: Document the indicator + invariant in `browsing.md`**

In `docs/modules/browsing.md`, append a new paragraph at the end of the **Behavior** section (before the **Mode Selector** heading):

```markdown
While the Photos grid is scrolled, a compact glass date pill appears just left
of the scrollbar and tracks the thumb vertically, fading out ~700ms after
scrolling stops. It shows the date section at the current scroll position in the
active mode (Year/Month/Day). The date is resolved by projecting the scrollbar
ratio × full-library live total through the already-loaded per-mode
`section_counts` (`section_model::section_for_global_offset`), NOT by reading the
realized tiles — so it stays correct in virtual-paged regions whose thumbnails
are not loaded. It is hidden when library metadata has not loaded, the library
is empty, or there is a single section. The pill is `can-target: false`
(click-through) and reuses `.glass-raised`; it is Photos-page only (album detail
pages are a follow-up).
```

- [ ] **Step 2: Add the pill to the visual naming map**

In `docs/ui-naming-reference/index.html`, add an entry for `scroll_date_revealer` / `scroll_date_label` (the scroll-date pill) matching the format of the existing entries (source of truth: `data/ui/photos-page.blp`, `src/ui/photos_page.rs`). Follow whatever row/list structure the file already uses for the mode selector and other Photos-page chrome.

- [ ] **Step 3: Run the full CI mirror**

Run:
```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments
xvfb-run -a cargo test --all
```
Expected: all three pass. (`xvfb-run` is required because the `#[gtk::test]` cases need a display, matching `.github/workflows/ci.yml`.)

- [ ] **Step 4: Commit**

```bash
git add docs/modules/browsing.md docs/ui-naming-reference/index.html
git commit -m "docs: document scroll-date pill and add it to the naming map"
```

---

## Self-Review Notes

- Spec coverage: date resolution via counts-projection (Task 1+2), pill on `grid_overlay` left of scrollbar tracking the thumb (Task 3+4), transient ~700ms fade (Task 4), mode-aware label (Task 1 `make_label_nocount` + Task 4 reads active grid), click-through `can-target` (Task 3), empty/single-section hide (Task 2 `counts.len() <= 1` + None when metadata missing), scope = Photos only (Global Constraints), tests (Task 1 pure, Task 2 gtk::test, Task 4 visual), docs + naming map (Task 5).
- Type consistency: `section_for_global_offset(&HashMap<SectionKey,u32>, u32) -> Option<SectionKey>` and `make_label_nocount(&SectionKey) -> String` (Task 1) match the call sites in Task 2 (`crate::core::section_model::section_for_global_offset(counts, offset)`) and Task 4 (`make_label_nocount(&key)`). `scroll_fraction(&self) -> f64` and `current_scroll_section_key(&self) -> Option<SectionKey>` (Task 2) match Task 4's usage. Template child ids `scroll_date_revealer` / `scroll_date_label` match across Task 3 (BLP) and Task 4 (Rust).
- Placeholder scan: no TBD/TODO; every code step shows full code. The two "match the existing pattern" notes (template-child binding style; `noop_callbacks` vs `MediaGridCallbacks::default`) are explicit fallbacks with both branches spelled out, not vague handoffs.
