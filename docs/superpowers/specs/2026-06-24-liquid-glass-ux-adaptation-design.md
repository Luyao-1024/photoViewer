# Liquid glass UX adaptation design

## Context

The app now has a GTK 4.22+ liquid glass mode selector using native CSS `backdrop-filter`. The effect works visually, but the surrounding UI still uses mostly flat GNOME/libadwaita surfaces. In the current screen, the bottom mode selector has a bright translucent glass material, while the sidebar, header bar, window controls, thumbnail selection, and content surfaces remain visually heavier and flatter.

The next adaptation should not redesign the product structure, but it must fix layout problems that make the interface feel unfinished or inconsistent. It should make the existing photo viewer feel like one coherent glass-oriented interface while preserving clarity, navigation speed, and GNOME platform expectations.

## Design goal

Unify the app around a restrained liquid glass visual language:

- Keep the current layout and information architecture.
- Repair layout issues that weaken hierarchy, spacing, alignment, or content visibility.
- Promote glass as the main foreground material for floating controls and navigation surfaces.
- Avoid turning every surface into glass; use solid dark surfaces where readability or depth requires it.
- Make selected, hovered, and active states feel related to the mode selector.
- Keep the style compatible with GTK 4.22+ Flatpak runtime behavior.

## Visual diagnosis from the current screen

The main mismatch comes from material contrast, not layout:

- The mode selector is glossy, rounded, bright, and translucent.
- The sidebar is a large opaque dark slab with no shared border, blur, or highlight language.
- The top header bar is flat and dark, while the centered title has no relationship to the glass system.
- Window controls remain default opaque circular controls.
- Thumbnail selected states use sharp blue outlines, which feel like debugger focus rings compared with the softer glass controls.
- Grid separators and black gutters create strong hard edges that compete with the glass material.
- The bottom mode selector floats over content without an explicit content-safe zone, so it can visually collide with thumbnails near the bottom.
- The sidebar, header, and grid do not share a clear alignment system, making the left navigation feel detached from the content canvas.
- The grid gutters are visually too hard for a glass-oriented interface; the content area needs calmer spacing and card rhythm.
- Page title hierarchy is weak: the centered title does not explain the relationship between sidebar navigation, current view, and content grid.

## Design principles

### 1. Glass is foreground, not wallpaper

Use liquid glass for interactive floating surfaces:

- mode selector
- sidebar navigation container
- header bar controls
- compact floating toolbars
- right-click context menus
- selected thumbnail overlays
- transient popovers and menus

Avoid full-window blur or heavy background distortion across the entire grid. The photo grid should remain the content layer.

### 2. One material stack

Define three reusable material levels:

- `glass-base`: subtle translucent panel for sidebar and header surfaces.
- `glass-raised`: stronger blur, brighter border, and shadow for floating controls.
- `glass-selected`: active state with extra edge light and a soft inner tint.

All glass UI should use the same ingredients: translucent fill, backdrop blur, border highlight, inset highlight, and shadow.

### 3. Soft selection, not hard outlines

Replace sharp blue selection borders with glass-compatible selection styling:

- Use a 1px to 2px luminous border with lower saturation.
- Add a soft inner overlay instead of only an external outline.
- Use a small corner radius that matches thumbnail cards.
- Keep keyboard focus visible, but separate focus from selection.

### 4. Preserve photo readability

Glass must never obscure the actual photo content. Any glass overlay on thumbnails should be subtle and localized. Text labels need stable contrast independent of image brightness.

### 5. Platform-compatible restraint

The app should still feel like a GNOME app. The target is not macOS cloning. The adaptation should use GNOME spacing, typography, and navigation behavior, with glass as a material treatment.

### 6. Layout first, material second

Liquid glass should reinforce a good layout, not hide layout problems. Before applying glass to a surface, confirm that its position, size, spacing, and relationship to nearby content are correct.

## Layout adaptation requirements

### Sidebar

The sidebar should become a deliberate navigation rail instead of a large flat block.

Required fixes:

- Define a stable sidebar width that feels intentional on desktop.
- Align sidebar item padding, icon/text rhythm if icons are introduced, and active state geometry.
- Avoid a full-width hard blue active row; use a rounded glass-selected item state.
- Consider a slightly inset rail with outer margin if the full-height pane continues to feel too heavy.
- Keep enough contrast for Chinese labels and future localized strings.

### Header bar

The header should clarify the current view and align with the content canvas.

Required fixes:

- Decide whether the page title is centered in the full window or centered in the content area; avoid ambiguous centering.
- Align header height, title baseline, and action button positions with the sidebar/content split.
- Use a quieter glass-base material than floating controls.
- Ensure window control padding does not visually collide with the title area.

### Photo grid

The grid should be the main content layer and should not look like hard tiled debug panels.

Required fixes:

- Replace overly hard black gutters with consistent app background and controlled grid gaps.
- Add enough card spacing so thumbnails breathe without wasting screen density.
- Keep card edges consistent with selection radius.
- Ensure selected cards do not resize or shift layout.
- Maintain scroll performance and avoid per-card backdrop blur.

### Bottom mode selector

The mode selector should remain a floating control but needs layout protection.

Required fixes:

- Add bottom scroll padding or content inset so the selector does not cover meaningful thumbnail content.
- Keep the selector centered relative to the content canvas, not necessarily the whole window, if the sidebar remains visible.
- Define responsive behavior for narrow windows: compact labels, reduced spacing, or docked placement.
- Ensure the selected segment indicator is visually aligned and not clipped by the rounded container.

### Responsive behavior

The glass adaptation must define behavior for at least three widths:

- Wide desktop: persistent sidebar, centered content controls, full grid density.
- Medium window: sidebar remains but spacing tightens; bottom selector keeps safe margins.
- Narrow window: sidebar should collapse, overlay, or switch to a navigation drawer before content becomes cramped.

## Proposed adaptation phases

### Phase 1: Create shared glass tokens

Add reusable CSS variables/classes for glass materials in `src/ui/grid_css.rs` or a dedicated UI CSS module.

Recommended tokens:

- panel fill alpha
- raised fill alpha
- border highlight alpha
- shadow color and elevation
- blur radius
- saturation multiplier
- active tint
- selected border color

Expected outcome: later UI changes do not hard-code separate glass values per widget.

### Phase 2: Repair layout structure

Before extending material treatment, fix the layout rules that determine alignment and spacing.

Design direction:

- Establish the sidebar/content/header alignment grid.
- Define content safe insets for floating controls.
- Normalize grid gaps, card radius, and card spacing.
- Decide whether the bottom selector centers in the whole window or content region.
- Document responsive breakpoints for sidebar and bottom selector behavior.

Expected outcome: the UI feels structurally coherent even before glass styling is applied.

### Phase 3: Adapt sidebar material

Convert the left sidebar from a flat opaque block into a restrained glass navigation rail.

Design direction:

- Keep the sidebar dark enough for text contrast.
- Add low-alpha translucent fill.
- Add backdrop blur and subtle right border highlight.
- Give selected navigation item the same glass-selected treatment as the mode selector.
- Add hover state with soft fill increase, not a bright rectangle.

The sidebar should feel like a persistent glass pane, not a floating pill.

### Phase 4: Adapt header bar

Make the header bar visually compatible with the glass system.

Design direction:

- Use a low-alpha glass-base material on the header background.
- Add a subtle bottom border highlight or shadow to separate it from content.
- Keep the centered title quiet and readable.
- Tune window control buttons so hover and active states use glass-like fills instead of default solid circles where possible.

The header should stay calmer than the mode selector because it spans the full window width.

### Phase 5: Adapt thumbnail cards and selected state

Replace the current sharp selected outline with a glass-compatible card state.

Design direction:

- Add subtle radius to media cards if not already present.
- On hover, add a faint glass veil and soft shadow.
- On selection, use luminous border plus subtle inner overlay.
- Keep the selection visible on dark and bright thumbnails.
- Do not blur the photo itself by default; blur should belong to UI chrome, not content.

This is likely the most important phase for visual cohesion because the screenshot shows blue selection outlines as the strongest mismatch.

### Phase 6: Adapt popovers, menus, and small controls

Apply glass-raised to transient UI:

- context menus
- right-click action menus on thumbnails and viewer content
- import/action popovers
- album picker surfaces
- floating action buttons or compact toolbars

These surfaces should share the bottom selector's border, shadow, blur, and rounded language.

Right-click menus need their own state treatment:

- Menu container uses `glass-raised` with enough opacity for text readability.
- Menu items use soft hover and pressed fills, not default flat dark rows.
- Destructive actions such as delete or move to trash use a restrained danger accent that still belongs to the glass system.
- Disabled items remain legible but clearly unavailable.
- Keyboard focus is visible and distinct from hover.
- Separators should be low-contrast hairlines, not hard blocks.
- Menu radius and padding should match the app's glass control scale.

### Phase 7: Add state matrix and accessibility checks

Define states before implementation:

- normal
- hover
- pressed
- selected
- keyboard focus
- disabled
- on bright content
- on dark content

Accessibility requirements:

- Text contrast must remain readable on mixed thumbnails.
- Keyboard focus must remain visibly distinct from selection.
- Reduced transparency or high-contrast environments should degrade to a stable opaque surface.
- Motion should be minimal; material changes should not distract during grid scrolling.

## Recommended implementation order

1. Add shared glass CSS classes and document their intended use.
2. Repair layout structure: sidebar/content alignment, grid gap rhythm, and bottom overlay safe inset.
3. Refactor the existing mode selector CSS to use the shared material classes.
4. Adapt selected thumbnail state because it is the largest visual mismatch.
5. Adapt sidebar selected and hover states.
6. Adapt header bar and window-control-adjacent button states.
7. Adapt popovers and secondary controls.
8. Adapt right-click context menus and their action states.
9. Add a fallback path for older or disabled transparency environments if needed.

## CSS class direction

Suggested class names:

```css
.glass-base { }
.glass-raised { }
.glass-selected { }
.glass-hoverable { }
.glass-on-light { }
.glass-on-dark { }
```

Widget-specific selectors should compose these classes rather than redefining the full material each time.

## Acceptance criteria

The adaptation is successful when:

- The sidebar, header, selection state, and bottom mode selector feel like the same design system.
- Layout alignment feels intentional before considering material effects.
- The bottom selector no longer hides important thumbnail content.
- Grid spacing and gutters look like a designed canvas, not debug separators.
- The photo grid remains the primary visual focus.
- No major control looks like an unrelated flat/default GTK control next to the liquid glass selector.
- Right-click menus, popovers, and toolbar buttons use the same glass action language.
- Selection remains clearer than hover.
- Keyboard focus remains clearer than selection.
- The app still works through the Flatpak GTK 4.22+ runtime without requiring host GTK upgrades.

## Non-goals

- No layout redesign.
- No information architecture redesign.
- No macOS-style traffic-light window controls.
- No compositor-level desktop blur.
- No full-screen blur over photo content.
- No custom rendering pipeline unless GTK CSS cannot express a required state.

## Open design decisions

The following choices should be made before implementation:

- Whether the sidebar should be a full-height glass pane or a slightly inset rounded rail.
- Whether selected thumbnails should use a blue-tinted glass accent or a neutral white highlight.
- Whether the header bar should be glass across the full width or only around the title/actions.
- Whether to provide an app preference for reduced transparency, or rely on system accessibility settings first.

## Screenshot audit: 2026-06-24

Evidence reviewed:

- Image #1: photo grid with batch action buttons and bottom mode selector.
- Image #2: viewer/detail page with a nested photo grid visible inside the image content.

### Step 1: Photo grid and batch selection state

General health: structurally usable, visually inconsistent.

Findings:

- The left sidebar is too visually heavy compared with the content. It reads as an opaque slab, while the bottom mode selector reads as a high-polish glass component. This makes the UI feel like two unrelated systems.
- Batch action buttons at the top left use default dark pill styling. They do not share the glass material, border highlight, or elevation of the mode selector.
- The selected thumbnail uses a hard bright blue rectangular outline. It is clear, but it looks like a debug/focus outline rather than a designed selection state.
- The grid edges and gutters are too sharp. The current vertical and horizontal divisions make the content look tiled and technical, which conflicts with the softer liquid glass direction.
- The bottom mode selector overlaps the visual field of the bottom row. It needs a content safe inset so it never covers meaningful thumbnails or labels.
- The mode selector appears centered in the full window, but the content canvas starts after the sidebar. This creates a subtle alignment mismatch. It should either center relative to the content area or use a deliberate full-window alignment rule.
- The header title is centered in the full window while operational actions sit on the content side. The title alignment should be resolved with the sidebar/content split.

Accessibility risks:

- The blue selected outline is visible, but it may be confused with keyboard focus. Selection and focus need separate states.
- Low-contrast disabled-looking batch buttons may be hard to read if their active/disabled state is not explicit.
- The bottom selector over bright thumbnails can lose edge definition without a stable safe area and contrast treatment.

Required fixes:

- Create a glass-compatible batch action toolbar using `glass-raised` or a calmer `glass-base` variant.
- Replace hard blue thumbnail selection with `glass-selected`: luminous border, subtle inner veil, and separate keyboard focus ring.
- Apply the same action vocabulary to right-click thumbnail menus: normal, hover, pressed, destructive, disabled, and keyboard focus states.
- Add bottom padding to the scroll/content area equal to the mode selector height plus margin.
- Normalize grid gaps and background so the grid feels like a canvas, not a set of hard panels.
- Decide whether floating controls align to the content region or the full window, then apply consistently.

### Step 2: Viewer/detail page with nested image content

General health: clear navigation exists, but the visual hierarchy is confused.

Findings:

- The viewed image is itself a screenshot of the app, which creates a nested app-within-app effect. The current viewer surface does not distinguish enough between real app chrome and image content.
- The sidebar remains fully opaque and persistent in viewer mode, which competes with the focused image-viewing task.
- The header action group uses default dark button styling. It does not relate to the liquid glass mode selector or future glass system.
- The image canvas has weak separation from the surrounding viewer background. A subtle frame or stage treatment would help users understand what is content versus app UI.
- The mode selector inside the displayed screenshot is image content, not a live control, but visually it resembles the real app control. The viewer needs stronger content framing to avoid this ambiguity.
- Header controls appear dense: edit, add, favorite, delete, info, and window controls all sit in one horizontal band with similar visual weight.

Accessibility risks:

- The nested screenshot can confuse users who rely on visual scanning because app chrome and image content look too similar.
- Icon-only actions need tooltips and keyboard-accessible labels; this cannot be confirmed from screenshots alone.
- The active image title is readable, but it competes with dense action controls and does not strongly anchor the page.

Required fixes:

- Introduce a viewer stage: content area background, image shadow/frame, and spacing that clearly separate image pixels from application chrome.
- Consider dimming or glass-softening the persistent sidebar in viewer mode, or provide a narrower viewer-focused sidebar treatment.
- Convert viewer toolbar actions to a coherent glass/action-button system with clear grouping: primary action, metadata actions, destructive action, window controls.
- Convert right-click viewer menus to the same glass menu system, with clear grouping for edit, favorite, delete, info, and open/show actions.
- Keep image content untouched; do not apply blur or material effects inside the photo itself.
- Ensure the viewer header title aligns with the image stage and remains distinct from toolbar groups.

## Updated adaptation priorities from audit

The next implementation should follow this revised order:

1. Define layout alignment: sidebar width, content canvas origin, header title alignment, floating control alignment.
2. Add content safe inset for the bottom mode selector.
3. Build shared glass materials and button states.
4. Adapt batch action toolbar and viewer toolbar buttons.
5. Adapt right-click context menus for thumbnail and viewer actions.
6. Replace thumbnail selected state with glass-compatible selection plus separate focus state.
7. Reduce hard grid gutters and create a calmer photo canvas.
8. Add viewer stage treatment so image content is clearly separated from app chrome.
9. Adapt sidebar material after the content/header alignment is stable.

## Explicit modification scope and layout specification

This section turns the audit findings into concrete implementation targets. It is the source of truth for the next code pass.

### A. Global glass style system

Files to modify:

- `src/ui/grid_css.rs`

Required changes:

- Add reusable glass material classes instead of repeating per-widget values.
- Move viewer favorite active styling out of `viewer_page.rs` and into the global CSS provider.
- Define button, menu, selected-card, sidebar, header, and viewer-stage classes in one place.

Required classes:

```css
.glass-base
.glass-raised
.glass-toolbar
.glass-toolbar-button
.glass-toolbar-danger
.glass-menu
.glass-menu-list
.glass-menu-item
.glass-menu-item-danger
.glass-menu-item-suggested
.glass-selected
.glass-focus-ring
.glass-sidebar
.glass-sidebar-row
.glass-header
.viewer-stage
.viewer-image-frame
.content-safe-bottom
```

Key style values:

```css
--glass-blur: 22px;
--glass-raised-blur: 28px;
--glass-fill: alpha(white, 0.10);
--glass-fill-strong: alpha(white, 0.16);
--glass-border: alpha(white, 0.30);
--glass-border-strong: alpha(white, 0.46);
--glass-shadow: alpha(black, 0.26);
--glass-radius-sm: 10px;
--glass-radius-md: 16px;
--glass-radius-lg: 24px;
--glass-safe-bottom: 112px;
```

GTK CSS does not support custom properties in the same way web CSS does, so these values should be documented as token comments or implemented through grouped selectors, not assumed to work as native variables unless verified.

### B. Main window and sidebar layout

Files to modify:

- `data/ui/window.blp`
- `src/ui/window.rs`
- `src/ui/grid_css.rs`

Current issue:

- Sidebar is a flat opaque column and does not share the liquid glass material.
- Sidebar row padding is hard-coded through label margins in Rust.
- Sidebar width is only defined as `sidebar-width-fraction: 0.2` and `min-sidebar-width: 200`, which can feel too wide or too narrow depending on window width.

Layout optimization plan:

- Keep the sidebar persistent on desktop, but make it a deliberate navigation rail.
- Use a stable desktop width target around `240px`.
- Keep minimum width around `220px` so Chinese labels do not feel cramped.
- Add an inset rail option through CSS padding rather than changing navigation architecture.
- Use row-level rounded selection instead of full-width hard selection.

Template changes:

```blueprint
Adw.OverlaySplitView split_view {
  sidebar-width-fraction: 0.20;
  min-sidebar-width: 220;
  max-sidebar-width: 280;
  css-classes: ["app-shell"];

  sidebar: Adw.NavigationPage sidebar_page {
    css-classes: ["glass-sidebar-page"];

    Gtk.ListBox sidebar_list {
      css-classes: ["glass-sidebar"];
      selection-mode: single;
    }
  };
}
```

Rust changes:

- Add `glass-sidebar-row` to every `ListBoxRow`.
- Add `glass-sidebar-label` to row labels.
- Avoid increasing label margins directly in Rust after the class exists.

Key CSS direction:

```css
.glass-sidebar {
  padding: 12px;
  background: alpha(white, 0.06);
  backdrop-filter: blur(24px) saturate(1.18) brightness(1.04);
  border-right: 1px solid alpha(white, 0.12);
}

.glass-sidebar row {
  min-height: 40px;
  border-radius: 12px;
  padding: 0 10px;
}

.glass-sidebar row:selected {
  background: alpha(white, 0.14);
  box-shadow: inset 0 1px alpha(white, 0.35), inset 0 -1px alpha(black, 0.12);
}
```

### C. Photos page header and batch toolbar

Files to modify:

- `data/ui/photos-page.blp`
- `src/ui/photos_page.rs`
- `src/ui/grid_css.rs`

Current issue:

- Batch action buttons use default header styling and do not match the glass mode selector.
- Header title alignment is ambiguous because the sidebar changes the perceived content center.

Layout optimization plan:

- Treat batch actions as a compact glass toolbar group in the header start area.
- Keep the page title aligned to the content area where possible.
- Use consistent button height and radius for all batch actions.
- Do not move the buttons into the grid overlay; they are page-level operations and belong in the header.

Template changes:

```blueprint
Adw.HeaderBar header_bar {
  css-classes: ["glass-header"];

  [start]
  Gtk.Box batch_toolbar {
    css-classes: ["glass-toolbar", "photos-batch-toolbar"];
    spacing: 6;

    Gtk.Button select_all_btn { css-classes: ["glass-toolbar-button"]; }
    Gtk.Button add_to_album_btn { css-classes: ["glass-toolbar-button"]; }
    Gtk.Button favorite_btn { css-classes: ["glass-toolbar-button"]; }
    Gtk.Button unfavorite_btn { css-classes: ["glass-toolbar-button"]; }
    Gtk.Button delete_to_trash_btn { css-classes: ["glass-toolbar-button", "glass-toolbar-danger"]; }
  }
}
```

If wrapping existing buttons in a new `Gtk.Box` is too invasive for the first pass, assign the classes directly to each existing button and defer grouping.

Key CSS direction:

```css
.glass-header {
  background: alpha(black, 0.18);
  backdrop-filter: blur(20px) saturate(1.10);
  border-bottom: 1px solid alpha(white, 0.08);
}

.glass-toolbar {
  padding: 4px;
  border-radius: 14px;
  background: alpha(white, 0.07);
  border: 1px solid alpha(white, 0.12);
}

.glass-toolbar-button {
  min-height: 34px;
  border-radius: 10px;
  padding: 0 14px;
  background: alpha(white, 0.08);
}
```

### D. Photo grid canvas and thumbnail layout

Files to modify:

- `src/ui/media_grid.rs`
- `src/ui/grid_css.rs`
- optionally `data/ui/media-grid.blp` if template-level wrappers are needed

Current issue:

- FlowBox uses `column_spacing(2)` and `row_spacing(2)`, creating hard tiled gutters.
- Selection uses default `FlowBoxChild:selected`, producing a bright rectangular blue outline.
- Headers and grid cards do not feel part of a designed canvas.

Layout optimization plan:

- Increase grid spacing from `2px` to `6px` or `8px`.
- Add content padding around each section, not just header margins.
- Preserve density by keeping thumbnail target size unchanged initially.
- Use rounded card clipping and selection overlay styling.
- Keep `FlowBox` selection logic unchanged.

Rust changes:

```rust
let flow = gtk::FlowBox::builder()
    .column_spacing(8)
    .row_spacing(8)
    .selection_mode(gtk::SelectionMode::Multiple)
    .build();
flow.add_css_class("thumb-grid");
```

Square tile changes:

- Keep `thumb-tile` class.
- Add `glass-thumb-card` if the square tile wrapper supports class assignment.
- Do not apply `backdrop-filter` per thumbnail; it is too expensive and visually wrong for photo content.

Key CSS direction:

```css
.thumb-grid {
  padding: 8px 8px 128px 8px;
  background: transparent;
}

.thumb-grid flowboxchild {
  border-radius: 10px;
  padding: 2px;
}

.thumb-grid flowboxchild:hover {
  background: alpha(white, 0.08);
}

.thumb-grid flowboxchild:selected {
  background: alpha(white, 0.10);
  border: 1px solid alpha(white, 0.48);
  box-shadow: 0 0 0 1px alpha(#5aa7ff, 0.55), inset 0 1px alpha(white, 0.35);
}

.thumb-grid flowboxchild:focus {
  outline: 2px solid alpha(#7db9ff, 0.80);
  outline-offset: 2px;
}
```

### E. Bottom mode selector safe area and alignment

Files to modify:

- `data/ui/photos-page.blp`
- `src/ui/media_grid.rs`
- `src/ui/grid_css.rs`

Current issue:

- The mode selector is overlayed at the bottom with `margin-bottom: 24`, but the scrollable grid does not reserve space for it.
- It appears centered in the full window, while the content area begins after the sidebar.

Layout optimization plan:

- Keep the mode selector floating.
- Add bottom content padding of at least `112px` to the scroll/content area.
- Align the selector relative to the content area, not the full app window, because the sidebar is persistent on desktop.
- For narrow widths, reduce selector padding and spacing before changing behavior.

Template direction:

```blueprint
Gtk.Overlay grid_overlay {
  css-classes: ["photos-content-overlay"];

  [overlay]
  $ModeSelector mode_selector {
    css-classes: ["mode-selector", "glass-raised"];
    halign: center;
    valign: end;
    margin-bottom: 24;
  }
}
```

Key CSS direction:

```css
.photos-content-overlay {
  padding-bottom: 0;
}

.content-safe-bottom,
.thumb-grid {
  padding-bottom: 128px;
}

box.mode-selector {
  min-height: 58px;
  border-radius: 28px;
}
```

### F. Right-click context menus

Files to modify:

- `src/ui/media_grid.rs`
- `src/ui/grid_css.rs`

Current issue:

- Right-click menu exists and has `media-grid-context-menu`, but menu items still rely on generic `flat`, `suggested-action`, and `destructive-action` styling.
- It does not yet match the glass visual language.

Layout optimization plan:

- Keep the existing popover construction and action callbacks.
- Rename or supplement classes so the menu is explicitly part of the glass system.
- Use a fixed minimum width so the menu does not feel cramped.
- Use consistent item height, radius, and spacing.
- Keep destructive actions visually distinct but not harsh.

Rust class changes:

```rust
popover.add_css_class("glass-menu");
menu.add_css_class("glass-menu-list");
button.add_css_class("glass-menu-item");
delete_btn.add_css_class("glass-menu-item-danger");
multi_btn.add_css_class("glass-menu-item-suggested");
```

Key CSS direction:

```css
.glass-menu {
  padding: 6px;
  border-radius: 16px;
  background: alpha(black, 0.42);
  border: 1px solid alpha(white, 0.22);
  backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
  box-shadow: 0 18px 48px alpha(black, 0.35), inset 0 1px alpha(white, 0.24);
}

.glass-menu-list {
  min-width: 190px;
  spacing: 3px;
}

.glass-menu-item {
  min-height: 36px;
  border-radius: 10px;
  padding: 0 12px;
  background: transparent;
}

.glass-menu-item:hover {
  background: alpha(white, 0.12);
}

.glass-menu-item-danger {
  color: #ffb4ab;
}

.glass-menu-item-danger:hover {
  background: alpha(#ff5449, 0.18);
}
```

### G. Viewer page toolbar and image stage

Files to modify:

- `data/ui/viewer-page.blp`
- `src/ui/viewer_page.rs`
- `src/ui/grid_css.rs`

Current issue:

- Viewer toolbar buttons use default styling.
- Favorite active styling is injected locally in Rust.
- The image has no strong stage/frame, so screenshots of the app can be confused with real app chrome.

Layout optimization plan:

- Keep actions in the header, but visually group them.
- Use glass toolbar button classes on edit, add, favorite, delete, details.
- Move `favorite-active` CSS to global style.
- Add an image stage background and frame/shadow around the displayed photo.
- Do not apply blur to the image itself.

Template changes:

```blueprint
Adw.HeaderBar header_bar {
  css-classes: ["glass-header", "viewer-header"];

  Gtk.Button edit_btn { css-classes: ["glass-toolbar-button"]; }
  Gtk.MenuButton add_to_album_btn { css-classes: ["glass-toolbar-button"]; }
  Gtk.Button favorite_btn { css-classes: ["glass-toolbar-button", "viewer-favorite-btn"]; }
  Gtk.Button delete_btn { css-classes: ["glass-toolbar-button", "glass-toolbar-danger"]; }
  Gtk.Button details_btn { css-classes: ["glass-toolbar-button"]; }
}

content: Gtk.Overlay image_overlay {
  css-classes: ["viewer-stage"];

  child: Gtk.Picture picture {
    css-classes: ["viewer-image-frame"];
  };
}
```

Key CSS direction:

```css
.viewer-stage {
  padding: 32px;
  background: radial-gradient(circle at center, alpha(white, 0.06), transparent 55%), alpha(black, 0.10);
}

.viewer-image-frame {
  border-radius: 14px;
  box-shadow: 0 24px 80px alpha(black, 0.38), 0 0 0 1px alpha(white, 0.10);
}

.viewer-favorite-btn.favorite-active {
  color: #f6c344;
  background: alpha(#f6c344, 0.14);
  border-color: alpha(#f6c344, 0.38);
}
```

### H. Viewer details sidebar

Files to modify:

- `data/ui/viewer-page.blp`
- `src/ui/grid_css.rs`

Current issue:

- Details sidebar uses `css-classes: ["background"]`, so it remains a default opaque panel.
- It visually competes with the image stage.

Layout optimization plan:

- Treat details sidebar as `glass-base`, not `glass-raised`.
- Keep width request around `380px` for metadata readability.
- Use a subtle left border and blur.
- Make close button match glass toolbar buttons.

Template changes:

```blueprint
sidebar: Gtk.Box details_panel {
  css-classes: ["viewer-details-panel", "glass-base"];
}

Gtk.Button details_close_btn {
  css-classes: ["glass-toolbar-button"];
}
```

Key CSS direction:

```css
.viewer-details-panel {
  background: alpha(black, 0.30);
  backdrop-filter: blur(22px) saturate(1.12);
  border-left: 1px solid alpha(white, 0.12);
}
```

### I. Editor and secondary popovers

Files to modify later:

- `data/ui/editor-page.blp`
- `src/ui/editor_page.rs`
- `src/ui/grid_css.rs`

Current issue:

- Editor save menu uses `Gtk.PopoverMenu`, which will still look default unless global menu styling covers it.

Layout optimization plan:

- First adapt photo grid and viewer context menus.
- Then apply the same `glass-menu` language to editor save menu and album picker surfaces.
- Do not change editor image processing or save behavior.

### J. Non-modification boundaries

Do not modify these areas for the glass/layout pass:

- `src/core/*`
- thumbnail cache generation
- database schema
- file watcher logic
- trash/favorite persistence logic
- Flatpak runtime manifest, unless a GTK runtime issue appears
- Cargo dependencies

### K. Implementation acceptance checklist

The pass is complete only when all of these are true:

- The bottom mode selector no longer covers the last meaningful row of thumbnails.
- The selected thumbnail state no longer looks like a hard default blue rectangle.
- Batch toolbar buttons, viewer toolbar buttons, and right-click menu items share the same glass action style.
- The sidebar selected row uses rounded glass selection instead of a full hard strip.
- Viewer image content is clearly framed as image content, especially when the image itself is a screenshot of the app.
- Keyboard focus remains visible and distinct from selected and hover states.
- Destructive actions remain identifiable without using harsh default red blocks.
- No core data, scan, cache, or persistence logic is changed.
