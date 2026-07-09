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
mod tests {
    use super::*;

    fn css_block(css: &str, selector: &str) -> Option<String> {
        let pattern = format!("{selector} {{");
        let start = css.find(&pattern)?;
        let open = css[start..].find('{')? + start;
        let close = css[open..].find('}')? + open;
        Some(css[start..=close].to_string())
    }

    /// `.viewer-favorite-btn.favorite-active:hover` must exist alongside the
    /// base `.viewer-favorite-btn.favorite-active` rule. Without the :hover
    /// override, the bare-at-rest/hover viewer-chrome rules would win and the
    /// favorited heart would not brighten on pointer-over.
    /// 没有 hover 规则时,鼠标悬停在已收藏的爱心上不会提亮红色。
    #[test]
    fn favorite_active_has_hover_override() {
        let css = build_css(true);
        assert!(
            css.contains(".viewer-favorite-btn.favorite-active"),
            "CSS must define the base .viewer-favorite-btn.favorite-active rule",
        );
        assert!(
            css.contains(".viewer-favorite-btn.favorite-active:hover"),
            "CSS must define a :hover override so the red heart brightens on pointer-over",
        );
    }

    #[test]
    fn viewer_media_surface_uses_theme_adaptive_background() {
        let css = build_css(true);
        let block = css_block(&css, ".viewer-media-surface")
            .expect("viewer media surface should have a dedicated CSS block");
        assert!(
            block.contains("@window_bg_color"),
            "viewer media surface should use theme background variables, got {block}",
        );
        assert!(
            block.contains("@window_fg_color"),
            "viewer media surface should use theme foreground variables for its edge, got {block}",
        );
        assert!(
            !block.contains("black"),
            "viewer media surface must not use a hardcoded black background, got {block}",
        );
    }

    #[test]
    fn viewer_video_child_picture_uses_plain_theme_background() {
        let css = build_css(true);
        let block = css_block(&css, "video.viewer-media-surface picture")
            .expect("GtkVideo's internal picture should have a dedicated CSS block");
        assert!(
            block.contains("background: @window_bg_color"),
            "GtkVideo child picture should use the plain theme background for playback letterboxing, got {block}",
        );
        assert!(
            !block.contains("@window_fg_color"),
            "GtkVideo child picture should not use the stage wash for playback letterboxing, got {block}",
        );
        assert!(
            !block.contains("alpha(") && !block.contains("radial-gradient"),
            "GtkVideo child picture must not use translucent/radial material for playback letterboxing, got {block}",
        );
    }

    #[test]
    fn viewer_video_error_background_has_dedicated_visual_style() {
        let css = build_css(true);
        let block = css_block(&css, ".viewer-video-error")
            .expect("video error background should have a dedicated CSS block");
        assert!(
            block.contains("radial-gradient") && block.contains("@accent_bg_color"),
            "video error background should use a distinct themed visual treatment, got {block}",
        );
        assert!(
            css.contains(".viewer-video-error-icon")
                && css.contains(".viewer-video-error-title")
                && css.contains(".viewer-video-error-subtitle"),
            "video error icon and text should have dedicated CSS rules"
        );
    }

    #[test]
    fn viewer_video_controls_use_light_glass_progress_style() {
        let css = build_css(true);
        let controls = css_block(&css, "video.viewer-media-surface controls")
            .expect("GtkVideo media controls should have a viewer-specific block");
        assert!(
            controls.contains("alpha(@window_bg_color"),
            "media controls should use a light theme-aware glass background, got {controls}",
        );
        assert!(
            controls.contains("border-top: 1px solid alpha(@window_fg_color"),
            "media controls should keep a subtle top hairline over the video, got {controls}",
        );

        let trough = css_block(&css, "video.viewer-media-surface controls scale trough")
            .expect("GtkVideo progress trough should have a viewer-specific block");
        assert!(
            trough.contains("min-height: 4px") && trough.contains("border-radius: 999px"),
            "progress trough should be a thin rounded rail, got {trough}",
        );
        assert!(
            trough.contains("alpha(@window_fg_color"),
            "progress trough should use a neutral theme-aware rail, got {trough}",
        );

        let highlight = css_block(
            &css,
            "video.viewer-media-surface controls scale trough highlight",
        )
        .expect("GtkVideo progress highlight should have a viewer-specific block");
        assert!(
            highlight.contains("@accent_bg_color"),
            "played progress should use the app accent color, got {highlight}",
        );

        let slider = css_block(&css, "video.viewer-media-surface controls scale slider")
            .expect("GtkVideo progress slider should have a viewer-specific block");
        assert!(
            slider.contains("min-width: 14px") && slider.contains("min-height: 14px"),
            "progress slider should be a compact circular thumb, got {slider}",
        );
        assert!(
            slider.contains("@window_bg_color") && slider.contains("box-shadow:"),
            "progress slider should be light with a small shadow, got {slider}",
        );
    }

    /// Viewer chrome (header buttons + prev/next overlay nav) floats over a
    /// full-bleed photo, so it must be bare at rest and only gain its glass
    /// material on hover/focus — in BOTH glass modes. The reset is scoped to
    /// .viewer-chrome / .viewer-overlay-nav-btn so the shared
    /// .glass-toolbar-button rule used by every other header (photos, trash,
    /// albums, editor) stays always-on. A favorited photo signals state
    /// via a translucent red heart icon, not a button capsule.
    #[test]
    fn viewer_chrome_is_glass_only_on_hover() {
        for liquid in [true, false] {
            let css = build_css(liquid);

            // At rest: viewer chrome + overlay nav are bare (transparent),
            // scoped to the viewer so other headers keep always-on glass.
            assert!(
                css.contains(".viewer-chrome .glass-toolbar-button,\n.viewer-overlay-nav-btn {"),
                "viewer chrome buttons must be reset to bare at rest ({liquid} mode)"
            );
            assert!(
                css.contains(".viewer-overlay-nav-btn {\n  background: transparent"),
                "overlay nav arrows must be bare at rest ({liquid} mode)"
            );

            // The prev/next capsule container is gone — bare background in
            // both modes, so only the buttons light up on hover.
            assert!(
                css.contains(".viewer-overlay-nav {\n  background: transparent"),
                "viewer overlay nav capsule must be bare/transparent ({liquid} mode)"
            );

            // On hover / keyboard focus the glass material returns.
            assert!(
                css.contains(".viewer-chrome .glass-toolbar-button:hover"),
                "viewer chrome buttons must regain material on hover ({liquid} mode)"
            );
            assert!(
                css.contains(".viewer-chrome .glass-toolbar-button:focus-visible"),
                "keyboard focus must also reveal viewer buttons ({liquid} mode)"
            );
            assert!(
                css.contains(".viewer-overlay-nav-btn:hover"),
                "overlay nav arrows must regain material on hover ({liquid} mode)"
            );

            // The shared (non-viewer) toolbar button material stays always-on.
            assert!(
                css.contains(".glass-toolbar-button,\n.glass-header windowcontrols button image {"),
                "shared toolbar button base rule must remain always-on ({liquid} mode)"
            );

            // Favorite state = translucent red heart icon, no gold capsule.
            assert!(
                css.contains(".viewer-favorite-btn.favorite-active {"),
                "favorited heart must have a color rule ({liquid} mode)"
            );
            assert!(
                css.contains("alpha(#ff5e51, 0.92)"),
                "favorited heart must be translucent red ({liquid} mode)"
            );
            assert!(
                !css.contains("alpha(#f6c344"),
                "favorite must no longer use the gold capsule color ({liquid} mode)"
            );
        }
    }

    /// The sidebar settings button mirrors the viewer-chrome hover-only
    /// treatment: bare icon at rest, glass capsule on hover/focus, in BOTH
    /// glass modes. Scoped to .sidebar-settings-button so the shared always-on
    /// .glass-toolbar-button rule (photos, trash, albums, editor headers) is
    /// unaffected.
    #[test]
    fn sidebar_settings_button_is_glass_only_on_hover() {
        for liquid in [true, false] {
            let css = build_css(liquid);

            // At rest: the settings button is bare (transparent), scoped to
            // .sidebar-settings-button so other glass-toolbar-button instances
            // keep their always-on material.
            assert!(
                css.contains(".sidebar-settings-button {\n  background: transparent"),
                "sidebar settings button must be bare at rest ({liquid} mode)"
            );

            // On hover / keyboard focus the glass material returns.
            assert!(
                css.contains(
                    ".sidebar-settings-button:hover,\n.sidebar-settings-button:focus-visible {"
                ),
                "sidebar settings button must regain material on hover/focus ({liquid} mode)"
            );
        }
    }

    /// The search button mirrors the viewer-chrome and sidebar-settings
    /// hover-only treatment: bare icon at rest, glass capsule on hover/focus,
    /// in BOTH glass modes. Scoped to .round-search-button so the shared
    /// always-on .glass-toolbar-button rule is unaffected.
    #[test]
    fn search_button_is_glass_only_on_hover() {
        for liquid in [true, false] {
            let css = build_css(liquid);

            assert!(
                css.contains(".round-search-button {\n  background: transparent"),
                "search button must be bare at rest ({liquid} mode)"
            );

            assert!(
                css.contains(".round-search-button:hover,\n.round-search-button:focus-visible {"),
                "search button must regain material on hover/focus ({liquid} mode)"
            );
        }
    }

    /// Hover/active/checked/selected state changes on glass chrome — toolbar
    /// buttons, menu items, context-menu items, sidebar rows, and the hover-only
    /// viewer/footer chrome — must ease via a shared CSS transition instead of
    /// snapping. The transition is material-independent (defined once in
    /// BASE_CSS) and property-only, so it never changes allocation.
    #[test]
    fn glass_chrome_eases_on_state_change() {
        for liquid in [true, false] {
            let css = build_css(liquid);

            assert!(
                css.contains(
                    ".glass-toolbar-button,\n.glass-menu-item,\n.glass-context-menu-item,\n.glass-sidebar-row,\n.sidebar-settings-button,\n.viewer-overlay-nav-btn"
                ),
                "glass chrome classes must share one transition selector ({liquid} mode)"
            );
            assert!(
                css.contains(
                    "transition: background 120ms ease, border-color 120ms ease, box-shadow 120ms ease, color 120ms ease"
                ),
                "glass chrome must ease background/border/shadow/color on state change ({liquid} mode)"
            );
        }
    }

    /// Each thumbnail carries a translucent-white checkmark pinned to its
    /// bottom-right; it is invisible at rest and revealed only when the
    /// wrapping FlowBoxChild is selected. This is the primary selected-state
    /// affordance.
    #[test]
    fn thumb_checkmark_shows_only_on_selected() {
        let css = build_css(true);

        // Default: hidden (opacity 0), translucent white, with an icon shadow
        // for legibility over bright thumbnails.
        assert!(
            css.contains(".thumb-checkmark {\n  color: alpha(white, 0.92);\n  opacity: 0;"),
            "thumb checkmark must be translucent white and hidden at rest"
        );

        // Revealed only on the selected flowbox child.
        assert!(
            css.contains(
                "flowbox.thumb-grid > flowboxchild:selected .thumb-checkmark {\n  opacity: 1;"
            ),
            "thumb checkmark must be revealed (opacity 1) on flowboxchild:selected"
        );
    }

    /// A loading thumbnail — a `.glass-thumb-card` carrying `.thumb-loading`
    /// but NOT the startup `.thumb-placeholder` skeleton — is held at opacity 0
    /// and fades in via the `.glass-thumb-card` opacity transition when
    /// `SquareTile::set_paintable` removes `.thumb-loading`. Startup skeleton
    /// placeholders (which carry both classes) must stay visible. This mirrors
    /// the `.thumb-checkmark` CSS-driven opacity pattern, so the fade does NOT
    /// rely on `widget.set_opacity` (which would bypass the CSS transition).
    #[test]
    fn thumbnail_fades_in_when_loaded() {
        let css = build_css(true);

        // The card owns the opacity transition that drives the fade.
        assert!(
            css.contains("transition: opacity 200ms ease;"),
            "glass-thumb-card must define an opacity transition for thumbnail fade-in"
        );

        // Loading tiles (excluding the startup placeholders) are hidden until
        // their texture arrives; the :not(.thumb-placeholder) scope is what
        // keeps the startup skeleton visible.
        assert!(
            css.contains(".glass-thumb-card.thumb-loading:not(.thumb-placeholder)"),
            "loading thumb cards must exclude .thumb-placeholder so the startup skeleton stays visible"
        );
        assert!(
            !css.contains(".glass-thumb-card.thumb-loading {\n  opacity: 0;"),
            "the opacity:0 loading rule must NOT apply to unscoped .thumb-loading (would hide the startup skeleton)"
        );
    }

    /// The right-click context menu eases in (opacity + scale) instead of
    /// appearing at full opacity. glass_context_menu::show tags the panel with
    /// .glass-context-menu-entering and drops it on the next idle.
    #[test]
    fn context_menu_eases_in_on_show() {
        let css = build_css(true);
        assert!(
            css.contains(".glass-context-menu {\n  padding: 8px 12px;"),
            "glass-context-menu base rule must exist"
        );
        assert!(
            css.contains("transition: opacity 140ms ease, transform 140ms cubic-bezier"),
            "context menu must ease opacity + transform on entrance"
        );
        assert!(
            css.contains(
                ".glass-context-menu-entering {\n  opacity: 0;\n  transform: scale(0.96);"
            ),
            "context menu entering state must start hidden + scaled down"
        );
    }

    /// The settings dialog backdrop (a class toggled on the persistent nav_view)
    /// fades opacity in/out rather than snapping. Works because the class is
    /// toggled on an already-mounted widget.
    #[test]
    fn settings_backdrop_fades_opacity() {
        for liquid in [true, false] {
            let css = build_css(liquid);
            let block = css_block(&css, ".settings-background-blur")
                .expect("settings-background-blur rule must exist");
            assert!(
                block.contains("transition: opacity 200ms ease"),
                "settings backdrop must fade opacity in both material modes ({liquid} mode), got {block}"
            );
        }
    }

    /// The year/month/day indicator is a single bar that slides between the
    /// three labels and stops exactly on the active one — a pure decelerate
    /// curve with no overshoot. Its translateX is written by a runtime
    /// CssProvider; the transition lives on the box.mode-dot rule.
    #[test]
    fn mode_selector_indicator_slides_to_target_without_overshoot() {
        let css = build_css(true);
        assert!(
            css.contains("transition: transform 300ms cubic-bezier(0.2, 0.0, 0.2, 1)"),
            "the indicator must decelerate onto the target with no overshoot"
        );
        // Guard against the overshoot back-out curve sneaking back in: a y
        // value > 1 would make the bar bounce past the target.
        assert!(
            !css.contains("cubic-bezier(0.34, 1.56, 0.64, 1)"),
            "the indicator must NOT use the overshoot back-out curve"
        );
        assert!(
            !css.contains("box.mode-dot.active"),
            "no per-dot active opacity rule — the indicator is a single sliding bar now"
        );
    }

    #[test]
    fn mode_selector_indicator_is_not_hidden_while_positioning() {
        let css = build_css(true);
        assert!(
            !css.contains("mode-dot-position-pending"),
            "the indicator should be positioned before first paint instead of hidden during startup"
        );
    }

    /// The fullscreen preview picture fades in on present (opacity 0 → 1 via a
    /// .fade-shown class added on the next idle). `transform` for EXIF
    /// rotation lives on a runtime CssProvider on the same selector; opacity is
    /// a separate property and must not conflict.
    #[test]
    fn fullscreen_preview_picture_fades_in() {
        let css = build_css(true);
        assert!(
            css.contains("picture.viewer-fullscreen-preview-picture {\n  opacity: 0;"),
            "fullscreen preview picture must start at opacity 0 for the entrance fade"
        );
        assert!(
            css.contains("picture.viewer-fullscreen-preview-picture.fade-shown {\n  opacity: 1;"),
            "fullscreen preview picture must reach opacity 1 via .fade-shown"
        );
    }

    /// The viewer loading spinner fades via CSS opacity (kept visible: true in
    /// its overlay slot) instead of show/hide. set_spinner_visible toggles
    /// .viewer-spinner-hidden.
    #[test]
    fn viewer_spinner_fades_via_opacity() {
        let css = build_css(true);
        assert!(
            css.contains(".viewer-spinner {\n  transition: opacity 180ms ease;"),
            "viewer spinner must define an opacity transition"
        );
        assert!(
            css.contains(".viewer-spinner.viewer-spinner-hidden {\n  opacity: 0;"),
            "viewer spinner hidden state must drop opacity to 0"
        );
    }

    #[test]
    fn current_viewer_thumbnail_is_prominently_enlarged() {
        let css = build_css(true);
        assert!(
            css.contains(".viewer-thumb-carousel"),
            "viewer filmstrip should expose a carousel surface class",
        );
        assert!(
            css.contains("transition: transform 220ms cubic-bezier(0.2, 0.0, 0.2, 1)"),
            "viewer filmstrip should animate carousel position changes",
        );
        assert!(
            css.contains(".viewer-thumb-strip {\n  padding: 0 24px;"),
            "viewer filmstrip should keep internal edge inset so thumbnails do not touch rounded carousel edges",
        );
        assert!(
            css.contains("inset 34px 0 24px -30px alpha(@window_bg_color, 0.72)")
                && css.contains("inset -34px 0 24px -30px alpha(@window_bg_color, 0.72)"),
            "carousel surface should draw subtle edge fades without extra layout widgets",
        );
        assert!(
            css.contains(".viewer-thumb-item.viewer-thumb-current"),
            "CSS must define the current filmstrip thumbnail state",
        );
        assert!(
            css.contains("transform: translateY(-3px) scale(1.24)"),
            "current filmstrip thumbnail should be prominently enlarged",
        );
        assert!(
            css.contains("opacity: 0.54"),
            "non-current filmstrip thumbnails should be visually de-emphasized",
        );
        assert!(
            css.contains("opacity: 1.0"),
            "current filmstrip thumbnail should stay fully bright",
        );
        assert!(
            !css.contains("margin 180ms ease"),
            "viewer thumbnail transitions must not animate layout-affecting margins",
        );
        assert!(
            !css.contains("margin-left: 10px") && !css.contains("margin-right: 10px"),
            "current filmstrip thumbnail must not change button allocation with margins",
        );
        assert!(
            !css.contains("button.viewer-thumb-item.viewer-thumb-current {\n  margin-left")
                && !css.contains("button.viewer-thumb-item.viewer-thumb-current {\n  padding: 4px"),
            "current filmstrip thumbnail must emphasize visually without changing padding",
        );
        assert!(
            css.contains("background: alpha(@window_fg_color, 0.12)"),
            "current filmstrip thumbnail should use the shared glass selection veil",
        );
        assert!(
            css.contains("button.viewer-thumb-item"),
            "viewer filmstrip thumbnails are GtkButtons and need a button-node reset",
        );
        assert!(
            css.contains("border: 0"),
            "non-current viewer thumbnails should not draw a button border",
        );
        assert!(
            css.contains("button.viewer-thumb-item:hover"),
            "hover state should also suppress the default GTK button frame",
        );
        assert!(
            css.contains("border: 2px solid alpha(@window_fg_color, 0.48)"),
            "current filmstrip thumbnail image should use the same glass ring as grid selection",
        );
        assert!(
            css.contains("outline: 2px solid alpha(@window_fg_color, 0.55)"),
            "current filmstrip thumbnail should draw an outer glass emphasis ring",
        );
        assert!(
            !css.contains("#78b8ff"),
            "viewer filmstrip should not use a separate blue accent family",
        );
        assert!(
            css.contains("button.viewer-thumb-item.viewer-thumb-current picture"),
            "current filmstrip thumbnail emphasis should be painted on the image node, not the reset button node",
        );
    }

    #[test]
    fn library_stats_text_is_larger_than_auxiliary_tile_text() {
        let css = build_css(true);
        assert!(
            css.contains(
                ".library-stats {\n  color: alpha(@window_fg_color, 0.68);\n  font-size: 12pt;"
            ),
            "library stats should stay plain text but read larger than thumbnail badges"
        );
    }

    /// Liquid Glass mode keeps the dramatic GTK-supported material signatures:
    /// bright raised top highlights and the heavy floating shadow. Also
    /// confirms the shared BASE rules are present.
    #[test]
    fn liquid_mode_keeps_drama_and_shared_parts() {
        let css = build_css(true);
        // material selectors exist
        for sel in [
            ".glass-base",
            ".glass-raised",
            ".glass-header",
            ".glass-menu > contents",
            ".glass-alert-dialog .background",
            ".settings-dialog-backdrop",
            ".settings-background-blur",
            ".viewer-details-panel",
            ".viewer-floating-panel",
        ] {
            assert!(
                css.contains(sel),
                "liquid mode missing material selector {sel}"
            );
        }
        // liquid drama
        assert!(
            css.contains("0 18px 48px"),
            "liquid mode must keep the heavy raised drop shadow"
        );
        assert!(
            css.contains("inset 0 1px alpha(@window_fg_color,0.58)"),
            "liquid mode must keep the bright raised top highlight"
        );
        // shared BASE
        assert!(
            css.contains("flowbox.thumb-grid"),
            "BASE shared rules present"
        );
    }

    /// Plain mode is semi-transparent with NO blur: same selectors covered, but
    /// there is no web `backdrop-filter` at all and none of the liquid drama
    /// (raised top highlight, heavy floating shadow). Only translucent fills +
    /// hairline borders remain.
    #[test]
    fn plain_mode_is_translucent_no_blur() {
        let css = build_css(false);
        // same material selectors covered (split is complete)
        for sel in [
            ".glass-base",
            ".glass-raised",
            ".glass-header",
            ".glass-menu > contents",
            ".glass-alert-dialog .background",
            ".settings-dialog-backdrop",
            ".settings-background-blur",
            ".viewer-details-panel",
            ".viewer-floating-panel",
        ] {
            assert!(
                css.contains(sel),
                "plain mode missing material selector {sel}"
            );
        }
        // NO unsupported web blur property.
        assert!(
            !css.contains("backdrop-filter:"),
            "plain mode must avoid unsupported backdrop-filter CSS"
        );
        // drops liquid drama
        assert!(
            !css.contains("inset 0 1px alpha(@window_fg_color,0.58)"),
            "plain mode must drop the raised top highlight"
        );
        assert!(
            !css.contains("0 18px 48px"),
            "plain mode must drop the heavy raised drop shadow"
        );
        // shared BASE still present
        assert!(
            css.contains("flowbox.thumb-grid"),
            "BASE shared rules present"
        );
    }

    /// The Liquid Glass setting must affect the full button language, not only
    /// panels. Toolbar buttons, popover menu rows, sidebar rows, the viewer
    /// overlay nav, and active favorite state should all carry the same liquid
    /// signatures: bright inset highlight + dimensional glass shadow.
    #[test]
    fn liquid_mode_gives_buttons_unified_liquid_material() {
        let css = build_css(true);

        for selector in [
            ".glass-toolbar-button",
            ".glass-header windowcontrols button image",
            ".glass-header windowcontrols button.close:hover image",
            ".glass-menu-item",
            ".glass-sidebar-row:selected",
            ".viewer-overlay-nav-btn",
            ".viewer-favorite-btn.favorite-active",
        ] {
            assert!(
                css.contains(selector),
                "liquid mode missing button material selector {selector}",
            );
        }

        for liquid_signature in [
            "inset 0 1px alpha(@window_fg_color,0.44)",
            "inset 0 1px alpha(@window_fg_color,0.36)",
            "0 12px 32px alpha(black, 0.24)",
        ] {
            assert!(
                css.contains(liquid_signature),
                "liquid mode missing shared button material signature {liquid_signature}",
            );
        }
    }

    #[test]
    fn glass_transparency_scales_material_background_without_touching_base_css() {
        let css = build_css_with_transparency(true, 0.5);

        assert!(
            css.contains(".glass-toolbar-button,\n.glass-header windowcontrols button image {\n  background: alpha(@window_bg_color, 0.24);"),
            "toolbar button background alpha should be halved at 50% transparency"
        );
        assert!(
            css.contains("0 12px 32px alpha(black, 0.12)"),
            "button shadow alpha should scale above its visibility floor"
        );
        assert!(
            css.contains(".glass-menu > contents"),
            "material surfaces should still be present"
        );
        assert!(
            css.contains(".thumb-checkmark {\n  color: alpha(white, 0.92);"),
            "base selection affordances should not be scaled with glass transparency"
        );
    }

    #[test]
    fn glass_transparency_hundred_keeps_interactive_edges_visible() {
        let opaque = build_css_with_transparency(true, 0.0);
        assert!(
            opaque.contains(".glass-toolbar-button,\n.glass-header windowcontrols button image {\n  background: alpha(@window_bg_color, 0.48);"),
            "0% transparency should keep the original fully opaque material"
        );
        assert!(
            opaque.contains("backdrop-filter: blur(22px)"),
            "0% transparency should keep liquid blur"
        );

        let transparent = build_css_with_transparency(true, 1.0);
        assert!(
            transparent.contains(".glass-toolbar-button,\n.glass-header windowcontrols button image {\n  background: alpha(@window_bg_color, 0.0);"),
            "100% transparency should remove material fill"
        );
        assert!(
            transparent.contains("border: 1px solid alpha(@window_fg_color, 0.1);"),
            "100% transparency should keep a minimum button border"
        );
        assert!(
            transparent.contains("0 12px 32px alpha(black, 0.1)"),
            "100% transparency should keep a minimum button shadow"
        );
        assert!(
            transparent
                .contains(".glass-alert-dialog .body {\n  color: alpha(@window_fg_color, 0.72);"),
            "text color opacity should not be scaled by glass transparency"
        );
        assert!(
            transparent.contains("backdrop-filter: blur(0px) saturate(1) brightness(1);"),
            "100% transparency should neutralize liquid blur without using `none`"
        );
        assert!(
            !transparent.contains("backdrop-filter: none"),
            "GTK backdrop-filter should not be set to none"
        );
    }

    #[test]
    fn settings_dialog_preferences_lists_use_scoped_translucent_material() {
        for liquid in [true, false] {
            let css = build_css(liquid);
            assert!(
                css.contains(".settings-dialog-content .boxed-list"),
                "settings preferences lists should override default boxed-list material"
            );
            assert!(
                css.contains("row.settings-action-row"),
                "settings rows should have a scoped row material hook"
            );
            assert!(
                css.contains(".settings-dialog-content label {\n  color: @window_fg_color;"),
                "settings dialog labels should follow the active light/dark theme"
            );
            assert!(
                css.contains("color: alpha(@window_fg_color, 0.62);"),
                "settings dialog dim labels should use a theme-aware translucent foreground"
            );
            assert!(
                css.contains(".settings-dialog-backdrop .background")
                    && css.contains("background: alpha(@window_bg_color"),
                "settings dialog background should use theme-aware window colors"
            );
            assert!(
                css.contains("background: alpha(@card_bg_color"),
                "settings preferences lists should use theme-aware card colors"
            );
        }
    }

    #[test]
    fn settings_modal_scrim_avoids_full_scene_filters() {
        let css = build_css(true);
        let blur_block = css_block(&css, ".settings-background-blur")
            .expect("settings background class should exist");
        assert!(
            !blur_block.contains("filter:"),
            "settings should not blur the full navigation view while the dialog animates"
        );

        let backdrop_block =
            css_block(&css, ".settings-dialog-backdrop").expect("settings backdrop should exist");
        assert!(
            !backdrop_block.contains("backdrop-filter:"),
            "settings backdrop should stay a lightweight scrim instead of a full-window backdrop blur"
        );
    }

    #[test]
    fn glass_transparency_fades_backdrop_filter_before_hundred() {
        let css = build_css_with_transparency(true, 0.9);

        assert!(
            css.contains("backdrop-filter: blur(2.8px) saturate(1.022) brightness(1.006);"),
            "90% transparency should fade the mode selector filter instead of keeping full blur"
        );
        assert!(
            !css.contains("backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);"),
            "high transparency should not keep the original full-strength glass-raised filter"
        );
    }

    #[test]
    fn liquid_mode_selector_keeps_original_glass_raised_material() {
        let css = build_css(true);

        for marker in [
            "box.mode-selector,\n.glass-segmented {",
            "box.mode-cell,\n.glass-segment {",
            "box.mode-dot,\n.glass-segment-indicator {",
            ".glass-raised {",
            "backdrop-filter: blur(28px) saturate(1.22) brightness(1.06)",
            "0 18px 48px alpha(black, 0.26)",
            "inset 0 1px alpha(@window_fg_color,0.58)",
        ] {
            assert!(
                css.contains(marker),
                "liquid mode selector should keep original glass-raised marker {marker}",
            );
        }
        assert!(
            !css.contains("box.mode-selector box.mode-cell.active"),
            "ModeSelector should not introduce a new active-cell implementation"
        );
    }

    #[test]
    fn glass_menu_surface_matches_raised_segmented_surface_visual_weight() {
        let liquid = build_css(true);
        for marker in [
            ".glass-menu > contents {\n  padding: 6px;\n  border-radius: 16px;\n  background: alpha(@window_bg_color, 0.72);",
            "background-clip: padding-box;\n  border: 1px solid alpha(@window_fg_color, 0.16);",
            "0 18px 48px alpha(black, 0.26)",
            "inset 0 1px alpha(@window_fg_color,0.58)",
        ] {
            assert!(
                liquid.contains(marker),
                "liquid menu surface should preserve raised visual marker {marker}",
            );
        }

        let plain = build_css(false);
        for marker in [
            ".glass-menu > contents {\n  padding: 6px;\n  border-radius: 16px;\n  background: alpha(@window_bg_color, 0.78);",
            ".glass-menu > contents {\n  padding: 6px;\n  border-radius: 16px;\n  background: alpha(@window_bg_color, 0.78);\n  background-clip: padding-box;\n  border: 1px solid alpha(@window_fg_color, 0.10);",
            "box-shadow: 0 4px 12px alpha(black, 0.22);",
        ] {
            assert!(
                plain.contains(marker),
                "plain menu surface should match glass-raised opacity marker {marker}",
            );
        }
    }

    #[test]
    fn segmented_glass_style_is_exposed_as_reusable_css_classes() {
        let css = build_css(true);

        for marker in [
            ".glass-segmented",
            ".glass-segment",
            ".glass-segment-label",
            ".glass-segment-indicator",
            ".glass-segment-label.active",
        ] {
            assert!(
                css.contains(marker),
                "segmented glass style should expose reusable marker {marker}",
            );
        }
    }

    #[test]
    fn compact_glass_menu_width_is_available_for_album_context_menus() {
        let css = build_css(true);

        assert!(
            css.contains(".glass-menu-compact,\n.glass-menu-list-compact {\n  min-width: 150px;"),
            "compact glass menus should have a narrower width than the default photo grid menu",
        );
        assert!(
            css.contains(".glass-menu {\n  padding: 0;\n  min-width: 190px;"),
            "default glass menu width should remain available for denser grid menus",
        );
    }

    #[test]
    fn glass_menu_items_are_transparent_at_rest_like_segmented_slots() {
        let css = build_css(true);

        assert!(
            css.contains(".glass-menu-item {\n  background: transparent;\n  background-clip: padding-box;\n  border: 1px solid transparent;"),
            "menu items should not add a second resting translucency layer over the raised menu surface",
        );
        assert!(
            css.contains(".glass-menu-item:hover {\n  background: alpha(@window_fg_color, 0.08);"),
            "menu items should still show lightweight hover state",
        );
    }

    #[test]
    fn custom_context_menu_reuses_raised_panel_material() {
        let css = build_css(true);

        assert!(
            css.contains(".glass-context-menu {\n  padding: 8px 12px;\n  border-radius: 24px;\n  min-width: 128px;"),
            "custom context menu should use the same capsule geometry family as the mode selector",
        );
        assert!(
            css.contains(".glass-context-menu-item {\n  background: transparent;\n  background-clip: padding-box;\n  border: 1px solid transparent;"),
            "custom context menu items should stay transparent at rest",
        );
        assert!(
            css.contains(
                ".glass-context-menu-item:hover {\n  background: alpha(@window_fg_color, 0.08);"
            ),
            "custom context menu should use lightweight internal hover state",
        );
    }

    /// Plain mode keeps the same selectors but removes the liquid button
    /// signatures. This makes the Settings switch visually meaningful across
    /// every button-like control.
    #[test]
    fn plain_mode_keeps_buttons_plain_not_liquid() {
        let css = build_css(false);

        for selector in [
            ".glass-toolbar-button",
            ".glass-header windowcontrols button image",
            ".glass-header windowcontrols button.close:hover image",
            ".glass-menu-item",
            ".glass-sidebar-row:selected",
            ".viewer-overlay-nav-btn",
            ".viewer-favorite-btn.favorite-active",
        ] {
            assert!(
                css.contains(selector),
                "plain mode missing button selector {selector}",
            );
        }

        for liquid_signature in [
            "inset 0 1px alpha(@window_fg_color,0.44)",
            "inset 0 1px alpha(@window_fg_color,0.36)",
            "0 12px 32px alpha(black, 0.24)",
        ] {
            assert!(
                !css.contains(liquid_signature),
                "plain mode must not keep liquid button material signature {liquid_signature}",
            );
        }
    }

    #[test]
    fn window_close_button_is_red_only_on_interaction() {
        for liquid in [true, false] {
            let css = build_css(liquid);
            let base_rule_start = css
                .find(".glass-header windowcontrols button.close image {")
                .expect("close window button base rule should exist");
            let base_rule = &css[base_rule_start
                ..css[base_rule_start..]
                    .find('}')
                    .map(|end| base_rule_start + end)
                    .expect("close window button base rule should close")];

            assert!(
                !base_rule.contains("#ff5449") && !base_rule.contains("#c01c28"),
                "close window button should not be red until hover/active ({liquid} mode)"
            );
            assert!(
                css.contains(".glass-header windowcontrols button.close:hover image"),
                "close window button should have a red hover rule ({liquid} mode)"
            );
            assert!(
                css.contains(".glass-header windowcontrols button.close:active image"),
                "close window button should have a red active rule ({liquid} mode)"
            );
            if liquid {
                assert!(
                    css.contains(".glass-header windowcontrols button.close:hover image {\n  background: alpha(#ff5449, 0.24);"),
                    "liquid close hover should match the danger toolbar treatment"
                );
            } else {
                assert!(
                    css.contains(".glass-header windowcontrols button.close:hover image {\n  background: alpha(#ff5449, 0.18);"),
                    "plain close hover should match the danger toolbar treatment"
                );
            }
        }
    }

    /// Both modes must carry the shared BASE rules (layout/state); the only
    /// thing that differs is the material block.
    #[test]
    fn both_modes_share_base_and_a11y() {
        let on = build_css(true);
        let off = build_css(false);
        for marker in [
            "flowbox.thumb-grid",
            "box.mode-selector",
            ".glass-sidebar-row",
            ".glass-toolbar-button",
            ".glass-menu-item",
            ".glass-thumb-card",
            ".thumb-image",
        ] {
            assert!(
                on.contains(marker) && off.contains(marker),
                "shared marker '{marker}' must be present in both modes"
            );
        }
    }

    /// The floating details panel overlays the photo, so its metadata content
    /// (AdwPreferencesPage / .boxed-list rows) must be forced transparent in
    /// BOTH glass modes — otherwise libadwaita's default opaque card
    /// backgrounds mask the glass and the photo can't show through.
    #[test]
    fn floating_panel_content_is_transparent() {
        for liquid in [true, false] {
            let css = build_css(liquid);
            assert!(
                css.contains(".viewer-floating-panel .boxed-list"),
                "floating panel must override boxed-list background ({liquid} mode)"
            );
            assert!(
                css.contains(".viewer-floating-panel preferencespage"),
                "floating panel must override preferencespage background ({liquid} mode)"
            );
        }
    }

    /// The details panel floats over bright photos. Keep the top/end/bottom
    /// breathing room, but do not leave a transparent leading gutter that can
    /// show as a bright vertical band beside the panel.
    #[test]
    fn floating_panel_has_no_leading_transparent_gutter() {
        for liquid in [true, false] {
            let css = build_css(liquid);
            assert!(
                css.contains("margin-left: 0;"),
                "floating panel should paint from the overlay sidebar's leading edge ({liquid} mode)"
            );
        }
    }

    /// Sidebar album covers must have square corners (border-radius: 0),
    /// overriding the default .glass-thumb-card 10px radius.
    #[test]
    fn sidebar_cover_has_square_corners() {
        for liquid in [true, false] {
            let css = build_css(liquid);
            // The double-class selector must override .glass-thumb-card's radius.
            let block = css_block(&css, ".glass-sidebar-cover.glass-thumb-card")
                .expect("sidebar cover must have a dedicated double-class rule");
            assert!(
                block.contains("border-radius: 0"),
                "sidebar album cover must have square corners ({liquid} mode), got {block}"
            );
            // The inner picture must also be square.
            let img_block = css_block(&css, ".glass-sidebar-cover .thumb-image")
                .expect("sidebar cover image must have a dedicated rule");
            assert!(
                img_block.contains("border-radius: 0"),
                "sidebar album cover image must have square corners ({liquid} mode), got {img_block}"
            );
        }
    }
}
