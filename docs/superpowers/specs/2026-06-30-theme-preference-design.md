# Theme Preference Design

## Summary

Add an Appearance theme preference with three choices: follow system, light, and dark. The preference is applied through libadwaita's process-wide `StyleManager` so the app can honor the user's GNOME/device setting by default while still allowing explicit light or dark overrides.

## User Experience

The Settings dialog's Appearance section gains a "Theme" row above the existing Liquid Glass controls. It uses three radio-style check buttons labeled "Follow System", "Light", and "Dark". Selecting a value applies it immediately and persists it for the next launch. No restart is required.

The existing Liquid Glass toggle and transparency slider remain independent. Theme selection changes the app color scheme; Liquid Glass selection changes material rendering.

## Architecture

`src/core/prefs.rs` owns persistence in the existing `settings.json` file. A new `ThemePreference` enum maps persisted string values to libadwaita `ColorScheme` values:

- `system` maps to `adw::ColorScheme::Default`
- `light` maps to `adw::ColorScheme::ForceLight`
- `dark` maps to `adw::ColorScheme::ForceDark`

Invalid or missing persisted values fall back to `system`.

`src/app.rs` reads the persisted preference during activation and applies it before the first window is created. `src/ui/window.rs` builds the Settings controls and applies changes through the same helper path when a radio button is activated.

## Testing

Preference tests cover default behavior, valid round trips, invalid value fallback, and preserving unrelated JSON keys. Settings UI tests verify that the Appearance section exposes all three theme choices. Existing i18n key coverage will fail if English and Chinese strings are not kept in sync.

## Documentation

`docs/modules/ui-liquid-glass.md` is updated because it already documents Appearance settings, style provider behavior, and the Settings ownership boundary.
