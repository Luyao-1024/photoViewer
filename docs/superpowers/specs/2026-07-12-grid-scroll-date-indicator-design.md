# Grid Scroll Date Indicator Design

## Goal

While the user scrolls the Photos media grid, show a compact date pill just to
the left of the scrollbar that reports the date section the current scroll
position corresponds to. The pill tracks the scrollbar thumb vertically,
appears only while scrolling, and fades out shortly after scrolling stops. It
must stay correct even when the user fast-scrolls into a region whose
thumbnails are not yet realized (virtual paging keeps only a bounded window of
tiles in memory).

Keep GTK's native scrollbar; do not replace it.

## Non-goals

- No custom scrollbar widget, no drag-to-jump scrubber. The pill is read-only
  display chrome; normal scrolling drives it.
- No change to album detail pages in this revision (their `grid_overlay` has
  the same shape and is a natural follow-up, but out of scope here).
- No change to embedded preview grids (search results / album picker) that
  disable internal scrolling via `set_content_sized_scroll`.

## Design

### Date resolution: project from already-loaded section counts

The scrollbar thumb represents the *entire* live library, not just the
realized tile window: virtual paging realizes only a bounded page (~500) while
top/bottom spacer widgets inflate the scroll range to the full collection.
Therefore a scroll position can fall in a region with no realized tiles, so
reading the top visible tile's `section_key` is insufficient (it goes stale or
missing during a fast drag / in-flight page swap).

Instead, resolve the date from data that already covers the whole library:

- `MediaRepository::section_counts(mode)` is already loaded in the background
  and applied authoritatively to section labels (see `docs/modules/browsing.md`
  "section counts"). It gives the full-library photo count per Year/Month/Day
  section.
- Build a cumulative-offset table over those counts, newest section first:
  section 0 covers global offset `[0, c0)`, section 1 covers `[c0, c0+c1)`,
  and so on.
- The scroll position gives a global offset directly via the existing
  `scroll_ratio_from_adjustment_value` (`src/ui/media_grid.rs:515`) times the
  authoritative full-library live count (the sidebar total, not the realized
  window size).
- The section whose `[start, end)` contains that offset is the current date;
  format it with a no-count variant of `section_model::make_label`.

This is correct everywhere — including unloaded regions — because it depends
only on DB-level counts and the scroll ratio, never on which tiles happen to
be realized. Pixel precision is section-granular (spacer heights are
approximations), which is fine for a date label.

Implementation notes:
- The cumulative table must be ordered the same as the grid (newest-first,
  descending `sort_datetime`). If `section_counts` does not guarantee order,
  sort its sections by date descending before building the table.
- Rebuild the table whenever `section_counts` is (re)loaded for the active
  mode — same lifecycle as the existing authoritative-count application.
- Guard against zero total count (empty library): show no pill.

### Where the pill lives

Reuse `PhotosPage`'s existing `grid_overlay` (`Gtk.Overlay` in
`data/ui/photos-page.blp:107`, which already hosts the floating mode selector
and the right-click menu). Add one `[overlay]` child:

- `Gtk.Revealer` (`transition-type: crossfade`) wrapping a `Gtk.Label`.
- `halign: end; valign: start`; `margin-end` ~= scrollbar width + small gap so
  the pill sits immediately left of the scrollbar.
- Styling reuses the shared `.glass-raised` pill classes so it matches both
  Liquid Glass and plain translucent modes (UI invariants: reuse `.glass-*`
  before adding selectors; any glass surface must work in both modes).
- One pill serves all three grids (Year/Month/Day): only one is visible in the
  stack at a time, and the pill reads the active grid's mode/counts.

### Vertical tracking

The pill's vertical position tracks the thumb. The thumb's travel fraction is
`value / (upper - page_size)` (the same ratio GTK uses to place the thumb).
Drive `margin-top` from it on each `value_changed`:
`margin-top = top_margin + fraction * (overlay_height - pill_height -
top_margin - bottom_margin)`, clamped to `[top_margin, overlay_height -
pill_height - bottom_margin]`, so the pill moves with the thumb and stays
inside the overlay at the extremes. No CSS transition on the margin (it tracks
input 1:1, like the thumb itself); the fade in/out is CSS on opacity via the
`Revealer`. Use GTK4 `can-target: false` on the Revealer/Label so the pill is
click-through and never steals pointer events from tiles or the scrollbar.

### What drives it

Hook into the existing `MediaGrid::connect_view_changed`
(`src/ui/media_grid.rs:798`), which `PhotosPage` already uses to refresh the
mode selector contrast. On each scroll event:

1. Recompute `margin-top`, update the label text, `set_reveal_child(true)`.
2. Reset a debounce timer (mirroring `schedule_mode_selector_contrast_update`
   at `src/ui/photos_page.rs:944`).
3. When the timer fires after ~700 ms with no further scroll events,
   `set_reveal_child(false)` to fade out.

Use the existing per-grid generation counter so callbacks from a stale
mode/rebuild are discarded.

### Label text (mode-aware)

Year mode shows `2026年`; Month shows `2026年7月`; Day shows `2026年7月12日`.
Derived from the resolved `SectionKey` via a count-free `make_label` variant
(or the existing label with the `· N 项` suffix stripped).

## Behavior and edge cases

- Empty library or a single section: the pill is not shown (nothing
  meaningful to indicate).
- Fast drag into an unloaded region: the counts projection keeps the date
  correct while the page swap is in flight; when the page lands and tiles
  realize, the label is already right (no flicker correction needed).
- Mode switch mid-scroll: the generation counter discards stale callbacks;
  the table is rebuilt for the new mode before the next update.
- Top/bottom extremes: `margin-top` is clamped so the pill never overflows the
  overlay.
- Embedded preview grids (internal scrolling disabled) are untouched.
- The pill must not capture pointer events; `can-target: false` (see Vertical
  tracking) keeps it click-through so tiles and the scrollbar underneath stay
  interactive.

## Testing

- Unit test the pure offset→section projection: given a vector of per-section
  counts, assert that representative offsets (including exact section
  boundaries `0`, `c0`, `c0+c1`, the last index, and mid-section values) map
  to the expected section, and that a zero-total input yields no section.
- Unit test ordering: feed unsorted `section_counts` and assert the table is
  built newest-first.
- One `#[gtk::test]` (run under `xvfb-run -a cargo test --all` to match CI):
  drive the grid's `vadjustment` to several positions, assert the pill label
  updates to the matching section date, then assert it hides after the idle
  timeout.
- Run the full CI mirror before push: `cargo fmt --all --check`,
  `cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A
  clippy::too_many_arguments`, and `xvfb-run -a cargo test --all`.

## Files touched (expected)

- `data/ui/photos-page.blp` — add the `Revealer`+`Label` overlay child to
  `grid_overlay`.
- `src/ui/photos_page.rs` — template child, scroll hook wiring, debounce,
  position/label update logic.
- `src/core/section_model.rs` (or a small new helper) — cumulative-offset →
  section projection + count-free label variant.
- `data/css/*` — reuse `.glass-raised`; add only what the pill needs.
- `docs/modules/browsing.md` — document the indicator and the counts-projection
  invariant (a scroll position maps to a date via full-library section counts,
  not via realized tiles).
- `docs/ui-naming-reference/index.html` — add the new pill widget id to the
  visual naming map (CLAUDE.md requires this when adding a UI widget).
