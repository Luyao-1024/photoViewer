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

## Runtime Notes

`backdrop-filter` renders correctly in the Flatpak GNOME 50 runtime. Older host GTK versions can print parser warnings and fall back to translucent fill, border, and shadow. This is expected; verify visuals with Flatpak rather than removing the property.

Do not reintroduce the abandoned CPU/GSK background capture approach or custom `snapshot` refraction path. The current implementation relies on GTK/GSK CSS rendering.

## Legacy Document

The original detailed Liquid Glass notes are still available at [`../liquid-glass.md`](../liquid-glass.md). Prefer this module document for current navigation and keep both in sync when changing material contracts.
