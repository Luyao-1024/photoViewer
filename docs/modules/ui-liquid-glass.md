# UI Liquid Glass Module

## Scope

This module owns the shared UI material system for glass chrome, including the user-facing Liquid Glass toggle and the plain translucent fallback mode.

## Key Files

| File | Role |
|---|---|
| `src/ui/grid_css.rs` | CSS source, mode split, provider install/reapply |
| `src/core/prefs.rs` | `liquid_glass` preference persistence |
| `src/ui/window.rs` | Settings UI and live toggle handling |
| `data/ui/mode-selector.blp` | Canonical segmented glass control |
| `src/ui/mode_selector.rs` | Mode selector behavior |
| `tests/ui_grid_css_install.rs` | CSS provider/mode assertions |
| `tests/ui_mode_selector.rs` | Mode selector template assertions |

## Material Split

`src/ui/grid_css.rs` builds CSS from four blocks:

| Block | Responsibility |
|---|---|
| `BASE_CSS` | Shared layout, size, radius, state selectors |
| `LIQUID_GLASS_MATERIAL_CSS` | Blur/saturate/brightness, highlights, heavier shadows |
| `PLAIN_GLASS_MATERIAL_CSS` | Plain translucent fallback with no `backdrop-filter` |
| `A11Y_CSS` | Reserved for GTK-supported accessibility/runtime class rules |

`build_css(bool)` chooses the material block. `install()` applies the startup preference and `reapply(bool)` swaps the display-level provider when the setting changes.

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

## Adding Glass Surfaces

1. Reuse existing classes first: `.glass-base`, `.glass-raised`, `.glass-header`, `.glass-menu`, `.glass-alert-dialog`, `.viewer-details-panel`, or `.glass-segmented`.
2. If a new selector is required, add it to both `LIQUID_GLASS_MATERIAL_CSS` and `PLAIN_GLASS_MATERIAL_CSS`.
3. Keep shape/layout/state rules in `BASE_CSS`.
4. Never put `backdrop-filter` in `BASE_CSS`.
5. Extend CSS tests when adding selectors or changing mode behavior.

## Button Material Modes

The shared `.glass-toolbar-button` material is **always-on** by default — photos, trash, albums, and editor headers all carry their glass capsule at rest. Two scopes opt out to a **hover-only** treatment (bare icon at rest, glass surface only on `:hover` / `:focus-visible`), because they float over content that should stay uncluttered:

- `.viewer-chrome .glass-toolbar-button` + `.viewer-overlay-nav-btn` — the viewer header actions and the bottom-right prev/next arrows, floating over a full-bleed photo.
- `.sidebar-settings-button` — the sidebar footer settings button (`preferences-system-symbolic`), floating over the sidebar surface.

Each scope gets its own bare-at-rest reset plus a hover/focus material rule in **both** `LIQUID_GLASS_MATERIAL_CSS` and `PLAIN_GLASS_MATERIAL_CSS`. Add new hover-only buttons by introducing a unique class and mirroring these two rules; do not loosen the shared `.glass-toolbar-button` rule, which other headers depend on being always-on.

## Runtime Notes

`backdrop-filter` renders correctly in the Flatpak GNOME 50 runtime. Older host GTK versions can print parser warnings and fall back to translucent fill, border, and shadow. This is expected; verify visuals with Flatpak rather than removing the property.

Do not reintroduce the abandoned CPU/GSK background capture approach or custom `snapshot` refraction path. The current implementation relies on GTK/GSK CSS rendering.

## Legacy Document

The original detailed Liquid Glass notes are still available at [`../liquid-glass.md`](../liquid-glass.md). Prefer this module document for current navigation and keep both in sync when changing material contracts.
