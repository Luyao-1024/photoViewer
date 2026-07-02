# Project-Wide Keyboard Adaptation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a centralized keyboard action system so viewer, browsing, editor, dialogs, and future pages can share consistent shortcuts without scattering raw `gdk::Key` matches.

**Architecture:** Create `src/ui/keyboard/` for action vocabulary, key binding lookup, scope resolution, and main-window routing. Pages expose small action handlers that call existing button/navigation methods. The first functional slice routes viewer shortcuts from any focused viewer child; later tasks move browsing/editor/dialog shortcuts onto the same system.

**Tech Stack:** Rust, gtk4-rs, Libadwaita, `gtk::EventControllerKey`, `gdk::Key`, `gdk::ModifierType`, existing `adw::NavigationView` pages.

**Implementation status (first slice):** `src/ui/keyboard/` now contains the
action vocabulary, binding resolver, focus guard, and named capture-phase
router. `MainWindow` resolves TextInput/Modal/Editor/Viewer/Browsing/Global
scopes conservatively and dispatches viewer/global actions. `ViewerPage`
handles router-backed Left/Right/Escape/Space plus image/chrome actions from
any focused viewer child. Global `Ctrl+F` opens Search and reuses the visible
Search page; `Ctrl+,` opens Settings; modal scopes block global fallthrough,
including visible glass context menus under the root overlay. Browsing bindings
intentionally remain resolved but ignored by the window router until page-level
browsing handlers are added.

---

## File Map

- Create `src/ui/keyboard/mod.rs`: module exports and install helper.
- Create `src/ui/keyboard/action.rs`: `KeyboardAction`, `KeyboardResult`.
- Create `src/ui/keyboard/binding.rs`: `KeyboardScope`, `KeyCombo`, default binding lookup, unit tests.
- Create `src/ui/keyboard/router.rs`: capture-phase key controller, active-page dispatch, text-input guard.
- Modify `src/ui/mod.rs`: export `keyboard`.
- Modify `src/ui/window.rs`: install the keyboard router in `MainWindow::new`; handle global settings/search/back actions.
- Modify `src/ui/viewer_page.rs`: expose `handle_keyboard_action`; move viewer-level shortcut behavior behind `KeyboardAction`.
- Modify `src/ui/photos_page.rs`: expose browsing-level action handler for search, selection cancel, and selection actions.
- Modify `src/ui/album_detail_page.rs`, `src/ui/search_page.rs`, `src/ui/trash_page.rs`: add browsing action handlers incrementally.
- Create `docs/modules/keyboard.md`: canonical keymap and conflict rules.
- Modify `docs/modules/ui-design.md`: link to `keyboard.md`.

## Task 1: Keyboard Action And Binding Model

**Files:**
- Create: `src/ui/keyboard/action.rs`
- Create: `src/ui/keyboard/binding.rs`
- Create: `src/ui/keyboard/mod.rs`
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: Add the module export**

Edit `src/ui/mod.rs` and add this near the other module declarations:

```rust
pub mod keyboard;
```

- [ ] **Step 2: Create `src/ui/keyboard/action.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardResult {
    Handled,
    Ignored,
}

impl KeyboardResult {
    pub fn is_handled(self) -> bool {
        matches!(self, Self::Handled)
    }
}
```

- [ ] **Step 3: Create `src/ui/keyboard/binding.rs` with failing tests first**

```rust
use gtk4::gdk;

use super::action::KeyboardAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyboardScope {
    TextInput,
    Modal,
    Editor,
    Viewer,
    Browsing,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub key: gdk::Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyCombo {
    pub fn new(key: gdk::Key, state: gdk::ModifierType) -> Self {
        Self {
            key,
            ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
            shift: state.contains(gdk::ModifierType::SHIFT_MASK),
            alt: state.contains(gdk::ModifierType::ALT_MASK)
                || state.contains(gdk::ModifierType::META_MASK),
        }
    }

    pub fn plain(key: gdk::Key) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }
}

pub fn resolve_binding(scope: KeyboardScope, combo: KeyCombo) -> Option<KeyboardAction> {
    scoped_binding(scope, combo).or_else(|| {
        if scope == KeyboardScope::Global {
            None
        } else {
            scoped_binding(KeyboardScope::Global, combo)
        }
    })
}

fn scoped_binding(scope: KeyboardScope, combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;
    use KeyboardScope::*;

    match scope {
        TextInput => None,
        Modal => match combo {
            c if c == KeyCombo::plain(gdk::Key::Escape) => Some(CancelOrClose),
            _ => None,
        },
        Editor => match combo {
            c if c == KeyCombo::plain(gdk::Key::Escape) => Some(CancelOrClose),
            _ => None,
        },
        Viewer => viewer_binding(combo),
        Browsing => browsing_binding(combo),
        Global => global_binding(combo),
    }
}

fn global_binding(combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;
    match combo {
        c if c == KeyCombo::plain(gdk::Key::Escape) => Some(CancelOrClose),
        KeyCombo { key: gdk::Key::Left, alt: true, ctrl: false, shift: false }
        | KeyCombo { key: gdk::Key::KP_Left, alt: true, ctrl: false, shift: false } => {
            Some(NavigateBack)
        }
        KeyCombo { key: gdk::Key::f, ctrl: true, alt: false, shift: false }
        | KeyCombo { key: gdk::Key::F, ctrl: true, alt: false, shift: true } => Some(Search),
        KeyCombo { key: gdk::Key::comma, ctrl: true, alt: false, shift: false } => {
            Some(OpenSettings)
        }
        _ => None,
    }
}

fn browsing_binding(combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;
    match combo {
        c if c == KeyCombo::plain(gdk::Key::Up) || c == KeyCombo::plain(gdk::Key::KP_Up) => {
            Some(BrowseUp)
        }
        c if c == KeyCombo::plain(gdk::Key::Down) || c == KeyCombo::plain(gdk::Key::KP_Down) => {
            Some(BrowseDown)
        }
        c if c == KeyCombo::plain(gdk::Key::Left) || c == KeyCombo::plain(gdk::Key::KP_Left) => {
            Some(BrowseLeft)
        }
        c if c == KeyCombo::plain(gdk::Key::Right) || c == KeyCombo::plain(gdk::Key::KP_Right) => {
            Some(BrowseRight)
        }
        c if c == KeyCombo::plain(gdk::Key::Return) || c == KeyCombo::plain(gdk::Key::KP_Enter) => {
            Some(ActivateFocused)
        }
        c if c == KeyCombo::plain(gdk::Key::space) || c == KeyCombo::plain(gdk::Key::KP_Space) => {
            Some(ToggleSelection)
        }
        KeyCombo { key: gdk::Key::a, ctrl: true, alt: false, shift: false }
        | KeyCombo { key: gdk::Key::A, ctrl: true, alt: false, shift: true } => Some(SelectAll),
        c if c == KeyCombo::plain(gdk::Key::Delete) => Some(Delete),
        _ => None,
    }
}

fn viewer_binding(combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;
    match combo {
        c if c == KeyCombo::plain(gdk::Key::Left) || c == KeyCombo::plain(gdk::Key::KP_Left) => {
            Some(ViewerPrevious)
        }
        c if c == KeyCombo::plain(gdk::Key::Right) || c == KeyCombo::plain(gdk::Key::KP_Right) => {
            Some(ViewerNext)
        }
        c if c == KeyCombo::plain(gdk::Key::Escape) => Some(CancelOrClose),
        c if c == KeyCombo::plain(gdk::Key::space) || c == KeyCombo::plain(gdk::Key::KP_Space) => {
            Some(ViewerTogglePlayback)
        }
        c if c == KeyCombo::plain(gdk::Key::plus)
            || c == KeyCombo::plain(gdk::Key::KP_Add)
            || c == KeyCombo::plain(gdk::Key::equal) =>
        {
            Some(ViewerZoomIn)
        }
        c if c == KeyCombo::plain(gdk::Key::minus)
            || c == KeyCombo::plain(gdk::Key::KP_Subtract) =>
        {
            Some(ViewerZoomOut)
        }
        c if c == KeyCombo::plain(gdk::Key::_0) || c == KeyCombo::plain(gdk::Key::KP_0) => {
            Some(ViewerZoomReset)
        }
        c if c == KeyCombo::plain(gdk::Key::r) => Some(ViewerRotateRight),
        KeyCombo { key: gdk::Key::R, shift: true, ctrl: false, alt: false } => {
            Some(ViewerRotateLeft)
        }
        c if c == KeyCombo::plain(gdk::Key::f) => Some(ViewerFullscreenPreview),
        c if c == KeyCombo::plain(gdk::Key::i) => Some(ViewerToggleDetails),
        c if c == KeyCombo::plain(gdk::Key::e) => Some(ViewerToggleEdit),
        c if c == KeyCombo::plain(gdk::Key::Delete) => Some(Delete),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyboardAction::*;
    use KeyboardScope::*;

    #[test]
    fn viewer_arrows_resolve_to_media_navigation() {
        assert_eq!(
            resolve_binding(Viewer, KeyCombo::plain(gdk::Key::Right)),
            Some(ViewerNext)
        );
        assert_eq!(
            resolve_binding(Viewer, KeyCombo::plain(gdk::Key::Left)),
            Some(ViewerPrevious)
        );
    }

    #[test]
    fn browsing_arrows_resolve_to_grid_navigation() {
        assert_eq!(
            resolve_binding(Browsing, KeyCombo::plain(gdk::Key::Right)),
            Some(BrowseRight)
        );
        assert_eq!(
            resolve_binding(Browsing, KeyCombo::plain(gdk::Key::Up)),
            Some(BrowseUp)
        );
    }

    #[test]
    fn text_input_suppresses_printable_app_shortcuts() {
        assert_eq!(resolve_binding(TextInput, KeyCombo::plain(gdk::Key::f)), None);
        assert_eq!(
            resolve_binding(
                TextInput,
                KeyCombo::new(gdk::Key::f, gdk::ModifierType::CONTROL_MASK)
            ),
            None
        );
    }

    #[test]
    fn non_global_scopes_fall_back_to_global_actions() {
        assert_eq!(
            resolve_binding(
                Viewer,
                KeyCombo::new(gdk::Key::comma, gdk::ModifierType::CONTROL_MASK)
            ),
            Some(OpenSettings)
        );
    }
}
```

- [ ] **Step 4: Create `src/ui/keyboard/mod.rs`**

```rust
pub mod action;
pub mod binding;
pub mod router;

pub use action::{KeyboardAction, KeyboardResult};
pub use binding::{resolve_binding, KeyCombo, KeyboardScope};
```

- [ ] **Step 5: Run binding tests before router exists**

Run:

```bash
cargo test keyboard::binding --lib
```

Expected before `router.rs` exists: compile failure stating `file not found for module router`. This confirms the module export is wired.

- [ ] **Step 6: Add temporary empty router module**

Create `src/ui/keyboard/router.rs`:

```rust
// Router implementation is added in Task 2.
```

- [ ] **Step 7: Run binding tests again**

Run:

```bash
cargo test keyboard::binding --lib
```

Expected: all `keyboard::binding` tests pass.

- [ ] **Step 8: Commit Task 1**

```bash
git add src/ui/mod.rs src/ui/keyboard
git commit -m "feat: add keyboard action bindings"
```

## Task 2: Main-Window Keyboard Router Skeleton

**Files:**
- Modify: `src/ui/keyboard/router.rs`
- Modify: `src/ui/window.rs`

- [ ] **Step 1: Write failing router installation test**

Add to the existing `#[cfg(test)] mod tests` in `src/ui/window.rs`:

```rust
#[gtk::test]
fn main_window_installs_single_keyboard_router() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardRouter")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);

    let key_controllers = window
        .observe_controllers()
        .snapshot()
        .into_iter()
        .filter_map(|controller| controller.downcast::<gtk::EventControllerKey>().ok())
        .count();

    assert_eq!(
        key_controllers, 1,
        "MainWindow should own one capture-phase keyboard router"
    );
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test main_window_installs_single_keyboard_router --lib
```

Expected: FAIL with `left: 0, right: 1`.

- [ ] **Step 3: Implement router installation**

Replace `src/ui/keyboard/router.rs` with:

```rust
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

use super::binding::{resolve_binding, KeyCombo, KeyboardScope};

pub fn install<W, F>(widget: &W, resolve_scope: F)
where
    W: IsA<gtk::Widget>,
    F: Fn() -> KeyboardScope + 'static,
{
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    key.connect_key_pressed(move |_, key, _keycode, state| {
        let combo = KeyCombo::new(key, state);
        let scope = resolve_scope();
        match resolve_binding(scope, combo) {
            Some(action) => {
                tracing::debug!("keyboard action resolved: {action:?}");
                glib::Propagation::Proceed
            }
            None => glib::Propagation::Proceed,
        }
    });
    widget.add_controller(key);
}

pub fn scope_for_focus(root: &gtk::Widget) -> KeyboardScope {
    if focus_is_text_input(root) {
        KeyboardScope::TextInput
    } else {
        KeyboardScope::Global
    }
}

fn focus_is_text_input(root: &gtk::Widget) -> bool {
    let Some(focus) = root.root().and_then(|root| root.focus()) else {
        return false;
    };
    focus.is::<gtk::Editable>()
        || focus.is::<gtk::TextView>()
        || focus.is::<gtk::SearchEntry>()
        || focus.is::<gtk::Entry>()
}
```

Then modify `src/ui/window.rs` imports:

```rust
use crate::ui::{grid_css, keyboard, theme};
```

Modify `MainWindow::new`:

```rust
pub fn new(app: &adw::Application) -> Self {
    let window: Self = gtk::glib::Object::builder()
        .property("application", app)
        .property("title", tr("app.title"))
        .build();
    keyboard::router::install(&window, {
        let weak = window.downgrade();
        move || {
            weak.upgrade()
                .map(|window| keyboard::router::scope_for_focus(window.upcast_ref()))
                .unwrap_or(keyboard::KeyboardScope::Global)
        }
    });
    window
}
```

- [ ] **Step 4: Run the router installation test**

Run:

```bash
cargo test main_window_installs_single_keyboard_router --lib
```

Expected: PASS.

- [ ] **Step 5: Commit Task 2**

```bash
git add src/ui/window.rs src/ui/keyboard/router.rs
git commit -m "feat: install keyboard router"
```

## Task 3: Active Page Scope Resolution

**Files:**
- Modify: `src/ui/keyboard/router.rs`
- Modify: `src/ui/window.rs`

- [ ] **Step 1: Write failing scope tests**

Add to `src/ui/window.rs` tests:

```rust
#[gtk::test]
fn keyboard_scope_is_viewer_when_viewer_page_is_visible() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardScopeViewer")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    let nav = window.nav_view();

    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(make_keyboard_test_media_item(1)));
    let viewer = crate::ui::ViewerPage::new(media_list, 0);
    nav.push(&viewer);

    assert_eq!(
        window.keyboard_scope_for_tests(),
        crate::ui::keyboard::KeyboardScope::Viewer
    );
}
```

Add this helper to the same `window.rs` test module:

```rust
fn make_keyboard_test_media_item(id: i64) -> crate::core::media::MediaItem {
    crate::core::media::MediaItem {
        id,
        uri: format!("file:///tmp/keyboard-{id}.jpg"),
        path: std::path::PathBuf::from(format!("/tmp/keyboard-{id}.jpg")),
        folder_path: std::path::PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 1024,
        blake3_hash: format!("keyboard-hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}
```

- [ ] **Step 2: Run the failing scope test**

Run:

```bash
cargo test keyboard_scope_is_viewer_when_viewer_page_is_visible --lib
```

Expected: FAIL because `keyboard_scope_for_tests` does not exist.

- [ ] **Step 3: Implement page scope resolver in `MainWindow`**

Add imports in `src/ui/window.rs`:

```rust
use crate::ui::{AlbumDetailPage, PhotosPage, SearchPage, ViewerPage};
```

Add methods on `impl MainWindow`:

```rust
fn resolve_keyboard_scope(&self) -> keyboard::KeyboardScope {
    let root_scope = keyboard::router::scope_for_focus(self.upcast_ref());
    if root_scope == keyboard::KeyboardScope::TextInput {
        return root_scope;
    }

    let Some(page) = self.imp().nav_view.get().visible_page() else {
        return keyboard::KeyboardScope::Global;
    };

    if let Ok(viewer) = page.clone().downcast::<ViewerPage>() {
        if viewer.is_editing_keyboard_scope() {
            return keyboard::KeyboardScope::Editor;
        }
        return keyboard::KeyboardScope::Viewer;
    }

    if page.clone().downcast::<PhotosPage>().is_ok()
        || page.clone().downcast::<AlbumDetailPage>().is_ok()
        || page.clone().downcast::<SearchPage>().is_ok()
        || page.clone().downcast::<TrashPage>().is_ok()
    {
        return keyboard::KeyboardScope::Browsing;
    }

    keyboard::KeyboardScope::Global
}

#[cfg(test)]
pub(crate) fn keyboard_scope_for_tests(&self) -> keyboard::KeyboardScope {
    self.resolve_keyboard_scope()
}
```

Modify router install in `MainWindow::new`:

```rust
keyboard::router::install(&window, {
    let weak = window.downgrade();
    move || {
        weak.upgrade()
            .map(|window| window.resolve_keyboard_scope())
            .unwrap_or(keyboard::KeyboardScope::Global)
    }
});
```

Add to `ViewerPage`:

```rust
pub(crate) fn is_editing_keyboard_scope(&self) -> bool {
    self.imp().editor_split_view.get().shows_sidebar() || self.imp().is_editing.get()
}
```

- [ ] **Step 4: Run scope tests**

Run:

```bash
cargo test keyboard_scope --lib
```

Expected: viewer scope tests pass. Existing tests still compile.

- [ ] **Step 5: Commit Task 3**

```bash
git add src/ui/window.rs src/ui/viewer_page.rs
git commit -m "feat: resolve keyboard scope from active page"
```

## Task 4: Dispatch Router Actions To MainWindow And Viewer

**Files:**
- Modify: `src/ui/keyboard/router.rs`
- Modify: `src/ui/window.rs`
- Modify: `src/ui/viewer_page.rs`

- [ ] **Step 1: Write failing viewer keyboard tests**

Add to `src/ui/viewer_page.rs` tests:

```rust
#[gtk::test]
fn viewer_keyboard_action_navigates_and_closes() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_cb = events.clone();
    viewer.connect_navigation(move |delta| {
        events_for_cb.borrow_mut().push(delta);
    });

    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerNext),
        crate::ui::keyboard::KeyboardResult::Handled
    );
    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerPrevious),
        crate::ui::keyboard::KeyboardResult::Handled
    );
    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::CancelOrClose),
        crate::ui::keyboard::KeyboardResult::Handled
    );

    assert_eq!(events.borrow().as_slice(), &[1, -1, NAV_POP]);
}
```

- [ ] **Step 2: Run the failing viewer action test**

Run:

```bash
cargo test viewer_keyboard_action_navigates_and_closes --lib
```

Expected: compile failure because `handle_keyboard_action` is not public.

- [ ] **Step 3: Implement viewer action handler**

Add import in `src/ui/viewer_page.rs`:

```rust
use crate::ui::keyboard::{KeyboardAction, KeyboardResult};
```

Add on `impl ViewerPage`:

```rust
pub(crate) fn handle_keyboard_action(&self, action: KeyboardAction) -> KeyboardResult {
    match action {
        KeyboardAction::ViewerNext => {
            if self.imp().is_editing.get() {
                KeyboardResult::Handled
            } else {
                self.navigate_by_delta(1);
                KeyboardResult::Handled
            }
        }
        KeyboardAction::ViewerPrevious => {
            if self.imp().is_editing.get() {
                KeyboardResult::Handled
            } else {
                self.navigate_by_delta(-1);
                KeyboardResult::Handled
            }
        }
        KeyboardAction::CancelOrClose => {
            if self.imp().editor_split_view.get().shows_sidebar() {
                self.stop_editing();
            } else if self.imp().details_split_view.get().shows_sidebar() {
                self.set_details_revealed(false, "keyboard action");
            } else {
                self.fire_nav(NAV_POP);
            }
            KeyboardResult::Handled
        }
        KeyboardAction::ViewerTogglePlayback => {
            if self.toggle_video_playback() {
                KeyboardResult::Handled
            } else {
                KeyboardResult::Ignored
            }
        }
        _ => KeyboardResult::Ignored,
    }
}
```

- [ ] **Step 4: Run viewer action test**

Run:

```bash
cargo test viewer_keyboard_action_navigates_and_closes --lib
```

Expected: PASS.

- [ ] **Step 5: Dispatch resolved actions**

Change `src/ui/keyboard/router.rs` install signature:

```rust
use super::action::{KeyboardAction, KeyboardResult};

pub fn install<W, F, H>(widget: &W, resolve_scope: F, handle_action: H)
where
    W: IsA<gtk::Widget>,
    F: Fn() -> KeyboardScope + 'static,
    H: Fn(KeyboardAction) -> KeyboardResult + 'static,
{
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    key.connect_key_pressed(move |_, key, _keycode, state| {
        let combo = KeyCombo::new(key, state);
        let scope = resolve_scope();
        let Some(action) = resolve_binding(scope, combo) else {
            return glib::Propagation::Proceed;
        };

        if handle_action(action).is_handled() {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    widget.add_controller(key);
}
```

Update `MainWindow::new`:

```rust
keyboard::router::install(
    &window,
    {
        let weak = window.downgrade();
        move || {
            weak.upgrade()
                .map(|window| window.resolve_keyboard_scope())
                .unwrap_or(keyboard::KeyboardScope::Global)
        }
    },
    {
        let weak = window.downgrade();
        move |action| {
            weak.upgrade()
                .map(|window| window.handle_keyboard_action(action))
                .unwrap_or(keyboard::KeyboardResult::Ignored)
        }
    },
);
```

Add on `MainWindow`:

```rust
fn handle_keyboard_action(&self, action: keyboard::KeyboardAction) -> keyboard::KeyboardResult {
    if let Some(page) = self.imp().nav_view.get().visible_page() {
        if let Ok(viewer) = page.clone().downcast::<ViewerPage>() {
            let result = viewer.handle_keyboard_action(action);
            if result.is_handled() {
                return result;
            }
        }
    }

    match action {
        keyboard::KeyboardAction::OpenSettings => {
            self.show_settings_dialog();
            keyboard::KeyboardResult::Handled
        }
        keyboard::KeyboardAction::NavigateBack | keyboard::KeyboardAction::CancelOrClose => {
            if self.imp().nav_view.get().pop() {
                keyboard::KeyboardResult::Handled
            } else {
                keyboard::KeyboardResult::Ignored
            }
        }
        _ => keyboard::KeyboardResult::Ignored,
    }
}
```

- [ ] **Step 6: Run router and viewer tests**

Run:

```bash
cargo test viewer_keyboard_action_navigates_and_closes --lib
cargo test main_window_installs_single_keyboard_router --lib
```

Expected: both pass.

- [ ] **Step 7: Commit Task 4**

```bash
git add src/ui/keyboard/router.rs src/ui/window.rs src/ui/viewer_page.rs
git commit -m "feat: dispatch keyboard actions to viewer"
```

## Task 5: Remove Viewer Focus Dependency

**Files:**
- Modify: `src/ui/viewer_page.rs`
- Modify: `src/ui/window.rs`

- [ ] **Step 1: Write failing integration-style test**

Add to `src/ui/window.rs` tests:

```rust
#[gtk::test]
fn viewer_right_key_navigates_when_focus_is_on_header_button() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardViewerFocus")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    let nav = window.nav_view();

    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(make_keyboard_test_media_item(1)));
    let viewer = ViewerPage::new(media_list, 0);

    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_cb = events.clone();
    viewer.connect_navigation(move |delta| events_for_cb.borrow_mut().push(delta));
    nav.push(&viewer);

    viewer.imp().details_btn.get().grab_focus();
    emit_key_for_tests(&window, gdk::Key::Right, gdk::ModifierType::empty());

    assert_eq!(events.borrow().as_slice(), &[1]);
}
```

Add helper in the same test module:

```rust
fn emit_key_for_tests<W: IsA<gtk::Widget>>(
    widget: &W,
    key: gdk::Key,
    state: gdk::ModifierType,
) {
    let controller = widget
        .observe_controllers()
        .snapshot()
        .into_iter()
        .find_map(|controller| controller.downcast::<gtk::EventControllerKey>().ok())
        .expect("keyboard router should be installed");
    let args: &[&dyn glib::value::ToValue] = &[&key, &0u32, &state];
    let _: bool = controller.emit_by_name("key-pressed", args);
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
cargo test viewer_right_key_navigates_when_focus_is_on_header_button --lib
```

Expected: FAIL if the router is not dispatching from `MainWindow` capture phase or if scope resolution does not see the viewer.

- [ ] **Step 3: Remove duplicate viewer picture/video key controllers**

In `ViewerPage::new`, remove:

```rust
obj.setup_keyboard();
```

Remove `install_viewer_key_controller`, `setup_keyboard`, and `handle_viewer_key`
only after tests prove the router covers equivalent behavior. Keep
`setup_video_playback_interactions` because mouse click playback remains local.

- [ ] **Step 4: Expand viewer handler for zoom and chrome actions**

Before adding handler cases, extract existing button closure bodies into these
named methods on `ViewerPage` and update the button setup code to call them:

```rust
fn keyboard_zoom_step(&self, direction: i32) {
    let next = step_zoom(self.imp().zoom_scale.get(), direction);
    self.set_viewer_zoom(next, self.imp().zoom_pan_x.get(), self.imp().zoom_pan_y.get());
}

fn keyboard_reset_zoom(&self) {
    self.reset_viewer_zoom();
}

fn keyboard_rotate_image(&self, degrees: i32) {
    self.rotate_viewer_image(degrees);
}

fn keyboard_open_fullscreen_preview(&self) {
    self.open_fullscreen_preview();
}
```

Inspect `setup_zoom_controls`, `setup_fullscreen_button`, and the rotate/reset
button setup code. Move the existing closure-local logic for reset, rotate, and
fullscreen preview into the named methods above, then call those methods from
both the existing buttons and the keyboard handler. Do not duplicate the logic.

Add these cases to `ViewerPage::handle_keyboard_action`:

```rust
KeyboardAction::ViewerZoomIn => {
    self.keyboard_zoom_step(1);
    KeyboardResult::Handled
}
KeyboardAction::ViewerZoomOut => {
    self.keyboard_zoom_step(-1);
    KeyboardResult::Handled
}
KeyboardAction::ViewerZoomReset => {
    self.keyboard_reset_zoom();
    KeyboardResult::Handled
}
KeyboardAction::ViewerRotateLeft => {
    self.keyboard_rotate_image(-90);
    KeyboardResult::Handled
}
KeyboardAction::ViewerRotateRight => {
    self.keyboard_rotate_image(90);
    KeyboardResult::Handled
}
KeyboardAction::ViewerFullscreenPreview => {
    self.keyboard_open_fullscreen_preview();
    KeyboardResult::Handled
}
KeyboardAction::ViewerToggleDetails => {
    let revealed = self.imp().details_split_view.get().shows_sidebar();
    self.set_details_revealed(!revealed, "keyboard action");
    KeyboardResult::Handled
}
KeyboardAction::ViewerToggleEdit => {
    if self.imp().edit_btn.get().is_sensitive() {
        self.start_editing();
        KeyboardResult::Handled
    } else {
        KeyboardResult::Ignored
    }
}
KeyboardAction::Delete => {
    self.imp().delete_btn.get().emit_clicked();
    KeyboardResult::Handled
}
```

- [ ] **Step 5: Run viewer and window keyboard tests**

Run:

```bash
cargo test viewer_keyboard_action --lib
cargo test viewer_right_key_navigates_when_focus_is_on_header_button --lib
cargo test escape_closes_details_panel_without_navigation_pop --lib
```

Expected: all pass.

- [ ] **Step 6: Commit Task 5**

```bash
git add src/ui/window.rs src/ui/viewer_page.rs
git commit -m "feat: route viewer keyboard shortcuts globally"
```

## Task 6: Browsing Scope Bridge

**Files:**
- Modify: `src/ui/photos_page.rs`
- Modify: `src/ui/album_detail_page.rs`
- Modify: `src/ui/search_page.rs`
- Modify: `src/ui/trash_page.rs`
- Modify: `src/ui/window.rs`

- [ ] **Step 1: Add page action handlers that return Ignored by default**

For each page, add a method with the same shape. Example for `PhotosPage`:

```rust
pub(crate) fn handle_keyboard_action(
    &self,
    action: crate::ui::keyboard::KeyboardAction,
) -> crate::ui::keyboard::KeyboardResult {
    match action {
        crate::ui::keyboard::KeyboardAction::Search => {
            self.open_search_from_keyboard();
            crate::ui::keyboard::KeyboardResult::Handled
        }
        crate::ui::keyboard::KeyboardAction::CancelOrClose => {
            if self.is_multi_select_mode() {
                self.clear_selection();
                crate::ui::keyboard::KeyboardResult::Handled
            } else {
                crate::ui::keyboard::KeyboardResult::Ignored
            }
        }
        _ => crate::ui::keyboard::KeyboardResult::Ignored,
    }
}
```

Extract existing search-button behavior into `open_search_from_keyboard` or a
more general existing method name. Do not duplicate search construction logic.

- [ ] **Step 2: Dispatch browsing actions from `MainWindow`**

Extend `MainWindow::handle_keyboard_action`:

```rust
if let Some(page) = self.imp().nav_view.get().visible_page() {
    if let Ok(photos) = page.clone().downcast::<PhotosPage>() {
        let result = photos.handle_keyboard_action(action);
        if result.is_handled() {
            return result;
        }
    }
    if let Ok(album) = page.clone().downcast::<AlbumDetailPage>() {
        let result = album.handle_keyboard_action(action);
        if result.is_handled() {
            return result;
        }
    }
    if let Ok(search) = page.clone().downcast::<SearchPage>() {
        let result = search.handle_keyboard_action(action);
        if result.is_handled() {
            return result;
        }
    }
    if let Ok(trash) = page.clone().downcast::<TrashPage>() {
        let result = trash.handle_keyboard_action(action);
        if result.is_handled() {
            return result;
        }
    }
}
```

- [ ] **Step 3: Keep FlowBox focus movement local**

Leave `grid_css.rs` arrow-key FlowBox movement in place for this task. The
router should return `Ignored` for `BrowseUp`, `BrowseDown`, `BrowseLeft`, and
`BrowseRight` until MediaGrid exposes an action-based focus API. This avoids a
behavior regression while the new system is introduced.

- [ ] **Step 4: Add tests for search shortcut and selection Escape**

Add focused tests to `tests/ux_click_flows.rs` or a new
`tests/keyboard_flows.rs`:

```rust
#[test]
fn ctrl_f_opens_search_from_photos_page() {
    gtk::init().expect("GTK init failed");
    let fixture = build_photos_page_with_nav();
    emit_key_for_tests(&fixture.window, gdk::Key::f, gdk::ModifierType::CONTROL_MASK);
    assert!(
        fixture.nav.visible_page().and_downcast::<SearchPage>().is_some(),
        "Ctrl+F should open SearchPage from browsing"
    );
}
```

- [ ] **Step 5: Run browsing keyboard tests**

Run:

```bash
cargo test keyboard_flows --test keyboard_flows
cargo test ux_click_flow_suite_including_album_sidebar_multi_select_deletes_real_albums --test ux_click_flows
```

Expected: all pass.

- [ ] **Step 6: Commit Task 6**

```bash
git add src/ui/photos_page.rs src/ui/album_detail_page.rs src/ui/search_page.rs src/ui/trash_page.rs src/ui/window.rs tests
git commit -m "feat: bridge browsing keyboard actions"
```

## Task 7: Documentation

**Files:**
- Create: `docs/modules/keyboard.md`
- Modify: `docs/modules/ui-design.md`
- Modify: `docs/modules/viewer.md`

- [ ] **Step 1: Create `docs/modules/keyboard.md`**

```markdown
# Keyboard Module

## Scope

Keyboard support is action-driven. Raw GTK key values are normalized in
`src/ui/keyboard/`, then routed to the active page as `KeyboardAction` values.
Pages should handle actions and call the same methods used by visible buttons.

## Scope Priority

`TextInput > Modal > Editor > Viewer > Browsing > Global`

Text input keeps native editing behavior. Modal surfaces consume close actions
before pages underneath see them. Viewer shortcuts do not navigate media while
the editor is open.

## Canonical Shortcuts

| Area | Keys | Action |
|---|---|---|
| Global | `Esc`, `Alt+Left` | Close or navigate back |
| Global | `Ctrl+F` | Search |
| Global | `Ctrl+,` | Settings |
| Viewer | `Left` / `Right` | Previous / next media |
| Viewer | `Space` | Toggle video playback |
| Viewer | `+` / `-` / `0` | Zoom in / out / reset |
| Viewer | `R` / `Shift+R` | Rotate right / left |
| Viewer | `F` | Fullscreen preview |
| Viewer | `I` | Details |
| Viewer | `E` | Edit |
| Browsing | Arrow keys | Move grid focus |
| Browsing | `Enter` | Open focused media |
| Browsing | `Space` | Toggle selection |
| Browsing | `Ctrl+A` | Select all |
| Browsing | `Delete` | Move selection to trash |
```

- [ ] **Step 2: Link from `docs/modules/ui-design.md`**

Add to the key implementation locations table:

```markdown
| Keyboard routing | `src/ui/keyboard/`, `docs/modules/keyboard.md` |
```

- [ ] **Step 3: Update viewer docs**

In `docs/modules/viewer.md`, keep the existing note that viewer controls are
button- and keyboard-driven, and add:

```markdown
Viewer keyboard shortcuts are routed through the shared keyboard subsystem,
not through picture/video-local key controllers. This keeps `Left`, `Right`,
and `Escape` reliable regardless of which viewer child currently owns focus.
```

- [ ] **Step 4: Run docs checks**

Run:

```bash
rg -n "keyboard subsystem|KeyboardAction|Keyboard Module" docs/modules
```

Expected: matches in `keyboard.md`, `ui-design.md`, and `viewer.md`.

- [ ] **Step 5: Commit Task 7**

```bash
git add docs/modules/keyboard.md docs/modules/ui-design.md docs/modules/viewer.md
git commit -m "docs: document keyboard routing"
```

## Task 8: Verification

**Files:**
- No source changes unless verification finds a defect.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt
```

Expected: exit code 0.

- [ ] **Step 2: Run focused keyboard tests**

Run:

```bash
cargo test keyboard --lib
cargo test viewer_keyboard_action --lib
cargo test keyboard_flows --test keyboard_flows
```

Expected: all pass.

- [ ] **Step 3: Run broader UI regression tests**

Run:

```bash
cargo test --lib
cargo test --test sidebar_navigation
cargo test --test ux_click_flows
```

Expected: all pass. Existing GTK `backdrop-filter` warnings are acceptable if
the test process exits successfully.

- [ ] **Step 4: Check diff hygiene**

Run:

```bash
git diff --check
git status --short
```

Expected: `git diff --check` exits 0. `git status --short` lists only files
intentionally changed by the keyboard work plus any pre-existing user changes.

- [ ] **Step 5: Final commit**

```bash
git add src/ui docs/modules tests
git commit -m "feat: add project-wide keyboard routing"
```

## Self-Review Notes

- Spec coverage: The plan covers action vocabulary, scope resolution, dispatch,
  viewer-first rollout, browsing bridge, docs, and verification.
- Placeholder scan: No task depends on an unspecified file or undefined phase.
  Where existing helper names may differ, the task instructs extracting existing
  button closures into named methods before wiring keyboard actions.
- Scope control: FlowBox arrow movement remains local until MediaGrid exposes a
  proper action API, which avoids breaking current browsing behavior during the
  router introduction.
