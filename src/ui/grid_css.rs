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
const BASE_CSS: &str = "
flowbox.thumb-grid > flowboxchild { padding: 0; }
/* 8px four-sided padding matches column/row spacing. Do NOT add a fixed
   padding-bottom here or on the ScrolledWindow: the ModeSelector is a glass
   overlay and should float over content instead of reserving a dark safe area.
   每段或滚动容器 padding-bottom 会留出深色空隙,看起来像黑带。 */
flowbox.thumb-grid { padding: 8px; background: transparent; }

/* Hover — soft veil on the flowboxchild, no border. */
flowbox.thumb-grid > flowboxchild:hover > .glass-thumb-card {
  background: alpha(@window_fg_color, 0.08);
  border-color: alpha(@window_fg_color, 0.18);
}

/* Keyboard focus — glass ring. Hidden once the pointer leaves the grid
   (see .pointer-left rule below) so the ring doesn't linger on the
   last-hovered tile after the mouse moves away. */
flowbox.thumb-grid > flowboxchild:focus > .glass-thumb-card {
  outline: 2px solid alpha(@window_fg_color, 0.55);
  outline-offset: 2px;
}

/* Selected — luminous glass border + soft inner veil. Glass alone
   communicates the selected state; no accent colour needed. */
flowbox.thumb-grid > flowboxchild:selected > .glass-thumb-card {
  background: alpha(@window_fg_color, 0.10);
  border-color: alpha(@window_fg_color, 0.48);
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.35);
}

/* Pointer-left — hide the focus ring once the mouse exits the grid.
   kbd-nav overrides this so keyboard users still see the cursor. */
flowbox.thumb-grid.pointer-left:not(.kbd-nav) > flowboxchild:focus > .glass-thumb-card {
  outline: none;
}

/* Kbd-nav neutralisation — see attach_kbd_nav comments; behaviour
   preserved from the prior implementation. */
flowbox.thumb-grid.kbd-nav > flowboxchild:hover:not(:focus) > .glass-thumb-card {
  background: transparent;
  border-color: transparent;
  outline: none;
}

/* Selection checkmark — a translucent white tick pinned to each tile's
   bottom-right corner. The checkmark widget is parented in every SquareTile
   but kept invisible (opacity 0) until the wrapping FlowBoxChild becomes
   :selected, so it appears only on selected photos. A subtle icon shadow
   keeps it legible over bright thumbnails. This is the primary selected-state
   affordance; the softer glass border on .glass-thumb-card is secondary. */
.thumb-checkmark {
  color: alpha(white, 0.92);
  opacity: 0;
  -gtk-icon-shadow: 0 1px 2px alpha(black, 0.55);
  transition: opacity 120ms ease;
}

flowbox.thumb-grid > flowboxchild:selected .thumb-checkmark {
  opacity: 1;
}

/* Segmented glass — the reusable shape used by the 年/月/日 mode selector.
   Apply `glass-raised glass-segmented` to the outer container, `glass-segment`
   to equal-width slots, `glass-segment-label` to labels, and
   `glass-segment-indicator` to the active underline/dot. The existing
   ModeSelector selectors stay here so its current implementation and visual
   contract remain unchanged. */
box.mode-selector,
.glass-segmented {
  padding: 8px 16px;
  border-radius: 24px;
  min-height: 58px;
}

box.mode-selector.on-light-background,
.glass-segmented.on-light-background {
  /* No material override — the .glass-raised rule already provides a
     light/dark balanced fill. Kept as a hook in case we later want a
     different border on bright photo backgrounds. */
}

/* 单个 label / dot 槽位 */
box.mode-cell,
.glass-segment {
  min-width: 60px;
  padding: 4px 12px;
}

/* 标签：默认半透明、title-3 字号。颜色跟随主题变量，与 glass-raised 容器
   底配套，亮/暗主题下都可见。 */
box.mode-selector label,
.glass-segment-label {
  font-size: 14pt;
  font-weight: 700;
  color: @window_fg_color;
  opacity: 0.55;
  transition: opacity 120ms ease;
}

/* 激活态：标签全亮 */
box.mode-selector label.active,
.glass-segment-label.active {
  opacity: 1.0;
}

/* 激活指示点 */
box.mode-dot,
.glass-segment-indicator {
  background: @window_fg_color;
  border-radius: 2px;
  min-width: 24px;
  min-height: 4px;
  margin-top: 2px;
}

/* glass-toolbar-button — individual buttons in glass header bars.
   This base rule owns only geometry; the actual material lives in the
   Liquid/Plain material blocks so Settings can switch every button at once. */
.glass-toolbar-button {
  min-height: 34px;
  min-width: 34px;
  border-radius: 10px;
  padding: 0 14px;
  color: inherit;
}

/* Viewer chrome buttons (the viewer header) are square (1:1) and float over
   the photo, so — like .viewer-overlay-nav-btn — they render a white icon with
   a dark halo instead of inheriting the theme foreground. This keeps every
   viewer control reading as one set of light affordances over the image, and
   stops a black icon (light theme) vanishing into a bright photo bleeding
   through the translucent header. Scoped to .viewer-chrome (only the viewer
   header carries it) so the shared .glass-toolbar-button in other headers
   (photos/trash/albums/editor) is untouched. The prev/next arrows
   (.viewer-overlay-nav-btn) are smaller and live in their own bottom-right
   pair, so they don't match this geometry override. */
.viewer-chrome .glass-toolbar-button {
  min-width: 38px;
  min-height: 38px;
  padding: 0;
  color: #ffffff;
  -gtk-icon-shadow: 0 1px 2px alpha(black, 0.9), 0 0 6px alpha(black, 0.65);
}

.glass-toolbar-button.crop-ratio-arrow-button {
  min-width: 28px;
  min-height: 40px;
  padding: 0 4px;
  border-radius: 8px;
}

.glass-header windowcontrols button {
  min-height: 28px;
  min-width: 28px;
  margin: 0 2px;
  padding: 0;
  background: transparent;
  border: 0;
  box-shadow: none;
  color: inherit;
}

.glass-header windowcontrols button image {
  min-height: 24px;
  min-width: 24px;
  border-radius: 999px;
  padding: 0;
}

.glass-toolbar-button:focus-visible,
.glass-toolbar-button:focus,
.glass-header windowcontrols button:focus-visible,
.glass-header windowcontrols button:focus {
  outline: none;
}

.glass-toolbar-button.glass-toolbar-danger,
.glass-toolbar-danger {
  background: alpha(#ff5449, 0.16);
  border-color: alpha(#ffb4ab, 0.34);
  color: #ffb4ab;
}

/* glass-toolbar-suggested — primary action accent (blue) for Save Copy,
   Accept, and similar high-confidence buttons. Composes with
   .glass-toolbar-button. Mirrors .glass-menu-item-suggested so the same
   action language is shared between toolbar and menu surfaces.
   用于工具栏主操作按钮(蓝色),与菜单项的 suggested 风格保持一致。 */
.glass-toolbar-suggested { color: #a8d2ff; }

/* glass-menu — popovers; GTK popovers are two-layer, style the inner
   `> contents` so the visible background matches the rounded edge. The
   `> contents` material lives in the per-mode material block below. */
.glass-menu {
  padding: 0;
  min-width: 190px;
}

.glass-menu-list {
  min-width: 190px;
}

.glass-menu-compact,
.glass-menu-list-compact {
  min-width: 150px;
}

.glass-menu-item {
  min-height: 36px;
  border-radius: 10px;
  padding: 0 12px;
  color: inherit;
}

.glass-menu-item:focus-visible,
.glass-menu-item:focus {
  outline: none;
}

.glass-menu-item:disabled {
  color: alpha(currentColor, 0.45);
}

.glass-menu-item-suggested { color: #a8d2ff; }

.glass-menu-item-danger { color: #ffb4ab; }

.glass-context-menu-layer {
  background: transparent;
}

.glass-context-menu {
  padding: 8px 12px;
  border-radius: 24px;
  min-width: 128px;
}

.glass-context-menu-item {
  min-height: 36px;
  border-radius: 14px;
  padding: 0 12px;
  color: @window_fg_color;
  font-weight: 600;
}

.glass-context-menu-item:focus-visible,
.glass-context-menu-item:focus {
  outline: none;
}

.glass-context-menu-item-suggested { color: #a8d2ff; }

.glass-context-menu-item-danger { color: #ffb4ab; }

/* glass-sidebar — list/layout only. The sidebar surface itself is the parent
   .glass-sidebar-surface.glass-base, so list and footer stay on one material. */
.glass-sidebar-surface,
.glass-sidebar,
.glass-sidebar-footer {
  background: transparent;
  background-color: transparent;
}

.glass-sidebar {
  padding: 3px 12px;
  border: 0;
}

/* The main sidebar list (Photos + Albums header) lives above the album
   scroll region. Keep its bottom padding at 3 px so the gap to the first
   album row matches the inter-row margin. */
.glass-sidebar-main {
  padding-bottom: 3px;
}

.glass-sidebar row {
  background: transparent;
  background-color: transparent;
}

.glass-sidebar-page {
  background: transparent;
}

.sidebar-settings-button {
  min-width: 40px;
  min-height: 40px;
  padding: 0;
  border-radius: 999px;
}

.glass-sidebar-row {
  min-height: 40px;
  border-radius: 12px;
  margin-bottom: 3px;
  padding: 0 10px;
}

.glass-sidebar-row:focus-visible,
.glass-sidebar-row:focus {
  outline: none;
}

.glass-sidebar-label {
  color: inherit;
  font-weight: 500;
}

/* Album sub-rows are nested under the Albums group header. They reuse the
   glass-sidebar-row material (hover/selected veil lives in the per-mode
   blocks); this only indents the content so the row reads as a child of the
   group while keeping its rounded highlight full-width — matching the Honor
   gallery tree layout in the reference screenshot.
   相册子行缩进于「相册」分组头之下;选中/hover 玻璃材质复用 .glass-sidebar-row,
   这里只做内容缩进,圆角高亮仍占满整行。 */
.glass-sidebar-subrow {
  padding-left: 28px;
  min-height: 36px;
}

/* The album list sits inside a ScrolledWindow nested below the Albums
   group header. Zero out vertical padding so the first album row sits
   close to the header and the last row sits close to the trash list. */
.glass-sidebar-album-list {
  padding-top: 0;
  padding-bottom: 0;
}

/* Leading symbolic icon — slightly muted so the label stays primary. */
.glass-sidebar-icon {
  opacity: 0.72;
}

/* Right-aligned count badge on album rows. */
.glass-sidebar-count {
  color: inherit;
  opacity: 0.72;
  font-size: 0.92em;
  font-weight: 500;
}

.glass-sidebar-row:selected .glass-sidebar-count {
  opacity: 0.92;
  font-weight: 600;
}

/* Albums group header — a non-selectable disclosure row. Bolder label, no
   hover/selected veil (it never claims the selection), and a muted arrow. */
.glass-sidebar-section {
  min-height: 34px;
  margin-top: 3px;
}

.glass-sidebar-section-label {
  color: inherit;
  font-weight: 700;
  opacity: 0.78;
}

.glass-sidebar-arrow {
  opacity: 0.6;
}

/* Drag-to-reorder an album row. The dragged row is dimmed while held, and the
   drop target shows an accent edge (top or bottom half) so the user sees where
   the album will land. Inset shadow layers on top of the glass material, so it
   reads correctly in both Liquid Glass and plain translucent modes. */
.glass-sidebar-row-dragging {
  opacity: 0.4;
}
.glass-sidebar-row-drop-above {
  box-shadow: inset 0 2px 0 @accent_bg_color;
}
.glass-sidebar-row-drop-below {
  box-shadow: inset 0 -2px 0 @accent_bg_color;
}

/* Album browser card drag sorting. These indicators sit on the card itself,
   so they work regardless of how FlowBox wraps cards across columns. */
.album-browser-card-dragging {
  opacity: 0.45;
}
.album-browser-card-drop-before {
  box-shadow: inset 0 3px 0 @accent_bg_color;
}
.album-browser-card-drop-after {
  box-shadow: inset 0 -3px 0 @accent_bg_color;
}

/* viewer-stage — image content area; subtle radial wash that frames
   the picture and separates it from app chrome. */
.viewer-stage {
  padding: 32px;
  background:
    radial-gradient(circle at center, alpha(@window_fg_color, 0.06), transparent 55%),
    alpha(@window_bg_color, 0.10);
}

.viewer-image-frame {
  border-radius: 14px;
  box-shadow:
    0 24px 80px alpha(black, 0.38),
    0 0 0 1px alpha(@window_fg_color, 0.10);
}

.viewer-details-name-row {
  min-height: 68px;
}

.viewer-details-name-entry {
  min-width: 180px;
}

/* glass-editor-preview — analogous to .viewer-stage, but calmer: the
   editor's adjustment sliders occupy the same screen and need every
   ounce of readable chrome, so this is a near-flat panel with a hairline
   border, not a heavy glass stage.
   比 viewer-stage 更克制:编辑器界面与调节滑块同屏,需要尽可能多的可读
   chrome,所以这是近乎平坦的面板加一道细线边框,而不是沉重的玻璃舞台。 */
.glass-editor-preview {
  padding: 24px;
  background: alpha(@window_bg_color, 0.06);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
}

/* Viewer favorite button active state. Class is added/removed by
   ViewerPage::refresh_favorite_button. Only the heart ICON recolors to a
   translucent red — the button itself stays bare (no capsule), matching the
   rest of the viewer chrome. 透明质感的半透明红,不是实心红。 */
.viewer-favorite-btn.favorite-active {
  color: alpha(#ff5e51, 0.92);
}

/* A touch brighter on hover so the red heart still reads against the lit
   glass capsule that appears behind it on pointer-over. */
.viewer-favorite-btn.favorite-active:hover {
  color: alpha(#ff7a6e, 0.96);
}

/* glass-thumb-card — photo tile wrapper. NO backdrop-filter here; it
   would be too expensive at 10k–100k tiles and would blur the photo. */
.glass-thumb-card {
  border-radius: 10px;
  border: 1px solid transparent;
  background: transparent;
}

.thumb-image {
  border-radius: 10px;
}

.thumb-motion-badge {
  color: alpha(white, 0.95);
  background: alpha(black, 0.42);
  border-radius: 999px;
  padding: 5px;
}

.thumb-video-duration {
  color: white;
  background: alpha(black, 0.50);
  border-radius: 999px;
  padding: 4px 8px;
  font-weight: 700;
  font-size: 10pt;
  text-shadow: 0 1px 2px alpha(black, 0.75);
}

.thumb-favorite-badge {
  color: white;
  -gtk-icon-shadow: 0 1px 3px alpha(black, 0.72);
}

.library-stats {
  border-radius: 999px;
  padding: 7px 14px;
  color: alpha(@window_fg_color, 0.88);
  font-size: 10pt;
  font-weight: 600;
}

/* thumb-loading — 缩略图生成期间的静态占位。缩略图到位后 SquareTile
   在 set_paintable 里移除该 class。 */
.thumb-loading {
  background-color: alpha(@window_fg_color, 0.05);
}

.thumb-placeholder {
  background-color: alpha(@window_fg_color, 0.08);
  box-shadow: inset 0 0 0 1px alpha(@window_fg_color, 0.10);
}

/* ── Viewer filmstrip — 缩略图预览栏 ────────────────────────────────────
   The bottom overlay in ViewerPage. The bar is layout-only so the thumbnails
   float without a capsule background; these rules own per-item emphasis.
   Items keep original aspect ratio (set via width-request after texture load
   in viewer_page.rs).
   ViewerPage 底部缩略图预览栏。容器只负责布局,不绘制背景胶囊；这里只管
   单项强调。缩略图保持原始宽高比。 */
.viewer-thumb-bar {
  padding: 10px 8px;
}

.viewer-thumb-strip {
  padding: 0;
  /* 让 strip 内 65 个按钮的 min-width 之和不再撑大 viewer ——
     GTK widget 的 min-width 默认继承子节点,设 0 切断累积 */
  min-width: 0;
}

button.viewer-thumb-item {
  padding: 1px;
  min-width: 36px;
  min-height: 56px;
  border-radius: 8px;
  background: transparent;
  background-image: none;
  border: 0;
  outline: none;
  transition: background-color 120ms ease, opacity 120ms ease, outline-color 120ms ease, transform 120ms ease;
  box-shadow: none;
  opacity: 0.42;
}

button.viewer-thumb-item:hover,
button.viewer-thumb-item:focus,
button.viewer-thumb-item:focus-visible,
button.viewer-thumb-item:active,
button.viewer-thumb-item:checked {
  background: transparent;
  background-image: none;
  box-shadow: none;
  outline: none;
  border-color: transparent;
}

button.viewer-thumb-item.viewer-thumb-current {
  margin-left: 12px;
  margin-right: 12px;
  padding: 4px;
  transform: scale(1.30);
  background: alpha(@window_fg_color, 0.10);
  outline: 2px solid alpha(@window_fg_color, 0.55);
  outline-offset: 2px;
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.35),
    0 0 0 4px alpha(@window_fg_color, 0.12),
    0 10px 24px alpha(black, 0.48);
  opacity: 1.0;
}

button.viewer-thumb-item picture {
  border-radius: 6px;
  border: none;
  outline: none;
  box-shadow: none;
}

button.viewer-thumb-item.viewer-thumb-current picture {
  border: 2px solid alpha(@window_fg_color, 0.48);
  box-shadow: 0 0 18px alpha(@window_fg_color, 0.20);
}

/* viewer-overlay-nav — layout box only for the prev/next overlay buttons.
   It has NO capsule material: the container is bare and each button draws
   its own glass surface on hover/focus (see the viewer-chrome hover rules in
   the material blocks), so the pair reads as two light arrows floating over
   the photo instead of a blocking chrome bar. */
.viewer-overlay-nav {
  padding: 0;
}

.viewer-overlay-nav-btn {
  min-width: 36px;
  min-height: 32px;
  padding: 0;
  border-radius: 10px;
  /* These buttons float bare over the photo at rest and gain a theme-following
     glass capsule on hover. The icon stays white in BOTH states so there is no
     jarring light→dark jump on hover; the dark halo keeps it legible over any
     photo and also over the light capsule a light theme produces on hover. */
  color: #ffffff;
  -gtk-icon-shadow: 0 1px 2px alpha(black, 0.9), 0 0 6px alpha(black, 0.65);
}

/* viewer-floating-panel 内容链透明化 ─────────────────────────────
   详情浮层叠在原图之上,但其内部 AdwPreferencesPage / AdwPreferencesGroup
   的 .boxed-list 行默认带不透明卡片背景(libadwaita 默认样式),会盖住浮层
   自身的半透明玻璃,导致看不到背后原图。这里把整条内容链强制透明、去掉卡片
   投影,只保留行间细分隔,让 alpha(black) 材质(+ 运行时支持时的
   backdrop-filter 模糊)成为可见面,原图能透出。两种玻璃模式共用,故置于此。
   选择器以 .viewer-floating-panel 前缀提升优先级,盖过 libadwaita 的
   `list.boxed-list > row` 等默认规则。 */
.viewer-floating-panel preferencespage,
.viewer-floating-panel preferencesgroup,
.viewer-floating-panel scrolledwindow,
.viewer-floating-panel viewport,
.viewer-floating-panel list,
.viewer-floating-panel .boxed-list {
  background: transparent;
  background-color: transparent;
  box-shadow: none;
}

.viewer-floating-panel list row,
.viewer-floating-panel .boxed-list > row,
.viewer-floating-panel .boxed-list row {
  background: transparent;
  background-color: transparent;
}

/* details-split-view 内部 sidebar 容器透明化 ──────────────────────
   Adw.OverlaySplitView wraps its sidebar child in an internal widget that
   carries an opaque theme background (sidebar_bg_color). In collapsed
   overlay mode this opaque wrapper sits between the photo and the
   translucent glass panel, blocking the photo from showing through. Force
   the wrapper layers transparent so the glass material on
   .viewer-floating-panel becomes the visible surface.
   OverlaySplitView 内部 sidebar 容器有不透明主题背景,会挡住玻璃浮层,
   强制透明让原图能透出。 */
.details-split-view {
  background: transparent;
  background-color: transparent;
}
.details-split-view > widget {
  background: transparent;
  background-color: transparent;
}

/* Settings content uses AdwPreferencesGroup rows for alignment, but the
   default boxed-list card is too opaque inside the glass dialog. Keep the
   content chain transparent and let the scoped material blocks below paint a
   consistent translucent list surface. */
.settings-dialog-content preferencesgroup,
.settings-dialog-content list,
.settings-dialog-content .boxed-list {
  box-shadow: none;
}

.settings-dialog-content .boxed-list > row,
.settings-dialog-content .boxed-list row,
row.settings-action-row {
  background: transparent;
  background-color: transparent;
}

.settings-dialog-content label {
  color: @window_fg_color;
}

.settings-dialog-content label.dim-label,
.settings-dialog-content row.settings-action-row label.subtitle {
  color: alpha(@window_fg_color, 0.62);
}
";

/* ── LIQUID_GLASS_MATERIAL_CSS ─ the dramatic Liquid Glass material:
backdrop blur+saturate+brightness, bright inset top highlights, and heavy
floating shadows. This is the default (opt-out) look. */
const LIQUID_GLASS_MATERIAL_CSS: &str = "
/* glass-base — sidebar, header, details panel */
.glass-base {
  background: alpha(@window_bg_color, 0.42);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.14);
  backdrop-filter: blur(22px) saturate(1.18) brightness(1.04);
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.32),
    inset 0 -1px alpha(black, 0.10);
}

/* glass-raised — floating controls (mode selector, menus, popovers) */
.glass-raised {
  background: alpha(@window_bg_color, 0.58);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.18);
  backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
  box-shadow:
    0 18px 48px alpha(black, 0.26),
    inset 0 1px alpha(@window_fg_color,0.58),
    inset 0 -1px alpha(black, 0.16);
}

/* glass-menu popover inner surface */
.glass-menu > contents {
  padding: 6px;
  border-radius: 16px;
  background: alpha(@window_bg_color, 0.72);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.16);
  backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
  box-shadow:
    0 18px 48px alpha(black, 0.26),
    inset 0 1px alpha(@window_fg_color,0.58),
    inset 0 -1px alpha(black, 0.16);
}

/* Unified Liquid Glass button material. These selectors intentionally cover
   all button-like chrome so the Settings switch changes the whole language. */
.glass-toolbar-button,
.glass-header windowcontrols button image {
  background: alpha(@window_bg_color, 0.48);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.14);
  box-shadow:
    0 12px 32px alpha(black, 0.24),
    inset 0 1px alpha(@window_fg_color,0.44),
    inset 0 -1px alpha(black, 0.12);
}

.glass-toolbar-button:hover,
.glass-header windowcontrols button:hover image {
  background: alpha(@window_bg_color, 0.62);
  border-color: alpha(@window_fg_color, 0.20);
  box-shadow:
    0 14px 36px alpha(black, 0.30),
    inset 0 1px alpha(@window_fg_color,0.52),
    inset 0 -1px alpha(black, 0.14);
}

.glass-toolbar-button:active,
.glass-toolbar-button:checked,
.glass-header windowcontrols button:active image,
.glass-header windowcontrols button:checked image {
  background: alpha(@window_bg_color, 0.72);
  border-color: alpha(@window_fg_color, 0.24);
  box-shadow:
    0 8px 22px alpha(black, 0.24),
    inset 0 1px alpha(@window_fg_color,0.34),
    inset 0 -1px alpha(black, 0.18);
}

.glass-header windowcontrols button.close image {
  color: inherit;
}

.glass-header windowcontrols button.close:hover image {
  background: alpha(#ff5449, 0.24);
  border-color: alpha(#ffb4ab, 0.42);
  color: #ffb4ab;
}

.glass-header windowcontrols button.close:active image {
  background: alpha(#ff5449, 0.30);
  border-color: alpha(#ffb4ab, 0.48);
  color: #ffb4ab;
}

.glass-toolbar-suggested:hover {
  background: alpha(#5aa7ff, 0.24);
  border-color: alpha(#a8d2ff, 0.42);
  color: #c8e0ff;
}

.glass-toolbar-danger:hover {
  background: alpha(#ff5449, 0.24);
  border-color: alpha(#ffb4ab, 0.42);
  color: #ffb4ab;
}

.glass-menu-item {
  background: transparent;
  background-clip: padding-box;
  border: 1px solid transparent;
}

.glass-menu-item:hover {
  background: alpha(@window_fg_color, 0.08);
  border-color: alpha(@window_fg_color, 0.14);
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.36),
    inset 0 -1px alpha(black, 0.10);
}

.glass-menu-item-suggested:hover {
  background: alpha(#5aa7ff, 0.24);
  border-color: alpha(#a8d2ff, 0.34);
  color: #c8e0ff;
}

.glass-menu-item-danger:hover {
  background: alpha(#ff5449, 0.24);
  border-color: alpha(#ffb4ab, 0.34);
  color: #ffcfca;
}

.glass-context-menu-item {
  background: transparent;
  background-clip: padding-box;
  border: 1px solid transparent;
  box-shadow: none;
}

.glass-context-menu-item:hover {
  background: alpha(@window_fg_color, 0.08);
  border-color: alpha(@window_fg_color, 0.12);
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.28);
}

.glass-context-menu-item-suggested:hover {
  background: alpha(#5aa7ff, 0.20);
  border-color: alpha(#a8d2ff, 0.30);
  color: #c8e0ff;
}

.glass-context-menu-item-danger:hover {
  background: alpha(#ff5449, 0.20);
  border-color: alpha(#ffb4ab, 0.30);
  color: #ffcfca;
}

.glass-sidebar-row {
  background: transparent;
  border: 1px solid transparent;
}

.glass-sidebar-row:hover {
  background: alpha(@accent_bg_color, 0.14);
  border-color: alpha(@accent_bg_color, 0.26);
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.28);
}

.glass-sidebar-row:selected {
  background: alpha(@accent_bg_color, 0.22);
  border-color: alpha(@accent_bg_color, 0.42);
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.36),
    inset 0 -1px alpha(black, 0.12);
}

.settings-dialog-content .boxed-list {
  background: alpha(@card_bg_color, 0.62);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.12);
  border-radius: 16px;
  backdrop-filter: blur(22px) saturate(1.12) brightness(1.02);
}

row.settings-action-row:hover {
  background: alpha(@window_fg_color, 0.06);
}

.settings-background-blur {
  opacity: 0.82;
}

.settings-dialog-backdrop .background {
  background: alpha(@window_bg_color, 0.76);
  border-color: alpha(@window_fg_color, 0.12);
  color: @window_fg_color;
}

.settings-dialog-backdrop {
  background: alpha(@window_bg_color, 0.26);
}

.settings-about-text {
  margin-top: 8px;
  font-size: 0.86em;
  opacity: 0.56;
}

/* viewer-overlay-nav capsule is intentionally bare — no background, border,
   or shadow. The prev/next buttons inside draw their own glass surface on
   hover/focus, so a container capsule would only add clutter over the photo. */
.viewer-overlay-nav {
  background: transparent;
  border: none;
  box-shadow: none;
}

.viewer-overlay-nav-btn {
  background: alpha(@window_bg_color, 0.52);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.16);
  box-shadow:
    inset 0 1px alpha(@window_fg_color,0.36),
    inset 0 -1px alpha(black, 0.10);
}

.viewer-overlay-nav-btn:hover {
  background: alpha(@window_bg_color, 0.68);
  border-color: alpha(@window_fg_color, 0.22);
}

/* ── Viewer chrome: glass material only on hover/focus ──────────────
   The viewer floats over a full-bleed photo, so its header buttons and the
   prev/next overlay arrows are bare at rest (icon only, no capsule) and only
   gain their glass material when the pointer hovers or keyboard focus lands
   on them — keeping the photo uncluttered. .glass-toolbar-button is shared
   by every header bar (photos, trash, albums, editor), so the bare-at-rest
   reset is scoped to .viewer-chrome (carried by the viewer header) and
   .viewer-overlay-nav-btn. A favorited photo signals state through its red
   heart icon, not a button capsule. */
.viewer-chrome .glass-toolbar-button,
.viewer-overlay-nav-btn {
  background: transparent;
  background-clip: padding-box;
  border-color: transparent;
  box-shadow: none;
}

.viewer-chrome .glass-toolbar-button:hover,
.viewer-chrome .glass-toolbar-button:focus-visible,
.viewer-overlay-nav-btn:hover,
.viewer-overlay-nav-btn:focus-visible {
  background: alpha(@window_bg_color, 0.62);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.20);
  box-shadow:
    0 14px 36px alpha(black, 0.30),
    inset 0 1px alpha(@window_fg_color,0.52),
    inset 0 -1px alpha(black, 0.14);
}

.viewer-chrome .glass-toolbar-button:active,
.viewer-chrome .glass-toolbar-button:checked {
  background: alpha(@window_bg_color, 0.72);
  border: 1px solid alpha(@window_fg_color, 0.24);
  box-shadow:
    0 8px 22px alpha(black, 0.24),
    inset 0 1px alpha(@window_fg_color,0.34),
    inset 0 -1px alpha(black, 0.18);
}

/* Delete keeps its red danger treatment on hover inside the viewer. */
.viewer-chrome .glass-toolbar-button.glass-toolbar-danger:hover {
  background: alpha(#ff5449, 0.24);
  border: 1px solid alpha(#ffb4ab, 0.42);
  box-shadow:
    0 14px 36px alpha(black, 0.30),
    inset 0 1px alpha(@window_fg_color,0.52),
    inset 0 -1px alpha(black, 0.14);
  color: #ffb4ab;
}

/* Favorite-active has no surface material here — it only recolors the heart
   icon via the .viewer-favorite-btn.favorite-active color rule in BASE_CSS. */

/* ── Sidebar settings button: glass material only on hover/focus (liquid) ──
   Same hover-only language as the viewer chrome: the footer settings button
   shows just its icon at rest and only draws a glass capsule on hover/focus,
   keeping the sidebar footer calm. `.sidebar-settings-button` is unique to
   this one button, so scoping to it alone is enough. */
.sidebar-settings-button {
  background: transparent;
  background-clip: padding-box;
  border-color: transparent;
  box-shadow: none;
}

.sidebar-settings-button:hover,
.sidebar-settings-button:focus-visible {
  background: alpha(@window_bg_color, 0.62);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.20);
  box-shadow:
    0 14px 36px alpha(black, 0.30),
    inset 0 1px alpha(@window_fg_color,0.52),
    inset 0 -1px alpha(black, 0.14);
}

.sidebar-settings-button:active {
  background: alpha(@window_bg_color, 0.72);
  border: 1px solid alpha(@window_fg_color, 0.24);
  box-shadow:
    0 8px 22px alpha(black, 0.24),
    inset 0 1px alpha(@window_fg_color,0.34),
    inset 0 -1px alpha(black, 0.18);
}

/* ── Glass alert dialog — 毛玻璃半透明弹框 + 液态玻璃按钮 ──────────────
   AdwAlertDialog 的 CSS 类加在最外层节点(1200x800 填满窗口)，
   可见卡片是深层后代 AdwGizmo.background(约 300x178)。
   因此根节点保持透明，毛玻璃材质放到 .background 上。 */
.glass-alert-dialog {
  background: transparent;
}

.glass-alert-dialog .background {
  background: alpha(@window_bg_color, 0.78);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.16);
  border-radius: 20px;
  backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
  box-shadow:
    0 24px 64px alpha(black, 0.40),
    inset 0 1px alpha(@window_fg_color,0.28);
  color: @window_fg_color;
}

.glass-alert-dialog .title-2 {
  font-weight: 700;
  color: @window_fg_color;
}

.glass-alert-dialog .body {
  color: alpha(@window_fg_color, 0.72);
}

/* Response buttons — 液态玻璃 pill 风格 */
.glass-alert-dialog button.text-button {
  min-height: 38px;
  border-radius: 12px;
  padding: 0 18px;
  background: alpha(@window_bg_color, 0.54);
  border: 1px solid alpha(@window_fg_color, 0.14);
  color: @window_fg_color;
  font-weight: 600;
  transition: background 120ms ease, border-color 120ms ease;
}

.glass-alert-dialog button.text-button:hover {
  background: alpha(@window_bg_color, 0.68);
  border-color: alpha(@window_fg_color, 0.22);
}

.glass-alert-dialog button.text-button:active {
  background: alpha(@window_bg_color, 0.78);
}

/* Destructive response — 红色调液态玻璃 */
.glass-alert-dialog button.destructive-action {
  color: #ffb4ab;
}

.glass-alert-dialog button.destructive-action:hover {
  background: alpha(#ff5449, 0.22);
  border-color: alpha(#ff5449, 0.48);
}

/* glass-header — header bar surface (calmer than glass-raised) */
.glass-header {
  background: alpha(@window_bg_color, 0.56);
  background-clip: padding-box;
  border-bottom: 1px solid alpha(@window_fg_color, 0.10);
  backdrop-filter: blur(20px) saturate(1.10) brightness(1.02);
}

/* viewer-details-panel — metadata sidebar uses glass-base, not opaque. */
.viewer-details-panel {
  background: alpha(@window_bg_color, 0.66);
  background-clip: padding-box;
  border-left: 1px solid alpha(@window_fg_color, 0.12);
  backdrop-filter: blur(22px) saturate(1.12);
}

/* viewer-floating-panel — 详情面板浮层:浮在原图之上(overlay),不再挤占。
   半透明深底 + 液态模糊 + 圆角 + 悬浮投影;margin 让其脱离边缘呈悬浮卡片。 */
.viewer-floating-panel {
  margin: 12px;
  margin-left: 0;
  border-radius: 16px;
  background: alpha(@window_bg_color, 0.70);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.14);
  backdrop-filter: blur(26px) saturate(1.20) brightness(1.05);
  box-shadow:
    0 14px 44px alpha(black, 0.42),
    inset 0 1px alpha(@window_fg_color,0.30);
}
";

/* ── PLAIN_GLASS_MATERIAL_CSS ─ plain semi-transparent surfaces, NO blur.
Same selectors as the liquid block, but drops `backdrop-filter` entirely
along with the bright inset top highlights and the heavy floating drop
shadows. The result is a clearly different look from Liquid Glass: sharp
translucent panels and controls with restrained borders.
普通半透明:没有 backdrop-filter、高光、厚重投影,只留半透明背景 + 细边 +
轻阴影,与液态玻璃差异明显。 */
const PLAIN_GLASS_MATERIAL_CSS: &str = "
.glass-base {
  background: alpha(@window_bg_color, 0.72);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
}

.glass-raised {
  background: alpha(@window_bg_color, 0.78);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
  box-shadow: 0 4px 12px alpha(black, 0.22);
}

.glass-menu > contents {
  padding: 6px;
  border-radius: 16px;
  background: alpha(@window_bg_color, 0.78);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
  box-shadow: 0 4px 12px alpha(black, 0.22);
}

.glass-toolbar-button,
.glass-header windowcontrols button image {
  background: alpha(@window_bg_color, 0.52);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
  box-shadow: 0 3px 10px alpha(black, 0.18);
}

.glass-toolbar-button:hover,
.glass-header windowcontrols button:hover image {
  background: alpha(@window_bg_color, 0.64);
  border-color: alpha(@window_fg_color, 0.16);
}

.glass-toolbar-button:active,
.glass-toolbar-button:checked,
.glass-header windowcontrols button:active image,
.glass-header windowcontrols button:checked image {
  background: alpha(@window_bg_color, 0.74);
  border-color: alpha(@window_fg_color, 0.20);
}

.glass-header windowcontrols button.close image {
  color: inherit;
}

.glass-header windowcontrols button.close:hover image {
  background: alpha(#ff5449, 0.18);
  border-color: alpha(#ffb4ab, 0.28);
  color: #ffb4ab;
}

.glass-header windowcontrols button.close:active image {
  background: alpha(#ff5449, 0.24);
  border-color: alpha(#ffb4ab, 0.34);
  color: #ffb4ab;
}

.glass-toolbar-suggested:hover {
  background: alpha(#5aa7ff, 0.18);
  border-color: alpha(#a8d2ff, 0.28);
  color: #c8e0ff;
}

.glass-toolbar-danger:hover {
  background: alpha(#ff5449, 0.18);
  border-color: alpha(#ffb4ab, 0.28);
  color: #ffb4ab;
}

.glass-menu-item {
  background: transparent;
  border: 1px solid transparent;
}

.glass-menu-item:hover {
  background: alpha(@window_fg_color, 0.07);
  border-color: alpha(@window_fg_color, 0.10);
}

.glass-menu-item-suggested:hover {
  background: alpha(#5aa7ff, 0.18);
  border-color: alpha(#a8d2ff, 0.22);
  color: #c8e0ff;
}

.glass-menu-item-danger:hover {
  background: alpha(#ff5449, 0.18);
  border-color: alpha(#ffb4ab, 0.22);
  color: #ffcfca;
}

.glass-context-menu-item {
  background: transparent;
  border: 1px solid transparent;
  box-shadow: none;
}

.glass-context-menu-item:hover {
  background: alpha(@window_fg_color, 0.06);
  border-color: alpha(@window_fg_color, 0.08);
}

.glass-context-menu-item-suggested:hover {
  background: alpha(#5aa7ff, 0.16);
  border-color: alpha(#a8d2ff, 0.20);
  color: #c8e0ff;
}

.glass-context-menu-item-danger:hover {
  background: alpha(#ff5449, 0.16);
  border-color: alpha(#ffb4ab, 0.20);
  color: #ffcfca;
}

.glass-sidebar-row {
  background: transparent;
  border: 1px solid transparent;
}

.glass-sidebar-row:hover {
  background: alpha(@accent_bg_color, 0.12);
  border-color: alpha(@accent_bg_color, 0.20);
}

.glass-sidebar-row:selected {
  background: alpha(@accent_bg_color, 0.18);
  border-color: alpha(@accent_bg_color, 0.32);
  box-shadow: 0 2px 8px alpha(black, 0.16);
}

.settings-dialog-content .boxed-list {
  background: alpha(@card_bg_color, 0.72);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
  border-radius: 16px;
}

row.settings-action-row:hover {
  background: alpha(@window_fg_color, 0.06);
}

.settings-background-blur {
  opacity: 0.72;
}

.settings-dialog-backdrop .background {
  background: alpha(@window_bg_color, 0.88);
  border-color: alpha(@window_fg_color, 0.10);
  color: @window_fg_color;
}

.settings-dialog-backdrop {
  background: alpha(@window_bg_color, 0.34);
}

.settings-about-text {
  margin-top: 8px;
  font-size: 0.86em;
  opacity: 0.56;
}

/* viewer-overlay-nav capsule is bare in plain mode too — see liquid block. */
.viewer-overlay-nav {
  background: transparent;
  border: none;
  box-shadow: none;
}

.viewer-overlay-nav-btn {
  background: alpha(@window_bg_color, 0.52);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
}

.viewer-overlay-nav-btn:hover {
  background: alpha(@window_bg_color, 0.64);
  border-color: alpha(@window_fg_color, 0.16);
}

/* ── Viewer chrome: glass material only on hover/focus (plain mode) ──
   Mirrors the liquid block but with the calmer plain material: viewer chrome
   buttons (the header) and overlay nav arrows are bare at rest and gain a
   translucent fill on hover/focus. No blur, no inset highlight, no heavy
   shadow — same restrained treatment as the other plain buttons. Scoped to
   .viewer-chrome (see liquid block). */
.viewer-chrome .glass-toolbar-button,
.viewer-overlay-nav-btn {
  background: transparent;
  background-clip: padding-box;
  border-color: transparent;
  box-shadow: none;
}

.viewer-chrome .glass-toolbar-button:hover,
.viewer-chrome .glass-toolbar-button:focus-visible,
.viewer-overlay-nav-btn:hover,
.viewer-overlay-nav-btn:focus-visible {
  background: alpha(@window_bg_color, 0.64);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.16);
}

.viewer-chrome .glass-toolbar-button:active,
.viewer-chrome .glass-toolbar-button:checked {
  background: alpha(@window_bg_color, 0.74);
  border: 1px solid alpha(@window_fg_color, 0.20);
}

/* Delete keeps its red danger treatment on hover inside the viewer. */
.viewer-chrome .glass-toolbar-button.glass-toolbar-danger:hover {
  background: alpha(#ff5449, 0.18);
  border: 1px solid alpha(#ffb4ab, 0.28);
  color: #ffb4ab;
}

/* ── Sidebar settings button: glass only on hover/focus (plain mode) ──
   Mirrors the liquid block with the calmer plain material: bare icon at rest,
   translucent fill on hover/focus. No blur, no inset highlight, no heavy
   shadow. Scoped to .sidebar-settings-button (see liquid block). */
.sidebar-settings-button {
  background: transparent;
  background-clip: padding-box;
  border-color: transparent;
  box-shadow: none;
}

.sidebar-settings-button:hover,
.sidebar-settings-button:focus-visible {
  background: alpha(@window_bg_color, 0.64);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.16);
}

.sidebar-settings-button:active {
  background: alpha(@window_bg_color, 0.74);
  border: 1px solid alpha(@window_fg_color, 0.20);
}

/* Favorite-active has no surface material here either — heart icon color
   only (see BASE_CSS). */

.glass-alert-dialog {
  background: transparent;
}

.glass-alert-dialog .background {
  background: alpha(@window_bg_color, 0.88);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
  border-radius: 20px;
  box-shadow: 0 8px 24px alpha(black, 0.30);
  color: @window_fg_color;
}

.glass-alert-dialog .title-2 {
  font-weight: 700;
  color: @window_fg_color;
}

.glass-alert-dialog .body {
  color: alpha(@window_fg_color, 0.72);
}

.glass-alert-dialog button.text-button {
  min-height: 38px;
  border-radius: 12px;
  padding: 0 18px;
  background: alpha(@window_bg_color, 0.62);
  border: 1px solid alpha(@window_fg_color, 0.10);
  color: @window_fg_color;
  font-weight: 600;
}

.glass-alert-dialog button.text-button:hover {
  background: alpha(@window_bg_color, 0.74);
  border-color: alpha(@window_fg_color, 0.18);
}

.glass-alert-dialog button.text-button:active {
  background: alpha(@window_bg_color, 0.82);
}

.glass-alert-dialog button.destructive-action {
  color: #ffb4ab;
}

.glass-alert-dialog button.destructive-action:hover {
  background: alpha(#ff5449, 0.22);
  border-color: alpha(#ff5449, 0.40);
}

.glass-header {
  background: alpha(@window_bg_color, 0.78);
  background-clip: padding-box;
  border-bottom: 1px solid alpha(@window_fg_color, 0.08);
}

.viewer-details-panel {
  background: alpha(@window_bg_color, 0.82);
  background-clip: padding-box;
  border-left: 1px solid alpha(@window_fg_color, 0.08);
}

/* viewer-floating-panel — 详情面板浮层(普通模式):无模糊,更深半透明底 + 轻投影。 */
.viewer-floating-panel {
  margin: 12px;
  margin-left: 0;
  border-radius: 12px;
  background: alpha(@window_bg_color, 0.84);
  background-clip: padding-box;
  border: 1px solid alpha(@window_fg_color, 0.10);
  box-shadow: 0 4px 14px alpha(black, 0.30);
}
";

/* GTK's CssProvider in the supported runtime rejects web-style @media
feature queries. Keep this hook empty until accessibility adaptation is
implemented through GTK settings or explicit runtime class toggles. */
const A11Y_CSS: &str = "";

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

    #[test]
    fn current_viewer_thumbnail_is_prominently_enlarged() {
        let css = build_css(true);
        assert!(
            css.contains(".viewer-thumb-item.viewer-thumb-current"),
            "CSS must define the current filmstrip thumbnail state",
        );
        assert!(
            css.contains("transform: scale(1.30)"),
            "current filmstrip thumbnail should be prominently enlarged",
        );
        assert!(
            css.contains("opacity: 0.42"),
            "non-current filmstrip thumbnails should be visually de-emphasized",
        );
        assert!(
            css.contains("opacity: 1.0"),
            "current filmstrip thumbnail should stay fully bright",
        );
        assert!(
            css.contains("margin-left: 12px") && css.contains("margin-right: 12px"),
            "current filmstrip thumbnail should reserve fixed side breathing room",
        );
        assert!(
            css.contains("background: alpha(@window_fg_color, 0.10)"),
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
}
