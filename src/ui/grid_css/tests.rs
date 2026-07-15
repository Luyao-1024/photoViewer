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

/// Selected thumbnails use the same full-tile dark material in FlowBox and
/// GridView. Keep this contract explicit so a later visual cleanup does not
/// restore the bright veil that made selection unclear on light photos.
#[test]
fn selected_thumbnail_uses_dark_emphasis_material() {
    let css = build_css(true);

    assert!(
        css.contains(".glass-thumb-card.media-selected {\n  background:\n    radial-gradient")
            && css.contains("alpha(black, 0.30)")
            && css.contains(
                "gridview.virtual-media-grid-view > child:hover > .glass-thumb-card.media-selected,\n.glass-thumb-card.media-selected.thumb-pointer-hover {\n  border-color: transparent;\n  box-shadow: none;"
            ),
        "selected thumbnails should use a layered dark emphasis material"
    );

    assert!(
        css.contains(
            "gridview.virtual-media-grid-view > child:hover > .glass-thumb-card.media-selected"
        ) && css.contains(
            "gridview.virtual-media-grid-view > child:hover .thumb-state-glass {\n  opacity: 1;"
        ) && css.contains("linear-gradient(180deg, alpha(black, 0.30), alpha(black, 0.42))")
            && css.contains("border-color: transparent;\n  box-shadow: none;"),
        "hovering a selected GridView tile should retain the borderless dark scrim"
    );

    assert!(
        css.contains("flowbox.thumb-grid > flowboxchild:selected {\n  background: transparent;")
            && css.contains(
                "gridview.virtual-media-grid-view > child:selected {\n  background: transparent;"
            ),
        "GTK selection wrappers must not add a separate blue background"
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
        css.contains(".glass-context-menu-entering {\n  opacity: 0;\n  transform: scale(0.96);"),
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
