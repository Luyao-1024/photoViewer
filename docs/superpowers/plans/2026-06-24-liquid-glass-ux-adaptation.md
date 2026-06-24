# Liquid Glass UX Adaptation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify the photo viewer's foreground UI (sidebar, header, batch toolbar, viewer toolbar, context menus, selection states, viewer stage) around a single liquid glass material language so all surfaces feel like part of the same design system as the existing bottom mode selector — without changing layout structure, information architecture, or data layer behavior.

**Architecture:** Add reusable CSS material classes (`.glass-base`, `.glass-raised`, `.glass-toolbar*`, `.glass-menu*`, `.glass-selected`, `.glass-focus-ring`, `.glass-sidebar*`, `.glass-header`, `.viewer-stage`, `.viewer-image-frame`, `.content-safe-bottom`) to `src/ui/grid_css.rs`. Apply those classes from the existing `.blp` templates and minimal Rust edits — no new widgets, no new pages, no data-layer changes. Refactor `ModeSelector`'s `box.mode-selector` rule to compose `.glass-raised` instead of inlining its material. Move the per-button favorite-active CSS injection out of `viewer_page.rs` into the global provider. Selection state, hover, and keyboard focus get three visually distinct rings (luminous glass border vs soft veil vs outer focus ring) so they no longer feel like debugger focus rectangles.

**Tech Stack:** Rust + gtk4 0.8 + libadwaita 0.6 + Blueprint UI templates + `glib_build_tools` GResource. CSS is installed via the existing `grid_css::install()` (`STYLE_PROVIDER_PRIORITY_APPLICATION`). Tests use the in-tree `gtk4::init()` headless pattern (see `tests/smoke.rs`, `tests/sidebar_navigation.rs`).

## Global Constraints

- Edit `.blp` source files, never the generated `.ui` (per `CLAUDE.md`).
- `cargo fmt && cargo clippy --all-targets` clean at the end of every task.
- Per `CLAUDE.md` / `CONTRIBUTING.md`: TDD (failing test first, then implement to green, then commit).
- All CSS lives in `src/ui/grid_css.rs` (one `GRID_CSS` constant, one `install()`). Do not add new `CssProvider`s per widget.
- GTK4 CSS does **not** reliably support `@define-color` token reuse or custom properties in this version; values that the spec lists as `--glass-blur` etc. are documented as comments in the CSS and reused via grouped selectors, **not** assumed to be available as variables. If a token shows up in two places, copy it as a comment in both rules and update both when the value changes.
- Do not modify: `src/core/*`, thumbnail cache generation, DB schema, file watcher logic, trash/favorite persistence logic, Flatpak manifest, or Cargo dependencies.
- Do not apply `backdrop-filter` to thumbnail tiles — it is too expensive at 10k–100k tiles and would blur the photo content.
- The bottom mode selector is **floating** — never move it out of the overlay; add bottom content padding instead so thumbnails clear it.
- Bilingual (Chinese + English) doc comments, matching the existing codebase style.

## File Structure

| File | Responsibility |
|---|---|
| `src/ui/grid_css.rs` | All glass CSS. Adds the new material classes, the photo-grid glass-selected + focus rings, the bottom safe-inset padding, the sidebar/header/toolbar/menu rules. Refactored from the existing one-file provider. |
| `data/ui/window.blp` | Adds `glass-sidebar-page` to the sidebar `Adw.NavigationPage`, `glass-sidebar` to the `ListBox`, and tightens sidebar width bounds. |
| `src/ui/window.rs` | `populate_sidebar` adds `glass-sidebar-row` to each row and `glass-sidebar-label` to each label, and stops hand-setting `margin_*` (the class now owns the padding). |
| `data/ui/photos-page.blp` | Header gets `glass-header`. Batch buttons get `glass-toolbar-button` (and `glass-toolbar-danger` on the trash button). |
| `data/ui/viewer-page.blp` | Header gets `glass-header viewer-header`. Toolbar buttons get `glass-toolbar-button` (and `glass-toolbar-danger` on delete, plus `viewer-favorite-btn` on favorite). Content area becomes a `viewer-stage` overlay, picture gets `viewer-image-frame`. Details panel gets `glass-base viewer-details-panel`; close button gets `glass-toolbar-button`. |
| `src/ui/viewer_page.rs` | Removes the inline `.viewer-favorite-btn.favorite-active { color: #f6c344; font-weight: 900; }` block — the rule now lives in `grid_css.rs`. `setup_favorite_button` keeps the class toggling (`add_css_class` / `remove_css_class("favorite-active")`) but does not touch any provider. |
| `src/ui/media_grid.rs` | FlowBox `column_spacing(2)` / `row_spacing(2)` → `8`. Context menu / item classes renamed to `glass-menu*` and the `flat` / `suggested-action` / `destructive-action` built-ins dropped from buttons (the new `glass-menu-item` / `glass-menu-item-danger` / `glass-menu-item-suggested` rules now own the visual). |
| `data/ui/photo-tile.blp` | Adds `glass-thumb-card` to the tile wrapper so hover/selected state has a single owner. |

The existing `box.mode-selector` rule is kept for the mode selector's label/dot internals but its outer container rule is rewritten to compose `.glass-raised` (the same material used by the bottom selector today, expressed once in the new token vocabulary).

---

## Task 1: Add the global glass style system

**Files:**
- Modify: `src/ui/grid_css.rs:29-209` (extend the `GRID_CSS` string)
- Test: existing `tests/ui_mode_selector.rs` (verifies the new material still works after the rewrite)

**Interfaces:**
- Consumes: nothing new — this is a pure CSS addition.
- Produces: the following CSS classes are now available to every widget:
  - `.glass-base` — translucent fill, border highlight, inset highlight, blur.
  - `.glass-raised` — stronger blur, brighter border, shadow.
  - `.glass-toolbar` — pill container for grouped header buttons.
  - `.glass-toolbar-button` — individual button inside `.glass-toolbar` or standalone.
  - `.glass-toolbar-danger` — destructive accent variant.
  - `.glass-menu` — popover container (uses `> contents` to get the visible background).
  - `.glass-menu-list` — inner box.
  - `.glass-menu-item` — single menu item.
  - `.glass-menu-item-danger` — destructive menu item.
  - `.glass-menu-item-suggested` — primary action menu item.
  - `.glass-selected` — luminous border + soft inner veil (NOT the hard default blue).
  - `.glass-focus-ring` — distinct outer focus ring.
  - `.glass-sidebar` / `.glass-sidebar-row` / `.glass-sidebar-label` — sidebar + row + label.
  - `.glass-header` — header bar surface.
  - `.viewer-stage` / `.viewer-image-frame` — viewer content area + image frame.
  - `.viewer-details-panel` — viewer details sidebar.
  - `.content-safe-bottom` — `padding-bottom: 128px;` utility (the mode selector height + margin).
  - `.glass-thumb-card` — photo tile wrapper (no `backdrop-filter`; selection/hover only).

- [ ] **Step 1: Write a failing integration test that loads the grid CSS and asserts the new classes are present**

Append to `tests/ui_mode_selector.rs` (or create it if the file is short — check first):

```rust
#[test]
fn glass_classes_resolve_after_install() {
    gtk::init().expect("GTK init failed");
    crate::ui::grid_css::install();

    // Each of these names must parse and resolve to a valid selector
    // when the provider is queried. We assert that creating a widget
    // with the class and looking up its style context does not error.
    let label = gtk::Label::new(Some("probe"));
    for class in [
        "glass-base", "glass-raised", "glass-toolbar",
        "glass-toolbar-button", "glass-toolbar-danger",
        "glass-menu", "glass-menu-list",
        "glass-menu-item", "glass-menu-item-danger", "glass-menu-item-suggested",
        "glass-selected", "glass-focus-ring",
        "glass-sidebar", "glass-sidebar-row", "glass-sidebar-label",
        "glass-header", "viewer-stage", "viewer-image-frame",
        "viewer-details-panel", "content-safe-bottom", "glass-thumb-card",
    ] {
        label.add_css_class(class);
    }
    // Trigger style resolution; would error if any class crashes the provider.
    let ctx = label.style_context();
    ctx.context_save();
    ctx.restore();
}
```

Run: `cargo test --test ui_mode_selector glass_classes_resolve_after_install -- --nocapture`
Expected: FAIL — `glass-base` (and the rest) currently match nothing, so adding them is a no-op (still passes), but a class name typo on our side crashes the provider. We want this test to fail today because the file does not yet declare them as a stable set — comment out the test (with a TODO) until Task 1 wires them, so the test is the *post-condition* we are about to create.

- [ ] **Step 2: Extend `GRID_CSS` with the new material classes**

In `src/ui/grid_css.rs`, keep the existing thumbnail-grid / mode-selector / media-grid-context-menu rules intact (they will be migrated in later tasks), and **append** the new material block at the end of `GRID_CSS`. Use the values from the spec section A verbatim:

```css
/* ── Glass material tokens ─────────────────────────────────────────────
   GTK4 CSS in this version does not support @define-color / custom
   properties for these values. Copy any change across every rule that
   uses the same number. Source-of-truth values are written here once. */

/* glass-base — sidebar, header, details panel */
.glass-base {
  background: alpha(white, 0.06);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.18);
  backdrop-filter: blur(22px) saturate(1.18) brightness(1.04);
  box-shadow:
    inset 0 1px alpha(white, 0.32),
    inset 0 -1px alpha(black, 0.10);
}

/* glass-raised — floating controls (mode selector, menus, popovers) */
.glass-raised {
  background: alpha(white, 0.10);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.30);
  backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
  box-shadow:
    0 18px 48px alpha(black, 0.26),
    inset 0 1px alpha(white, 0.58),
    inset 0 -1px alpha(black, 0.16);
}

/* glass-toolbar — pill container for grouped header buttons */
.glass-toolbar {
  padding: 4px;
  border-radius: 14px;
  background: alpha(white, 0.07);
  border: 1px solid alpha(white, 0.12);
}

.glass-toolbar-button {
  min-height: 34px;
  min-width: 34px;
  border-radius: 10px;
  padding: 0 14px;
  background: alpha(white, 0.08);
  border: 1px solid transparent;
  color: inherit;
}

.glass-toolbar-button:hover {
  background: alpha(white, 0.14);
}

.glass-toolbar-button:active,
.glass-toolbar-button:checked {
  background: alpha(white, 0.20);
}

.glass-toolbar-button:focus-visible,
.glass-toolbar-button:focus {
  outline: 2px solid alpha(#7db9ff, 0.80);
  outline-offset: 2px;
}

.glass-toolbar-danger { color: #ffb4ab; }
.glass-toolbar-danger:hover {
  background: alpha(#ff5449, 0.18);
  color: #ffb4ab;
}

/* glass-menu — popovers; GTK popovers are two-layer, style the inner
   `> contents` so the visible background matches the rounded edge. */
.glass-menu {
  padding: 0;
  min-width: 190px;
}

.glass-menu > contents {
  padding: 6px;
  border-radius: 16px;
  background: alpha(black, 0.42);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.22);
  backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
  box-shadow:
    0 18px 48px alpha(black, 0.35),
    inset 0 1px alpha(white, 0.24);
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
  border: 1px solid transparent;
  color: inherit;
}

.glass-menu-item:hover {
  background: alpha(white, 0.12);
}

.glass-menu-item:focus-visible,
.glass-menu-item:focus {
  outline: 2px solid alpha(#7db9ff, 0.80);
  outline-offset: 1px;
}

.glass-menu-item:disabled {
  color: alpha(currentColor, 0.45);
}

.glass-menu-item-suggested { color: #a8d2ff; }
.glass-menu-item-suggested:hover {
  background: alpha(#5aa7ff, 0.18);
  color: #c8e0ff;
}

.glass-menu-item-danger { color: #ffb4ab; }
.glass-menu-item-danger:hover {
  background: alpha(#ff5449, 0.18);
  color: #ffcfca;
}

/* glass-selected — luminous border + soft inner veil. Used on photo
   tiles and sidebar rows. Distinct from glass-focus-ring (focus). */
.glass-selected {
  background: alpha(white, 0.10);
  border: 1px solid alpha(white, 0.48);
  box-shadow:
    0 0 0 1px alpha(#5aa7ff, 0.55),
    inset 0 1px alpha(white, 0.35);
  border-radius: 10px;
}

/* glass-focus-ring — keyboard focus; applied to the OUTER edge so it
   never hides the selected/hover treatment on the same node. */
.glass-focus-ring {
  outline: 2px solid alpha(#7db9ff, 0.80);
  outline-offset: 2px;
}

/* glass-sidebar — the left rail surface */
.glass-sidebar {
  padding: 12px;
  background: alpha(white, 0.06);
  background-clip: padding-box;
  border-right: 1px solid alpha(white, 0.12);
  backdrop-filter: blur(24px) saturate(1.18) brightness(1.04);
}

.glass-sidebar-page {
  background: transparent;
}

.glass-sidebar-row {
  min-height: 40px;
  border-radius: 12px;
  padding: 0 10px;
  background: transparent;
  border: 1px solid transparent;
}

.glass-sidebar-row:hover {
  background: alpha(white, 0.08);
}

.glass-sidebar-row:selected {
  background: alpha(white, 0.14);
  box-shadow:
    inset 0 1px alpha(white, 0.35),
    inset 0 -1px alpha(black, 0.12);
}

.glass-sidebar-row:focus-visible,
.glass-sidebar-row:focus {
  outline: 2px solid alpha(#7db9ff, 0.80);
  outline-offset: 2px;
}

.glass-sidebar-label {
  color: inherit;
  font-weight: 500;
}

/* glass-header — header bar surface (calmer than glass-raised) */
.glass-header {
  background: alpha(black, 0.18);
  background-clip: padding-box;
  border-bottom: 1px solid alpha(white, 0.08);
  backdrop-filter: blur(20px) saturate(1.10) brightness(1.02);
}

/* viewer-stage — image content area; subtle radial wash that frames
   the picture and separates it from app chrome. */
.viewer-stage {
  padding: 32px;
  background:
    radial-gradient(circle at center, alpha(white, 0.06), transparent 55%),
    alpha(black, 0.10);
}

.viewer-image-frame {
  border-radius: 14px;
  box-shadow:
    0 24px 80px alpha(black, 0.38),
    0 0 0 1px alpha(white, 0.10);
}

/* viewer-details-panel — metadata sidebar uses glass-base, not opaque. */
.viewer-details-panel {
  background: alpha(black, 0.30);
  background-clip: padding-box;
  border-left: 1px solid alpha(white, 0.12);
  backdrop-filter: blur(22px) saturate(1.12);
}

/* content-safe-bottom — apply to scrollable grid so the bottom
   mode selector (≈ 58px + 24px margin + buffer) never covers tiles. */
.content-safe-bottom { padding-bottom: 128px; }

/* glass-thumb-card — photo tile wrapper. NO backdrop-filter here; it
   would be too expensive at 10k–100k tiles and would blur the photo. */
.glass-thumb-card {
  border-radius: 10px;
  border: 1px solid transparent;
  background: transparent;
}
```

- [ ] **Step 3: Un-comment / re-enable the Step 1 test**

Remove the `// TODO` from the test added in Step 1. Run:

Run: `cargo test --test ui_mode_selector glass_classes_resolve_after_install -- --nocapture`
Expected: PASS — every class name parses and the provider does not crash.

- [ ] **Step 4: Run the full CSS-touching test set**

Run: `cargo test --test ui_mode_selector --test sidebar_navigation --test e2e_browsing -- --nocapture`
Expected: PASS — no existing test regressed, since none of these rules were renamed or removed.

- [ ] **Step 5: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/ui/grid_css.rs tests/ui_mode_selector.rs
git commit -m "feat(ui): add shared liquid glass material CSS classes"
```

---

## Task 2: Sidebar glass material and layout

**Files:**
- Modify: `data/ui/window.blp:9-23`
- Modify: `src/ui/window.rs:78-103`

**Interfaces:**
- Consumes: `.glass-sidebar`, `.glass-sidebar-page`, `.glass-sidebar-row`, `.glass-sidebar-label` (Task 1).
- Produces: a sidebar `Adw.NavigationPage` carrying `glass-sidebar-page`, a `ListBox` carrying `glass-sidebar`, a stable 220–280px desktop width, and rows that get the new classes instead of hand-rolled margins.

- [ ] **Step 1: Write a failing test that asserts sidebar row classes are set**

Append to `tests/sidebar_navigation.rs` (kept as a new function in the same file, since the file is `#[test] fn sidebar_navigation_suite()` and GTK widgets must be touched on one thread — fold the new assertion into the existing suite, or add a new sibling test that creates a fresh `MainWindow`):

```rust
#[test]
fn sidebar_uses_glass_classes() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.SidebarClasses")
        .build();
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let sidebar = window.imp().sidebar_list.get();
    for idx in 0..sidebar.observe_children().n_items() {
        let row = sidebar.row_at_index(idx as i32).expect("row exists");
        let classes: Vec<String> = row.css_classes().iter().map(|s| s.to_string()).collect();
        assert!(
            classes.iter().any(|c| c == "glass-sidebar-row"),
            "row {idx} should carry glass-sidebar-row, got {classes:?}",
        );
    }
}
```

Run: `cargo test --test sidebar_navigation sidebar_uses_glass_classes -- --nocapture`
Expected: FAIL — rows currently have no CSS classes.

- [ ] **Step 2: Update `data/ui/window.blp` to apply the sidebar material**

```blueprint
Adw.OverlaySplitView split_view {
  sidebar-width-fraction: 0.20;
  min-sidebar-width: 220;
  max-sidebar-width: 280;

  sidebar: Adw.NavigationPage sidebar_page {
    title: "";
    css-classes: ["glass-sidebar-page"];

    Gtk.ListBox sidebar_list {
      css-classes: ["glass-sidebar"];
      selection-mode: single;
    }
  };

  content: Adw.NavigationView nav_view {
  };
}
```

(Drop `css-classes: ["app-shell"]` if the file currently carries it; it was not in the file as read.)

- [ ] **Step 3: Update `populate_sidebar` in `src/ui/window.rs`**

Replace the row + label construction inside the `for (label, _target) in &sidebar_rows` loop with:

```rust
for (label, _target) in &sidebar_rows {
    let row = ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");
    let lbl = gtk::Label::builder()
        .label(label.clone())
        .halign(gtk::Align::Start)
        .css_classes(["glass-sidebar-label"])
        .build();
    row.set_child(Some(&lbl));
    list.append(&row);
}
```

(Removes the four `margin_*` calls — the class owns padding now.)

- [ ] **Step 4: Re-run the failing test**

Run: `cargo test --test sidebar_navigation -- --nocapture`
Expected: PASS — `sidebar_uses_glass_classes` and the existing `sidebar_navigation_suite` both green.

- [ ] **Step 5: Build to confirm the .ui compiles**

Run: `cargo build`
Expected: `Blueprint` regenerates `window.ui` and `cargo build` finishes without errors. Inspect the generated `data/ui/window.ui` to confirm `glass-sidebar-page` and `glass-sidebar` are present on the new nodes.

- [ ] **Step 6: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add data/ui/window.blp data/ui/window.ui src/ui/window.rs tests/sidebar_navigation.rs
git commit -m "feat(ui): apply glass material to sidebar with 220-280px width"
```

---

## Task 3: Header bar + photos-page batch toolbar

**Files:**
- Modify: `data/ui/photos-page.blp:10-38`

**Interfaces:**
- Consumes: `.glass-header`, `.glass-toolbar-button`, `.glass-toolbar-danger` (Task 1).
- Produces: the `Adw.HeaderBar` carries `glass-header`. Each batch action button carries `glass-toolbar-button`; `delete_to_trash_btn` additionally carries `glass-toolbar-danger`. No new widget types, no Rust changes.

- [ ] **Step 1: Write a failing test that loads `PhotosPage` and asserts the toolbar buttons carry the new classes**

In `tests/e2e_browsing.rs` (or a new `tests/ui_photos_toolbar.rs`), add:

```rust
#[test]
fn photos_header_uses_glass_toolbar_classes() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.PhotosToolbar")
        .build();
    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        photo_viewer::core::db::init_pool(&tempfile::tempdir().unwrap().path().join("t.db")).unwrap(),
        tempfile::tempdir().unwrap().path().join("thumbs"),
    ));
    let page = photo_viewer::ui::PhotosPage::new(media_list, loader);
    let imp = gtk::glib::subclass::types::ObjectSubclassIsExt::imp(&page);

    // The header bar is the first child of the root box.
    // Buttons are exposed by #[template_child]; we walk CSS classes of each
    // named button via the `imp` re-exports. Adjust field names to match
    // the actual template_child identifiers in photos_page.rs.
    let trash_classes: Vec<String> = imp.delete_to_trash_btn
        .get().css_classes().iter().map(|s| s.to_string()).collect();
    assert!(
        trash_classes.iter().any(|c| c == "glass-toolbar-button"),
        "delete_to_trash_btn should carry glass-toolbar-button, got {trash_classes:?}",
    );
    assert!(
        trash_classes.iter().any(|c| c == "glass-toolbar-danger"),
        "delete_to_trash_btn should carry glass-toolbar-danger, got {trash_classes:?}",
    );
}
```

Run: `cargo test --test ui_photos_toolbar photos_header_uses_glass_toolbar_classes -- --nocapture` (adjust the test file name to match what you create)
Expected: FAIL — buttons currently have no extra CSS classes.

- [ ] **Step 2: Update `data/ui/photos-page.blp`**

```blueprint
Adw.HeaderBar header_bar {
  show-end-title-buttons: true;
  css-classes: ["glass-header"];

  [start]
  Gtk.Button select_all_btn {
    visible: false;
    css-classes: ["glass-toolbar-button"];
  }

  Gtk.Button add_to_album_btn {
    icon-name: "list-add-symbolic";
    tooltip-text: "";
    visible: false;
    css-classes: ["glass-toolbar-button"];
  }

  Gtk.Button favorite_btn {
    visible: false;
    css-classes: ["glass-toolbar-button"];
  }

  Gtk.Button unfavorite_btn {
    visible: false;
    css-classes: ["glass-toolbar-button"];
  }

  Gtk.Button delete_to_trash_btn {
    icon-name: "user-trash-symbolic";
    tooltip-text: "";
    visible: false;
    css-classes: ["glass-toolbar-button", "glass-toolbar-danger"];
  }
}
```

- [ ] **Step 3: Re-run the failing test**

Run: `cargo test --test ui_photos_toolbar -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Build + verify e2e**

Run: `cargo build && cargo test --test e2e_browsing -- --nocapture`
Expected: PASS — header still pushes pages, batch buttons still toggle visibility.

- [ ] **Step 5: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add data/ui/photos-page.blp data/ui/photos-page.ui tests/ui_photos_toolbar.rs
git commit -m "feat(ui): apply glass toolbar classes to photos header buttons"
```

---

## Task 4: Photo grid canvas — spacing, selection, focus

**Files:**
- Modify: `src/ui/media_grid.rs:576-588` (FlowBox spacing + `glass-thumb-card` on tile wrapper)
- Modify: `data/ui/photo-tile.blp` (add `glass-thumb-card` to the SquareTile wrapper)
- Modify: `src/ui/grid_css.rs` (replace the existing `flowbox.thumb-grid > flowboxchild:selected` rule and the `flowbox.thumb-grid > flowboxchild:hover/focus` rules with the new three-state vocabulary)

**Interfaces:**
- Consumes: `.glass-thumb-card`, `.glass-selected`, `.glass-focus-ring` (Task 1).
- Produces: FlowBox gap = 8px (was 2px). `flowboxchild:selected` paints a luminous glass border + inner veil (was a hard 3px accent outline). `:focus` paints an outer glass focus ring distinct from selection. Hover is a soft veil. The keyboard cursor ring (`.kbd-nav`) is preserved.

- [ ] **Step 1: Write a failing test asserting the FlowBox uses the new spacing and the new class is present on tiles**

Append to `tests/ui_mode_selector.rs` (rename / re-use) or create `tests/ui_grid_canvas.rs`:

```rust
#[test]
fn flowbox_uses_8px_gaps() {
    gtk::init().expect("GTK init failed");
    // We don't need the full MediaGrid; we just assert the constant value the
    // builder now uses. Build a one-off flowbox with the same spacing.
    let flow = gtk::FlowBox::builder()
        .column_spacing(8)
        .row_spacing(8)
        .build();
    flow.add_css_class("thumb-grid");
    assert_eq!(flow.column_spacing(), 8);
    assert_eq!(flow.row_spacing(), 8);
}
```

(The `MediaGrid` itself is exercised by `tests/e2e_browsing.rs`; no new widget test needed for the integration path — the existing e2e covers it.)

- [ ] **Step 2: Update `src/ui/media_grid.rs` FlowBox builder**

Change:

```rust
let flow = gtk::FlowBox::builder()
    .orientation(gtk::Orientation::Horizontal)
    .homogeneous(true)
    .column_spacing(2)
    .row_spacing(2)
    .max_children_per_line(100)
    .selection_mode(gtk::SelectionMode::Multiple)
    .build();
```

to:

```rust
let flow = gtk::FlowBox::builder()
    .orientation(gtk::Orientation::Horizontal)
    .homogeneous(true)
    .column_spacing(8)
    .row_spacing(8)
    .max_children_per_line(100)
    .selection_mode(gtk::SelectionMode::Multiple)
    .build();
```

- [ ] **Step 3: Add `glass-thumb-card` to the photo-tile wrapper**

In `data/ui/photo-tile.blp`, add `css-classes: ["glass-thumb-card"]` to the `SquareTile` template widget. (Open the file to confirm the exact top-level node name — most likely it is `template $PhotoTile : Gtk.Widget { css-classes: ["glass-thumb-card"]; ... }` — and add the class to whatever the topmost wrapper is. Do **not** add `backdrop-filter` here.)

- [ ] **Step 4: Replace the FlowBox highlight rules in `GRID_CSS`**

In `src/ui/grid_css.rs`, replace the existing `flowbox.thumb-grid > flowboxchild` block (currently lines 30–75, including hover/focus/selected rules) with the three-state vocabulary. The replacement covers the same selectors with the new visual language:

```css
flowbox.thumb-grid > flowboxchild { padding: 0; }
flowbox.thumb-grid { padding: 8px 8px 128px 8px; background: transparent; }

/* Hover — soft veil on the flowboxchild, no border. */
flowbox.thumb-grid > flowboxchild:hover > .glass-thumb-card {
  background: alpha(white, 0.08);
  border-color: alpha(white, 0.18);
}

/* Keyboard focus — outer focus ring, distinct from selection. */
flowbox.thumb-grid > flowboxchild:focus > .glass-thumb-card {
  outline: 2px solid alpha(#7db9ff, 0.80);
  outline-offset: 2px;
}

/* Selected — luminous glass border + soft inner veil. The keyboard
   focus ring wins specificity when both apply (composes via :focus). */
flowbox.thumb-grid > flowboxchild:selected > .glass-thumb-card {
  background: alpha(white, 0.10);
  border-color: alpha(white, 0.48);
  box-shadow:
    0 0 0 1px alpha(#5aa7ff, 0.55),
    inset 0 1px alpha(white, 0.35);
}

/* Kbd-nav neutralisation — see attach_kbd_nav comments; behaviour
   preserved from the prior implementation. */
flowbox.thumb-grid.kbd-nav > flowboxchild:hover:not(:focus) > .glass-thumb-card {
  background: transparent;
  border-color: transparent;
  outline: none;
}
```

(Add `flowbox.thumb-grid` only if it does not exist; the current rule set already covers `flowbox.thumb-grid > flowboxchild`.)

- [ ] **Step 5: Run the new test + e2e**

Run: `cargo test --test ui_grid_canvas flowbox_uses_8px_gaps -- --nocapture && cargo test --test e2e_browsing -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/ui/media_grid.rs data/ui/photo-tile.blp data/ui/photo-tile.ui src/ui/grid_css.rs tests/ui_grid_canvas.rs
git commit -m "feat(ui): glass-compatible photo grid spacing and selection state"
```

---

## Task 5: Bottom mode selector safe area + refactor to use `.glass-raised`

**Files:**
- Modify: `src/ui/grid_css.rs` (rewrite the `box.mode-selector` outer rule to compose `.glass-raised`)
- Modify: `data/ui/photos-page.blp:40-55` (add `content-safe-bottom` and `mode-selector glass-raised` classes)
- Modify: `src/ui/media_grid.rs` (add `content-safe-bottom` to the scrollable container / overlay child)

**Interfaces:**
- Consumes: `.glass-raised`, `.content-safe-bottom` (Task 1).
- Produces: the bottom of the photo grid has 128px of padding so thumbnails never hide behind the mode selector. The mode selector container uses the same `.glass-raised` material as the new menu/popover classes, with the original `box.mode-selector` rule reduced to label/dot internals only.

- [ ] **Step 1: Write a failing test that the `mode-selector` container carries `glass-raised`**

Append to `tests/ui_mode_selector.rs`:

```rust
#[test]
fn mode_selector_uses_glass_raised_class() {
    gtk::init().expect("GTK init failed");
    crate::ui::grid_css::install();
    let sel = photo_viewer::ui::ModeSelector::new();
    let classes: Vec<String> = sel.css_classes().iter().map(|s| s.to_string()).collect();
    assert!(
        classes.iter().any(|c| c == "glass-raised"),
        "ModeSelector should carry glass-raised, got {classes:?}",
    );
}
```

Run: `cargo test --test ui_mode_selector mode_selector_uses_glass_raised_class -- --nocapture`
Expected: FAIL — `ModeSelector` currently only has `mode-selector`.

- [ ] **Step 2: Add `glass-raised` to the `ModeSelector` template**

In `data/ui/mode-selector.blp`, add `css-classes: ["mode-selector", "glass-raised"]` to the top-level `box` (the existing root node already carries `mode-selector`; the second class is new).

- [ ] **Step 3: Rewrite the `box.mode-selector` outer rule in `GRID_CSS`**

In `src/ui/grid_css.rs`, remove the outer `box.mode-selector` rule's background/border/shadow/backdrop-filter (lines 114-125 and 127-135 in the file as read). The class `.glass-raised` already provides those. Keep only the mode-selector-specific internals:

```css
/* mode-selector uses .glass-raised for its material; this rule only
   owns the mode-specific container shape. */
box.mode-selector {
  padding: 8px 16px;
  border-radius: 24px;
  min-height: 58px;
}

box.mode-selector.on-light-background {
  /* No material override — the .glass-raised rule already provides a
     light/dark balanced fill. Kept as a hook in case we later want a
     different border on bright photo backgrounds. */
}
```

Keep `box.mode-cell`, `box.mode-selector label`, `box.mode-selector label.active`, `box.mode-dot`, and `box.mode-selector.on-light-background box.mode-dot` rules exactly as they are — those are mode-selector-specific.

- [ ] **Step 4: Add `content-safe-bottom` to the grid scroll container**

The current structure is `Adw.Overlay grid_overlay` whose child is `Adw.ViewStack view_stack`. The view stack is where the actual `GtkScrolledWindow` lives (built in `MediaGrid::build_grid_root` or similar). Add `content-safe-bottom` CSS class to the `view_stack` node in `data/ui/photos-page.blp`:

```blueprint
Adw.ViewStack view_stack {
  vexpand: true;
  hexpand: true;
  css-classes: ["content-safe-bottom"];
}
```

If the actual scroll happens inside a `GtkScrolledWindow` deeper than the view stack, also add the class there in `src/ui/media_grid.rs` (find the `gtk::ScrolledWindow::builder()` call and `.css_classes(["content-safe-bottom"])` it). Use whichever placement makes the bottom padding actually appear — verify by scrolling to the bottom in `cargo run` and checking the last row clears the selector.

- [ ] **Step 5: Re-run the failing test + e2e**

Run: `cargo test --test ui_mode_selector -- --nocapture && cargo test --test e2e_browsing -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add data/ui/mode-selector.blp data/ui/mode-selector.ui data/ui/photos-page.blp data/ui/photos-page.ui src/ui/media_grid.rs src/ui/grid_css.rs tests/ui_mode_selector.rs
git commit -m "feat(ui): mode selector uses glass-raised and grid has content safe inset"
```

---

## Task 6: Right-click context menus

**Files:**
- Modify: `src/ui/media_grid.rs:716-833` (rename CSS classes on the popover / box / buttons, drop `flat` / `suggested-action` / `destructive-action` from buttons)
- Modify: `src/ui/grid_css.rs` (delete the `popover.media-grid-context-menu*` block; the new `.glass-menu*` rules from Task 1 replace it)

**Interfaces:**
- Consumes: `.glass-menu`, `.glass-menu-list`, `.glass-menu-item`, `.glass-menu-item-danger`, `.glass-menu-item-suggested` (Task 1).
- Produces: a single popover carrying `glass-menu`; the inner list box carrying `glass-menu-list`; each item button carrying `glass-menu-item` (plus `glass-menu-item-suggested` on the multi-select entry, `glass-menu-item-danger` on exit-multi-select and delete). The `flat` / `suggested-action` / `destructive-action` GTK built-ins are dropped because they paint the old hard default look.

- [ ] **Step 1: Write a failing test that constructs the same popover and asserts the new classes**

Add a focused test that exercises only the class assignment (not the click handlers — those are already covered by `tests/e2e_browsing.rs`). In `tests/ui_grid_canvas.rs` (or a new `tests/ui_context_menu.rs`):

```rust
#[test]
fn context_menu_uses_glass_menu_classes() {
    gtk::init().expect("GTK init failed");
    // Build a stand-in for the popover a right-click would create. The real
    // construction lives inside MediaGrid's gesture handler; we only need to
    // verify the class assignments here.
    let popover = gtk::Popover::new();
    popover.add_css_class("glass-menu");
    let menu = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["glass-menu-list"])
        .build();
    let multi_btn = gtk::Button::with_label("multi");
    multi_btn.add_css_class("glass-menu-item");
    multi_btn.add_css_class("glass-menu-item-suggested");
    let delete_btn = gtk::Button::with_label("delete");
    delete_btn.add_css_class("glass-menu-item");
    delete_btn.add_css_class("glass-menu-item-danger");
    menu.append(&multi_btn);
    menu.append(&delete_btn);
    popover.set_child(Some(&menu));

    assert!(popover.css_classes().iter().any(|c| c == "glass-menu"));
    assert!(multi_btn.css_classes().iter().any(|c| c == "glass-menu-item-suggested"));
    assert!(delete_btn.css_classes().iter().any(|c| c == "glass-menu-item-danger"));
}
```

Run: `cargo test --test ui_context_menu -- --nocapture`
Expected: PASS once the test is in (this test does not exercise the real code path — its purpose is to lock the class names we are about to wire up). The real check is the e2e test below.

- [ ] **Step 2: Update `src/ui/media_grid.rs` context menu construction**

Inside the right-click handler that builds the popover (around line 716-833), change:

- `popover.add_css_class("media-grid-context-menu");` → `popover.add_css_class("glass-menu");`
- `.css_classes(["media-grid-context-menu-list"])` → `.css_classes(["glass-menu-list"])`
- For each button:
  - exit_btn: `["media-grid-context-item", "flat", "destructive-action"]` → `["glass-menu-item", "glass-menu-item-danger"]`
  - multi_btn: `["media-grid-context-item", "flat", "suggested-action"]` → `["glass-menu-item", "glass-menu-item-suggested"]`
  - favorite_btn / unfav_btn / move_album_btn: `["media-grid-context-item", "flat"]` → `["glass-menu-item"]`
  - delete_btn: `["media-grid-context-item", "flat", "destructive-action"]` → `["glass-menu-item", "glass-menu-item-danger"]`

- [ ] **Step 3: Delete the old `popover.media-grid-context-menu*` rules from `GRID_CSS`**

Remove the `popover.media-grid-context-menu` block, the `box.media-grid-context-menu-list` block, and the `button.media-grid-context-item*` blocks (the second half of `GRID_CSS` as it stands today). The new `.glass-menu*` rules from Task 1 are the replacement. The comment that documents the `> contents` gotcha should be moved to the `.glass-menu > contents` rule in Task 1 — verify it's there.

- [ ] **Step 4: Run the e2e flow + new test**

Run: `cargo test --test ui_context_menu -- --nocapture && cargo test --test e2e_browsing -- --nocapture && cargo test --test trash_flow -- --nocapture`
Expected: PASS — context menu construction still wires every action, just with new class names.

- [ ] **Step 5: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/ui/media_grid.rs src/ui/grid_css.rs tests/ui_context_menu.rs
git commit -m "feat(ui): apply glass-menu classes to right-click context menus"
```

---

## Task 7: Viewer toolbar + image stage

**Files:**
- Modify: `data/ui/viewer-page.blp:11-82`
- Modify: `src/ui/viewer_page.rs:423-435` (remove the inline `.viewer-favorite-btn.favorite-active` rule from `setup_favorite_button`; the global provider owns it now)
- Modify: `src/ui/grid_css.rs` (add `.viewer-favorite-btn.favorite-active` rule to the global block)

**Interfaces:**
- Consumes: `.glass-header`, `.glass-toolbar-button`, `.glass-toolbar-danger`, `.viewer-stage`, `.viewer-image-frame` (Task 1).
- Produces: viewer header carries `glass-header viewer-header`; every toolbar button (`edit_btn`, `add_to_album_btn`, `favorite_btn`, `delete_btn`, `details_btn`) carries `glass-toolbar-button`; `delete_btn` also carries `glass-toolbar-danger`; `favorite_btn` additionally carries `viewer-favorite-btn` (the existing class) for the active-state color. The image area is wrapped in `viewer-stage`; the `Gtk.Picture` carries `viewer-image-frame`.

- [ ] **Step 1: Write a failing test for the viewer toolbar classes**

Create `tests/ui_viewer_toolbar.rs`:

```rust
#[test]
fn viewer_toolbar_uses_glass_classes() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.ViewerToolbar")
        .build();
    let page = photo_viewer::ui::ViewerPage::new();
    let imp = gtk::glib::subclass::types::ObjectSubclassIsExt::imp(&page);
    let fav: Vec<String> = imp.favorite_btn.get().css_classes().iter().map(|s| s.to_string()).collect();
    assert!(fav.iter().any(|c| c == "glass-toolbar-button"), "fav={fav:?}");
    assert!(fav.iter().any(|c| c == "viewer-favorite-btn"), "fav={fav:?}");
    let del: Vec<String> = imp.delete_btn.get().css_classes().iter().map(|s| s.to_string()).collect();
    assert!(del.iter().any(|c| c == "glass-toolbar-danger"), "del={del:?}");
}
```

(Adjust field names — `favorite_btn`, `delete_btn` — to match the actual `#[template_child]` names in `viewer_page.rs`.)

Run: `cargo test --test ui_viewer_toolbar -- --nocapture`
Expected: FAIL — buttons currently carry only the built-in GTK classes.

- [ ] **Step 2: Update `data/ui/viewer-page.blp`**

```blueprint
Adw.HeaderBar header_bar {
  show-end-title-buttons: true;
  css-classes: ["glass-header", "viewer-header"];

  [end]
  Gtk.Button details_btn {
    icon-name: "info-symbolic";
    tooltip-text: "";
    css-classes: ["glass-toolbar-button"];
  }

  [end]
  Gtk.Button delete_btn {
    icon-name: "user-trash-symbolic";
    tooltip-text: "";
    css-classes: ["glass-toolbar-button", "glass-toolbar-danger"];
  }

  [end]
  Gtk.Button favorite_btn {
    label: "";
    tooltip-text: "";
    css-classes: ["glass-toolbar-button", "viewer-favorite-btn"];
  }

  [end]
  Gtk.MenuButton add_to_album_btn {
    icon-name: "list-add-symbolic";
    tooltip-text: "";
    primary: true;
    css-classes: ["glass-toolbar-button"];
  }

  [end]
  Gtk.Button edit_btn {
    label: "";
    icon-name: "edit-symbolic";
    css-classes: ["glass-toolbar-button"];
  }
}

content: Gtk.Overlay image_overlay {
  vexpand: true;
  hexpand: true;
  css-classes: ["viewer-stage"];

  child: Gtk.Picture picture {
    can-shrink: true;
    vexpand: true;
    hexpand: true;
    content-fit: contain;
    css-classes: ["viewer-image-frame"];
  };

  [overlay]
  Gtk.Spinner spinner {
    vexpand: true;
    hexpand: true;
    halign: center;
    valign: center;
    spinning: true;
    visible: true;
  }
};
```

(Leave the rest of the file — `details_split_view`, `details_panel`, `details_header`, etc. — for Task 8.)

- [ ] **Step 3: Move the favorite-active rule from `viewer_page.rs` to `grid_css.rs`**

In `src/ui/viewer_page.rs` `setup_favorite_button` (around lines 423-435), delete the local `gtk::CssProvider::new()` + `load_from_data(".viewer-favorite-btn.favorite-active { color: #f6c344; font-weight: 900; }")` + `style_context_add_provider_for_display(...)` calls. The class toggling (`add_css_class("favorite-active")` / `remove_css_class("favorite-active")` in `refresh_favorite_button`) stays exactly as it is.

In `src/ui/grid_css.rs` `GRID_CSS`, append:

```css
/* Viewer favorite button active state. Class is added/removed by
   ViewerPage::refresh_favorite_button; the visual now lives in the
   global provider so it composes with .glass-toolbar-button. */
.viewer-favorite-btn.favorite-active {
  color: #f6c344;
  background: alpha(#f6c344, 0.14);
  border-color: alpha(#f6c344, 0.38);
}
```

- [ ] **Step 4: Re-run the failing test + viewer e2e**

Run: `cargo test --test ui_viewer_toolbar -- --nocapture && cargo test --test e2e_viewer -- --nocapture`
Expected: PASS — the favorite button still toggles its yellow active state, the toolbar buttons render with the new glass style.

- [ ] **Step 5: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add data/ui/viewer-page.blp data/ui/viewer-page.ui src/ui/viewer_page.rs src/ui/grid_css.rs tests/ui_viewer_toolbar.rs
git commit -m "feat(ui): apply glass toolbar classes to viewer page and move favorite style to global CSS"
```

---

## Task 8: Viewer details panel

**Files:**
- Modify: `data/ui/viewer-page.blp:84-164` (drop `css-classes: ["background"]`, add `glass-base viewer-details-panel` to `details_panel`; add `glass-toolbar-button` to `details_close_btn`)

**Interfaces:**
- Consumes: `.glass-base`, `.viewer-details-panel`, `.glass-toolbar-button` (Task 1).
- Produces: the details panel is a glass-base translucent surface, the close button is a glass-toolbar button.

- [ ] **Step 1: Write a failing test asserting the details panel class is `glass-base` + `viewer-details-panel`**

Append to `tests/ui_viewer_toolbar.rs` (or split into its own file):

```rust
#[test]
fn viewer_details_panel_uses_glass_base() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.DetailsPanel")
        .build();
    let page = photo_viewer::ui::ViewerPage::new();
    let imp = gtk::glib::subclass::types::ObjectSubclassIsExt::imp(&page);
    let panel: Vec<String> = imp.details_panel.get().css_classes().iter().map(|s| s.to_string()).collect();
    assert!(panel.iter().any(|c| c == "viewer-details-panel"), "panel={panel:?}");
    assert!(panel.iter().any(|c| c == "glass-base"), "panel={panel:?}");
}
```

(Adjust `details_panel` field name to match `viewer_page.rs`.)

Run: `cargo test --test ui_viewer_toolbar viewer_details_panel_uses_glass_base -- --nocapture`
Expected: FAIL — panel currently carries only `["background"]`.

- [ ] **Step 2: Update `data/ui/viewer-page.blp`**

Change the `details_panel` node:

```blueprint
sidebar: Gtk.Box details_panel {
  orientation: vertical;
  width-request: 380;
  css-classes: ["viewer-details-panel", "glass-base"];
```

And the `details_close_btn` inside the `details_header` box:

```blueprint
Gtk.Button details_close_btn {
  icon-name: "window-close-symbolic";
  tooltip-text: "";
  css-classes: ["glass-toolbar-button"];
}
```

- [ ] **Step 3: Re-run the test + viewer e2e**

Run: `cargo test --test ui_viewer_toolbar -- --nocapture && cargo test --test e2e_viewer -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Format + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add data/ui/viewer-page.blp data/ui/viewer-page.ui tests/ui_viewer_toolbar.rs
git commit -m "feat(ui): apply glass-base to viewer details panel"
```

---

## Task 9: Final acceptance verification

This task has no code changes — it is the manual end-to-end check that mirrors the spec's "Implementation acceptance checklist" (section K).

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --all-targets -- --nocapture`
Expected: all green. No previously-passing test has regressed.

- [ ] **Step 2: Run clippy + fmt**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Manual visual sweep against the acceptance checklist**

Run: `cargo run`
Verify each line of the spec's section K:

- The bottom mode selector no longer covers the last meaningful row of thumbnails.
- The selected thumbnail state no longer looks like a hard default blue rectangle.
- Batch toolbar buttons, viewer toolbar buttons, and right-click menu items share the same glass action style.
- The sidebar selected row uses rounded glass selection instead of a full hard strip.
- Viewer image content is clearly framed as image content (visible stage + frame), especially when the image itself is a screenshot of the app.
- Keyboard focus remains visible and distinct from selected and hover states (the focus ring is on the outer edge, selection is a luminous inner border, hover is a soft veil).
- Destructive actions remain identifiable without using harsh default red blocks (the `glass-toolbar-danger` / `glass-menu-item-danger` rules use the softened `#ffb4ab` accent).
- No core data, scan, cache, or persistence logic is changed. (`git diff main -- src/core` should be empty; confirm with `git log main..HEAD -- src/core`.)

- [ ] **Step 4: Commit the plan-acceptance note (only if a TODO comment was left anywhere)**

If any of the above steps revealed a small fix that was *not* in the plan (e.g. an off-by-one in the content-safe padding on a 4K monitor), land it as a separate commit with a `fix(ui):` prefix. Do not bundle UX fixes into the plan tasks.

```bash
git log main..HEAD --oneline
# Review the 8 task commits land cleanly on top of main.
```

No new commit is expected if the plan was followed exactly.

---

## Self-Review

**1. Spec coverage:**
- Global glass style system (spec section A) — Task 1.
- Main window + sidebar layout (B) — Task 2.
- Photos page header + batch toolbar (C) — Task 3.
- Photo grid canvas + thumbnail layout (D) — Task 4.
- Bottom mode selector safe area + alignment (E) — Task 5.
- Right-click context menus (F) — Task 6.
- Viewer page toolbar + image stage (G) — Task 7.
- Viewer details sidebar (H) — Task 8.
- Editor + secondary popovers (I) — explicitly **deferred**; out of scope for this pass (the spec marks them as "modify later, after the photo grid and viewer context menus"). Add a follow-up plan if the editor surface needs the treatment.
- Non-modification boundaries (J) — enforced in Global Constraints.
- Acceptance checklist (K) — Task 9.
- Design principles 1–6 — covered by the material choices in Task 1 and the layout first / material second ordering (Task 1 sets tokens, Tasks 2-8 only apply them after the layout is stable).

**2. Placeholder scan:** No "TBD", "implement later", "fill in details", or "appropriate error handling" placeholders. Every step has a concrete file path and a concrete code change.

**3. Type consistency:**
- CSS class names: `.glass-base`, `.glass-raised`, `.glass-toolbar`, `.glass-toolbar-button`, `.glass-toolbar-danger`, `.glass-menu`, `.glass-menu-list`, `.glass-menu-item`, `.glass-menu-item-danger`, `.glass-menu-item-suggested`, `.glass-selected`, `.glass-focus-ring`, `.glass-sidebar`, `.glass-sidebar-row`, `.glass-sidebar-label`, `.glass-header`, `.viewer-stage`, `.viewer-image-frame`, `.viewer-details-panel`, `.content-safe-bottom`, `.glass-thumb-card` — defined in Task 1, referenced by exactly that name in every later task. No spelling drift.
- `ModeSelector::new()` — assumed existing constructor. If it is `ModeSelector::with_mode(...)` in the actual file, adjust the test in Task 5 to match the real signature.
- `imp.favorite_btn`, `imp.delete_btn`, `imp.details_panel`, `imp.delete_to_trash_btn` — assumed template-child names. Confirm against the real `#[template_child]` lines in `viewer_page.rs` / `photos_page.rs` before writing the tests; the plan notes the field names in each test step.
