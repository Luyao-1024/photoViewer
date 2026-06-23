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
//! Install is idempotent (process-wide `Once`), so multiple pages may call
//! `install()` without coordinating.

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

const GRID_CSS: &str = "
flowbox.thumb-grid > flowboxchild { padding: 0; }
flowbox.thumb-grid > flowboxchild:hover,
flowbox.thumb-grid > flowboxchild:focus,
flowbox.thumb-grid > flowboxchild:selected,
flowbox.thumb-grid > flowboxchild:selected:focus {
  background: transparent;
  outline: none;
}

/* One highlight style, shared by two triggers so they look identical:
   `:hover` (mouse) and `:focus` (keyboard cursor). `:focus` is used instead of
   `:focus-visible` because the window stays in pointer mode after our
   hover-grab; `:focus-visible` would not match on the first arrow press. */
flowbox.thumb-grid > flowboxchild:hover .thumb-tile,
flowbox.thumb-grid > flowboxchild:focus .thumb-tile {
  outline: 2px solid @accent_color;
  outline-offset: -2px;
}

/* While keyboard-navigating, neutralise `:hover` so the highlight follows the
   keyboard cursor, not the resting pointer — but ONLY for tiles that are not
   also keyboard-focused. The `:not(:focus)` guard is essential: without it this
   rule's specificity beats `flowboxchild:focus` above, so when the cursor moves
   onto the tile the pointer happens to rest on (matching both `:hover` and
   `:focus`), the outline would vanish. `.kbd-nav` is added on arrow-key press
   and removed on the next pointer motion (see attach_kbd_nav). */
flowbox.thumb-grid.kbd-nav > flowboxchild:hover:not(:focus) {
  outline: none;
}
flowbox.thumb-grid.kbd-nav > flowboxchild:hover:not(:focus) .thumb-tile {
  outline: none;
}

/* Multi-select (Shift/Ctrl click) — uses the GTK `:selected` pseudo on
   flowboxchild. Painted with a thicker, slightly translucent accent ring so
   the user can tell at a glance which tiles are part of the active batch
   operation. The keyboard focus ring (`:focus`) wins specificity when both
   apply so the cursor stays visible. */
flowbox.thumb-grid > flowboxchild:selected .thumb-tile {
  outline: 3px solid alpha(@accent_color, 0.85);
  outline-offset: -3px;
}
flowbox.thumb-grid > flowboxchild:selected:focus .thumb-tile {
  outline: 3px solid @accent_color;
  outline-offset: -3px;
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

/* ModeSelector 容器：降低透明度的圆角面板 */
box.mode-selector {
  background: alpha(@card_bg_color, 0.75);
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

/* 右键菜单（媒体网格）样式 */
popover.media-grid-context-menu {
  padding: 0px;
  border-radius: 10px;
  background: alpha(@card_bg_color, 0.98);
  min-width: 160px;
}

box.media-grid-context-menu-list {
  padding: 4px;
}

button.media-grid-context-item {
  min-height: 30px;
  padding: 6px 10px;
  border-radius: 8px;
  font-weight: 500;
}

button.media-grid-context-item + button.media-grid-context-item {
  margin-top: 2px;
}

button.media-grid-context-item:hover {
  background: alpha(@accent_color, 0.16);
}
";

static CSS_INSTALLED: std::sync::Once = std::sync::Once::new();

/// Register the thumbnail-grid highlight CSS with the default display.
/// Idempotent: subsequent calls are no-ops.
pub fn install() {
    CSS_INSTALLED.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(GRID_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
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
