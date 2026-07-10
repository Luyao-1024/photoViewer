# Album Crossfade Navigation Design

## Goal

Make opening an album, switching between albums, and returning from an album
detail page use the same crossfade-style transition as the Photos page's
Year/Month/Day mode selector instead of the current right-to-left
`Adw.NavigationView` page push animation.

## Design

Keep `Adw.NavigationView` as the outer navigation owner for viewer, search,
trash, and other page-level flows. Add a shared `Gtk.Stack` inside the root
content area for the browsing pages: the Photos page and the currently active
`AlbumDetailPage`. Configure that stack with `transition-type: crossfade` and
the same duration used by the Photos mode stack.

Album selection will load/build the detail page as it does today, then replace
the stack's visible browsing child rather than pushing a second navigation
page. Selecting another album replaces the active detail child, so only one
detail page participates in layout and the transition is a peer-to-peer
crossfade. Returning to Photos selects the Photos child. Existing viewer
navigation continues to use `Adw.NavigationView`; the viewer's navigation
target must remain the outer navigation view so opening and closing a viewer
from an album still works.

## Behavior and edge cases

- Re-selecting the visible album is a no-op.
- Album data loading and progressive backfill remain unchanged.
- The sidebar's active album state remains synchronized with the visible
  browsing child.
- Keyboard Back/Escape and navigation gestures continue to return to the
  Photos child for browsing pages and retain their current behavior for viewer
  pages.
- The stack owns the browsing transition; no new per-page animation selectors
  are introduced.

## Testing

Add focused source/UI structure coverage for the browsing stack and its
crossfade configuration, plus regression coverage that album selection swaps
the active browsing child rather than increasing the outer navigation stack.
Run the focused UI tests, formatting, and the relevant cargo test target.
