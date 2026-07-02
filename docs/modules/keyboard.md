# Keyboard Module

## Scope

The keyboard module owns app-level shortcut normalization, scope selection, and
main-window routing. Pages should receive `KeyboardAction` values instead of
matching raw `gdk::Key` values when behavior is not inherently local to a
single widget.

Local widget key handlers are still appropriate for native text editing,
inline rename cancellation, context-menu dismissal, fullscreen-preview close,
and editor crop interactions.

## Key Files

| File | Role |
|---|---|
| `src/ui/keyboard/action.rs` | Shared `KeyboardAction` and `KeyboardResult` vocabulary |
| `src/ui/keyboard/binding.rs` | `KeyCombo`, `KeyboardScope`, and default binding lookup |
| `src/ui/keyboard/router.rs` | Capture-phase `EventControllerKey` installation and text-input guard |
| `src/ui/window.rs` | Active-page scope resolution and global/window dispatch |
| `src/ui/viewer_page.rs` | Viewer action handler for navigation, close, playback, and chrome shortcuts |

## Routing Contract

`MainWindow` installs a single named capture-phase keyboard router:
`photo-viewer-keyboard-router`. The router resolves the active scope, maps the
key to a `KeyboardAction`, dispatches it, and stops propagation only when the
handler returns `KeyboardResult::Handled`.

Scope priority is conservative:

```text
TextInput > Modal > Editor > Viewer > Browsing > Global
```

The first functional slice routes viewer shortcuts from the window, so
Left/Right/Escape/Space work even when focus is on header buttons, the
filmstrip, or other viewer children. Browsing arrow movement remains owned by
the grid for now; browsing actions may resolve from bindings but are ignored by
the window router until page-level handlers are added.

## Default Keymap

| Scope | Keys | Action |
|---|---|---|
| Global | `Escape` | `CancelOrClose` |
| Global | `Alt+Left` | `NavigateBack` |
| Global | `Ctrl+F` | `Search` |
| Global | `Ctrl+,` | `OpenSettings` |
| Browsing | `Up` / `Down` / `Left` / `Right` | `BrowseUp` / `BrowseDown` / `BrowseLeft` / `BrowseRight` |
| Browsing | `Enter` | `ActivateFocused` |
| Browsing | `Space` | `ToggleSelection` |
| Browsing | `Ctrl+A` | `SelectAll` |
| Browsing | `Delete` | `Delete` |
| Viewer | `Left` / `Right` | `ViewerPrevious` / `ViewerNext` |
| Viewer | `Escape` | `CancelOrClose` |
| Viewer | `Space` | `ViewerTogglePlayback` |
| Viewer | `+` / `Shift+=` / keypad `+` / `-` / `0` | `ViewerZoomIn` / `ViewerZoomOut` / `ViewerZoomReset` |
| Viewer | `R` / `Shift+R` | `ViewerRotateRight` / `ViewerRotateLeft` |
| Viewer | `F` | `ViewerFullscreenPreview` |
| Viewer | `I` | `ViewerToggleDetails` |
| Viewer | `E` | `ViewerToggleEdit` |
| Viewer | `H` | `ViewerToggleFavorite` |
| Viewer | `Delete` | `Delete` |

Modal and editor scopes do not fall back to global shortcuts. This prevents
Search, Settings, and navigation commands from leaking through dialogs, glass
context menus, or editing surfaces. `MainWindow` treats a visible
`.glass-context-menu-layer` under the root overlay as modal even if focus has
not moved into the menu layer yet.

Text input scope deliberately does not map printable shortcuts, including
`Ctrl+F`, so entries and search fields keep native editing behavior unless a
widget-specific handler forwards an action.

`Ctrl+F` opens Search from browsing pages and focuses an already-visible
`SearchPage` instead of pushing a duplicate page.
