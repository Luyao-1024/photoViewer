# Day-View Tile Size Reduction — Design Spec

**Date:** 2026-06-22
**Status:** Approved (pending user spec review)
**Scope:** `MediaGrid` `GroupBy::Day` on-screen tile pixel size.

## Problem

The three Photos-page grouping modes (`GroupBy::Year`, `Month`, `Day`) are
configured in `src/ui/media_grid.rs:98-116` (`spec_for_mode`) with screen tile
sizes **90 / 180 / 360 px**. The `Day` mode at 360 px produces tiles that
visually dominate the viewport — on the default 1200 px window with a 20 %
sidebar (≈960 px content area), a 360 px tile leaves room for roughly 2.6
columns, and a single tile occupies ~37 % of the content width. The 1 : 2 : 4
ratio across the three modes also makes the jump from `Month` (180) to `Day`
(360) more aggressive than the perceptual step the user wants.

The disk thumbnail buckets (`Small=256`, `Medium=512`, `Large=1024`) and their
JPEG quality (82 / 85 / 88) remain unchanged — only the on-screen pixel size
for the Day mode is being adjusted.

## Goals

1. Reduce the `Day`-view tile from **360 × 360 px** to **270 × 270 px** on
   screen (a 25 % reduction in linear dimension, ~44 % reduction in area).
2. Keep `Year` (90 px) and `Month` (180 px) untouched.
3. Keep the `ThumbnailSize::Large` (1024 px) disk bucket for `Day`, so the
   single-photo viewer continues to load the same source texture and the
   detail at full-screen zoom is unchanged.
4. Smoother visual hierarchy across the three modes: 1 : 2 : 3 (with the
   last step 1.5 × instead of 2 ×).

## Non-Goals

- Changing `Year` or `Month` sizes — those are already in a comfortable range.
- Changing disk thumbnail bucket boundaries (`Small=256`, `Medium=512`,
  `Large=1024`) — they remain aligned with the on-screen sizes at ~0.35 for
  Year/Month, and ~0.26 for Day (down from 0.35). Day still has plenty of
  source pixels to downscale cleanly.
- Changing `TrashPage` (125 px), `AlbumDetailPage` (250 px), or `AlbumsPage`
  cover (240 px) tile sizes — these are independent of `GroupBy`.
- Changing `ViewerPage`'s `ThumbnailSize::Large` source — viewer still loads
  1024-px source regardless of grid tile size.
- Adding a runtime knob for tile size — out of scope; this is a one-line
  constant change.

## Approach

Two-line code change in `src/ui/media_grid.rs`:

```diff
-//! - Day   → 360×360 px (thumbnail bucket Large / 1024)
+//! - Day   → 270×270 px (thumbnail bucket Large / 1024)
```

```diff
 GroupBy::Day => ViewSpec {
-    pixel_size: 360,
+    pixel_size: 270,
     thumb_size: ThumbnailSize::Large,
 },
```

`spec.pixel_size` flows only into `SquareTile::set_target(spec.pixel_size)` in
`build_photo_picture` (`src/ui/media_grid.rs:377`), which drives the
`set_size_request(target, target)` on the cell. No other consumers exist (grep
for `360` / `pixel_size` confirms scope).

## Design Properties

### Screen ↔ disk bucket utilization

| Mode | Screen px | Bucket px | screen / bucket |
|---|---|---|---|
| Year  |  90 |  256 (Small)  | 0.35 |
| Month | 180 |  512 (Medium) | 0.35 |
| Day   | 270 | 1024 (Large)  | 0.26 |

Day's bucket ratio drops from 0.35 → 0.26. On a 2 × HiDPI display, 270 CSS px
= 540 device px downsampled from 1024 — still ~1.9 × oversampled, well above
the Nyquist limit for clean rendering. Single-photo zoom in `ViewerPage` still
loads the full 1024 px source and is unaffected.

### Visual hierarchy

| Mode | Tile side | Step ratio | Tile count per 960 px content row |
|---|---|---|---|
| Year  |  90 | — | ~10 columns |
| Month | 180 | 2.0 × | ~5 columns |
| Day   | 270 | **1.5 ×** (was 2.0 ×) | ~3.5 columns (was ~2.6) |

The Day tile is now 3 × Year (area: 9 ×) instead of 4 × (area: 16 ×). The
hierarchy is still clearly readable — Day is meaningfully larger than Month —
but the Day grid no longer feels like single-tile review.

## Verification

1. **Build:** `cargo build` succeeds with no new warnings.
2. **Visual smoke test:**
   - Switch to "日" view; confirm tiles are noticeably smaller than before.
   - At default window size, ~3.5 Day tiles fit per row.
   - Year / Month views look identical to the prior build.
3. **Click-through:** Clicking a Day-view tile still opens `ViewerPage` and
   loads the `Large` (1024 px) thumbnail — single-photo zoom unchanged.
4. **Regression check:** `TrashPage` (125 px) and `AlbumDetailPage` (250 px)
   tiles are visually identical to the prior build.

## Risk

Low. The change touches a single constant and its doc-comment. The only
runtime effect is the cell allocation size requested from GTK; no layout
calculations, no logic, no persisted state.

## Files Touched

- `src/ui/media_grid.rs` — 1 line of code + 1 doc-comment line.