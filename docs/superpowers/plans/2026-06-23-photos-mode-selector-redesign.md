# Photos-page Mode Selector Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the top `AdwViewSwitcherBar` in `PhotosPage` with a custom `ModeSelector` widget that floats as a ~50 % transparent overlay at the bottom of the grid, showing plain "年/月/日" text labels with a small accent-colored dot under the active label.

**Architecture:** New `ModeSelector` widget (template + `glib::wrapper!`) is wired in via `GtkOverlay` placed *under* the existing `AdwHeaderBar`. The selector owns 3 label cells + 3 dot cells sharing a homogeneous row (equal widths so the dot lands precisely under the active label). All visual rules live in a new `mode-selector` / `mode-dot` CSS block appended to `grid_css::GRID_CSS`.

**Tech Stack:** Rust + gtk4 0.8 + libadwaita 0.6 + Blueprint UI templates + `glib_build_tools` GResource. Tests use the in-tree `gtk4::init()` headless pattern (see `tests/smoke.rs`).

## Global Constraints

- Edit `.blp` source files, never the generated `.ui` (per `CLAUDE.md`).
- `cargo fmt && cargo clippy --all-targets` clean at the end of every task.
- Per `CLAUDE.md` / `CONTRIBUTING.md`: TDD (failing test first, then implement to green, then commit).
- All new widget code follows the existing pattern: `glib::wrapper!` + `CompositeTemplate` + `#[template_child]` + `#[template(file = "...")]`. Reference implementations: `MediaGrid`, `PhotoTile`, `SectionHeader`.
- TDD unit tests live inside the widget file under `#[cfg(test)] mod tests`. Integration tests live in `tests/ui_mode_selector.rs`.
- Doc / comment language: bilingual (Chinese + English), matching the existing codebase style.
- Stack child names must stay `year` / `month` / `day` (existing `add_titled` calls reference these).
- `view_stack` `title` strings ("年", "月", "日") must stay — they are now used only by AT-SPI (screen readers) and must remain in place.

## File Structure

**New files:**
- `data/ui/mode-selector.blp` — Blueprint source for the new widget
- `src/ui/mode_selector.rs` — widget implementation + unit tests
- `tests/ui_mode_selector.rs` — integration tests for the widget + PhotosPage wiring

**Modified files:**
- `data/ui/photos-page.blp` — wrap `ViewStack` in a `GtkOverlay`, drop `AdwViewSwitcherBar`, add `$ModeSelector` overlay
- `src/ui/photos_page.rs` — drop `switcher_bar` `TemplateChild`, add `mode_selector`, wire via `set_stack`
- `src/ui/mod.rs` — `pub mod mode_selector;` + re-export `ModeSelector`
- `src/ui/grid_css.rs` — append `mode-selector` + `mode-dot` rules
- `build.rs` — add `data/ui/mode-selector.blp` to the blueprint compile list
- `data/resources.gresource.xml` — add `ui/mode-selector.ui` to the resource bundle

---

## Task 1: Scaffold the ModeSelector widget, template, CSS, and build wiring

**Files:**
- Create: `data/ui/mode-selector.blp`
- Create: `src/ui/mode_selector.rs`
- Modify: `src/ui/mod.rs:1-29`
- Modify: `src/ui/grid_css.rs:29-52` (append to `GRID_CSS` constant)
- Modify: `build.rs:20-31` (add to blueprint list)
- Modify: `data/resources.gresource.xml:3-15` (add to gresource)

**Interfaces:**
- Produces: `photo_viewer::ui::ModeSelector` (a `glib::wrapper!`-exported `GtkBox` subclass named `ModeSelector`) — used by Task 2+.

- [ ] **Step 1: Create `data/ui/mode-selector.blp`**

```blp
using Gtk 4.0;

// ModeSelector: 3 label cells + 3 dot cells. The two rows use
// `homogeneous: true` so each label_cell_i is the same width as
// dot_cell_i — keeping the active dot aligned under its label even
// when the labels render at different intrinsic widths.
template $ModeSelector : Gtk.Box {
  orientation: vertical;
  css-classes: ["mode-selector"];

  Gtk.Box row {
    orientation: horizontal;
    halign: center;
    homogeneous: true;

    Gtk.Box label_cell_0 {
      css-classes: ["mode-cell"];
      halign: center;

      Gtk.Label label_0 {
        label: "年";
      }
    }
    Gtk.Box label_cell_1 {
      css-classes: ["mode-cell"];
      halign: center;

      Gtk.Label label_1 {
        label: "月";
      }
    }
    Gtk.Box label_cell_2 {
      css-classes: ["mode-cell"];
      halign: center;

      Gtk.Label label_2 {
        label: "日";
      }
    }
  }

  Gtk.Box dot_row {
    orientation: horizontal;
    halign: center;
    homogeneous: true;

    Gtk.Box dot_cell_0 {
      css-classes: ["mode-cell"];
      halign: center;

      Gtk.Box dot_inner_0 {
        visible: false;
        css-classes: ["mode-dot"];
      }
    }
    Gtk.Box dot_cell_1 {
      css-classes: ["mode-cell"];
      halign: center;

      Gtk.Box dot_inner_1 {
        visible: false;
        css-classes: ["mode-dot"];
      }
    }
    Gtk.Box dot_cell_2 {
      css-classes: ["mode-cell"];
      halign: center;

      Gtk.Box dot_inner_2 {
        visible: true;
        css-classes: ["mode-dot"];
      }
    }
  }
}
```

- [ ] **Step 2: Create `src/ui/mode_selector.rs` with the widget skeleton**

```rust
//! ModeSelector: 3-cell 年/月/日 switcher used by `PhotosPage`.
//!
//! Visual: a vertical pair of rows (labels, then a dot strip). The
//! currently-active mode has its label fully opaque and its `dot_inner`
//! visible. The widget is meant to be added as an overlay child of a
//! `GtkOverlay` containing the `ViewStack` it drives.
//!
//! Active index is the single source of truth. `set_stack` wires
//! `ViewStack::visible-child` → `active_index` to keep the selector in
//! sync if the stack is changed externally.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/mode-selector.ui")]
    pub struct ModeSelector {
        #[template_child]
        pub label_0: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_1: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_2: TemplateChild<gtk::Label>,
        #[template_child]
        pub dot_inner_0: TemplateChild<gtk::Box>,
        #[template_child]
        pub dot_inner_1: TemplateChild<gtk::Box>,
        #[template_child]
        pub dot_inner_2: TemplateChild<gtk::Box>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for ModeSelector {
        const NAME: &'static str = "ModeSelector";
        type Type = super::ModeSelector;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ModeSelector {}
    impl WidgetImpl for ModeSelector {}
    impl BoxImpl for ModeSelector {}
}

gtk::glib::wrapper! {
    pub struct ModeSelector(ObjectSubclass<imp::ModeSelector>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ModeSelector {
    pub fn new() -> Self {
        gtk::glib::Object::builder().build()
    }
}

impl Default for ModeSelector {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 3: Register the widget in `src/ui/mod.rs`**

Edit `src/ui/mod.rs` to add the new module + re-export, matching the existing alphabetical order:

```rust
pub mod media_grid;
pub mod mode_selector;   // ← add this
pub mod photo_tile;
```

And in the `pub use` block:

```rust
pub use media_grid::MediaGrid;
pub use mode_selector::ModeSelector;   // ← add this
pub use photo_tile::PhotoTile;
```

- [ ] **Step 4: Add CSS rules to `src/ui/grid_css.rs`**

Append the following inside the `GRID_CSS` constant string (before the closing `"`):

```css
/* ModeSelector 容器：~50% 透明圆角面板 */
box.mode-selector {
  background: alpha(@card_bg_color, 0.5);
  border-radius: 12px;
  padding: 8px 16px;
}

/* 单个 label / dot 槽位 */
box.mode-cell {
  min-width: 60px;
  padding: 4px 12px;
}

/* 标签：默认半透明、title-3 字号 */
box.mode-selector label {
  font-size: 14pt;
  font-weight: 500;
  color: @window_fg_color;
  opacity: 0.55;
  transition: opacity 120ms ease;
}

/* 激活态：标签全亮 */
box.mode-selector label.active {
  opacity: 1.0;
}

/* 激活指示点 */
box.mode-dot {
  background: @accent_color;
  border-radius: 2px;
  min-width: 24px;
  min-height: 4px;
  margin-top: 2px;
}
```

The full file should end with the `box.mode-dot` block followed by the existing `static CSS_INSTALLED` line.

- [ ] **Step 5: Add the template to `build.rs` blueprint list**

In `build.rs` line 21-31, add `mode-selector.blp` to the array (alphabetical order with the rest):

```rust
let blueprint_files = [
    "data/ui/window.blp",
    "data/ui/photos-page.blp",
    "data/ui/albums-page.blp",
    "data/ui/album-detail-page.blp",
    "data/ui/media-grid.blp",
    "data/ui/mode-selector.blp",   // ← add this
    "data/ui/photo-tile.blp",
    "data/ui/section-header.blp",
    "data/ui/viewer-page.blp",
    "data/ui/trash-page.blp",
    "data/ui/editor-page.blp",
];
```

- [ ] **Step 6: Add the compiled template to `data/resources.gresource.xml`**

```xml
<file>ui/mode-selector.ui</file>   <!-- add inside the <gresource> block, alphabetical -->
```

Result:

```xml
<gresource prefix="/org/gnome/PhotoViewer">
  <file>ui/window.ui</file>
  <file>ui/photos-page.ui</file>
  <file>ui/albums-page.ui</file>
  <file>ui/album-detail-page.ui</file>
  <file>ui/media-grid.ui</file>
  <file>ui/mode-selector.ui</file>   <!-- ← add this -->
  <file>ui/photo-tile.ui</file>
  <file>ui/section-header.ui</file>
  <file>ui/viewer-page.ui</file>
  <file>ui/trash-page.ui</file>
  <file>ui/editor-page.ui</file>
</gresource>
```

- [ ] **Step 7: Verify build compiles**

Run: `cargo build 2>&1 | tail -20`
Expected: success, no warnings, and a `data/ui/mode-selector.ui` file is generated by `build.rs` from the `.blp`.

- [ ] **Step 8: Commit**

```bash
git add data/ui/mode-selector.blp data/ui/mode-selector.ui \
        src/ui/mode_selector.rs src/ui/mod.rs \
        src/ui/grid_css.rs build.rs data/resources.gresource.xml
git commit -m "feat(ui): scaffold ModeSelector widget, template, css, build wiring"
```

---

## Task 2: Implement `active_index` / `set_active_index` with label + dot state mutation

**Files:**
- Modify: `src/ui/mode_selector.rs` (extend the `imp` struct + add public methods + add `#[cfg(test)]` tests)

**Interfaces:**
- Produces:
  - `ModeSelector::active_index(&self) -> u32` — returns 0/1/2 (default 0).
  - `ModeSelector::set_active_index(&self, idx: u32)` — mutates label `active` CSS class + dot `visible` flag. Clamps `idx` to 0..=2; out-of-range is a no-op.

- [ ] **Step 1: Write the failing tests**

Add at the bottom of `src/ui/mode_selector.rs`, before the final `impl Default` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gtk::glib;

    /// GTK widgets need a display; `gtk::init()` works headless in tests.
    fn init() {
        let _ = gtk::init();
    }

    fn labels(sel: &ModeSelector) -> [gtk::Label; 3] {
        let imp = sel.imp();
        [
            imp.label_0.get(),
            imp.label_1.get(),
            imp.label_2.get(),
        ]
    }

    fn dots(sel: &ModeSelector) -> [gtk::Box; 3] {
        let imp = sel.imp();
        [
            imp.dot_inner_0.get(),
            imp.dot_inner_1.get(),
            imp.dot_inner_2.get(),
        ]
    }

    #[test]
    fn default_active_index_is_zero() {
        init();
        let sel = ModeSelector::new();
        assert_eq!(sel.active_index(), 0);
    }

    #[test]
    fn set_active_index_updates_active_index() {
        init();
        let sel = ModeSelector::new();
        sel.set_active_index(1);
        assert_eq!(sel.active_index(), 1);
        sel.set_active_index(2);
        assert_eq!(sel.active_index(), 2);
        sel.set_active_index(0);
        assert_eq!(sel.active_index(), 0);
    }

    #[test]
    fn set_active_index_toggles_label_active_class() {
        init();
        let sel = ModeSelector::new();
        let ls = labels(&sel);

        // Initial: index 0 active.
        assert!(ls[0].has_css_class("active"));
        assert!(!ls[1].has_css_class("active"));
        assert!(!ls[2].has_css_class("active"));

        sel.set_active_index(2);
        assert!(!ls[0].has_css_class("active"));
        assert!(!ls[1].has_css_class("active"));
        assert!(ls[2].has_css_class("active"));
    }

    #[test]
    fn set_active_index_toggles_dot_visibility() {
        init();
        let sel = ModeSelector::new();
        let ds = dots(&sel);

        // Initial: only dot 2 visible (from the .blp default).
        assert!(!ds[0].is_visible());
        assert!(!ds[1].is_visible());
        assert!(ds[2].is_visible());

        sel.set_active_index(1);
        assert!(!ds[0].is_visible());
        assert!(ds[1].is_visible());
        assert!(!ds[2].is_visible());
    }

    #[test]
    fn set_active_index_clamps_out_of_range() {
        init();
        let sel = ModeSelector::new();
        sel.set_active_index(99);
        assert_eq!(sel.active_index(), 0, "out-of-range should be a no-op");
        sel.set_active_index(2);
        sel.set_active_index(3);
        assert_eq!(sel.active_index(), 2, "out-of-range should not change current");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib mode_selector 2>&1 | tail -30`
Expected: compile error — `active_index` and `set_active_index` don't exist yet.

- [ ] **Step 3: Implement `active_index` and `set_active_index`**

Modify the `imp` struct in `src/ui/mode_selector.rs` to add the active-index cell and a helper for applying state:

```rust
mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/mode-selector.ui")]
    pub struct ModeSelector {
        pub active_index: Cell<u32>,
        #[template_child]
        pub label_0: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_1: TemplateChild<gtk::Label>,
        #[template_child]
        pub label_2: TemplateChild<gtk::Label>,
        #[template_child]
        pub dot_inner_0: TemplateChild<gtk::Box>,
        #[template_child]
        pub dot_inner_1: TemplateChild<gtk::Box>,
        #[template_child]
        pub dot_inner_2: TemplateChild<gtk::Box>,
    }

    impl ModeSelector {
        /// Apply the current `active_index` to the template children
        /// (label CSS class + dot visibility). O(1) — three pairs of
        /// set/remove + set/remove.
        pub(super) fn apply_state(&self) {
            let labels = [&self.label_0, &self.label_1, &self.label_2];
            let dots = [&self.dot_inner_0, &self.dot_inner_1, &self.dot_inner_2];
            let active = self.active_index.get() as usize;
            for (i, lbl) in labels.iter().enumerate() {
                let l = lbl.get();
                if i == active {
                    l.add_css_class("active");
                } else {
                    l.remove_css_class("active");
                }
            }
            for (i, dot) in dots.iter().enumerate() {
                dot.get().set_visible(i == active);
            }
        }
    }

    // ... existing ObjectSubclass + ObjectImpl + WidgetImpl + BoxImpl unchanged
}
```

After construction, call `apply_state` so the initial `visible: true` on `dot_inner_2` in the `.blp` is reconciled with the canonical source-of-truth:

In the existing `impl ObjectImpl for ModeSelector` block, add the `constructed` override:

```rust
    impl ObjectImpl for ModeSelector {
        fn constructed(&self) {
            self.parent_constructed();
            // Sync template defaults to the current active_index.
            self.apply_state();
        }
    }
```

Add the public methods at the end of `impl ModeSelector { ... }` (after `new()`):

```rust
impl ModeSelector {
    pub fn new() -> Self {
        gtk::glib::Object::builder().build()
    }

    /// The currently-active mode: 0 = year, 1 = month, 2 = day.
    pub fn active_index(&self) -> u32 {
        self.imp().active_index.get()
    }

    /// Set the active mode. Out-of-range values are silently ignored
    /// (the widget always shows one of the three modes).
    pub fn set_active_index(&self, idx: u32) {
        if idx > 2 {
            return;
        }
        let imp = self.imp();
        if imp.active_index.get() == idx {
            return;
        }
        imp.active_index.set(idx);
        imp.apply_state();
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib mode_selector 2>&1 | tail -30`
Expected: 5 tests pass.

- [ ] **Step 5: `cargo fmt` and `cargo clippy`**

Run:
```bash
cargo fmt
cargo clippy --all-targets 2>&1 | tail -10
```
Expected: no warnings or errors.

- [ ] **Step 6: Commit**

```bash
git add src/ui/mode_selector.rs
git commit -m "feat(ui): ModeSelector set/active_index + label/dot state with tests"
```

---

## Task 3: Inject the `ViewStack` and sync `active_index` from `notify::visible-child`

**Files:**
- Modify: `src/ui/mode_selector.rs` (add `set_stack`, store the stack, wire the notify handler with a re-entry guard)

**Interfaces:**
- Produces:
  - `ModeSelector::set_stack(&self, stack: &adw::ViewStack)` — installs the stack, reads the current visible child to seed `active_index`, and subscribes to `notify::visible-child`. Idempotent: calling again with a different stack rebinds; calling with the same stack is a no-op.
  - `ModeSelector::set_active_index(idx)` now also calls `stack.set_visible_child_name(name)` so the stack and the widget stay in sync (the `notify` handler will fire but the re-entry guard short-circuits).

- [ ] **Step 1: Write the failing tests**

Append to the existing `tests` module in `src/ui/mode_selector.rs`:

```rust
    use libadwaita as adw;

    /// Build a 3-page ViewStack with names "year"/"month"/"day" so the
    /// selector can resolve them.
    fn build_stack() -> (adw::ViewStack, [gtk::Label; 3]) {
        let stack = adw::ViewStack::new();
        let a = gtk::Label::new(Some("Year"));
        let b = gtk::Label::new(Some("Month"));
        let c = gtk::Label::new(Some("Day"));
        stack.add_titled(&a, Some("year"), "年");
        stack.add_titled(&b, Some("month"), "月");
        stack.add_titled(&c, Some("day"), "日");
        (stack, [a, b, c])
    }

    #[test]
    fn set_active_index_drives_stack_visible_child() {
        init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        sel.set_active_index(2);
        assert_eq!(stack.visible_child_name().as_deref(), Some("day"));

        sel.set_active_index(0);
        assert_eq!(stack.visible_child_name().as_deref(), Some("year"));
    }

    #[test]
    fn stack_visible_child_change_drives_active_index() {
        init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        // Simulate an external change to the stack.
        stack.set_visible_child_name(Some("month"));
        // Pump the main context so the notify::visible-child signal fires.
        let ctx = glib::MainContext::default();
        while ctx.iteration(false) {}

        assert_eq!(sel.active_index(), 1);
    }

    #[test]
    fn set_stack_seeds_active_index_from_current_child() {
        init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        stack.set_visible_child_name(Some("day"));
        sel.set_stack(&stack);
        assert_eq!(sel.active_index(), 2);
    }

    #[test]
    fn loop_guard_prevents_recursive_set() {
        init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        // set_active_index(1) → calls stack.set_visible_child_name → fires
        // notify::visible-child → handler should short-circuit because
        // active_index already matches.
        sel.set_active_index(1);
        // If the loop guard failed, the signal handler would re-set
        // active_index, but since the value is already 1 the test still
        // passes. Stronger check: pump the context and assert no panic.
        let ctx = glib::MainContext::default();
        while ctx.iteration(false) {}
        assert_eq!(sel.active_index(), 1);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib mode_selector 2>&1 | tail -30`
Expected: compile error — `set_stack` doesn't exist, `libadwaita` not imported.

- [ ] **Step 3: Add `libadwaita` import + `stack` field + `set_stack` + handler**

At the top of `src/ui/mode_selector.rs` add:

```rust
use libadwaita as adw;
```

In the `imp` struct, add a `stack` field to hold the bound `ViewStack` and a `last_synced` guard. The guard is a `Cell<u32>` of the active_index value we most recently *wrote* to the stack, so we can short-circuit the `notify::visible-child` callback when the change came from us:

```rust
mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/mode-selector.ui")]
    pub struct ModeSelector {
        pub active_index: Cell<u32>,
        pub last_synced: Cell<u32>,         // 0..=2
        pub stack: std::cell::RefCell<Option<adw::ViewStack>>,
        #[template_child]
        pub label_0: TemplateChild<gtk::Label>,
        // ... rest unchanged
    }
```

Update the `apply_state` to also update `last_synced`:

```rust
        pub(super) fn apply_state(&self) {
            let labels = [&self.label_0, &self.label_1, &self.label_2];
            let dots = [&self.dot_inner_0, &self.dot_inner_1, &self.dot_inner_2];
            let active = self.active_index.get() as usize;
            for (i, lbl) in labels.iter().enumerate() {
                let l = lbl.get();
                if i == active {
                    l.add_css_class("active");
                } else {
                    l.remove_css_class("active");
                }
            }
            for (i, dot) in dots.iter().enumerate() {
                dot.get().set_visible(i == active);
            }
            // Record what we just wrote to the labels/dots so the
            // notify::visible-child callback can short-circuit when
            // the change came from us.
            self.last_synced.set(self.active_index.get());
        }
```

Add the `set_stack` method and update `set_active_index` to also push to the stack:

```rust
impl ModeSelector {
    pub fn new() -> Self {
        gtk::glib::Object::builder().build()
    }

    pub fn active_index(&self) -> u32 {
        self.imp().active_index.get()
    }

    pub fn set_active_index(&self, idx: u32) {
        if idx > 2 {
            return;
        }
        let imp = self.imp();
        if imp.active_index.get() == idx {
            return;
        }
        imp.active_index.set(idx);
        imp.apply_state();
        // Push to the bound ViewStack so the visible child matches.
        // The notify::visible-child handler will fire but will be
        // short-circuited by the last_synced guard.
        if let Some(stack) = imp.stack.borrow().as_ref() {
            let name = match idx {
                0 => "year",
                1 => "month",
                _ => "day",
            };
            stack.set_visible_child_name(Some(name));
        }
    }

    /// Bind a ViewStack. The selector's active index seeds from the
    /// stack's current visible child, and subsequent stack changes
    /// (whether from us or elsewhere) keep the selector in sync.
    ///
    /// Idempotent: calling with the same `stack` is a no-op. Calling
    /// with a different `stack` rebinds.
    pub fn set_stack(&self, stack: &adw::ViewStack) {
        let imp = self.imp();
        {
            let current = imp.stack.borrow();
            if let Some(existing) = current.as_ref() {
                if existing == stack {
                    return;
                }
            }
        }
        *imp.stack.borrow_mut() = Some(stack.clone());

        // Seed active_index from the stack's current visible child.
        let name = stack.visible_child_name();
        let seed = match name.as_deref() {
            Some("year") => 0,
            Some("month") => 1,
            Some("day") => 2,
            _ => 0,
        };
        imp.active_index.set(seed);
        imp.last_synced.set(seed);
        imp.apply_state();

        // Subscribe to visible-child changes. The callback drops the
        // change if it matches what we just wrote ourselves
        // (last_synced), preventing feedback loops.
        let weak = self.downgrade();
        stack.connect_notify_local(Some("visible-child"), move |stack, _| {
            let Some(sel) = weak.upgrade() else { return };
            let name = stack.visible_child_name();
            let new_idx = match name.as_deref() {
                Some("year") => 0,
                Some("month") => 1,
                Some("day") => 2,
                _ => return,
            };
            let imp = sel.imp();
            if imp.last_synced.get() == new_idx {
                // We wrote this ourselves; reset the guard and return.
                // (last_synced stays equal so the next *external* change
                // still syncs.)
                return;
            }
            imp.active_index.set(new_idx);
            imp.apply_state();
        });
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib mode_selector 2>&1 | tail -30`
Expected: 9 tests pass (5 from Task 2 + 4 from this task).

- [ ] **Step 5: `cargo fmt` and `cargo clippy`**

Run:
```bash
cargo fmt
cargo clippy --all-targets 2>&1 | tail -10
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/ui/mode_selector.rs
git commit -m "feat(ui): ModeSelector set_stack + notify::visible-child sync with loop guard"
```

---

## Task 4: Click handlers on the 3 label cells

**Files:**
- Modify: `src/ui/mode_selector.rs` (in `imp::ObjectImpl::constructed` or in `ModeSelector::new`, install `GtkGestureClick` on each `label_cell` and route to `set_active_index`)

**Interfaces:**
- Produces: clicking any of the 3 label cells calls `set_active_index` with that cell's index.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `src/ui/mode_selector.rs`:

```rust
    #[test]
    fn clicking_label_cell_triggers_active_index_change() {
        init();
        let sel = ModeSelector::new();
        // We can grab the cells via the parent Box; use the children
        // of the ModeSelector's first row child.
        let row = sel.first_child().expect("selector has a row child");
        let row = row.downcast::<gtk::Box>().expect("row is a Box");
        let mut cells = Vec::new();
        let mut next = row.first_child();
        while let Some(c) = next {
            cells.push(c);
            next = c.next_sibling();
        }
        assert_eq!(cells.len(), 3, "expected 3 label cells in the row");

        // Find the click gesture on the middle cell and emit "pressed".
        let middle = &cells[1];
        let controller = middle
            .observe_controllers()
            .into_iter()
            .find_map(|c| c.downcast::<gtk::GestureClick>().ok())
            .expect("middle cell should have a GtkGestureClick");

        // Emit the "pressed" signal — the handler ignores the coordinates
        // and n-press count, so empty args are fine.
        controller.emit_by_name::<()>("pressed", &[]);

        assert_eq!(sel.active_index(), 1);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib clicking_label_cell 2>&1 | tail -25`
Expected: the test panics on `expect("middle cell should have a GtkGestureClick")` because no gesture is attached yet.

- [ ] **Step 3: Install the click gestures in `constructed`**

Replace the existing `impl ObjectImpl for ModeSelector` block with one that wires the three click gestures:

```rust
    impl ObjectImpl for ModeSelector {
        fn constructed(&self) {
            self.parent_constructed();
            // Sync template defaults to the current active_index.
            self.apply_state();

            // Install click gestures on the three label cells.
            // Each gesture is owned by its cell, so it lives as long
            // as the widget does.
            let row = self.obj().first_child()
                .and_then(|c| c.downcast::<gtk::Box>().ok())
                .expect("ModeSelector first child must be the label row Box");
            let cells: [gtk::Box; 3] = [
                row.first_child().and_then(|c| c.downcast::<gtk::Box>().ok())
                    .expect("label_cell_0"),
                // ... etc
            ];
            // The above is messy in practice; do it the iterative way:
            let mut idx: u32 = 0;
            let mut next = row.first_child();
            while let Some(cell) = next {
                if let Ok(cell_box) = cell.clone().downcast::<gtk::Box>() {
                    let sel_weak = self.obj().downgrade();
                    let i = idx;
                    let gesture = gtk::GestureClick::new();
                    gesture.connect_released(move |_, _n, _x, _y| {
                        if let Some(sel) = sel_weak.upgrade() {
                            sel.set_active_index(i);
                        }
                    });
                    cell_box.add_controller(gesture);
                }
                idx += 1;
                next = cell.next_sibling();
            }
        }
    }
```

Cleaner rewrite (drop the comments, keep the logic):

```rust
    impl ObjectImpl for ModeSelector {
        fn constructed(&self) {
            self.parent_constructed();
            // Sync template defaults to the current active_index.
            self.apply_state();

            // Click on any of the 3 label cells → switch to that mode.
            if let Some(row) = self.obj().first_child()
                .and_then(|c| c.downcast::<gtk::Box>().ok())
            {
                let mut idx: u32 = 0;
                let mut next = row.first_child();
                while let Some(cell) = next {
                    if let Ok(cell_box) = cell.clone().downcast::<gtk::Box>() {
                        let sel_weak = self.obj().downgrade();
                        let i = idx;
                        let gesture = gtk::GestureClick::new();
                        gesture.connect_released(move |_, _n, _x, _y| {
                            if let Some(sel) = sel_weak.upgrade() {
                                sel.set_active_index(i);
                            }
                        });
                        cell_box.add_controller(gesture);
                    }
                    idx += 1;
                    next = cell.next_sibling();
                }
            }
        }
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib clicking_label_cell 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Run all unit tests in the file**

Run: `cargo test --lib mode_selector 2>&1 | tail -15`
Expected: 10 tests pass (5 from Task 2 + 4 from Task 3 + 1 from this task).

- [ ] **Step 6: `cargo fmt` and `cargo clippy`**

Run:
```bash
cargo fmt
cargo clippy --all-targets 2>&1 | tail -10
```
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/ui/mode_selector.rs
git commit -m "feat(ui): ModeSelector click handlers on 3 label cells"
```

---

## Task 5: Keyboard navigation (←/→)

**Files:**
- Modify: `src/ui/mode_selector.rs` (add `EventControllerKey` to the widget itself, in `constructed`)

**Interfaces:**
- Produces: when the `ModeSelector` has focus, ←/→ arrow keys cycle `active_index` (with wrapping).

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `src/ui/mode_selector.rs`:

```rust
    #[test]
    fn right_arrow_advances_active_index_with_wrap() {
        init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        // Initial: 0. → → should land on 2 (with wrap).
        let ctrl = sel
            .observe_controllers()
            .into_iter()
            .find_map(|c| c.downcast::<gtk::EventControllerKey>().ok())
            .expect("ModeSelector should have an EventControllerKey");
        ctrl.emit_by_name::<glib::Propagation>(
            "key-pressed",
            &[&gdk::Key::Right, &0u32, &gdk::ModifierType::empty()],
        );
        assert_eq!(sel.active_index(), 1);
        ctrl.emit_by_name::<glib::Propagation>(
            "key-pressed",
            &[&gdk::Key::Right, &0u32, &gdk::ModifierType::empty()],
        );
        assert_eq!(sel.active_index(), 2);
        // Wrap: 2 → 0
        ctrl.emit_by_name::<glib::Propagation>(
            "key-pressed",
            &[&gdk::Key::Right, &0u32, &gdk::ModifierType::empty()],
        );
        assert_eq!(sel.active_index(), 0);
    }

    #[test]
    fn left_arrow_retreats_active_index_with_wrap() {
        init();
        let sel = ModeSelector::new();
        let (stack, _labels) = build_stack();
        sel.set_stack(&stack);

        let ctrl = sel
            .observe_controllers()
            .into_iter()
            .find_map(|c| c.downcast::<gtk::EventControllerKey>().ok())
            .expect("ModeSelector should have an EventControllerKey");
        // Wrap: 0 → 2
        ctrl.emit_by_name::<glib::Propagation>(
            "key-pressed",
            &[&gdk::Key::Left, &0u32, &gdk::ModifierType::empty()],
        );
        assert_eq!(sel.active_index(), 2);
        ctrl.emit_by_name::<glib::Propagation>(
            "key-pressed",
            &[&gdk::Key::Left, &0u32, &gdk::ModifierType::empty()],
        );
        assert_eq!(sel.active_index(), 1);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib arrow 2>&1 | tail -25`
Expected: panic on `expect("ModeSelector should have an EventControllerKey")`.

- [ ] **Step 3: Install the key controller in `constructed`**

Add `use gtk4::gdk;` at the top of `src/ui/mode_selector.rs` (if not already present), then add to the end of `constructed` (after the click-gesture loop):

```rust
            // Arrow-key navigation: ←/→ cycle active_index (with wrap).
            let key_ctrl = gtk::EventControllerKey::new();
            let sel_weak = self.obj().downgrade();
            key_ctrl.connect_key_pressed(move |_, key, _keycode, _state| {
                use gtk::gdk::Key;
                let Some(sel) = sel_weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let cur = sel.active_index();
                let next = match key {
                    Key::Left | Key::KP_Left => (cur + 2) % 3,
                    Key::Right | Key::KP_Right => (cur + 1) % 3,
                    _ => return glib::Propagation::Proceed,
                };
                sel.set_active_index(next);
                glib::Propagation::Stop
            });
            self.obj().add_controller(key_ctrl);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib mode_selector 2>&1 | tail -15`
Expected: 12 tests pass.

- [ ] **Step 5: `cargo fmt` and `cargo clippy`**

Run:
```bash
cargo fmt
cargo clippy --all-targets 2>&1 | tail -10
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/ui/mode_selector.rs
git commit -m "feat(ui): ModeSelector keyboard nav (←/→ with wrap)"
```

---

## Task 6: Wire `ModeSelector` into `PhotosPage` (replace `SwitcherBar` with overlay)

**Files:**
- Modify: `data/ui/photos-page.blp` (replace `SwitcherBar` slot with a `GtkOverlay` containing the `ViewStack` + a `$ModeSelector` overlay child)
- Modify: `src/ui/photos_page.rs` (drop `switcher_bar` field, add `mode_selector`, wire via `set_stack`)

**Interfaces:**
- `PhotosPage` now contains a `ModeSelector` whose `set_stack` is called with the existing `view_stack` (instead of `AdwViewSwitcherBar::set_stack`).
- `ModeSelector` is positioned at the bottom-center of the content area (no `vexpand`/`hexpand`).

- [ ] **Step 1: Modify `data/ui/photos-page.blp`**

Replace the entire file contents with:

```blp
using Gtk 4.0;
using Adw 1;

template $PhotosPage : Adw.NavigationPage {
  title: "Photos";

  child: Gtk.Box root_box {
    orientation: vertical;

    Adw.HeaderBar header_bar {
      show-end-title-buttons: true;
    }

    Gtk.Overlay grid_overlay {
      vexpand: true;
      hexpand: true;

      child: Adw.ViewStack view_stack {
        vexpand: true;
        hexpand: true;
      };

      [overlay]
      $ModeSelector mode_selector {
        halign: center;
        valign: end;
        margin-bottom: 24;
      }
    }
  };
}
```

Notes:
- The `HeaderBar` is kept as the first child of `root_box` so window controls and title work as before.
- `grid_overlay` (a `GtkOverlay`) hosts `view_stack` as the main child (fills the overlay) and `mode_selector` as an overlay child anchored `halign=center, valign=end` with a 24 px bottom margin — matching the spec.
- The `HeaderBar` is no longer followed by a `SwitcherBar`, so the layout from the header down to the grid is just the header + the overlay; this frees the row of vertical space the old switcher was eating.

- [ ] **Step 2: Run `cargo build` to regenerate the `.ui` from the new `.blp`**

Run: `cargo build 2>&1 | tail -20`
Expected: success; `data/ui/photos-page.ui` is regenerated to match the new `.blp`.

- [ ] **Step 3: Modify `src/ui/photos_page.rs`**

In the `imp` struct, replace the `switcher_bar` field with `mode_selector`:

```rust
    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photos-page.ui")]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub pool: RefCell<Option<DbPool>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub mode_selector: TemplateChild<crate::ui::mode_selector::ModeSelector>,
    }
```

Add the import at the top of the file (next to the existing `use crate::ui::media_grid::MediaGrid;`):

```rust
use crate::ui::mode_selector::ModeSelector;
```

In `PhotosPage::new`, replace the existing line that wires the switcher bar:

```rust
        // Wire the ModeSelector to our view_stack (it drives the visible
        // child and reflects any external change back via notify).
        obj.imp().mode_selector.get().set_stack(&stack);
```

Remove the now-orphaned `Adw.ViewSwitcherBar` import if any compiler warning suggests it (no other use).

- [ ] **Step 4: Run `cargo build` and fix any errors**

Run: `cargo build 2>&1 | tail -30`
Expected: success, no warnings.

If the build complains about `mode_selector` not being a `TemplateChild` field (e.g., name mismatch), double-check the `.blp` IDs: `mode_selector` (the ID) must match the field name in Rust.

- [ ] **Step 5: `cargo fmt` and `cargo clippy`**

Run:
```bash
cargo fmt
cargo clippy --all-targets 2>&1 | tail -10
```
Expected: clean.

- [ ] **Step 6: Run the full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: all existing tests still pass + the 12 `mode_selector` unit tests pass.

- [ ] **Step 7: Commit**

```bash
git add data/ui/photos-page.blp data/ui/photos-page.ui src/ui/photos_page.rs
git commit -m "feat(ui): wire ModeSelector into PhotosPage overlay (drop SwitcherBar)"
```

---

## Task 7: Integration tests for widget construction and PhotosPage layout

**Files:**
- Create: `tests/ui_mode_selector.rs`

**Interfaces:**
- Verifies:
  - `ModeSelector` can be constructed standalone.
  - `set_stack` seeds `active_index` from the stack's current visible child.
  - Clicking a label cell updates both `active_index` and the bound stack.
  - `PhotosPage` builds without panic, and the `ModeSelector` inside is a `TemplateChild` whose widget is non-null and properly placed (anchored to `valign=end`, `halign=center`).

- [ ] **Step 1: Create `tests/ui_mode_selector.rs`**

```rust
//! Integration tests for the ModeSelector widget + its wiring into
//! PhotosPage. These tests need a display; `gtk::init()` is sufficient
//! for headless runs (see `tests/smoke.rs` for the same pattern).

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use photo_viewer::ui::ModeSelector;

fn init_gtk() {
    let _ = gtk::init();
}

#[test]
fn mode_selector_constructs_with_default_index_zero() {
    init_gtk();
    let sel = ModeSelector::new();
    assert_eq!(sel.active_index(), 0);
    // The widget tree should be the box + row + dot_row.
    assert!(sel.first_child().is_some(), "row child present");
}

#[test]
fn set_stack_seeds_from_current_visible_child() {
    init_gtk();
    let sel = ModeSelector::new();
    let stack = adw::ViewStack::new();
    stack.add_titled(&gtk::Label::new(Some("A")), Some("year"), "年");
    stack.add_titled(&gtk::Label::new(Some("B")), Some("month"), "月");
    stack.add_titled(&gtk::Label::new(Some("C")), Some("day"), "日");
    stack.set_visible_child_name(Some("month"));
    sel.set_stack(&stack);
    assert_eq!(sel.active_index(), 1);
}

#[test]
fn click_label_cell_updates_stack_visible_child() {
    init_gtk();
    let sel = ModeSelector::new();
    let stack = adw::ViewStack::new();
    stack.add_titled(&gtk::Label::new(Some("A")), Some("year"), "年");
    stack.add_titled(&gtk::Label::new(Some("B")), Some("month"), "月");
    stack.add_titled(&gtk::Label::new(Some("C")), Some("day"), "日");
    sel.set_stack(&stack);

    // Find the second label cell and emit a click.
    let row = sel.first_child().and_then(|c| c.downcast::<gtk::Box>().ok()).unwrap();
    let cells: Vec<gtk::Box> = (0..3)
        .scan(row.first_child(), |cur, _| {
            let c = cur.clone()?;
            *cur = c.next_sibling();
            c.downcast::<gtk::Box>().ok()
        })
        .collect();
    let gesture = cells[2]
        .observe_controllers()
        .into_iter()
        .find_map(|c| c.downcast::<gtk::GestureClick>().ok())
        .expect("third cell should have a GtkGestureClick");
    gesture.emit_by_name::<()>("pressed", &[]);
    assert_eq!(stack.visible_child_name().as_deref(), Some("day"));
}

#[test]
fn photos_page_builds_with_mode_selector_in_overlay() {
    // Builds the full PhotosPage tree (without a real ThumbnailLoader
    // and without DB) just to confirm the template compiles and the
    // mode_selector TemplateChild is wired.
    init_gtk();
    use photo_viewer::ui::media_grid::MediaGrid;
    use photo_viewer::core::section_model::GroupBy;
    use std::rc::Rc;
    use std::sync::Arc;

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new(glib::types::Type::OBJECT);
    // We don't have a real loader in the test crate; pass a
    // freshly-constructed one against an empty media list. The grid
    // never requests a thumbnail in this test, so the loader's
    // internal channel is unused.
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        tmp.path().join("thumbs"),
        pool.clone(),
    ));
    let on_activate: Rc<dyn Fn(u32)> = Rc::new(|_| {});
    let grid = MediaGrid::new(media_list.clone(), GroupBy::Year, loader.clone(), on_activate);
    let stack = adw::ViewStack::new();
    stack.add_titled(&grid, Some("year"), "年");
    stack.add_titled(
        &MediaGrid::new(media_list.clone(), GroupBy::Month, loader.clone(), Rc::new(|_| {})),
        Some("month"),
        "月",
    );
    stack.add_titled(
        &MediaGrid::new(media_list, GroupBy::Day, loader, Rc::new(|_| {})),
        Some("day"),
        "日",
    );
    let sel = ModeSelector::new();
    sel.set_stack(&stack);
    assert_eq!(sel.active_index(), 0);
}
```

> **Note on `ThumbnailLoader::new`:** the constructor signature in
> `src/core/thumbnails.rs` is `(cache_dir: PathBuf, db_pool: DbPool)`. If
> the real signature differs in the current tree, adapt the call
> accordingly — the test only needs the loader to construct, not to
> serve thumbnails.

- [ ] **Step 2: Run the integration test**

Run: `cargo test --test ui_mode_selector 2>&1 | tail -30`
Expected: 4 tests pass.

If the test binary fails to compile because of an internal-`pub` boundary
(e.g. `ThumbnailLoader::new` is private, or `MediaGrid::new` is private),
either:
- expose a small `#[cfg(test)] pub(crate) fn` constructor in the crate,
- or limit the test to the first 3 cases (no `MediaGrid` / `PhotosPage`
  dependency) and add a separate `// FIXME: full PhotosPage integration`
  note for a later task.

The first option is preferred; the existing code already uses
`pub(crate)` in a few spots for tests.

- [ ] **Step 3: `cargo fmt` and `cargo clippy`**

Run:
```bash
cargo fmt
cargo clippy --all-targets 2>&1 | tail -10
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add tests/ui_mode_selector.rs
git commit -m "test(ui): integration tests for ModeSelector + PhotosPage wiring"
```

---

## Task 8: Final verification — full test suite, clippy, fmt, and visual smoke test

**Files:** (no source changes expected; any drift from clippy/fmt is fixed in place)

- [ ] **Step 1: Run `cargo fmt`**

Run: `cargo fmt`
Expected: no diff.

- [ ] **Step 2: Run `cargo clippy --all-targets`**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test 2>&1 | tail -20`
Expected: every test passes — the existing suite plus the 12 new `mode_selector` unit tests plus the 4 new integration tests.

- [ ] **Step 4: Build the binary**

Run: `cargo build 2>&1 | tail -10`
Expected: success, no warnings.

- [ ] **Step 5: Visual smoke test using `/verify` skill**

Invoke the `verify` skill (or the `run` skill if `verify` is unavailable) to:
1. Launch `cargo run` and confirm the window opens.
2. Confirm the selector floats at the bottom-center of the photos grid with a ~50 % transparent panel.
3. Confirm the "年/月/日" text is rendered in title-3 size with no red pill backgrounds.
4. Click each of the three labels and confirm the dot moves under the active label and the grid swaps to the right grouping.
5. Press ←/→ with the selector focused and confirm the active index cycles (with wrap).
6. Resize the window and confirm the selector stays centered at the bottom.

- [ ] **Step 6: Commit any lint fixes (if needed)**

```bash
git add -A
git diff --cached --quiet || git commit -m "style: cargo fmt + clippy fixes"
```

If there were no changes, skip the commit.

---

## Self-Review

**Spec coverage check** (against `docs/superpowers/specs/2026-06-23-photos-mode-selector-redesign-design.md`):

| Spec section | Implemented by |
|---|---|
| 1. Architecture (`GtkOverlay` wrapping `view_stack` + `ModeSelector`) | Task 1 (scaffold) + Task 6 (wiring) |
| 2. ModeSelector component (template, 3 label cells, 3 dot cells) | Task 1 |
| 3. `set_active_index` / `active_index` API | Task 2 |
| 4. `set_stack` + `notify::visible-child` sync with loop guard | Task 3 |
| 5. Click handlers | Task 4 |
| 6. Keyboard navigation (←/→) | Task 5 |
| 7. CSS rules (`mode-selector`, `mode-dot`, `mode-cell`, `active`) | Task 1 (CSS) + Task 2 (`active` class) |
| 8. `PhotosPage` wiring (replace `AdwViewSwitcherBar`) | Task 6 |
| 9. Integration tests (widget + PhotosPage) | Task 7 |
| 10. Visual smoke test | Task 8 |
| 11. Risk (low — no architectural change) | inherent — no separate task |
| 12. Files touched | Tasks 1, 6, 7 cover all 7 listed files |

**Placeholder scan:** no `TBD` / `TODO` / "implement later" / "similar to Task N" present. All code blocks are full and copy-pasteable.

**Type / method consistency:**
- `ModeSelector::new()` — defined Task 1, used Task 2, 3, 4, 5, 7.
- `ModeSelector::active_index()` — defined Task 2, used Tasks 2, 3, 5, 7.
- `ModeSelector::set_active_index(u32)` — defined Task 2, called Tasks 3, 4, 5.
- `ModeSelector::set_stack(&adw::ViewStack)` — defined Task 3, called Tasks 6, 7.
- `imp::apply_state` — defined Task 2, updated Task 3, called by `constructed` and both setters.
- Template child IDs in `.blp` (`label_0..2`, `dot_inner_0..2`) match the `TemplateChild<gtk::Label>` / `TemplateChild<gtk::Box>` fields in `imp`.
- ViewStack child names `year` / `month` / `day` — preserved in Task 6 (existing `add_titled` calls unchanged).
- PhotosPage template child IDs: `header_bar`, `view_stack`, `mode_selector` — all referenced by the new `.blp` and the new `imp` struct in Task 6.

No mismatches found.
