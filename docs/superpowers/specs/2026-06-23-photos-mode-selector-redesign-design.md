# Photos-page 年/月/日 模式选择器重构 — 设计规范

**日期：** 2026-06-23
**状态：** 设计中（等待用户审阅）
**范围：** `PhotosPage` 内 `AdwViewSwitcherBar` 的位置、视觉与交互重构。

## 背景 / 问题

当前 `PhotosPage` 在 `AdwHeaderBar` 与 `AdwViewStack` 之间放置一个 `AdwViewSwitcherBar`
（见 `data/ui/photos-page.blp:14-17`、`src/ui/photos_page.rs:104-106`）。
它有三个问题：

1. **位置在顶部**：占据了 `Photos` 标题下方的固定条带，与缩略图内容区争夺屏幕空间。
2. **样式突兀**：Adwaita 默认用系统强调色（Fedora 默认为红色）填充激活按钮背景形成 pill 视觉，与暗色主题下的整体观感冲突。
3. **缺乏层次感**：默认字号约 10pt，与下方的 section header（heading）相比显得局促。

## 目标

1. 将年/月/日选择器从顶部移动到显示区域**底部**并以**悬浮覆盖**形式呈现，不占据缩略图区域。
2. 容器使用 **~50% 透明度** 的圆角面板背景，使缩略图内容透过可见。
3. 标签**只有文字**——移除 Adwaita 的红色 pill 背景。
4. 标签**字体略大**到 title-3 尺寸（约 14pt），与 section header 形成清晰的视觉层次。
5. 激活态用**激活点（dot）**指示——一个小色块（accent color）位于激活标签正下方。
6. 标签本身**未激活时半透明**（opacity ≈ 0.55），激活时全亮。
7. 保持**键盘导航**（←/→ 切换模式）——原有功能不可退化。

## 非目标

- 不修改 `MediaGrid` 的 tile 尺寸、分组逻辑、滚动行为。
- 不修改 `TrashPage` / `AlbumDetailPage` / `AlbumsPage` 的选择器（这些页没有年/月/日选择器）。
- 不修改 Viewer、Editor、扫描、缩略图、DB 等下游功能。
- 不引入主题切换 / 颜色定制。accent color 仍由系统决定。
- 不增加新的快捷键（保留 ←/→ 即可）。

## 方案

### 架构

```
PhotosPage (AdwNavigationPage)
└── GtkOverlay (新结构)
    ├── GtkViewStack view_stack         ← 已存在，3 个 MediaGrid 子页
    └── ModeSelector (新 widget)        ← 悬浮在网格底部
        └── GtkBox vertical (css: mode-selector)
            ├── row (GtkBox horizontal, halign: center)
            │   ├── label_cell "年"   (GtkBox css: mode-cell, 固定宽, GestureClick)
            │   ├── label_cell "月"   (GtkBox css: mode-cell, 固定宽, GestureClick)
            │   └── label_cell "日"   (GtkBox css: mode-cell, 固定宽, GestureClick)
            └── dot_row (GtkBox horizontal, halign: center, valign: start, 高 6px)
                ├── dot_cell (GtkBox css: mode-cell, 固定宽)
                │   └── dot_inner (GtkBox css: mode-dot, 24×4, 默认 visible=false)
                ├── dot_cell (GtkBox css: mode-cell, 固定宽)
                │   └── dot_inner (GtkBox css: mode-dot, 24×4, 默认 visible=false)
                └── dot_cell (GtkBox css: mode-cell, 固定宽)
                    └── dot_inner (GtkBox css: mode-dot, 24×4, 默认 visible=true)
```

`PhotosPage` 整体布局由 `GtkBox { HeaderBar, SwitcherBar, ViewStack }` 改为 `GtkOverlay { ViewStack, ModeSelector }`。
`AdwHeaderBar` 保留（窗口控制 + 标题仍工作），**只是不再被 SwitcherBar 占用下方的空间**。

### 组件

**`ModeSelector` widget** —— 新文件 `src/ui/mode_selector.rs`，沿用 `PhotoTile` / `MediaGrid` 的 `glib::wrapper!` 模板子类的模式。

公开 API：
- `pub fn new() -> Self` —— 构造一个空的选择器（stack 由 `set_stack` 注入）。
- `pub fn set_stack(&self, stack: &adw::ViewStack)` —— 注入 ViewStack 引用并连接 `notify::visible-child` 信号以同步激活索引。
- `pub fn active_index(&self) -> u32` —— 返回当前激活索引（0=年, 1=月, 2=日）。
- `pub fn set_active_index(&self, idx: u32)` —— 设置激活索引（内部会同步更新 ViewStack 可见子项）。

模板：
- `#[template(file = "../../data/ui/mode-selector.ui")]`
- 三个 label cell 和三个 dot cell 共享 `GtkSizeGroup(horizontal)`，保证 dot 始终精确落在激活标签正下方。

**`ModeSelector` 内部行为**：
- 3 个 label 上挂 `GtkGestureClick`：点击触发 `set_active_index(i)`。
- 顶层 widget 上挂 `EventControllerKey`：← / → 切换相邻模式。
- `set_active_index(i)` 做三件事：
  1. 移除所有 label 的 `active` CSS class，给索引为 `i` 的 label 加上。
  2. 隐藏所有 `dot_inner`，给索引为 `i` 的 `dot_inner` 设 `visible=true`（dot 视觉位置不动——它在自己的 cell 里，靠 visibility 切换显隐）。
  3. 调用 `stack.set_visible_child_name(["year", "month", "day"][i])`。
- ViewStack 可见子项改变时（来自外部或自身）→ 通过 `notify::visible-child` 回调同步内部 active_index，**避免循环触发**（通过比较上次同步的值来短路）。

### 视觉

CSS 追加到 `src/ui/grid_css.rs` 的 `GRID_CSS` 常量末尾：

```css
/* ModeSelector 容器：~50% 透明圆角面板 */
box.mode-selector {
  background: alpha(@card_bg_color, 0.5);
  border-radius: 12px;
  padding: 8px 16px;
  margin: 0 24px 24px 24px;
}

/* 单个标签 / dot 槽位：固定宽，dot 与 label 共享 size-group 同宽 */
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

**ModeSelector 自身的布局定位**（在 `photos-page.ui` 中设置）：
- `halign = GtkAlign::Center`
- `valign = GtkAlign::End`
- `margin-bottom = 24`
- 不设置 `vexpand/hexpand`——保持原始请求大小，浮在 Overlay 底部

### 数据流

```
用户点击 "月"
  → ModeSelector 内的 GestureClick handler
  → set_active_index(1)
    → 更新 label 的 active CSS class
    → 隐藏非激活 dot_inner，显示索引 1 的 dot_inner
    → stack.set_visible_child_name("month")
      → ViewStack 触发 notify::visible-child
        → ModeSelector connected handler
          → 比较上次同步值（短路，无副作用）

用户按 → 键（ModeSelector 处于焦点状态时）
  → EventControllerKey handler
  → active = (active + 1) % 3
  → set_active_index(new_active)
  → （后续同上）

用户点击缩略图（既有流程，不变）
  → MediaGrid on_activate → PhotosPage::open_viewer → push ViewerPage
  → ModeSelector 不受影响（它只是覆盖在网格上方，不拦截点击事件，除非 ModeSelector 自身处于焦点）
```

**单一数据源原则**：ModeSelector 内部 `active_index` 为单一数据源；ViewStack 的可见子项是其下游表现。
**反向同步保留**（notify::visible-child → 内部 active_index）以保证未来如果有任何其他代码或快捷键直接改变 ViewStack，ModeSelector 也能跟上。

### 错误处理 / 边界

- **空列表**：`is_empty` 时 `view_stack` 显示 `empty_states::no_photos()`，ModeSelector 仍浮在最上层（覆盖在空态之上）。空态下用户切换年/月/日仍有效（虽然没照片可看）。可接受行为。
- **键盘冲突**：ModeSelector 接管 ←/→；MediaGrid 也接管 ←/→。两个 widget 通过焦点互斥：
  - ModeSelector 自身是 focusable widget（点击或 Tab 触发）。
  - 默认情况下 `ViewStack` 的 `focusable` 链指向 MediaGrid；点击缩略图区域 ModeSelector 失焦。
  - 反之亦然：点击 ModeSelector 上某个 label，整个 stack 焦点链切换到 ModeSelector。
  - 因此同一时刻只有一个组件消费 ←/→。
- **视图栈 title 保留**：`stack.add_titled(&grid, Some("year"), "年")` 仍然调用——title 不再驱动 UI 渲染，但保留用于无障碍读屏（screen reader 会读出 "year view" 而非 "年" 文本标签）。
- **多次实例化**：`PhotosPage::new` 一次构造一个 `ModeSelector` 并绑定到一个 `ViewStack`。当前 codebase 只有 `PhotosPage` 使用这个模式选择器，不存在多实例冲突。

## 设计属性

### 性能 / 渲染

- 12 个子节点（3 label cell + 3 dot cell + 容器）是非常轻量的 widget 树，每次重建（`PhotosPage::new`）一次性创建，无运行时重分配。
- `transition: opacity 120ms ease` 在 label 状态切换时产生 120ms 过渡动画——CSS 引擎原生支持，不引入额外帧循环。
- 不使用 `GtkCustomLayout` 或自定义 `WidgetImpl::measure`——沿用默认 `GtkBox` 测量，避免触发 gtk4-gridview-thumbnail-sizing-pitfall 类似的坑。

### 可访问性

- 三个 label 都可被 screen reader 聚焦并播报（"年" / "月" / "日"）。
- 激活 label 加上 `active` CSS class，AT-SPI 可读出 `ACCESSIBLE_STATE_SELECTED`（通过 CSS class → 状态映射）。
- ←/→ 键盘导航与现有 MediaGrid 行为一致。

## 验证

按 CLAUDE.md / CONTRIBUTING.md 的 TDD 约定：

### 单元测试（`src/ui/mode_selector.rs` 内 `#[cfg(test)]` 模块）

1. `set_active_index` 改变 `active_index()` 返回值。
2. 三个 label cell 通过 `GtkSizeGroup` 保持同宽——断言 `size_group` 非空且包含 6 个 widget。
3. ViewStack 同步：手动 `stack.set_visible_child_name("month")` 触发 `notify::visible-child`，`active_index()` 应变为 1。
4. 循环短路：连续两次 `set_visible_child_name("month")` 不会引起 active_index 的多余 set。

### 集成测试（`tests/ui_mode_selector.rs`）

1. 构建 `PhotosPage` 后，断言 `view_stack` 的父节点是 `GtkOverlay`（不是 `GtkBox`）。
2. 断言 ModeSelector 自身是 `view_stack` 的兄弟节点（在同一个 Overlay 下）。
3. 断言 ModeSelector 的 `halign == Center`、`valign == End`。
4. 模拟点击 label "日"，断言 `view_stack.visible_child_name() == Some("day")`。
5. 测试 widget 的尺寸约束：ModeSelector 本身不设置 `vexpand/hexpand`（不强占空间）。

### 视觉验证（`/verify` skill）

1. 启动应用，截图确认 selector 浮在网格底部、水平居中。
2. 切换年/月/日，确认 dot 跟随移动。
3. 窗口缩放，selector 始终水平居中、不超出底部 24px 边距。
4. 鼠标悬停到 selector 上方时，背景仍为半透明（不被 hover 状态破坏）。
5. 暗色主题下视觉观感与现有主题一致（无突兀的红色）。

## 风险

低。改动局限在 `PhotosPage` 内部：

- `GtkOverlay` 是 GTK 标准 widget，已在 codebase 其它地方（无）使用过，但 API 简单（`add_overlay`）。
- `ModeSelector` 是新 widget，但模板模式（`glib::wrapper!` + `CompositeTemplate`）已在 `MediaGrid` / `PhotoTile` / `PhotosPage` / `Window` 验证。
- CSS 改动通过 `grid_css::install()` 注入到全局 `CssProvider`——与现有 `flowbox.thumb-grid` 等规则并存，无冲突。
- 键盘焦点链变更（ModeSelector 现在能 focus）属于功能增加，不是回归。

## 涉及文件

新增：
- `data/ui/mode-selector.blp`（Blueprint 源）
- `data/ui/mode-selector.ui`（编译产物，由 build.rs 生成）
- `src/ui/mode_selector.rs`（新 widget + 单元测试）
- `tests/ui_mode_selector.rs`（集成测试）

修改：
- `data/ui/photos-page.blp` + `data/ui/photos-page.ui`：Box → Overlay，移除 SwitcherBar，添加 ModeSelector 子节点
- `src/ui/photos_page.rs`：移除 `switcher_bar` TemplateChild，添加 `mode_selector` 字段，修改 `new()` 构造
- `src/ui/mod.rs`：导出 `ModeSelector`
- `src/ui/grid_css.rs`：在 `GRID_CSS` 末尾追加 mode-selector / mode-dot 规则
