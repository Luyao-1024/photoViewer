# UI Liquid Glass Module

## Scope

This module owns the shared UI material system for glass chrome, including the user-facing Liquid Glass toggle and the plain translucent fallback mode.

## Key Files

| File | Role |
|---|---|
| `src/ui/grid_css.rs` | CSS assembly, transparency scaling, provider install/reapply |
| `src/ui/grid_css/tests.rs` | Unit tests for CSS assembly and material-mode behavior |
| `data/css/base.css` | Shared layout, size, radius, and state selectors |
| `data/css/liquid.css` | Liquid Glass material selectors |
| `data/css/plain.css` | Plain translucent fallback material selectors |
| `data/css/a11y.css` | Shared keyboard-focus affordances, independent of transparency |
| `src/ui/grid_css/tests/render.rs` | GTK color resolution checks and optional material screenshots |
| `src/ui/theme.rs` | Maps persisted theme preference to libadwaita color schemes |
| `src/ui/glass_context_menu.rs` | Overlay-backed right-click menu using raised glass material |
| `src/core/prefs.rs` | `theme`, `liquid_glass`, and material transparency preference persistence |
| `src/ui/window/settings.rs` | Settings dialog UI and live appearance preference handling |
| `data/ui/mode-selector.blp` | Canonical segmented glass control |
| `src/ui/mode_selector.rs` | Mode selector behavior |
| `tests/ui_grid_css_install.rs` | CSS provider/mode assertions |
| `tests/ui_mode_selector.rs` | Mode selector template assertions |

Keep `grid_css` behavior tests in `src/ui/grid_css/tests.rs`; integration
coverage under `tests/ui_grid_css_*` should stay focused on provider
installation and source-file wiring.

## Appearance Preferences

The Settings dialog's Appearance section owns two separate concerns:

- Theme selects the libadwaita color scheme: follow system, light, or dark.
  `src/ui/theme.rs` applies the persisted `ThemePreference` through
  `AdwStyleManager`, so changes are live and app startup honors the saved
  choice before the first window is built.
- Liquid Glass controls the shared material layer. The toggle switches between
  Liquid Glass and plain translucent CSS, while the transparency slider adjusts
  material strength.

Keep these controls independent: theme should not rewrite glass settings, and
glass settings should not force light or dark mode.

## Material Split

`src/ui/grid_css.rs` builds CSS from four source files:

| Block | Responsibility |
|---|---|
| `data/css/base.css` | Shared layout, size, radius, state selectors |
| `data/css/liquid.css` | Blur/saturate/brightness, highlights, heavier shadows |
| `data/css/plain.css` | Plain translucent fallback with no `backdrop-filter` |
| `data/css/a11y.css` | GTK `:focus-visible` outlines and menu focus treatment |

Window chrome fills, borders, and text use libadwaita theme colors. Suggested
and destructive actions use `@accent_color` and `@error_color`, including
hover states, so light mode does not inherit pale dark-mode action text.
Specular top highlights use translucent white independently of text color;
drop shadows use black. Neither light source should reverse with the theme.
GTK resolves these colors live without a CSS reinstall on theme changes.

Photo-backed affordances (selection marks, badges, date text, navigation
arrows) keep white foregrounds with local dark backing or shadows. The mode
selector pairs a dark tint with white text or a light tint with dark text,
based on the sampled photo background rather than the window theme.

`build_css(bool)` chooses the material block. `install()` applies the startup preference and `reapply(bool)` swaps the display-level provider when the setting changes.

`liquid_glass_transparency` ranges from 0 (full material strength, still
translucent) to 100 (maximum transparency). Decorative background alpha can
reach zero; borders and shadows keep their visibility floor. Reading surfaces
use the generated `@glass_reading_bg` color: menus, dialogs, and details/editor
panels retain alpha 0.72–0.78 in Liquid mode and 0.82–0.88 in Plain mode.
The photo-backed mode selector retains a paired tint at alpha 0.66–0.78.
The Settings subtitle explicitly explains these readability exceptions.

Text, icons, and focus outlines are independent of the transparency slider.
Blur falls with the square root of material strength, preserving more detail
suppression at intermediate transparency; saturation and brightness converge
linearly to neutral. At 100, the filter is `blur(0px) saturate(1) brightness(1)`
without a discontinuity or unsupported `none` value.

Use shallow shadows for toolbar buttons, medium shadows for floating navigation
and menus, and deeper shadows for modal dialogs. The filmstrip's edge washes
and shadow belong to each material file, so Plain mode and the slider also
affect them. Settings preference cards reuse the dialog's blur rather than
adding another backdrop filter to every group. Details and editor preference
children remain transparent, with their shared parent owning the material.

## Canonical Segmented Style

The Year/Month/Day mode selector is the visual baseline. Its style has been extracted into reusable classes without changing the original core implementation:

```text
outer: glass-raised glass-segmented
slot:  glass-segment
text:  glass-segment-label
text(active): glass-segment-label active
indicator: glass-segment-indicator
light background: outer add on-light-background
```

The style is intentionally one glass container with lightweight internal state. Do not add active background blocks to individual segments.

Search field toggles compose `glass-raised glass-segmented` with native
`GtkToggleButton.glass-segment` children. Their checked state uses text contrast
and a reserved bottom underline; native grouping and accessibility remain intact.
Photos background contrast waits 120 ms for a stable candidate, repeated samples
do not reset that timer, and unloaded cells retain the last known contrast.
Once accepted, the capsule tint, labels, and underline interpolate together
over 350 ms (`ease-in-out`) in both material modes. Give the painted text and
underline explicit color endpoints instead of transitioning inherited colors
through intermediate boxes: GTK can otherwise delay/restart child transitions.
Reversing contrast mid-animation must continue from the current colors.

The thumbnail state veil is a borderless, unpadded overlay filling the picture's
allocation. Do not add even a transparent border: GtkFrame can leave an
unpainted strip that exposes white photos along the straight edges. Keep the
tile's existing outer geometry and rounded clipping unchanged.

## Adding Glass Surfaces

1. Reuse existing classes first: `.glass-base`, `.glass-raised`, `.glass-header`, `.glass-menu`, `.glass-alert-dialog`, `.viewer-details-panel`, or `.glass-segmented`.
2. If a new selector is required, add it to both `data/css/liquid.css` and `data/css/plain.css`.
3. Keep shape/layout/state rules in `data/css/base.css`.
4. Never put `backdrop-filter` in `data/css/base.css`.
5. Extend CSS tests when adding selectors or changing mode behavior.

Right-click menus should use the custom overlay `GlassContextMenu` and compose
`.glass-raised glass-context-menu` so they render as normal overlay children,
matching the Year/Month/Day selector's glass path. `GtkPopover` menus may still
use `.glass-menu > contents` for button-triggered popovers, but do not use them
for right-click context menus. Menu items stay transparent at rest so the menu
reads as one raised panel instead of a stack of separate buttons; hover and
accent states remain separate.

## Button Material Modes

The shared `.glass-toolbar-button` material is **always-on** by default — photos, trash, albums, and editor headers all carry their glass capsule at rest. Three scopes opt out to a **hover-only** treatment (bare icon at rest, glass surface only on `:hover` / `:focus-visible`), because they float over content that should stay uncluttered:

- `.viewer-chrome .glass-toolbar-button` + `.viewer-overlay-nav-btn` — the viewer header actions and the bottom-right prev/next arrows, floating over a full-bleed photo.
- `.sidebar-settings-button` — the sidebar footer settings button (`preferences-system-symbolic`), floating over the sidebar surface.
- `.round-search-button` — the compact search action in browsing headers.

Each scope gets its own bare-at-rest reset plus a hover/focus material rule in **both** `LIQUID_GLASS_MATERIAL_CSS` and `PLAIN_GLASS_MATERIAL_CSS`. Add new hover-only buttons by introducing a unique class and mirroring these two rules; do not loosen the shared `.glass-toolbar-button` rule, which other headers depend on being always-on.

## Runtime Notes

`backdrop-filter` renders correctly in the Flatpak GNOME 50 runtime. Older host GTK versions can print parser warnings and fall back to translucent fill, border, and shadow. This is expected; verify visuals with Flatpak rather than removing the property.

For a reproducible material fixture, run:

```bash
GDK_BACKEND=x11 PHOTOVIEWER_GLASS_SCREENSHOTS=target/visual-checks/glass \
  tools/with-at-spi.sh xvfb-run -a cargo test --lib ui::grid_css::tests::render
```

This checks GTK parsing and resolved foreground/tint colors across both themes,
both material modes, and transparency 0/50/100, and optionally exports PNGs.
It also checks intermediate/reversed selector color transitions and rendered
white-thumbnail edge coverage for hover and selection in both material modes.
It uses the host GTK and complements the Flatpak visual check; it is not a
substitute for photo-backed full-application validation.

Overlay glass pixels depend on the content behind them. During the Photos
Year/Month/Day `GtkStack` crossfade, keep explicitly queueing redraws of the
mode selector for the full transition even when its foreground contrast class
does not change. Otherwise GTK/GSK can retain a stale backdrop strip until the
underlying grid next scrolls.

Do not reintroduce the abandoned CPU/GSK background capture approach or custom `snapshot` refraction path. The current implementation relies on GTK/GSK CSS rendering.

Avoid full-window `filter` or `backdrop-filter` on modal scrims. The Settings
dialog may dim the gallery while open, but large-scene blur during dialog
animations is too expensive; keep blur on bounded chrome surfaces such as the
dialog card, menus, and side panels.

## Legacy Pointer

[`../liquid-glass.md`](../liquid-glass.md) is kept only as a compatibility pointer for older links. This module document is the active source of truth for Liquid Glass material contracts.
