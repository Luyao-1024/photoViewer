//! Highlight CSS + keyboard navigation for thumbnail FlowBoxes (MediaGrid,
//! AlbumDetailPage).
//!
//! The tile has ONE highlight style, shared by two triggers so they look
//! identical: the `:hover` pseudo-class (mouse) and `:focus` (keyboard cursor).
//! Both paint a clean accent `outline` on the `flowboxchild` node. We use
//! `:focus` rather than `:focus-visible`: GTK only flips the window into
//! `focus-visible` ("keyboard mode") once keyboard focus changes, and our
//! hover-grab (which gives the FlowBox keyboard focus so it can receive arrow
//! keys) keeps it in pointer mode — so `:focus-visible` would not match on the
//! first arrow press and the cursor ring would not appear. `:focus` matches the
//! focused child unconditionally.
//!
//! Arrow-key cursor movement is driven MANUALLY (see [`move_cursor`]) rather
//! than relying on GTK's built-in FlowBox `move_cursor`, for the same reason:
//! the built-in one only advances when the window is in keyboard mode, so right
//! after a mouse interaction the first few arrow presses would not move.
//! `selection_mode = None`; `TrashPage` deliberately does NOT install this — it
//! keeps click-driven multi-select for batch restore / delete.
//!
//! Install is idempotent (process-wide `OnceLock`), so multiple pages may call
//! `install()` without coordinating. [`is_installed`] / [`assert_installed`]
//! let other code paths (e.g. the viewer's favorite button, which depends on
//! the `.viewer-favorite-btn.favorite-active` rule) verify install has run at
//! least once on this process.

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use std::sync::OnceLock;

const GRID_CSS: &str = "
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

/* Album grid (AlbumsPage) — 与 photo grid (thumb-grid) 一致的 outline-based
   选中样式。GTK 默认会给 `:selected` 涂一整块强调色背景,在大型相册卡片
   上非常刺眼。这里清除背景填充、只保留外圈细线高亮,与图片选择时的视觉
   语言保持一致。

   注意 outline 不能直接画在 `flowboxchild` 上 —— flowboxchild 包含
   封面 + 名字 + 计数 3 个子部件,选中外圈会是长方形。所以我们把
   outline 转移到封面 widget (`.album-cover`) 上,这样 270×270 的封面
   周围就是 270×270 的正方形高亮,无论是 hover / focus / selected 都
   遵循同一规则。 */
flowbox.album-grid > flowboxchild { padding: 0; }

flowbox.album-grid > flowboxchild:hover,
flowbox.album-grid > flowboxchild:focus,
flowbox.album-grid > flowboxchild:selected,
flowbox.album-grid > flowboxchild:selected:focus {
  outline: none;
  background: transparent;
}

/* 封面轮廓(被各种状态触发)。GTK4 CSS 支持后代选择器,`.album-cover`
   写在被 hover / focus / selected 的 flowboxchild 下面时会同时命中。 */
flowbox.album-grid > flowboxchild:hover .album-cover,
flowbox.album-grid > flowboxchild:focus .album-cover {
  outline: 2px solid @accent_color;
  outline-offset: -2px;
}
flowbox.album-grid > flowboxchild:selected .album-cover,
flowbox.album-grid > flowboxchild:selected:focus .album-cover {
  outline: 3px solid @accent_color;
  outline-offset: -3px;
}

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

/* 单个 label / dot 槽位 */
box.mode-cell {
  min-width: 60px;
  padding: 4px 12px;
}

/* 标签：默认半透明、title-3 字号 */
box.mode-selector label {
  font-size: 14pt;
  font-weight: 700;
  color: #ffffff;
  opacity: 0.55;
  transition: opacity 120ms ease;
}

box.mode-selector.on-light-background label {
  color: #000000;
}

/* 激活态：标签全亮 */
box.mode-selector label.active {
  opacity: 1.0;
}

/* 激活指示点 */
box.mode-dot {
  background: #ffffff;
  border-radius: 2px;
  min-width: 24px;
  min-height: 4px;
  margin-top: 2px;
}

box.mode-selector.on-light-background box.mode-dot {
  background: #000000;
}

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

/* Viewer favorite button active state. Class is added/removed by
   ViewerPage::refresh_favorite_button; the visual now lives in the
   global provider so it composes with .glass-toolbar-button. */
.viewer-favorite-btn.favorite-active {
  color: #f6c344;
  background: alpha(#f6c344, 0.14);
  border-color: alpha(#f6c344, 0.38);
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
";

static CSS_INSTALLED: OnceLock<()> = OnceLock::new();

/// Has [`install`] been called at least once on this process?
/// Reads are non-blocking and safe to call from any thread.
pub fn is_installed() -> bool {
    CSS_INSTALLED.get().is_some()
}

/// `debug_assert!(is_installed())` with a descriptive message and caller
/// location. No-op in release builds.
#[track_caller]
pub fn assert_installed() {
    debug_assert!(
        is_installed(),
        "grid_css::install() must be called before this point — see src/ui/grid_css.rs"
    );
}

/// Register the thumbnail-grid highlight CSS with the default display.
/// Idempotent: subsequent calls are no-ops (the body only runs the first
/// time, guarded by [`CSS_INSTALLED`]). The first call flips
/// [`is_installed`] to `true`.
pub fn install() {
    // `OnceLock::set` returns `Ok(())` only on the first call; gate the
    // provider registration on that so defensive `install()` calls from
    // MediaGrid / TrashPage / AlbumsPage constructors do not accumulate
    // duplicate CssProviders on the default display.
    if CSS_INSTALLED.set(()).is_ok() {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(GRID_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }
}

/// Move the keyboard cursor inside `flow` in the direction of `key`, focusing
/// the neighbour tile.
///
/// We compute the neighbour ourselves from the children's allocations rather
/// than relying on GTK's FlowBox `move_cursor` binding: that binding only
/// advances when the window is in keyboard mode (`focus-visible`), which is not
/// the case right after our hover-grab, so the first arrow press would be a
/// no-op. Moving focus directly works regardless of mode.
///
/// Returns `Stop` when the key was consumed (cursor moved or clamped at an
/// edge), `Proceed` otherwise (non-arrow keys, or no focused child to move
/// from).
fn move_cursor(flow: &gtk::FlowBox, key: gdk::Key) -> glib::Propagation {
    use gdk::Key;

    // Collect all children (FlowBox has no n_children API; iterate until None).
    let mut children: Vec<gtk::FlowBoxChild> = Vec::new();
    let mut i = 0;
    while let Some(c) = flow.child_at_index(i) {
        children.push(c);
        i += 1;
    }
    if children.is_empty() {
        return glib::Propagation::Proceed;
    }

    // Cursor = the currently focused child. grab_focus_on_hover keeps the
    // hovered child focused, so this is also the tile under the pointer when
    // the user starts arrow-keying.
    let focused_pos = children.iter().position(|c| c.is_focus());
    let Some(fpos) = focused_pos else {
        // Nothing focused yet — anchor on the first child so subsequent arrows
        // work. (grab_focus_on_hover normally prevents reaching here.)
        let _ = children[0].grab_focus();
        return glib::Propagation::Stop;
    };

    let foc_alloc = children[fpos].allocation();
    let foc_cx = foc_alloc.x() + foc_alloc.width() / 2;

    let target_pos: Option<usize> = match key {
        Key::Left | Key::KP_Left => fpos.checked_sub(1),
        Key::Right | Key::KP_Right => {
            if fpos + 1 < children.len() {
                Some(fpos + 1)
            } else {
                None
            }
        }
        Key::Up | Key::Down | Key::KP_Up | Key::KP_Down => {
            // Rows are identified by shared allocation.y (children flow
            // left-to-right, wrapping). Build the sorted-unique list of row
            // ys, find the current/target row, then pick the child in the
            // target row whose centre x is closest to the focused tile's.
            let mut rows: Vec<i32> = children.iter().map(|c| c.allocation().y()).collect();
            rows.sort_unstable();
            rows.dedup();
            let Some(cur_row) = rows.iter().position(|&y| y == foc_alloc.y()) else {
                return glib::Propagation::Proceed;
            };
            let target_row_idx = match key {
                Key::Up | Key::KP_Up => cur_row.checked_sub(1),
                Key::Down | Key::KP_Down => {
                    if cur_row + 1 < rows.len() {
                        Some(cur_row + 1)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let Some(tri) = target_row_idx else {
                return glib::Propagation::Stop; // already on the top/bottom row
            };
            let target_y = rows[tri];
            children
                .iter()
                .enumerate()
                .filter(|(_, c)| c.allocation().y() == target_y)
                .min_by_key(|(_, c)| {
                    let a = c.allocation();
                    (a.x() + a.width() / 2 - foc_cx).abs()
                })
                .map(|(pos, _)| pos)
        }
        _ => return glib::Propagation::Proceed,
    };

    if let Some(tpos) = target_pos {
        let _ = children[tpos].grab_focus();
    }
    glib::Propagation::Stop
}

/// Focus the `flowboxchild` under `(x, y)` (coords relative to `flow`), so the
/// hovered tile becomes the keyboard-nav anchor. Hit-tests child allocations
/// rather than attaching a motion controller per child — important because
/// MediaGrid builds one tile per photo and the library targets 10k–100k photos.
fn focus_child_at(flow: &gtk::FlowBox, x: f64, y: f64) {
    let (xi, yi) = (x as i32, y as i32);
    let mut i = 0;
    while let Some(c) = flow.child_at_index(i) {
        let a = c.allocation();
        if xi >= a.x() && xi < a.x() + a.width() && yi >= a.y() && yi < a.y() + a.height() {
            let _ = c.grab_focus();
            return;
        }
        i += 1;
    }
}

/// Attach keyboard + motion controllers to `flow`.
///
/// - Arrow keys move the keyboard cursor (see [`move_cursor`]) and add a
///   `kbd-nav` CSS class so the `:hover` hint on the resting pointer is
///   neutralised — the highlight follows the keyboard cursor instead.
/// - Pointer enter/motion clears `kbd-nav` (handing the highlight back to the
///   mouse) AND focuses the tile under the pointer so the next arrow press
///   starts from it.
///
/// Call this once per FlowBox right after adding the `thumb-grid` class.
pub fn attach_kbd_nav(flow: &gtk::FlowBox) {
    let key = gtk::EventControllerKey::new();
    let flow_weak = flow.downgrade();
    key.connect_key_pressed(move |_, key, _, _| {
        let is_arrow = matches!(
            key,
            gdk::Key::Up
                | gdk::Key::Down
                | gdk::Key::Left
                | gdk::Key::Right
                | gdk::Key::KP_Up
                | gdk::Key::KP_Down
                | gdk::Key::KP_Left
                | gdk::Key::KP_Right
        );
        if !is_arrow {
            return glib::Propagation::Proceed;
        }
        let Some(f) = flow_weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        f.add_css_class("kbd-nav");
        move_cursor(&f, key)
    });

    // Pointer enter/motion → hand the highlight back to the mouse (clear
    // `kbd-nav`) and make the tile under the pointer the keyboard-nav anchor.
    let motion = gtk::EventControllerMotion::new();
    let flow_weak = flow.downgrade();
    motion.connect_enter(move |_, x, y| {
        if let Some(f) = flow_weak.upgrade() {
            f.remove_css_class("kbd-nav");
            focus_child_at(&f, x, y);
        }
    });
    let flow_weak = flow.downgrade();
    motion.connect_motion(move |_, x, y| {
        if let Some(f) = flow_weak.upgrade() {
            f.remove_css_class("kbd-nav");
            focus_child_at(&f, x, y);
        }
    });

    flow.add_controller(key);
    flow.add_controller(motion);
}
