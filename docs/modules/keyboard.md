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
| `src/ui/keyboard/shortcuts_window.rs` | `GROUPS` reference table, `GtkShortcutsWindow`, and tooltip key hints |
| `src/ui/window.rs` | Active-page scope resolution and global/window dispatch |
| `src/ui/window/settings.rs` | The 「键盘」 settings group that opens the reference |
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

Viewer shortcuts are routed from the window, so Left/Right/Escape/Space work
even when focus is on header buttons, the filmstrip, or other viewer children.
Photos and album browsing also handle selection-oriented shortcuts from the window:
`Ctrl+A` toggles the current visible grid's select-all state, `Delete` moves the
selected photos through the existing trash path, and `Escape` clears active
selection before any navigation-back fallback runs. Arrow movement remains GTK
GridView-native so it crosses virtualized rows and scrolls new ranges into view.
Enter activates the focused ready tile; Space enters multi-select and toggles
that focused tile without replacing the model.

### One physical press dispatches at most one action

Holding a key down makes GDK emit `key-pressed` again and again, and every
repeat is indistinguishable from a fresh press by the time it reaches the
router. The router latches the resolved `KeyCombo` for the duration of a
physical press and releases the latch on any `key-released`, so a held key is
one action.

Without that, every auto-repeat was dispatched as new user intent, which
thrashed every toggle and spammed every non-idempotent action:

- `F` (immersive browsing) walked immersive on/off/on at the OS repeat rate.
  Each repeat both re-revealed the chrome — any activity counts — and toggled
  immersion again, so the 220 ms transition never completed and a held `F`
  looked like the picture twitching rather than a mode change. The end state
  depended on the repeat count's parity, so the feature appeared to do nothing.
- `H` / `I` / `E` toggled the heart, details panel, and editor repeatedly, and
  `Delete` re-ran the move-to-trash path.
- Holding `→` spent the whole hold bumping `nav_token`, so each press cancelled
  the previous press's pending switch.

GTK 4 exposes no auto-repeat flag on `GdkKeyEvent`, which is why the latch is
released on a key release rather than filtered by time: a time window would
either swallow a genuine fast double-tap or let repeats through on a slow
machine.

The regression case is
`tests/ux_viewer_pointer_flows.rs::chrome_returns_and_stays_clickable_after_immersive_fold`,
which delivers a whole gesture — press, five auto-repeats, release — and
asserts that exactly one action was dispatched.

**Test helpers must model the release.** `emit_key_for_tests` (`src/ui/window.rs`)
and `emit_window_key` (`tests/ux_click_flows.rs`) used to emit `key-pressed`
alone, which models a key held forever: under the latch every second press of
the same key would be swallowed. Both now emit a matching `key-released`. Any
new helper that synthesizes a key press has to do the same, or it silently
tests a permanently-held key.

## Default Keymap

| Scope | Keys | Action |
|---|---|---|
| Global | `Escape` | `CancelOrClose` |
| Global | `Alt+Left` | `NavigateBack` |
| Global | `Ctrl+F` | `Search` |
| Global | `Ctrl+,` | `OpenSettings` |
| Global | `F1` / `Ctrl+/` / `Ctrl+?` | `ShowShortcuts` |
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
| Viewer | `F` | `ViewerImmersive` |
| Viewer | `Shift+F` | `ViewerFullscreenPreview` |
| Viewer | `I` | `ViewerToggleDetails` |
| Viewer | `E` | `ViewerToggleEdit` |
| Viewer | `H` | `ViewerToggleFavorite` |
| Viewer | `Delete` | `Delete` |

`Escape` in the viewer scope unwinds one layer per press, in this order: editor
panel → details panel → immersive browsing → navigation pop. Consuming the press
at the innermost layer is what keeps "back" predictable now that `F` changes the
same page instead of opening a second window.

`ViewerPrevious` / `ViewerNext` at a resolved end of the query (the arrow
itself is already dimmed, see [`docs/modules/viewer.md`](viewer.md) § *Navigation
Buttons*) return `KeyboardResult::Ignored` rather than `Handled`, so the press
is not silently consumed while there is nothing on screen to act on. While the
editor is open the same keys stay `Handled` so they do not leak into the
underlying grid's focus handlers.

Modal and editor scopes do not fall back to global shortcuts. This prevents
Search, Settings, and navigation commands from leaking through dialogs, glass
context menus, or editing surfaces. `MainWindow` treats any open glass context
menu as modal while focus remains on its triggering control; the menu handles
Escape from the root overlay's capture phase rather than taking focus itself.

Text input scope deliberately does not map printable shortcuts, including
`Ctrl+F`, so entries and search fields keep native editing behavior unless a
widget-specific handler forwards an action.

`Ctrl+F` opens Search from browsing pages and focuses an already-visible
`SearchPage` instead of pushing a duplicate page.

## Shortcut Discoverability

`shortcuts_window::GROUPS` is the single published list of bindings. It renders
as a modal `GtkShortcutsWindow` (`F1`, `Ctrl+/`, or the 「键盘」 row in Settings;
`Ctrl+?` is accepted too because `/` needs Shift on some layouts), and it feeds
the key hints appended to button tooltips through
`tooltip_with_key(label_key, action)`.

Two rules follow from that:

- **Adding, renaming, or removing a binding means adding a `GROUPS` row.**
  `shortcuts_window::tests` asserts both directions: every declared accelerator
  really resolves to the action it claims, and every action the router can reach
  in a scope has a row (Browsing and Viewer rows may fall through to the Global
  table). A binding without a row, or a row without a binding, fails the test.
- **`GtkShortcutsShortcut` takes one accelerator per row**, so a dual-convention
  binding such as `F1` / `Ctrl+/` is listed twice.

Implementation notes worth keeping:

- The window is built from a generated Builder XML skeleton rather than
  `ShortcutsWindow::add_section`, because that API needs the `v4_14` gtk4
  feature while this crate pins `v4_8`. Translated strings are attached as
  properties, never interpolated into the XML, so `&` and `<` in a catalog
  cannot break the parse.
- The window is rebuilt on every open so a runtime locale switch cannot leave a
  stale translated copy behind.
- `display_accelerator` spells keys the way the reference window does (letters
  capitalised, `Shift+R` kept explicit) but keeps its own glyphs for
  non-printables, since GTK translates `Up` to `上` and that reads badly in a
  tooltip. Folding Shift away used to render rotate-left as `R`, the glyph
  rotate-right already owned, so both actions advertised the same hint.
- `ViewerRotateLeft` is declared `<Shift>R` and `binding.rs` matches
  `Key::R | Key::r` with Shift, because `gtk_accelerator_parse("<Shift>R")`
  reports lowercase `r` plus the Shift modifier.
- `Restore` has no binding (the trash toolbar is its only entry point), so
  `accelerator_for` returns `None` and its tooltip degrades to a bare label.
- Ctrl+wheel zoom is implemented in `ViewerPage::stage_input` and is intentionally
  absent from the keyboard reference because that window advertises key
  accelerators rather than pointer gestures. Drag panning is available only
  after the image is enlarged.
