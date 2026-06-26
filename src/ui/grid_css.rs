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
  background: alpha(white, 0.08);
  border-color: alpha(white, 0.18);
}

/* Keyboard focus — glass ring. Hidden once the pointer leaves the grid
   (see .pointer-left rule below) so the ring doesn't linger on the
   last-hovered tile after the mouse moves away. */
flowbox.thumb-grid > flowboxchild:focus > .glass-thumb-card {
  outline: 2px solid alpha(white, 0.55);
  outline-offset: 2px;
}

/* Selected — luminous glass border + soft inner veil. Glass alone
   communicates the selected state; no accent colour needed. */
flowbox.thumb-grid > flowboxchild:selected > .glass-thumb-card {
  background: alpha(white, 0.10);
  border-color: alpha(white, 0.48);
  box-shadow:
    inset 0 1px alpha(white, 0.35);
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
   写在被 hover / focus / selected 的 flowboxchild 下面时会同时命中。
   使用玻璃质感的白色描边,不使用系统强调色。 */
flowbox.album-grid > flowboxchild:hover .album-cover,
flowbox.album-grid > flowboxchild:focus .album-cover {
  outline: 2px solid alpha(white, 0.48);
  outline-offset: -2px;
}
flowbox.album-grid > flowboxchild:selected .album-cover,
flowbox.album-grid > flowboxchild:selected:focus .album-cover {
  outline: 3px solid alpha(white, 0.48);
  outline-offset: -3px;
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

/* 标签：默认半透明、title-3 字号 */
box.mode-selector label,
.glass-segment-label {
  font-size: 14pt;
  font-weight: 700;
  color: #ffffff;
  opacity: 0.55;
  transition: opacity 120ms ease;
}

box.mode-selector.on-light-background label,
.glass-segmented.on-light-background .glass-segment-label {
  color: #000000;
}

/* 激活态：标签全亮 */
box.mode-selector label.active,
.glass-segment-label.active {
  opacity: 1.0;
}

/* 激活指示点 */
box.mode-dot,
.glass-segment-indicator {
  background: #ffffff;
  border-radius: 2px;
  min-width: 24px;
  min-height: 4px;
  margin-top: 2px;
}

box.mode-selector.on-light-background box.mode-dot,
.glass-segmented.on-light-background .glass-segment-indicator {
  background: #000000;
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

/* glass-sidebar — the left rail surface. The sidebar's blur comes from
   .glass-base applied alongside (see window.blp); these rules only own
   row shape + hover/selected state. */
.glass-sidebar {
  padding: 12px;
  border-top: 0;
  border-bottom: 0;
  border-left: 0;
  border-right: 1px solid alpha(white, 0.14);
}

.glass-sidebar-page {
  background: transparent;
}

.glass-sidebar-row {
  min-height: 40px;
  border-radius: 12px;
  margin-bottom: 6px;
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

/* glass-editor-preview — analogous to .viewer-stage, but calmer: the
   editor's adjustment sliders occupy the same screen and need every
   ounce of readable chrome, so this is a near-flat panel with a hairline
   border, not a heavy glass stage.
   比 viewer-stage 更克制:编辑器界面与调节滑块同屏,需要尽可能多的可读
   chrome,所以这是近乎平坦的面板加一道细线边框,而不是沉重的玻璃舞台。 */
.glass-editor-preview {
  padding: 24px;
  background: alpha(black, 0.06);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.06);
}

/* Viewer favorite button active state. Class is added/removed by
   ViewerPage::refresh_favorite_button; the visual now lives in the
   global provider so it composes with .glass-toolbar-button. */
.viewer-favorite-btn.favorite-active {
  color: #f6c344;
}

/* hover keeps the gold theme — without this rule, .glass-toolbar-button:hover
   would replace the gold background with a generic alpha(white, 0.14),
   so the active-favorite would briefly look un-favorited on pointer-over. */
.viewer-favorite-btn.favorite-active:hover {
  color: #ffd86b;
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

/* thumb-loading — 缩略图生成期间的静态占位。缩略图到位后 SquareTile
   在 set_paintable 里移除该 class。 */
.thumb-loading {
  background-color: alpha(white, 0.05);
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
  background: alpha(white, 0.10);
  outline: 2px solid alpha(white, 0.55);
  outline-offset: 2px;
  box-shadow:
    inset 0 1px alpha(white, 0.35),
    0 0 0 4px alpha(white, 0.12),
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
  border: 2px solid alpha(white, 0.48);
  box-shadow: 0 0 18px alpha(white, 0.20);
}

/* viewer-overlay-nav — previous/next controls floating over the image. The
   material is mode-specific so it follows the Settings Liquid Glass switch. */
.viewer-overlay-nav {
  padding: 6px;
  border-radius: 16px;
}

.viewer-overlay-nav-btn {
  min-width: 42px;
  min-height: 38px;
  padding: 0;
  border-radius: 10px;
  color: #ffffff;
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
";

/* ── LIQUID_GLASS_MATERIAL_CSS ─ the dramatic Liquid Glass material:
backdrop blur+saturate+brightness, bright inset top highlights, and heavy
floating shadows. This is the default (opt-out) look. */
const LIQUID_GLASS_MATERIAL_CSS: &str = "
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

/* glass-menu popover inner surface */
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

/* Unified Liquid Glass button material. These selectors intentionally cover
   all button-like chrome so the Settings switch changes the whole language. */
.glass-toolbar-button,
.glass-header windowcontrols button image {
  background: alpha(white, 0.12);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.28);
  box-shadow:
    0 12px 32px alpha(black, 0.24),
    inset 0 1px alpha(white, 0.44),
    inset 0 -1px alpha(black, 0.12);
}

.glass-toolbar-button:hover,
.glass-header windowcontrols button:hover image {
  background: alpha(white, 0.18);
  border-color: alpha(white, 0.38);
  box-shadow:
    0 14px 36px alpha(black, 0.30),
    inset 0 1px alpha(white, 0.52),
    inset 0 -1px alpha(black, 0.14);
}

.glass-toolbar-button:active,
.glass-toolbar-button:checked,
.glass-header windowcontrols button:active image,
.glass-header windowcontrols button:checked image {
  background: alpha(white, 0.24);
  border-color: alpha(white, 0.44);
  box-shadow:
    0 8px 22px alpha(black, 0.24),
    inset 0 1px alpha(white, 0.34),
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
  background: alpha(white, 0.05);
  background-clip: padding-box;
  border: 1px solid transparent;
}

.glass-menu-item:hover {
  background: alpha(white, 0.16);
  border-color: alpha(white, 0.26);
  box-shadow:
    inset 0 1px alpha(white, 0.36),
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

.glass-sidebar-row {
  background: transparent;
  border: 1px solid transparent;
}

.glass-sidebar-row:hover {
  background: alpha(white, 0.10);
  border-color: alpha(white, 0.18);
  box-shadow:
    inset 0 1px alpha(white, 0.28);
}

.glass-sidebar-row:selected {
  background: alpha(white, 0.16);
  border-color: alpha(white, 0.28);
  box-shadow:
    inset 0 1px alpha(white, 0.36),
    inset 0 -1px alpha(black, 0.12);
}

.viewer-overlay-nav {
  background: alpha(black, 0.24);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.30);
  box-shadow:
    0 16px 42px alpha(black, 0.34),
    inset 0 1px alpha(white, 0.36),
    inset 0 -1px alpha(black, 0.16);
}

.viewer-overlay-nav-btn {
  background: alpha(white, 0.13);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.26);
  box-shadow:
    inset 0 1px alpha(white, 0.36),
    inset 0 -1px alpha(black, 0.10);
}

.viewer-overlay-nav-btn:hover {
  background: alpha(white, 0.22);
  border-color: alpha(white, 0.38);
}

.viewer-favorite-btn.favorite-active {
  background: alpha(#f6c344, 0.16);
  border-color: alpha(#f6c344, 0.42);
  box-shadow:
    0 10px 28px alpha(black, 0.22),
    inset 0 1px alpha(white, 0.34),
    inset 0 -1px alpha(#6b4b00, 0.20);
}

.viewer-favorite-btn.favorite-active:hover {
  background: alpha(#f6c344, 0.24);
  border-color: alpha(#ffd86b, 0.56);
}

/* ── Glass alert dialog — 毛玻璃半透明弹框 + 液态玻璃按钮 ──────────────
   AdwAlertDialog 的 CSS 类加在最外层节点(1200x800 填满窗口)，
   可见卡片是深层后代 AdwGizmo.background(约 300x178)。
   因此根节点保持透明，毛玻璃材质放到 .background 上。 */
.glass-alert-dialog {
  background: transparent;
}

.glass-alert-dialog .background {
  background: alpha(black, 0.48);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.24);
  border-radius: 20px;
  backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
  box-shadow:
    0 24px 64px alpha(black, 0.40),
    inset 0 1px alpha(white, 0.28);
  color: #ffffff;
}

.glass-alert-dialog .title-2 {
  font-weight: 700;
  color: #ffffff;
}

.glass-alert-dialog .body {
  color: alpha(white, 0.72);
}

/* Response buttons — 液态玻璃 pill 风格 */
.glass-alert-dialog button.text-button {
  min-height: 38px;
  border-radius: 12px;
  padding: 0 18px;
  background: alpha(white, 0.10);
  border: 1px solid alpha(white, 0.22);
  color: #ffffff;
  font-weight: 600;
  transition: background 120ms ease, border-color 120ms ease;
}

.glass-alert-dialog button.text-button:hover {
  background: alpha(white, 0.18);
  border-color: alpha(white, 0.36);
}

.glass-alert-dialog button.text-button:active {
  background: alpha(white, 0.26);
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
  background: alpha(black, 0.18);
  background-clip: padding-box;
  border-bottom: 1px solid alpha(white, 0.08);
  backdrop-filter: blur(20px) saturate(1.10) brightness(1.02);
}

/* viewer-details-panel — metadata sidebar uses glass-base, not opaque. */
.viewer-details-panel {
  background: alpha(black, 0.30);
  background-clip: padding-box;
  border-left: 1px solid alpha(white, 0.12);
  backdrop-filter: blur(22px) saturate(1.12);
}

/* viewer-floating-panel — 详情面板浮层:浮在原图之上(overlay),不再挤占。
   半透明深底 + 液态模糊 + 圆角 + 悬浮投影;margin 让其脱离边缘呈悬浮卡片。 */
.viewer-floating-panel {
  margin: 12px;
  margin-left: 0;
  border-radius: 16px;
  background: alpha(black, 0.32);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.20);
  backdrop-filter: blur(26px) saturate(1.20) brightness(1.05);
  box-shadow:
    0 14px 44px alpha(black, 0.42),
    inset 0 1px alpha(white, 0.30);
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
  background: alpha(black, 0.55);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.08);
}

.glass-raised {
  background: alpha(black, 0.62);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.10);
  box-shadow: 0 4px 12px alpha(black, 0.22);
}

.glass-menu > contents {
  padding: 6px;
  border-radius: 16px;
  background: alpha(black, 0.70);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.10);
  box-shadow: 0 6px 18px alpha(black, 0.28);
}

.glass-toolbar-button,
.glass-header windowcontrols button image {
  background: alpha(white, 0.07);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.10);
  box-shadow: 0 3px 10px alpha(black, 0.18);
}

.glass-toolbar-button:hover,
.glass-header windowcontrols button:hover image {
  background: alpha(white, 0.12);
  border-color: alpha(white, 0.16);
}

.glass-toolbar-button:active,
.glass-toolbar-button:checked,
.glass-header windowcontrols button:active image,
.glass-header windowcontrols button:checked image {
  background: alpha(white, 0.16);
  border-color: alpha(white, 0.20);
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
  background: alpha(white, 0.11);
  border-color: alpha(white, 0.12);
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

.glass-sidebar-row {
  background: transparent;
  border: 1px solid transparent;
}

.glass-sidebar-row:hover {
  background: alpha(white, 0.07);
  border-color: alpha(white, 0.08);
}

.glass-sidebar-row:selected {
  background: alpha(white, 0.12);
  border-color: alpha(white, 0.14);
  box-shadow: 0 2px 8px alpha(black, 0.16);
}

.viewer-overlay-nav {
  background: alpha(black, 0.46);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.12);
  box-shadow: 0 6px 18px alpha(black, 0.28);
}

.viewer-overlay-nav-btn {
  background: alpha(white, 0.08);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.10);
}

.viewer-overlay-nav-btn:hover {
  background: alpha(white, 0.14);
  border-color: alpha(white, 0.16);
}

.viewer-favorite-btn.favorite-active {
  background: alpha(#f6c344, 0.12);
  border-color: alpha(#f6c344, 0.28);
  box-shadow: 0 3px 10px alpha(black, 0.18);
}

.viewer-favorite-btn.favorite-active:hover {
  background: alpha(#f6c344, 0.18);
  border-color: alpha(#f6c344, 0.38);
}

.glass-alert-dialog {
  background: transparent;
}

.glass-alert-dialog .background {
  background: alpha(black, 0.72);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.10);
  border-radius: 20px;
  box-shadow: 0 8px 24px alpha(black, 0.30);
  color: #ffffff;
}

.glass-alert-dialog .title-2 {
  font-weight: 700;
  color: #ffffff;
}

.glass-alert-dialog .body {
  color: alpha(white, 0.72);
}

.glass-alert-dialog button.text-button {
  min-height: 38px;
  border-radius: 12px;
  padding: 0 18px;
  background: #2a2a30;
  border: 1px solid alpha(white, 0.10);
  color: #ffffff;
  font-weight: 600;
}

.glass-alert-dialog button.text-button:hover {
  background: #36363e;
  border-color: alpha(white, 0.18);
}

.glass-alert-dialog button.text-button:active {
  background: #40404a;
}

.glass-alert-dialog button.destructive-action {
  color: #ffb4ab;
}

.glass-alert-dialog button.destructive-action:hover {
  background: alpha(#ff5449, 0.22);
  border-color: alpha(#ff5449, 0.40);
}

.glass-header {
  background: alpha(black, 0.50);
  background-clip: padding-box;
  border-bottom: 1px solid alpha(white, 0.08);
}

.viewer-details-panel {
  background: alpha(black, 0.60);
  background-clip: padding-box;
  border-left: 1px solid alpha(white, 0.08);
}

/* viewer-floating-panel — 详情面板浮层(普通模式):无模糊,更深半透明底 + 轻投影。 */
.viewer-floating-panel {
  margin: 12px;
  margin-left: 0;
  border-radius: 12px;
  background: alpha(black, 0.62);
  background-clip: padding-box;
  border: 1px solid alpha(white, 0.10);
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
    let material = if liquid_glass {
        LIQUID_GLASS_MATERIAL_CSS
    } else {
        PLAIN_GLASS_MATERIAL_CSS
    };
    format!("{BASE_CSS}\n{material}\n{A11Y_CSS}")
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
    // MediaGrid / TrashPage / AlbumsPage constructors do not accumulate
    // duplicate CssProviders on the default display.
    if CSS_INSTALLED.set(()).is_ok() {
        register(&build_css(crate::core::prefs::liquid_glass_enabled()));
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
    register(&build_css(liquid_glass));
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

    /// `.viewer-favorite-btn.favorite-active:hover` must exist alongside the
    /// base `.viewer-favorite-btn.favorite-active` rule. Without the :hover
    /// override, `.glass-toolbar-button:hover` wins (same specificity, defined
    /// later in the source so it would override) and the gold favorite state
    /// would briefly look un-favorited on pointer-over.
    /// 没有 hover 规则时,鼠标悬停在已收藏的星标按钮上会丢失金色高亮。
    #[test]
    fn favorite_active_has_hover_override() {
        let css = build_css(true);
        assert!(
            css.contains(".viewer-favorite-btn.favorite-active"),
            "CSS must define the base .viewer-favorite-btn.favorite-active rule",
        );
        assert!(
            css.contains(".viewer-favorite-btn.favorite-active:hover"),
            "CSS must define a :hover override so the gold state survives pointer-over",
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
            css.contains("background: alpha(white, 0.10)"),
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
            css.contains("border: 2px solid alpha(white, 0.48)"),
            "current filmstrip thumbnail image should use the same glass ring as grid selection",
        );
        assert!(
            css.contains("outline: 2px solid alpha(white, 0.55)"),
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
            css.contains("inset 0 1px alpha(white, 0.58)"),
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
            !css.contains("inset 0 1px alpha(white, 0.58)"),
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
            ".viewer-overlay-nav",
            ".viewer-overlay-nav-btn",
            ".viewer-favorite-btn.favorite-active",
        ] {
            assert!(
                css.contains(selector),
                "liquid mode missing button material selector {selector}",
            );
        }

        for liquid_signature in [
            "inset 0 1px alpha(white, 0.44)",
            "inset 0 1px alpha(white, 0.36)",
            "0 12px 32px alpha(black, 0.24)",
            "0 16px 42px alpha(black, 0.34)",
        ] {
            assert!(
                css.contains(liquid_signature),
                "liquid mode missing shared button material signature {liquid_signature}",
            );
        }
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
            "inset 0 1px alpha(white, 0.58)",
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
            ".viewer-overlay-nav",
            ".viewer-overlay-nav-btn",
            ".viewer-favorite-btn.favorite-active",
        ] {
            assert!(
                css.contains(selector),
                "plain mode missing button selector {selector}",
            );
        }

        for liquid_signature in [
            "inset 0 1px alpha(white, 0.44)",
            "inset 0 1px alpha(white, 0.36)",
            "0 12px 32px alpha(black, 0.24)",
            "0 16px 42px alpha(black, 0.34)",
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
