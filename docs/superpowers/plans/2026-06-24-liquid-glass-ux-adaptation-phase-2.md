# Liquid Glass UX Adaptation — Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the remaining liquid-glass surface work that Phase 1 explicitly deferred — apply the glass material to the editor page, the editor's save popover, and the album picker Copy/Move dialog — and add a `prefers-reduced-transparency` / `prefers-contrast` accessibility fallback so the system degrades to stable opaque surfaces when the user requests it.

**Architecture:** This is a pure CSS + tiny template/Rust touch-up pass. All visual rules extend the existing `GRID_CSS` constant in `src/ui/grid_css.rs`; templates get a few extra `css-classes: [...]` entries; only `album_picker.rs` has any meaningful Rust change (drop the `pill` + `suggested-action` / `destructive-action` classes and use the new glass vocabulary). No new widgets, no new pages, no data-layer changes. The accessibility fallback uses CSS `@media (prefers-reduced-transparency: reduce)` and `@media (prefers-contrast: more)` to override the glass material with opaque surfaces — no Rust involvement.

**Tech Stack:** Rust + gtk4 0.8 + libadwaita 0.6 + Blueprint UI templates + `glib_build_tools` GResource. CSS is installed via the existing `grid_css::install()` (`STYLE_PROVIDER_PRIORITY_APPLICATION`). Tests use the in-tree `gtk::init()` headless pattern (see `tests/smoke.rs`, `tests/sidebar_navigation.rs`).

## Global Constraints

- Edit `.blp` source files, never the generated `.ui` (per `CLAUDE.md`).
- `cargo fmt && cargo clippy --all-targets` clean at the end of every task.
- Per `CLAUDE.md` / `CONTRIBUTING.md`: TDD (failing test first, then implement to green, then commit).
- All CSS lives in `src/ui/grid_css.rs` (one `GRID_CSS` constant, one `install()`). Do not add new `CssProvider`s per widget.
- GTK4 CSS in this version does **not** reliably support `@define-color` token reuse or custom properties; values that the spec lists as `--glass-*` are documented as comments in the CSS and reused via grouped selectors, not assumed to be available as variables. If a token shows up in two places, copy it as a comment in both rules and update both when the value changes.
- Do not modify: `src/core/*`, thumbnail cache generation, DB schema, file watcher logic, trash/favorite persistence logic, Flatpak manifest, or Cargo dependencies.
- Do not apply `backdrop-filter` to thumbnail tiles — it is too expensive at 10k–100k tiles and would blur the photo content.
- Bilingual (Chinese + English) doc comments, matching the existing codebase style.
- The accessibility fallback must be non-intrusive: when neither `prefers-reduced-transparency` nor `prefers-contrast` is set, the glass look is identical to today. When set, surfaces degrade to a stable opaque neutral (alpha-white fills → solid neutral gray, blur removed).

## File Structure

| File | Responsibility |
|---|---|
| `src/ui/grid_css.rs` | Adds `.glass-editor-preview` (subtle stage for the editor canvas) and the `@media (prefers-reduced-transparency: reduce)` / `@media (prefers-contrast: more)` fallback blocks. |
| `data/ui/editor-page.blp` | `header_bar` gets `glass-header`. `cancel_btn`, `save_copy_btn`, `save_menu_btn`, `rotate_*`, `start_crop_btn` get `glass-toolbar-button`. `save_copy_btn` also gets `glass-toolbar-suggested`. `preview_overlay` gets `glass-editor-preview`. The PreferencesPage groups stay default libadwaita (those are read-only metadata surfaces, not chrome). |
| `src/ui/editor_page.rs` | No code change required (everything is in the `.blp`). But: confirm `setup_save_menu` adds `glass-menu` to the popover it builds — needs a one-line `add_css_class` because `PopoverMenu::from_model` constructs a `GtkPopoverMenu`, not a template child. |
| `src/ui/album_picker.rs` | Drop `pill` + `suggested-action` on `copy_btn`; drop `pill` + `destructive-action` on `move_btn`. Replace with the glass vocabulary: `glass-toolbar-button` + `glass-toolbar-suggested` and `glass-toolbar-button` + `glass-toolbar-danger`. The dialog's `Adw::ToolbarView` `top-bar` (the dialog header) also gets `glass-base`. |
| `tests/ui_editor_page.rs` | New test file: asserts `EditorPage` constructs with the new glass classes on header + buttons, the save-menu popover is a `glass-menu` after `setup_save_menu`, and the `glass-editor-preview` is applied to `preview_overlay`. |
| `tests/ui_album_picker.rs` | New test file: opens an `AlbumPickerDialog` (via `AlbumPickerDialog::present` against an `AdwNavigationView` built in-test) and asserts the Copy/Move buttons carry the glass vocabulary, no longer `pill` / `suggested-action` / `destructive-action`. |
| `tests/ui_accessibility_fallback.rs` | New test file: asserts the `@media` blocks are present in `GRID_CSS` and the fallback rules (e.g. `background: #2a2a2a`) reference a stable opaque surface. |

The Phase 1 `box.mode-selector` rule and all the `.glass-*` classes from Phase 1 are reused as-is.

---

## Task 1: Editor page header + buttons

**Files:**
- Modify: `data/ui/editor-page.blp:10-28, 30-52` (header classes + buttons)
- Test: `tests/ui_editor_page.rs` (new)

**Interfaces:**
- Consumes: `.glass-header`, `.glass-toolbar-button`, `.glass-toolbar-danger` (Phase 1).
- Produces: `EditorPage` template applies the glass vocabulary to all header buttons; the new `.glass-editor-preview` class is ready for Task 2 to apply to the preview area.

- [ ] **Step 1: Write the failing test**

Create `tests/ui_editor_page.rs`:

```rust
//! EditorPage carries the same liquid-glass material as the rest of the
//! app: header gets `glass-header`, action buttons get `glass-toolbar-button`,
//! save_copy gets `glass-toolbar-suggested`, preview_overlay gets
//! `glass-editor-preview` (Task 2), and the save-menu popover built by
//! `setup_save_menu` gets `glass-menu`.

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use photo_viewer::core::media::MediaItem;
use photo_viewer::ui::{grid_css, EditorPage};

fn probe_classes<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

#[test]
fn editor_page_uses_glass_classes() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.EditorPageGlass")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    grid_css::install();

    let media = MediaItem {
        id: 1,
        uri: "file:///tmp/one.jpg".into(),
        path: "/tmp/one.jpg".into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        width: Some(100),
        height: Some(100),
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 100,
        blake3_hash: "hash".into(),
        trashed_at: None,
    };
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();

    let page = EditorPage::new(media, pool);
    let imp = page.imp();

    // Header gets glass-header.
    let header = probe_classes(&imp.header_bar.get());
    assert!(header.iter().any(|c| c == "glass-header"),
        "header_bar should carry glass-header, got {header:?}");

    // Header buttons (cancel / save_copy / save_menu) carry glass-toolbar-button.
    let cancel = probe_classes(&imp.cancel_btn.get());
    let save_copy = probe_classes(&imp.save_copy_btn.get());
    let save_menu = probe_classes(&imp.save_menu_btn.get());
    for (name, classes) in [("cancel_btn", &cancel), ("save_copy_btn", &save_copy), ("save_menu_btn", &save_menu)] {
        assert!(classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}");
    }

    // save_copy_btn also carries glass-toolbar-suggested (the primary action).
    assert!(save_copy.iter().any(|c| c == "glass-toolbar-suggested"),
        "save_copy_btn should carry glass-toolbar-suggested, got {save_copy:?}");

    // Adjustment + crop buttons carry glass-toolbar-button.
    for (name, btn) in [
        ("rotate_90_cw", imp.rotate_90_cw.get()),
        ("rotate_180", imp.rotate_180.get()),
        ("rotate_90_ccw", imp.rotate_90_ccw.get()),
        ("start_crop_btn", imp.start_crop_btn.get()),
    ] {
        let classes = probe_classes(&btn);
        assert!(classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}");
    }
}
```

Run: `cargo test --test ui_editor_page editor_page_uses_glass_classes -- --nocapture`
Expected: FAIL — the buttons currently carry no glass classes (or the `.blp` doesn't yet have the class lists).

- [ ] **Step 2: Add the missing `glass-toolbar-suggested` rule to `grid_css.rs`**

Append to `GRID_CSS` (right after `.glass-toolbar-danger:hover`):

```css
.glass-toolbar-suggested { color: #a8d2ff; }
.glass-toolbar-suggested:hover {
  background: alpha(#5aa7ff, 0.18);
  color: #c8e0ff;
}
```

- [ ] **Step 3: Edit `data/ui/editor-page.blp`**

In `editor-page.blp`, change the `Adw.HeaderBar` to add `glass-header` and the four header buttons to carry `glass-toolbar-button` (and `glass-toolbar-suggested` for save_copy). Also update the three rotate buttons and the crop button. The exact edit is:

```blueprint
Adw.HeaderBar header_bar {
  show-end-title-buttons: true;
  css-classes: ["glass-header"];

  [start]
  Gtk.Button cancel_btn {
    label: "";
    css-classes: ["glass-toolbar-button"];
  }

  [end]
  Gtk.Button save_copy_btn {
    label: "";
    css-classes: ["glass-toolbar-button", "glass-toolbar-suggested"];
  }

  [end]
  Gtk.MenuButton save_menu_btn {
    label: "";
    css-classes: ["glass-toolbar-button"];
  }
}
```

And in the preferences page groups:

```blueprint
Gtk.Button rotate_90_cw {
  label: "";
  icon-name: "object-rotate-right-symbolic";
  css-classes: ["glass-toolbar-button"];
}

Gtk.Button rotate_180 {
  label: "";
  css-classes: ["glass-toolbar-button"];
}

Gtk.Button rotate_90_ccw {
  label: "";
  icon-name: "object-rotate-left-symbolic";
  css-classes: ["glass-toolbar-button"];
}
```

And:

```blueprint
Gtk.Button start_crop_btn {
  label: "";
  css-classes: ["glass-toolbar-button"];
}
```

- [ ] **Step 4: Re-run the test**

Run: `cargo test --test ui_editor_page -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add data/ui/editor-page.blp data/ui/editor-page.ui src/ui/grid_css.rs tests/ui_editor_page.rs
git commit -m "feat(ui): apply glass toolbar classes to editor page"
```

---

## Task 2: Editor save-menu popover + preview stage

**Files:**
- Modify: `data/ui/editor-page.blp:30-52` (preview_overlay class)
- Modify: `src/ui/editor_page.rs:288-298` (set `glass-menu` on the popover in `setup_save_menu`)
- Test: extend `tests/ui_editor_page.rs` (one new `#[test]`)

**Interfaces:**
- Consumes: `.glass-menu` (Phase 1), the new `.glass-editor-preview` class (defined here).
- Produces: the editor's `Save ▼` popover matches the right-click `glass-menu` look; the preview canvas has a subtle `glass-editor-preview` surface so the photo reads as content, not chrome.

- [ ] **Step 1: Add `.glass-editor-preview` to `GRID_CSS`**

Append to `GRID_CSS` (right after `.viewer-stage`):

```css
/* glass-editor-preview — analogous to .viewer-stage, but calmer: the
   editor's adjustment sliders occupy the same screen and need every
   ounce of readable chrome, so this is a near-flat panel with a hairline
   border, not a heavy glass stage. */
.glass-editor-preview {
  padding: 24px;
  background: alpha(black, 0.06);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.06);
}
```

- [ ] **Step 2: Apply `glass-editor-preview` to `preview_overlay` in the `.blp`**

Change in `data/ui/editor-page.blp`:

```blueprint
Gtk.Overlay preview_overlay {
  vexpand: true;
  hexpand: true;
  css-classes: ["glass-editor-preview"];

  child: Gtk.ScrolledWindow preview_scroll {
```

- [ ] **Step 3: Apply `glass-menu` to the popover in `setup_save_menu`**

In `src/ui/editor_page.rs:288-298`, change the `setup_save_menu` body to add the class right after constructing the popover:

```rust
let popover = gtk::PopoverMenu::from_model(Some(&menu));
popover.set_has_arrow(false);
popover.add_css_class("glass-menu");
self.imp().save_menu_btn.get().set_popover(Some(&popover));
```

- [ ] **Step 4: Write the failing test for the popover**

Append to `tests/ui_editor_page.rs`:

```rust
#[test]
fn editor_save_menu_uses_glass_menu() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.EditorSaveMenu")
        .build();
    app.register(None::<&gtk::gio::Cancellable>).unwrap();
    grid_css::install();

    let media = MediaItem {
        id: 1, uri: "file:///tmp/one.jpg".into(), path: "/tmp/one.jpg".into(),
        folder_path: "/tmp".into(), mime_type: "image/jpeg".into(),
        width: Some(100), height: Some(100), taken_at: None,
        file_mtime: chrono::Utc::now(), file_size: 100, blake3_hash: "h".into(),
        trashed_at: None,
    };
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let page = EditorPage::new(media, pool);
    let imp = page.imp();

    // preview_overlay carries glass-editor-preview.
    let preview_classes = probe_classes(&imp.preview_overlay.get());
    assert!(preview_classes.iter().any(|c| c == "glass-editor-preview"),
        "preview_overlay should carry glass-editor-preview, got {preview_classes:?}");

    // The save_menu_btn's popover is a GtkPopoverMenu carrying glass-menu.
    let popover: gtk::Popover = imp.save_menu_btn.get().popover().expect("save_menu_btn should have a popover");
    let pop_classes = probe_classes(&popover);
    assert!(pop_classes.iter().any(|c| c == "glass-menu"),
        "save popover should carry glass-menu, got {pop_classes:?}");
}
```

Run: `cargo test --test ui_editor_page -- --nocapture`
Expected: FAIL — neither `glass-editor-preview` nor `glass-menu` is on those widgets yet.

- [ ] **Step 5: Re-run the test**

Run: `cargo test --test ui_editor_page -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add data/ui/editor-page.blp data/ui/editor-page.ui src/ui/editor_page.rs tests/ui_editor_page.rs
git commit -m "feat(ui): apply glass menu + preview stage to editor page"
```

---

## Task 3: Album picker Copy/Move dialog

**Files:**
- Modify: `src/ui/album_picker.rs:184-190` (drop `pill` + `suggested-action` / `destructive-action`; add glass vocabulary)
- Test: `tests/ui_album_picker.rs` (new)

**Interfaces:**
- Consumes: `.glass-toolbar-button`, `.glass-toolbar-suggested`, `.glass-toolbar-danger` (Task 1 + Phase 1).
- Produces: the `AlbumPickerDialog` Copy/Move dialog buttons match the rest of the app's glass action language.

- [ ] **Step 1: Write the failing test**

Create `tests/ui_album_picker.rs`:

```rust
//! AlbumPickerDialog Copy/Move buttons use the glass vocabulary introduced
//! in Phase 1 + Task 1. They must NOT carry the old `pill` / `suggested-action`
//! / `destructive-action` libadwaita defaults.

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use photo_viewer::ui::{grid_css, AlbumPickerDialog};
use std::sync::Arc;

fn probe_classes<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

#[test]
fn album_picker_buttons_use_glass() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.AlbumPickerGlass")
        .build();
    app.register(None::<&gtk::gio::Cancellable>).unwrap();
    grid_css::install();

    let host_nav = adw::NavigationView::new();
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();

    // present() pushes two pages: an album-list, then (if any album exists)
    // a Copy/Move chooser. The chooser is the second page pushed. We just
    // need at least one chooser to be reachable; with an empty DB the
    // chooser is the "no albums yet" empty-state page, which doesn't carry
    // the buttons we're testing. So seed at least one album.
    let pool_seeder = pool.clone();
    let conn = pool_seeder.get().expect("pool get");
    conn.execute(
        "INSERT INTO albums (name, folder_path, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params!["Test Album", "/tmp/test-album", chrono::Utc::now().to_rfc3339()],
    ).expect("seed album");

    AlbumPickerDialog::present(&host_nav, pool, vec![1]);
    // The dialog is now on the navigation view's stack. Find the chooser page
    // (the second pushed page) and assert on its buttons.
    let nav_imp = host_nav;
    // Walk down to find the inner navigation view; the dialog pushes an
    // AdwNavigationView on top. We expect the chooser to be reachable via
    // visible_page after the chooser pushes.
    // Simpler: the buttons are exposed nowhere as template children, so
    // walk the page tree and assert that at least one button has the
    // expected classes.
    let mut found_copy = None;
    let mut found_move = None;
    let mut to_visit: Vec<gtk::Widget> = vec![host_nav.upcast::<gtk::Widget>()];
    while let Some(w) = to_visit.pop() {
        let mut next = w.first_child();
        while let Some(c) = next {
            to_visit.push(c.clone());
            next = c.next_sibling();
        }
        if let Some(btn) = w.downcast_ref::<gtk::Button>() {
            let label = btn.label().unwrap_or_default().to_string();
            let classes = probe_classes(&btn);
            if label == "复制" || label == "Copy" {
                found_copy = Some(classes);
            } else if label == "移动" || label == "Move" {
                found_move = Some(classes);
            }
        }
    }
    let copy = found_copy.expect("Copy button should exist after AlbumPickerDialog::present");
    let mov = found_move.expect("Move button should exist after AlbumPickerDialog::present");
    assert!(copy.iter().any(|c| c == "glass-toolbar-button"),
        "Copy button should carry glass-toolbar-button, got {copy:?}");
    assert!(copy.iter().any(|c| c == "glass-toolbar-suggested"),
        "Copy button should carry glass-toolbar-suggested, got {copy:?}");
    assert!(!copy.iter().any(|c| c == "pill"),
        "Copy button must NOT carry pill, got {copy:?}");
    assert!(!copy.iter().any(|c| c == "suggested-action"),
        "Copy button must NOT carry suggested-action, got {copy:?}");
    assert!(mov.iter().any(|c| c == "glass-toolbar-button"),
        "Move button should carry glass-toolbar-button, got {mov:?}");
    assert!(mov.iter().any(|c| c == "glass-toolbar-danger"),
        "Move button should carry glass-toolbar-danger, got {mov:?}");
    assert!(!mov.iter().any(|c| c == "pill"),
        "Move button must NOT carry pill, got {mov:?}");
    assert!(!mov.iter().any(|c| c == "destructive-action"),
        "Move button must NOT carry destructive-action, got {mov:?}");
}
```

(Adjust the seed + button labels to match the project's actual `tr()` strings — `tests/common/mod.rs` and `i18n/zh.json` have the canonical list. The structural assertions stay the same.)

Run: `cargo test --test ui_album_picker album_picker_buttons_use_glass -- --nocapture`
Expected: FAIL — buttons still carry `pill` + the libadwaita action classes.

- [ ] **Step 2: Drop the old classes in `album_picker.rs`**

In `src/ui/album_picker.rs:184-190`, replace the four `add_css_class` calls with:

```rust
let copy_btn = gtk::Button::with_label(&tr("album_picker.copy"));
copy_btn.add_css_class("glass-toolbar-button");
copy_btn.add_css_class("glass-toolbar-suggested");

let move_btn = gtk::Button::with_label(&tr("album_picker.move"));
move_btn.add_css_class("glass-toolbar-button");
move_btn.add_css_class("glass-toolbar-danger");
```

- [ ] **Step 3: Re-run the test**

Run: `cargo test --test ui_album_picker -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/ui/album_picker.rs tests/ui_album_picker.rs
git commit -m "feat(ui): apply glass vocabulary to album picker dialog buttons"
```

---

## Task 4: Accessibility fallback — `prefers-reduced-transparency`

**Files:**
- Modify: `src/ui/grid_css.rs` (append a `@media (prefers-reduced-transparency: reduce)` block)
- Test: `tests/ui_accessibility_fallback.rs` (new — string-presence assertions, mirroring the Phase 1 `favorite_active_has_hover_override` style)

**Interfaces:**
- Consumes: every glass class defined in Phase 1.
- Produces: when the user's environment signals `prefers-reduced-transparency: reduce`, all glass surfaces degrade to a stable opaque neutral (alpha-white fills → solid neutral dark; `backdrop-filter` removed). The same look should be reachable by setting the system GNOME accessibility setting.

- [ ] **Step 1: Write the failing test**

Create `tests/ui_accessibility_fallback.rs`:

```rust
//! The grid CSS provider defines a `prefers-reduced-transparency: reduce`
//! media block that overrides every glass material with a stable opaque
//! neutral. This is the platform-level accessibility fallback (per the
//! spec's section 7) and is opt-out: when the media query does NOT match,
//! the rules are ignored and the glass look is unchanged.

use gtk4 as gtk;
use gtk4::prelude::*;
use photo_viewer::ui::grid_css;

const GRID_CSS: &str = include_str!("../src/ui/grid_css.rs"); // unused; we read via the install test

#[test]
fn reduced_transparency_fallback_present() {
    // GRID_CSS is private; check via a fresh CssProvider the same way
    // the install tests do. We just need the text to contain the @media block.
    // Hack: re-export the constant for tests by checking `grid_css::GRID_CSS`
    // via a public getter we add in step 2. For now, fail loudly if the
    // block is not present.
    // The real assertion runs after the getter is added in step 2.
    let css = grid_css::css_for_tests();
    assert!(css.contains("@media (prefers-reduced-transparency: reduce)"),
        "GRID_CSS must contain a reduced-transparency @media block");
    // The block must override at least .glass-base and .glass-raised with
    // opaque backgrounds (no `alpha(...)`).
    let block_start = css.find("@media (prefers-reduced-transparency: reduce)")
        .expect("reduced-transparency block not present");
    let block = &css[block_start..];
    assert!(block.contains(".glass-base"), "block must override .glass-base");
    assert!(block.contains(".glass-raised"), "block must override .glass-raised");
    // No `backdrop-filter` inside the block (full removal, not just dim).
    assert!(!block.contains("backdrop-filter"),
        "reduced-transparency block must remove backdrop-filter entirely");
}
```

Run: `cargo test --test ui_accessibility_fallback reduced_transparency_fallback_present -- --nocapture`
Expected: FAIL — the getter `grid_css::css_for_tests()` doesn't exist yet, and neither does the `@media` block.

- [ ] **Step 2: Expose the CSS string for tests**

In `src/ui/grid_css.rs`, add (just below the `GRID_CSS` constant):

```rust
/// Test-only getter for the CSS string. Not for production use.
#[doc(hidden)]
pub fn css_for_tests() -> &'static str {
    GRID_CSS
}
```

- [ ] **Step 3: Add the `@media` block to `GRID_CSS`**

Append to `GRID_CSS`:

```css
/* ── Accessibility fallback ──────────────────────────────────────────
   When the user has enabled reduced-transparency (GNOME Settings → Accessibility
   → Seeing → Reduce animation OR a system "prefers-reduced-transparency" hint),
   every glass surface degrades to a stable opaque neutral. The fallback is
   scoped to surfaces that use alpha fills or backdrop-filter, so non-glass
   elements (Adwaita defaults, photo tiles) are untouched. */
@media (prefers-reduced-transparency: reduce) {
  .glass-base,
  .glass-raised,
  .glass-header,
  .glass-sidebar,
  .glass-toolbar-button,
  .glass-menu > contents,
  .viewer-stage,
  .viewer-details-panel,
  .glass-editor-preview {
    background: #1f1f23;
    background-clip: padding-box;
    border-color: alpha(white, 0.10);
    backdrop-filter: none;
    box-shadow: none;
  }
  .glass-toolbar-button {
    background: #2a2a30;
  }
  .glass-menu > contents {
    background: #1f1f23;
  }
}
```

- [ ] **Step 4: Re-run the test**

Run: `cargo test --test ui_accessibility_fallback -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/ui/grid_css.rs tests/ui_accessibility_fallback.rs
git commit -m "feat(ui): add reduced-transparency accessibility fallback to glass CSS"
```

---

## Task 5: Accessibility fallback — `prefers-contrast: more`

**Files:**
- Modify: `src/ui/grid_css.rs` (append a second `@media (prefers-contrast: more)` block)
- Test: extend `tests/ui_accessibility_fallback.rs` (one new `#[test]`)

**Interfaces:**
- Consumes: every glass class defined in Phase 1.
- Produces: when the user's environment signals `prefers-contrast: more` (high-contrast accessibility mode), every glass surface gets a thicker, fully-opaque border and a higher-contrast text color, so the UI remains usable without depending on subtle alpha effects.

- [ ] **Step 1: Write the failing test**

Append to `tests/ui_accessibility_fallback.rs`:

```rust
#[test]
fn high_contrast_fallback_present() {
    let css = grid_css::css_for_tests();
    assert!(css.contains("@media (prefers-contrast: more)"),
        "GRID_CSS must contain a high-contrast @media block");
    let block_start = css.find("@media (prefers-contrast: more)")
        .expect("high-contrast block not present");
    let block = &css[block_start..];
    // Bumps border thickness + forces opaque borders.
    assert!(block.contains("border: 2px solid"),
        "high-contrast block must force thicker opaque borders");
    assert!(block.contains(".glass-base"), "block must override .glass-base");
    assert!(block.contains(".glass-raised"), "block must override .glass-raised");
}
```

Run: `cargo test --test ui_accessibility_fallback high_contrast_fallback_present -- --nocapture`
Expected: FAIL — the block doesn't exist yet.

- [ ] **Step 2: Add the `@media` block to `GRID_CSS`**

Append to `GRID_CSS` (right after the reduced-transparency block):

```css
/* High-contrast accessibility fallback. Same scope as the
   reduced-transparency block. We force 2px opaque borders and a
   slightly brighter text color so the design language remains
   readable when the user has bumped contrast in GNOME Settings. */
@media (prefers-contrast: more) {
  .glass-base,
  .glass-raised,
  .glass-header,
  .glass-sidebar,
  .glass-toolbar-button,
  .glass-menu > contents,
  .viewer-stage,
  .viewer-details-panel,
  .glass-editor-preview {
    border: 2px solid alpha(white, 0.80);
    background: #1f1f23;
  }
  .glass-menu > contents {
    background: #1f1f23;
  }
  .glass-toolbar-button,
  .glass-menu-item,
  .glass-sidebar-row {
    color: #ffffff;
  }
  /* Hover/focus states still need a visible response in high-contrast mode. */
  .glass-toolbar-button:hover,
  .glass-menu-item:hover,
  .glass-sidebar-row:hover {
    background: alpha(white, 0.32);
  }
}
```

- [ ] **Step 3: Re-run the test**

Run: `cargo test --test ui_accessibility_fallback -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/ui/grid_css.rs tests/ui_accessibility_fallback.rs
git commit -m "feat(ui): add high-contrast accessibility fallback to glass CSS"
```

---

## Task 6: Final acceptance verification + flatpak build

**Files:** none (verification only, plus the visual-confirmation note in `.superpowers/sdd/progress.md`)

**Interfaces:** none.

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --all-targets -- --nocapture`
Expected: all green. The new test files (`ui_editor_page`, `ui_album_picker`, `ui_accessibility_fallback`) must pass alongside the Phase 1 suite (60+ tests).

- [ ] **Step 2: Run clippy + fmt**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Build the flatpak and install for visual confirmation**

```bash
flatpak-builder --user --install --force-clean .flatpak-builder build-aux/flatpak/org.gnome.PhotoViewer.json
flatpak run org.gnome.PhotoViewer
```

Verify each of the new surfaces against the spec's section K + the editor-page items added by Phase 2:

- Editor header reads as one system with the photos header (`glass-header` + glass-toolbar buttons).
- The save menu popover matches the right-click popover (`glass-menu` background + `glass-menu-item` rows).
- The AlbumPicker Copy/Move dialog uses the same button language as the viewer toolbar (suggested blue for Copy, soft red for Move).
- Enable GNOME Settings → Accessibility → Reduce animation, restart the app: every glass surface is now a solid `#1f1f23` panel — the photo grid, header, sidebar, mode selector, all glass controls.
- Enable GNOME Settings → Accessibility → High contrast, restart the app: borders are 2px, no subtle alpha effects, text is fully opaque white.

- [ ] **Step 4: Append a Phase 2 completion note to `.superpowers/sdd/progress.md`**

```markdown
## Phase 2: Editor + Album Picker + Accessibility

- [x] Task 1: Editor header + buttons — `feat(ui): apply glass toolbar classes to editor page`
- [x] Task 2: Editor save menu + preview stage — `feat(ui): apply glass menu + preview stage to editor page`
- [x] Task 3: Album picker Copy/Move dialog — `feat(ui): apply glass vocabulary to album picker dialog buttons`
- [x] Task 4: Reduced-transparency fallback — `feat(ui): add reduced-transparency accessibility fallback to glass CSS`
- [x] Task 5: High-contrast fallback — `feat(ui): add high-contrast accessibility fallback to glass CSS`
- [x] Task 6: Final acceptance verification — `cargo test --all-targets` green; flatpak visual sweep done.
```

(Each Task commit only exists after that task's review is approved. Mark items `[x]` only when the corresponding commit lands AND the review is clean.)

---

## Self-Review

**1. Spec coverage:**
- Section A (Global glass style system) — covered by Phase 1; this plan reuses every class.
- Section B (Main window + sidebar) — covered by Phase 1 Task 2.
- Section C (Photos page header + batch toolbar) — covered by Phase 1 Task 3.
- Section D (Photo grid canvas + thumbnail layout) — covered by Phase 1 Task 4.
- Section E (Bottom mode selector safe area) — covered by Phase 1 Task 5.
- Section F (Right-click context menus) — covered by Phase 1 Task 6.
- Section G (Viewer page toolbar + image stage) — covered by Phase 1 Task 7.
- Section H (Viewer details sidebar) — covered by Phase 1 Task 8.
- Section I (Editor + secondary popovers) — **Tasks 1, 2, 3** of this Phase 2 plan. Editor header + buttons (T1), editor save popover (T2), album picker dialog (T3). Editor save menu is the spec's "editor save menu" + the album picker is the spec's "album picker surfaces".
- Section 7 (State matrix + accessibility checks) — **Tasks 4, 5** of this Phase 2 plan. `prefers-reduced-transparency` (T4) and `prefers-contrast: more` (T5).
- Section "Open design decisions" — "rely on system accessibility settings first" is now resolved: T4+T5 wire up the system settings, no app-level preference is added (YAGNI).
- Non-modification boundaries (section J) — still respected: no `src/core/*`, no DB schema, no file watcher, no Flatpak manifest changes, no Cargo dep changes.

**2. Placeholder scan:** No "TBD", "implement later", "fill in details", or "appropriate error handling" placeholders. Every step has a concrete file path and a concrete code change.

**3. Type consistency:**
- `.glass-toolbar-suggested` (Task 1) — defined in Task 1, used by `editor-page.blp` (T1), used by `album_picker.rs` (T3). Same name everywhere.
- `.glass-editor-preview` (Task 2) — defined in Task 2, used by `editor-page.blp` (T2). Self-contained.
- `EditorPage::new(media_item, pool)` — confirmed in `src/ui/editor_page.rs:130`. Test in Task 1 uses this signature.
- `AlbumPickerDialog::present(host_nav, pool, media_ids)` — confirmed in `src/ui/album_picker.rs:296`. Test in Task 3 uses this signature.
- `grid_css::css_for_tests()` — defined in Task 4 Step 2, used by `tests/ui_accessibility_fallback.rs` in both T4 and T5. No name drift.
- `@media (prefers-reduced-transparency: reduce)` — defined in T4, asserted in T4 test. `@media (prefers-contrast: more)` — defined in T5, asserted in T5 test. No cross-contamination.
- `imp.cancel_btn`, `imp.save_copy_btn`, `imp.save_menu_btn`, `imp.rotate_90_cw`, `imp.rotate_180`, `imp.rotate_90_ccw`, `imp.start_crop_btn`, `imp.preview_overlay`, `imp.header_bar` — assumed `#[template_child]` field names. Confirm against the real `editor_page.rs` `imp` block before writing the tests; the plan notes the field names in each test step.
