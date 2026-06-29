# UI Design Reference

This document describes the user-facing screens and shared visual rules for
future UI work. It complements the behavior-focused module docs: read this file
for layout, visual hierarchy, and interaction intent, then read the matching
module document before changing code.

## Scope

The app is a GNOME desktop photo manager. Its UI should feel quiet, direct, and
content-first: photos and videos are the main surface, while controls sit as
lightweight glass chrome. Prefer standard GTK/Libadwaita widgets, existing
Blueprint templates, and the shared `.glass-*` CSS system over custom drawing.

Key implementation locations:

| Area | Files |
|---|---|
| Window shell and sidebar | `data/ui/window.blp`, `src/ui/window.rs` |
| Photos browsing | `data/ui/photos-page.blp`, `data/ui/media-grid.blp`, `src/ui/photos_page.rs`, `src/ui/media_grid.rs` |
| Mode selector | `data/ui/mode-selector.blp`, `src/ui/mode_selector.rs` |
| Tiles and section headers | `data/ui/photo-tile.blp`, `data/ui/section-header.blp`, `src/ui/photo_tile.rs`, `src/ui/section_header.rs` |
| Viewer | `data/ui/viewer-page.blp`, `src/ui/viewer_page.rs` |
| Editor panel | `data/ui/editor-panel.blp`, `src/ui/editor_panel.rs` |
| Album detail | `data/ui/album-detail-page.blp`, `src/ui/album_detail_page.rs` |
| Trash | `data/ui/trash-page.blp`, `src/ui/trash_page.rs` |
| Shared material | `src/ui/grid_css.rs`, `docs/modules/ui-liquid-glass.md` |

## Global Design Principles

- Content owns the screen. Browsing grids, the active media item, and album or
  trash collections should occupy the largest continuous region.
- Chrome floats or frames content without competing with it. Header bars,
  sidebars, panels, toolbar buttons, and popovers should use shared glass
  classes when they sit above visual content.
- Keep geometry stable. Sidebar width, viewer panel width, toolbar button size,
  thumbnail strip height, and segmented-control dimensions should not shift
  when content loads, selection changes, or panels open.
- Use icons for compact actions. Prefer named symbolic icons and tooltips for
  toolbar actions; reserve text labels for destructive confirmations, banners,
  empty states, and footer actions where clarity matters.
- Maintain a usable non-Liquid fallback. Any new glass surface must work in both
  Liquid Glass and plain translucent modes.

## Window Shell And Sidebar

The main window uses an `Adw.OverlaySplitView` with a fixed 240px sidebar and a
content `Adw.NavigationView`. The sidebar is the primary app navigation, not a
secondary filter drawer.

Design intent:

- The sidebar is a stable left rail with glass material. It must not shrink or
  expand when viewer pages, side panels, or album pages are pushed.
- Top-level rows are Photos, Albums group header, and Trash.
- Settings is exposed as a fixed gear button at the sidebar footer and opens a
  popup dialog (rather than a navigation row/page).
- The sidebar uses a single material owner: `.glass-sidebar-surface.glass-base`
  wraps both the navigation list and the settings footer. The `Gtk.ListBox`,
  rows, and footer stay transparent/layout-only. Do not put `glass-base` on
  the list and footer separately; two independently painted glass surfaces
  create a visible color break at the settings button.
- The Albums header is a collapsible group control. Album rows appear in a
  dedicated bounded scroll region directly under it, including virtual albums
  such as Favorites, Photos, and Videos. The main sidebar list itself remains
  limited to Photos, Albums, and Trash.
- Album rows use indentation, icons, and right-aligned counts to communicate
  hierarchy without adding extra panels.
- Drag-to-reorder album rows should feel like a subtle in-place list operation:
  dim the dragged row and show an above/below drop cue without changing the
  sidebar's width or row height.

When changing navigation, keep the row-to-target mapping in `src/ui/window.rs`
as the source of truth and avoid hardcoded list indices.

## Photos Page

The Photos page is the root content page. It combines a top header, a stacked
Year/Month/Day grid, and a floating mode selector.

Design intent:

- The page opens directly into media browsing. Avoid landing or instructional
  content when media exists.
- The `Adw.HeaderBar` is glass chrome. Its selection actions appear only when
  the user has selected media, keeping normal browsing visually calm.
- Selection mode should make batch actions discoverable without permanently
  occupying header space. Add-to-album, favorite, and trash actions belong in
  the header because they apply to the selected set.
- The batch-action toolbar is split across the header: select-all (text label,
  toggling 全选 / 取消全选) stays on the left (`[start]`); the icon-only batch
  actions live on the right (`[end]`) — add-to-album (`list-add-symbolic`, the
  `+`), favorite (`emblem-favorite-symbolic`), and trash (`user-trash-symbolic`).
  All use always-on glass (the shared `.glass-toolbar-button` material): the
  capsule is present at rest so the action set stands out when it appears in
  selection mode. Tooltips carry the action labels. See
  [Toolbar Button Glass States](#toolbar-button-glass-states).
- Favorite is a single heart button that acts as a smart toggle, mirroring the
  viewer's single-item favorite: if every selected photo is favorited, the heart
  turns translucent red (`.viewer-favorite-btn.favorite-active`, identical to the
  viewer) and clicking unfavorites all; if none are favorited, clicking favorites
  all; only a mixed selection (some favorited, some not) opens the `glass-menu`
  popover with 收藏 / 取消收藏. Do not split favorite/unfavorite back into two
  header buttons. (The per-tile right-click context menu keeps its own separate
  favorite/unfavorite entries.)
- The grid area should extend behind the floating selector. Do not add fixed
  bottom padding or a dark reserved band for the selector.

Empty or loading states should use simple status-page style messaging and should
not introduce a separate visual language from the rest of the app.

## Media Grids And Tiles

`MediaGrid` is a vertical scroll surface made from date sections. Each section
has a full-width header followed by a grid of square thumbnail tiles.

Design intent:

- Section headers give time structure; thumbnails remain the visual focus.
- Headers stay outside the `GridView` so they span the full width and do not
  distort tile rows.
- Year, Month, and Day views use the same media store but different grouping and
  tile sizing. Day view is the densest layout and needs separate visual checks.
- Photo and video tiles should feel like one collection. Video-specific marks
  should be lightweight and must not change tile dimensions.
- In Day view, favorited media uses a white heart at the tile's top-right.
  Dynamic photos use a bottom-left playback glyph; ordinary videos use a
  bottom-left duration badge from persisted metadata.
- Tile hover, selection, and focus states should be visible but restrained.
  Avoid large opaque overlays that obscure thumbnail content.
- Selection is shown primarily by a translucent-white checkmark
  (`object-select-symbolic`) pinned to the tile's bottom-right corner, plus a
  softer secondary glass border. The checkmark widget (`SquareTile`'s
  `.thumb-checkmark` child) is always present but invisible (CSS `opacity: 0`)
  and revealed via `flowboxchild:selected .thumb-checkmark { opacity: 1 }`, so
  it tracks the FlowBox selection automatically. Keep this as the canonical
  grid selection affordance.

## Year/Month/Day Mode Selector

The mode selector is the canonical Liquid Glass segmented control. It floats at
the bottom center of the Photos page.

Design intent:

- One raised glass capsule contains all segments.
- Segments are equal width and use label contrast plus a short bottom indicator
  for active state.
- There is no per-segment active background block.
- The control is navigation chrome over content, so it should remain compact and
  stable while the grid underneath scrolls.

Reusable classes and material rules are documented in
[`ui-liquid-glass.md`](ui-liquid-glass.md). Any new segmented control should
start from this visual model unless there is a specific product reason not to.

## Album Detail Page

Album detail pages reuse the browsing grid language inside a dedicated
navigation page.

Design intent:

- The album title belongs in the page/header context, while the content area is
  still a media grid.
- Album detail should feel like a scoped version of Photos, not a separate app
  mode.
- Keep album filtering invisible to the user: the page should show the album's
  media and preserve the same activation, selection, tile, and empty-state
  expectations as the main Photos page.
- Virtual albums and folder albums should share the same page treatment.

When changing album UI, verify both folder-derived albums and virtual albums
because their data sources differ even though the screen design is shared.

## Viewer Page

The viewer is a focused media stage pushed inside the existing navigation view.
It supports images and videos, plus overlay chrome for actions, navigation,
details, and editing.

Design intent:

- The active image or video is the page's visual center. It should be contained,
  correctly oriented, and never force the app window or sidebar to grow.
- The top header contains item actions, left-to-right: favorite, edit, delete
  (trash), and details. (Album assignment is reached from the photos grid batch
  menu, not the viewer.) These actions stay compact and icon-led, and use
  hover-only glass (scoped via `.viewer-chrome`): bare at rest, glass on hover.
- Previous/next controls are viewer chrome. In the current design they sit as a
  compact bare pair in the bottom-right corner of the stage — no capsule
  container, each button draws its own glass surface only on hover/focus —
  minimizing coverage of the media.
- The bottom filmstrip is a secondary navigation aid. It should stay low,
  bounded, horizontally scrollable, and centered on the active item after layout.
- The built-in `GtkVideo` controls own video playback and seeking. Do not add a
  duplicate progress control above the filmstrip.
- When editing is active, visible viewer navigation controls are hidden so the
  editing task does not compete with browsing controls.

The viewer must preserve the main sidebar's stable width. Overlay panels should
not become layout that squeezes or reallocates the media stage unexpectedly.

## Details Panel

The details panel is an overlay sidebar on the viewer, presented as translucent
glass above the media stage.

Design intent:

- The panel is inspection chrome, not a separate page. It should be easy to open
  and close while keeping the media visible behind it.
- The header contains a concise title and close button. Metadata rows live in an
  `Adw.PreferencesPage` so they remain scannable and platform-native.
- Photo and video metadata share the same visual row model. Dynamic rows may
  differ by media kind, but the panel should not change width or material.
- Hidden panel content must not continue participating in layout in a way that
  produces negative allocation warnings.

Because the panel uses Libadwaita preferences widgets inside glass, CSS must
keep internal boxed-list and preferences backgrounds transparent.

## Editor Panel And Crop Overlay

The editor is a right-side panel opened from the viewer for image media only.
Videos are view-only and must keep edit entry disabled.

Design intent:

- Editing happens in context: the image preview remains on the viewer stage,
  while controls live in the side panel.
- The editor header contains the panel title, reset, and close. Reset is a
  circular icon button and is enabled only when there are pending edits.
- Rotate controls are a compact horizontal group. Adjustment controls use
  standard scales in preference rows. Crop controls include a visual ratio
  selector and a compact crop action.
- Footer actions are always available at the bottom: Cancel, Save Copy, and Save
  Overwrite. Save Copy is the suggested action; Save Overwrite is destructive
  and still requires confirmation.
- Crop selection is drawn directly over the image with `GtkDrawingArea`, not in
  the side panel. The rectangle should remain visibly selected during drag or
  resize and should update without forcing full preview rendering on every move.

The editor panel width should stay within the viewer's side-panel range. Avoid
adding controls that require a wider panel unless the whole viewer layout is
reconsidered.

## Trash Page

Trash is a reversible recovery surface with an explicit permanent-delete path.

Design intent:

- The page has a glass header with an Empty Trash action, a banner explaining
  trash behavior, a thumbnail grid, and an action bar for multi-selection.
- Restore is the primary positive action for selected trashed media.
- Permanent delete and Empty Trash are destructive and should use destructive
  styling plus confirmation where implemented.
- Selection state should be obvious and should reveal the action bar without
  resizing the grid unexpectedly.
- Empty trash state should clearly communicate that there are no items to
  restore, not that the app has no library.

Trash UI must stay aligned with system trash reconciliation. Avoid designs that
imply app-local deletion if the operation is actually backed by host trash.

## Settings

Settings is reached from the sidebar footer gear button and currently owns user-facing visual
preferences such as the Liquid Glass toggle. It opens as a popup dialog; while
the dialog is visible, the gallery content behind it is dimmed/blurred in the
Liquid Glass mode so the modal layer reads clearly above the library.

Design intent:

- Settings should be quiet and preference-oriented, using standard Libadwaita
  rows and switches.
- The Liquid Glass setting changes the visual material live. It should affect
  the full chrome language consistently: sidebar, headers, toolbar buttons,
  menus, panels, and segmented controls.
- Keep static software information such as app name, version, author, and
  license as compact small footer text at the bottom of the settings dialog.
- Settings should not become a general-purpose page for operational actions
  such as scan, restore, or album management unless the information architecture
  is revisited.
- Thumbnail generation speed belongs in Settings as a quiet storage/runtime
  preference. It should stay a simple three-option control for slow, normal, and
  fast background generation rather than exposing raw worker counts.
- Settings that cannot apply live should show the shared restart-required
  confirmation after a successful save. Choosing yes relaunches the current
  executable and quits the current process; choosing no leaves the setting saved
  for the next manual restart.

When adding settings, prefer clear row labels and standard controls over custom
compact chrome.

## Shared Liquid Glass Material

Liquid Glass is the app's shared chrome material, with a plain translucent mode
as the fallback.

Design intent:

- Use `.glass-base`, `.glass-raised`, `.glass-header`, `.glass-menu`,
  `.glass-alert-dialog`, `.viewer-details-panel`, and `.glass-segmented` before
  adding new selectors.
- Shape and sizing rules belong in base CSS. Material-specific blur, saturation,
  highlights, and shadows belong in the Liquid or Plain material blocks.
- New glass surfaces must have matching Liquid and Plain definitions.
- Avoid opaque child backgrounds inside floating glass panels.
- When a visual area contains multiple child widgets that should read as one
  surface, put the material class on the shared parent and reset child
  backgrounds to transparent. The sidebar/settings-footer color mismatch was
  caused by `Gtk.ListBox` painting over the shared parent while the footer
  exposed a separate surface.
- Verify important material changes through the Flatpak GNOME runtime when
  backdrop-filter behavior matters.

## Toolbar Button Glass States

Toolbar buttons share the `.glass-toolbar-button` material, but their **rest
state depends on where they live**. This is a standing rule — new buttons
inherit the state of their container, and the shared always-on
`.glass-toolbar-button` rule must never be loosened to change one group.

**Hover-only glass** — bare icon/text at rest; the glass capsule appears only on
`:hover` / `:focus-visible` (and deepens on `:active`). Use this for chrome that
floats over content where a calm rest state matters:

- Viewer header actions — `.viewer-chrome` (favorite, edit, delete, details).
- Viewer previous/next overlay arrows — `.viewer-overlay-nav-btn` (bottom-right).
- Sidebar footer settings button — `.sidebar-settings-button`.

**Always-on glass** — the glass capsule is present at rest. Use this inside
dedicated sub-window surfaces where a persistent affordance reads as part of
the surface: the editor panel, the details panel, the settings dialog, the
album picker, and alert dialogs. The Photos page batch-action toolbar (which
only appears in selection mode) is also always-on so the action set stands out.

Rule of thumb: **a button on the main window stage that overlays media/photo
content is hover-only; a button inside a dialog or floating side panel is
always-on.** Each hover-only group is scoped by a dedicated class (listed
above). When adding a hover-only group, define the bare-at-rest reset plus the
hover/focus/active material in **both** the Liquid and Plain material blocks
(mirroring `.viewer-chrome`), keep any `glass-toolbar-danger` red hover
override, and extend the `grid_css` tests.

## Design Checklist For UI Changes

Before editing UI code:

1. Read this document and the matching module doc.
2. Edit `data/ui/*.blp` templates, not generated `.ui` files.
3. Reuse existing GTK widgets, icons, CSS classes, and sizing patterns.
4. Check whether the change affects both Liquid Glass and Plain modes.
5. Check whether hidden widgets still participate in layout.
6. Add or update focused UI/template/CSS tests when changing contracts.
7. Run the smallest relevant test first, then broaden when shared behavior is
   affected.
