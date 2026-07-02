# Project-Wide Keyboard Adaptation Design

## Goal

Build a project-wide keyboard interaction system that keeps shortcuts consistent
across Photos, albums, search, trash, viewer, editor, dialogs, and future pages
without spreading raw `gdk::Key` matches through page implementations.

The design should preserve current mouse/button behavior and the recent removal
of touch-specific viewer/navigation gestures. Keyboard input becomes a first
class desktop control path, not a replacement for visible controls.

## Current State

Keyboard handling is currently local and uneven:

- `ViewerPage` handles `Left`, `Right`, `Escape`, and video `Space` through
  `EventControllerKey` installed on the image and video widgets.
- `ModeSelector` handles `Left` / `Right` directly.
- `PhotosPage` has a local `Escape` controller for selection mode.
- `grid_css.rs` installs FlowBox arrow-key focus movement.
- `GlassContextMenu`, rename entry, fullscreen preview, and editor crop logic
  each own small Escape/key handlers.

This works for focused widgets, but the behavior depends on which child owns
focus. For example, viewer navigation can fail when focus sits on a header
button, the filmstrip, or another child instead of the picture/video widget.
Adding shortcuts by copying more page-local key handlers would increase
conflicts and make future windows harder to reason about.

## Architecture

Introduce a small keyboard subsystem under `src/ui/keyboard/` with three
separate concerns:

1. **Key normalization and default bindings** translate `gdk::Key` plus
   modifiers into app-level `KeyboardAction` values.
2. **Scope resolution** decides which action table applies from the current
   UI context: text input, dialog/menu, editor, viewer, browsing, or global.
3. **Action dispatch** sends the resolved `KeyboardAction` to the active page
   or focused overlay through registered handlers.

Pages should handle actions, not raw keys. A page may still keep local key
controllers for truly widget-local behavior, such as rename entry Escape,
context-menu Escape, fullscreen-preview Escape, or crop rectangle nudging.

## Core Types

`KeyboardAction` is the public vocabulary:

```rust
pub enum KeyboardAction {
    CancelOrClose,
    NavigateBack,
    BrowseUp,
    BrowseDown,
    BrowseLeft,
    BrowseRight,
    ActivateFocused,
    ToggleSelection,
    SelectAll,
    Search,
    OpenSettings,
    Delete,
    Restore,
    ViewerPrevious,
    ViewerNext,
    ViewerZoomIn,
    ViewerZoomOut,
    ViewerZoomReset,
    ViewerRotateLeft,
    ViewerRotateRight,
    ViewerFullscreenPreview,
    ViewerToggleDetails,
    ViewerToggleEdit,
    ViewerToggleFavorite,
    ViewerTogglePlayback,
}
```

`KeyboardScope` describes where the key event is being interpreted:

```rust
pub enum KeyboardScope {
    TextInput,
    Modal,
    Editor,
    Viewer,
    Browsing,
    Global,
}
```

`KeyboardResult` communicates whether a page consumed the action:

```rust
pub enum KeyboardResult {
    Handled,
    Ignored,
}
```

The router stops propagation only for `Handled`.

## Default Bindings

Global bindings:

| Keys | Action |
|---|---|
| `Escape`, `Alt+Left` | `CancelOrClose` |
| `Ctrl+F` | `Search` |
| `Ctrl+,` | `OpenSettings` |

Browsing bindings:

| Keys | Action |
|---|---|
| `Up`, `KP_Up` | `BrowseUp` |
| `Down`, `KP_Down` | `BrowseDown` |
| `Left`, `KP_Left` | `BrowseLeft` |
| `Right`, `KP_Right` | `BrowseRight` |
| `Enter`, `KP_Enter` | `ActivateFocused` |
| `Space`, `KP_Space` | `ToggleSelection` |
| `Ctrl+A` | `SelectAll` |
| `Delete` | `Delete` |

Viewer bindings:

| Keys | Action |
|---|---|
| `Left`, `KP_Left` | `ViewerPrevious` |
| `Right`, `KP_Right` | `ViewerNext` |
| `Escape` | `CancelOrClose` |
| `Space`, `KP_Space` | `ViewerTogglePlayback` |
| `plus`, `KP_Add`, `equal` | `ViewerZoomIn` |
| `minus`, `KP_Subtract` | `ViewerZoomOut` |
| `0`, `KP_0` | `ViewerZoomReset` |
| `R` | `ViewerRotateRight` |
| `Shift+R` | `ViewerRotateLeft` |
| `F` | `ViewerFullscreenPreview` |
| `I` | `ViewerToggleDetails` |
| `E` | `ViewerToggleEdit` |
| `H` | `ViewerToggleFavorite` |
| `Delete` | `Delete` |

Editor bindings:

| Keys | Action |
|---|---|
| `Escape` | `CancelOrClose` |
| `Ctrl+S` | reserved for a future explicit save action |

Text input scope does not map printable character keys. It lets entries and
search fields keep native editing behavior. `Escape` and `Enter` remain local
to text widgets unless a widget-specific handler deliberately forwards them.

## Scope Resolution

Scope priority is:

```text
TextInput > Modal > Editor > Viewer > Browsing > Global
```

Resolution rules:

- If the focused widget is an editable text widget, use `TextInput`.
- If a custom glass context menu, alert dialog, settings dialog, album picker,
  or other modal surface is open, use `Modal`.
- If the visible page is `ViewerPage` and the editor sidebar is open, use
  `Editor`.
- If the visible page is `ViewerPage`, use `Viewer`.
- If the visible page is `PhotosPage`, `AlbumDetailPage`, `SearchPage`, or
  `TrashPage`, use `Browsing`.
- Otherwise use `Global`.

The first implementation should keep scope detection explicit and conservative.
Avoid a large abstraction over every page type until repeated patterns are
proven.

## Dispatch Model

Install one `EventControllerKey` on the main window or content navigation root
in `Capture` phase. The router:

1. Receives the key event.
2. Normalizes key and modifier state.
3. Resolves the active `KeyboardScope`.
4. Looks up a `KeyboardAction` for that scope, falling back to `Global`.
5. Dispatches the action to the registered handler for the active page or
   window shell.
6. Stops propagation only when the handler returns `Handled`.

Page handlers are small methods such as:

```rust
fn handle_keyboard_action(&self, action: KeyboardAction) -> KeyboardResult
```

Handlers should call existing page methods wherever possible:

- Viewer next/previous should use the same path as overlay previous/next
  buttons.
- Viewer cancel should close editor/details first, then pop the viewer.
- Browsing delete/select/favorite should reuse existing selection actions.
- Search should call the same method as the search toolbar button.
- Settings should call `MainWindow::show_settings_dialog()`.

## Conflict Rules

- Visible controls remain the source of user-facing affordance. Shortcuts
  should trigger the same actions as buttons.
- Viewer shortcuts must not fire while the editor owns the interaction.
- Text entry must not lose normal typing, cursor movement, selection, or
  rename behavior.
- Modal/dialog shortcuts must not leak into the page beneath.
- Local widget handlers are allowed only when the behavior is inherently local:
  context-menu dismissal, inline rename, fullscreen-preview window close,
  crop-rectangle movement, and native text editing.

## Migration Plan

1. Create `src/ui/keyboard/` with action, binding, scope, and router modules.
2. Add unit tests for binding lookup and scope fallback.
3. Install the router in `MainWindow` without changing page behavior.
4. Move viewer `Left` / `Right` / `Escape` / `Space` into the router-backed
   action path and add tests for focus on viewer child controls.
5. Move viewer zoom, rotate, details, edit, fullscreen, delete, and favorite
   keyboard actions onto the same action path.
6. Move browsing grid navigation and selection actions toward `Browsing`
   actions, keeping FlowBox-specific focus calculations local.
7. Add Search, Trash, AlbumDetail, and Settings global actions.
8. Remove duplicated page-level key controllers once equivalent router-backed
   tests pass.
9. Document the canonical keymap in `docs/modules/ui-design.md` or a dedicated
   `docs/modules/keyboard.md`.

## Testing Strategy

Tests should verify the contract rather than GTK internals:

- Binding tests: key plus modifiers resolves to the expected `KeyboardAction`
  in each scope.
- Scope tests: focused text entries suppress printable app shortcuts.
- Viewer tests: `Left`, `Right`, and `Escape` work when focus is on image,
  video, header buttons, filmstrip, and details controls.
- Editor tests: while editing, `Left` / `Right` do not navigate media and
  `Escape` closes editor state first.
- Browsing tests: arrow keys move focus, Enter opens the focused tile, and
  selection shortcuts operate only in selection-capable grids.
- Modal tests: `Escape` closes the top modal/menu without triggering page
  navigation underneath.

## Non-Goals

- Do not introduce user-editable shortcut preferences in the first pass.
- Do not restore touch-only navigation, pinch, or pan gestures.
- Do not replace GTK native text editing shortcuts.
- Do not redesign visible UI controls solely to expose shortcuts.
- Do not add a command palette until the action vocabulary has stabilized.

## Rollout Notes

The first functional slice should be viewer-focused because that is where the
current key/focus gap is most visible. Later browsing and editor migration can
reuse the same router without forcing a large one-shot rewrite.

First-slice implementation note: the router is installed on `MainWindow` as the
named capture-phase controller `photo-viewer-keyboard-router`. Viewer
Left/Right/Escape/Space actions now dispatch through `ViewerPage` even when
focus is on a viewer child control. Browsing actions are bound for consistency
but are left to the existing grid-local behavior until the browsing-page
handler migration.
