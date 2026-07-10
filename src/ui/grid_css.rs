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
//! [`install`] without coordinating. [`is_installed`] / [`assert_installed`]
//! let other code paths (e.g. the viewer's favorite button, which depends on
//! the `.viewer-favorite-btn.favorite-active` rule) verify install has run at
//! least once on this process.
//!
//! ## Liquid Glass toggle / 液态玻璃开关
//!
//! The CSS is assembled at install time from three parts: [`BASE_CSS`] (shared
//! layout/state rules), a *material* block that differs by mode, and [`A11Y_CSS`]
//! (shared accessibility fallbacks). [`build_css`] picks the material block from
//! [`LIQUID_GLASS_MATERIAL_CSS`] (the dramatic Liquid Glass look — backdrop
//! blur/saturation, bright inset highlights, luminous hairlines, and
//! dimensional floating shadows) or [`PLAIN_GLASS_MATERIAL_CSS`] (plain
//! semi-transparent surfaces — translucent fills + hairline borders, so the
//! off state looks clearly different from the liquid glass material).
//! [`install`] reads [`crate::core::prefs::liquid_glass_enabled`]; [`reapply`]
//! swaps the provider's CSS live when the user toggles the setting (no restart).

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::sync::OnceLock;

/* ── BASE_CSS ─ shared between both glass modes (layout / state rules that
do NOT define a surface material). 液态/毛玻璃两模式共用,与材质无关。 */
const BASE_CSS: &str = include_str!("../../data/css/base.css");

/* ── LIQUID_GLASS_MATERIAL_CSS ─ the dramatic Liquid Glass material:
backdrop blur+saturate+brightness, bright inset top highlights, and heavy
floating shadows. This is the default (opt-out) look. */
const LIQUID_GLASS_MATERIAL_CSS: &str = include_str!("../../data/css/liquid.css");

/* ── PLAIN_GLASS_MATERIAL_CSS ─ plain semi-transparent surfaces, NO blur.
Same selectors as the liquid block, but drops `backdrop-filter` entirely
along with the bright inset top highlights and the heavy floating drop
shadows. The result is a clearly different look from Liquid Glass: sharp
translucent panels and controls with restrained borders.
普通半透明:没有 backdrop-filter、高光、厚重投影,只留半透明背景 + 细边 +
轻阴影,与液态玻璃差异明显。 */
const PLAIN_GLASS_MATERIAL_CSS: &str = include_str!("../../data/css/plain.css");

/* GTK's CssProvider in the supported runtime rejects web-style @media
feature queries. Keep this hook empty until accessibility adaptation is
implemented through GTK settings or explicit runtime class toggles. */
const A11Y_CSS: &str = include_str!("../../data/css/a11y.css");

/// Assemble the full CSS string for the given glass mode. `true` → Liquid
/// Glass (default), `false` → calmer classic frosted glass.
fn build_css(liquid_glass: bool) -> String {
    build_css_with_transparency(liquid_glass, 0.0)
}

fn build_css_with_transparency(liquid_glass: bool, transparency: f64) -> String {
    let material = if liquid_glass {
        LIQUID_GLASS_MATERIAL_CSS
    } else {
        PLAIN_GLASS_MATERIAL_CSS
    };
    let material = scale_material_alpha(material, transparency);
    format!("{BASE_CSS}\n{material}\n{A11Y_CSS}")
}

fn scale_material_alpha(material: &str, transparency: f64) -> String {
    let transparency = if transparency.is_finite() {
        transparency.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if transparency <= f64::EPSILON {
        return material.to_string();
    }
    let material_alpha = 1.0 - transparency;

    let mut out = String::with_capacity(material.len());
    let mut in_box_shadow = false;
    for line in material.lines() {
        let trimmed = line.trim_start();
        let should_scale_background =
            trimmed.starts_with("background:") || trimmed.starts_with("background-color:");
        let should_scale_border = trimmed.starts_with("border:") || trimmed.starts_with("border-");
        let should_scale_shadow = in_box_shadow || trimmed.starts_with("box-shadow:");

        let scaled = if should_scale_background {
            scale_alpha_calls(line, material_alpha, 0.0)
        } else if should_scale_border || should_scale_shadow {
            scale_alpha_calls(line, material_alpha, 0.10)
        } else if trimmed.starts_with("backdrop-filter:") {
            scale_backdrop_filter(line, material_alpha)
        } else {
            line.to_string()
        };
        out.push_str(&scaled);
        out.push('\n');

        if trimmed.starts_with("box-shadow:") {
            in_box_shadow = true;
        }
        if in_box_shadow && trimmed.ends_with(';') {
            in_box_shadow = false;
        }
    }
    out
}

fn scale_alpha_calls(line: &str, material_alpha: f64, floor: f64) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("alpha(") {
        out.push_str(&rest[..start]);
        let alpha_start = start + "alpha(".len();
        let Some(end_rel) = rest[alpha_start..].find(')') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let end = alpha_start + end_rel;
        let call_inner = &rest[alpha_start..end];
        if let Some((color, value)) = call_inner.rsplit_once(',') {
            if let Ok(alpha) = value.trim().parse::<f64>() {
                out.push_str("alpha(");
                out.push_str(color);
                out.push_str(", ");
                out.push_str(&format_alpha(scale_alpha(alpha, material_alpha, floor)));
                out.push(')');
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push_str(&rest[start..=end]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn scale_alpha(alpha: f64, material_alpha: f64, floor: f64) -> f64 {
    if alpha <= f64::EPSILON {
        return 0.0;
    }
    let scaled = alpha * material_alpha;
    let visible_floor = alpha.min(floor);
    scaled.max(visible_floor)
}

fn scale_backdrop_filter(line: &str, material_alpha: f64) -> String {
    let Some(blur) = parse_filter_number(line, "blur(", "px)") else {
        return line.to_string();
    };
    let saturate = parse_filter_number(line, "saturate(", ")").unwrap_or(1.0);
    let brightness = parse_filter_number(line, "brightness(", ")").unwrap_or(1.0);

    let scaled_blur = blur * material_alpha;
    let scaled_saturate = 1.0 + (saturate - 1.0) * material_alpha;
    let scaled_brightness = 1.0 + (brightness - 1.0) * material_alpha;
    let indent_len = line.len() - line.trim_start().len();

    format!(
        "{}backdrop-filter: blur({}px) saturate({}) brightness({});",
        &line[..indent_len],
        format_filter_number(scaled_blur),
        format_filter_number(scaled_saturate),
        format_filter_number(scaled_brightness),
    )
}

fn parse_filter_number(line: &str, prefix: &str, suffix: &str) -> Option<f64> {
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find(suffix)?;
    rest[..end].parse::<f64>().ok()
}

fn format_filter_number(value: f64) -> String {
    if value.abs() <= f64::EPSILON {
        return "0".to_string();
    }
    let mut s = format!("{:.3}", value);
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

fn format_alpha(alpha: f64) -> String {
    let mut s = format!("{:.3}", alpha.clamp(0.0, 1.0));
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.push('0');
    }
    s
}

static CSS_INSTALLED: OnceLock<()> = OnceLock::new();

// The currently-registered display-level provider, so `reapply` can remove it
// before adding the replacement (remove+add forces a full restyle of every
// widget, including popovers and AdwAlertDialogs). `gtk::CssProvider` wraps a
// raw pointer and is NOT `Send`/`Sync`, so it cannot live in a `static`;
// `thread_local!` sidesteps that — every GTK call (page constructors, the
// settings toggle handler) runs on the single main thread.
thread_local! {
    static ACTIVE_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

/// Test-only getter for the CSS string (Liquid Glass mode). Not for production use.
#[doc(hidden)]
pub fn css_for_tests() -> String {
    build_css(true)
}

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

/// Register `css` with the default display, first removing any provider we
/// previously registered so a swap forces a global restyle. Stores the live
/// provider in [`ACTIVE_PROVIDER`] for the next swap.
fn register(css: &str) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(css);
    if let Some(display) = gtk::gdk::Display::default() {
        // Take the previous provider out of the slot (releasing the borrow)
        // before touching the display, then store the new one afterwards —
        // no nested borrows of the thread_local.
        let old = ACTIVE_PROVIDER.with(|slot| slot.borrow_mut().take());
        if let Some(old) = old {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        ACTIVE_PROVIDER.with(|slot| *slot.borrow_mut() = Some(provider));
    }
}

/// Register the thumbnail-grid + glass CSS with the default display.
/// Idempotent: subsequent calls are no-ops (the body only runs the first
/// time, guarded by [`CSS_INSTALLED`]). The first call picks the material
/// block from [`crate::core::prefs::liquid_glass_enabled`].
pub fn install() {
    // `OnceLock::set` returns `Ok(())` only on the first call; gate the
    // provider registration on that so defensive `install()` calls from
    // MediaGrid / TrashPage / AlbumDetailPage constructors do not accumulate
    // duplicate CssProviders on the default display.
    if CSS_INSTALLED.set(()).is_ok() {
        register(&build_css_with_transparency(
            crate::core::prefs::liquid_glass_enabled(),
            crate::core::prefs::liquid_glass_transparency(),
        ));
    }
}

/// Re-apply the CSS for the given glass mode, live. Called by the Settings
/// page when the user toggles the Liquid Glass switch: persists elsewhere,
/// then this swaps the provider so every glass surface (sidebar / header /
/// mode selector / popover / alert dialog / details panel) restyles
/// immediately without an app restart.
pub fn reapply(liquid_glass: bool) {
    // Defensive installs from page constructors read the pref at runtime, so
    // mark install as already-done to keep them no-ops after a live reapply.
    let _ = CSS_INSTALLED.set(());
    register(&build_css_with_transparency(
        liquid_glass,
        crate::core::prefs::liquid_glass_transparency(),
    ));
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
    // `kbd-nav` and `pointer-left`) and make the tile under the pointer
    // the keyboard-nav anchor.
    let motion = gtk::EventControllerMotion::new();
    let flow_weak = flow.downgrade();
    motion.connect_enter(move |_, x, y| {
        if let Some(f) = flow_weak.upgrade() {
            f.remove_css_class("kbd-nav");
            f.remove_css_class("pointer-left");
            focus_child_at(&f, x, y);
        }
    });
    let flow_weak = flow.downgrade();
    motion.connect_motion(move |_, x, y| {
        if let Some(f) = flow_weak.upgrade() {
            f.remove_css_class("kbd-nav");
            f.remove_css_class("pointer-left");
            focus_child_at(&f, x, y);
        }
    });

    // Pointer leave → hide the focus ring so it doesn't linger on the
    // last-hovered tile. The ring is suppressed via the `pointer-left`
    // CSS class; actual focus is retained so arrow-key nav still works
    // (kbd-nav overrides pointer-left to re-show the ring).
    let flow_weak = flow.downgrade();
    motion.connect_leave(move |_| {
        if let Some(f) = flow_weak.upgrade() {
            f.add_css_class("pointer-left");
        }
    });

    flow.add_controller(key);
    flow.add_controller(motion);
}

#[cfg(test)]
mod tests;
