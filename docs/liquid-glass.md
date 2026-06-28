# 液态玻璃（Liquid Glass）

> 全应用玻璃材质系统。年/月/日 切换器是最早的落地实例，后来统一到侧边栏、顶栏、
> 菜单、弹框、详情面板等所有 chrome 表面。**液态玻璃是可开关的特效**：默认开启，
> 用户可在「设置 → 外观」关闭，关闭时降级为普通半透明（无模糊）。

参考实现：[`shuding/liquid-glass`](https://github.com/shuding/liquid-glass)

## 当前实现

`PhotosPage` 底部居中悬浮的 **年 / 月 / 日** 切换器是当前玻璃风格的基准控件。
它的关键不是把每个按钮都画成独立玻璃块，而是：

- 外层是一个整体的 `.glass-raised` 玻璃胶囊；
- 内部是等宽分段槽位；
- 当前状态只通过 label 亮度和底部短横线表达；
- 不给单个分段额外绘制 active 背景块。

实现路径是：

1. `data/ui/mode-selector.blp` 给根节点同时挂 `mode-selector` 和 `glass-raised`。
2. `src/ui/grid_css.rs` 中的 `.glass-raised` 提供半透明填充、边框、内高光、投影和：

```css
backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
```

3. `box.mode-selector` 只保留尺寸、圆角和内边距；不再自己绘制材质。
4. `ModeSelector` 不重写 `snapshot`。背景采样、模糊和增强交给 GTK/GSK 的
   `backdrop-filter` 节点处理。

这意味着玻璃效果依赖支持 `backdrop-filter` 的 GTK 运行时。普通系统 GTK 如果还不支持
该属性，会打印 `No property named "backdrop-filter"` 的 parser warning，并退化为半透明
填充、边框和阴影；Flatpak/GNOME 新运行时才是目标验证环境。当前项目选择保留
`backdrop-filter`，因为这是现有视觉效果最接近 Liquid Glass 的实现。

GTK `CssProvider` 在当前支持环境里会拒绝 Web 风格的 `@media` feature query 和
`@keyframes`。因此 `A11Y_CSS` 当前保持为空；后续无障碍适配应通过 GTK 设置或显式运行时
CSS class 实现，不要重新加入 `@media` / `@keyframes`。

## 双模式：液态玻璃 / 普通半透明（可开关）

液态玻璃是用户可关闭的特效。`src/ui/grid_css.rs` 把整份 CSS 拆成 4 段，按当前偏好拼装：

| 常量 | 作用 | 是否随开关变化 |
|---|---|---|
| `BASE_CSS` | 布局/状态规则（尺寸、圆角、`:hover`/`:selected` 等） | 共享 |
| `LIQUID_GLASS_MATERIAL_CSS` | **开**：`backdrop-filter: blur() saturate() brightness()` + 顶部内高光 + 厚重悬浮投影 | 仅「开」 |
| `PLAIN_GLASS_MATERIAL_CSS` | **关**：普通半透明填充，**无 `backdrop-filter`**（无模糊）+ 细边 + 轻阴影；按钮/控件用不透明实色 | 仅「关」 |
| `A11Y_CSS` | 当前为空；预留给未来通过 GTK 设置或运行时 class 实现的无障碍回退 | 共享 |

- `build_css(liquid_glass: bool)` = `BASE_CSS` + (`LIQUID` 或 `PLAIN`) + `A11Y_CSS`。
- `grid_css::install()` 在启动时按偏好注册一次；`grid_css::reapply(bool)` 在用户拨动开关时
  **实时**替换 display 级 `CssProvider`（先移除旧 provider 再加新的，强制全局 restyle，
  含 popover 与 `AdwAlertDialog`），无需重启。
- `CssProvider` 在 gtk4-rs 里不是 `Send`/`Sync`，无法放进 `static`，所以当前 provider 存在
  `thread_local!` 里（GTK 全在主线程）。
- 偏好持久化在 `~/.config/photoViewer/settings.json` 的 `liquid_glass` 字段，由
  `src/core/prefs.rs` 读写，默认 `true`（opt-out）。
- 同一设置页的透明度滑杆持久化为 `liquid_glass_transparency`，范围 0-100：0 是完整玻璃材质，
  100 是背景完全透明。实现上只让 material `background` alpha 降到 0；`border*` 和
  `box-shadow` 保留很小的可见下限，文字/图标颜色不随透明度缩放。液态
  `backdrop-filter` 的 blur/saturate/brightness 会随透明度连续衰减，最终到
  `blur(0px) saturate(1) brightness(1)`；不要改成 `none`。

## 分段玻璃控件：从年/月/日抽出的通用样式

年/月/日 的风格已经抽成一套通用 CSS 类。其他控件如果需要复用这种“整体玻璃容器 +
内部轻状态”的观感，应使用以下结构：

```text
outer: glass-raised glass-segmented
slot:  glass-segment
text:  glass-segment-label
text(active): glass-segment-label active
indicator: glass-segment-indicator
light background: outer add on-light-background
```

这些通用类和旧的 ModeSelector selector 共享同一套声明：

| 旧 selector | 通用类 | 作用 |
|---|---|---|
| `box.mode-selector` | `.glass-segmented` | 外层胶囊尺寸、圆角、padding |
| `box.mode-cell` | `.glass-segment` | 内部等宽槽位 |
| `box.mode-selector label` | `.glass-segment-label` | 默认半透明文字 |
| `box.mode-selector label.active` | `.glass-segment-label.active` | 激活文字全亮 |
| `box.mode-dot` | `.glass-segment-indicator` | 底部短横线/指示点 |

注意：不要为了复用这套风格给 `ModeSelector` 或其他分段控件新增
`box.mode-cell.active` / `.glass-segment.active` 背景块。这个风格的核心是“整体玻璃底 +
轻量内部状态”，不是“三个独立按钮”。

## 新增玻璃样式时**必须**适配两种模式

> 这是本系统最重要的约定：任何新的玻璃材质都要同时存在于「液态玻璃」和「普通半透明」两个模式里，
> 否则在某个开关状态下会出现裸背景、错位的模糊或残留的玻璃高光。

按优先级处理：

1. **优先复用既有玻璃类**——`.glass-base`、`.glass-raised`、`.glass-header`、`.glass-menu`、
   `.glass-alert-dialog`、`.viewer-details-panel` 都已经是双模式的。直接给 widget 挂类即可，
   不用动 CSS。需要年/月/日这种分段控件时，优先使用 `.glass-segmented` 这一组通用类。
2. **只有需要全新的表面观感时才新增选择器**，并且要同时加进 `LIQUID_GLASS_MATERIAL_CSS`
   **和** `PLAIN_GLASS_MATERIAL_CSS`（同名选择器）：
   - 液态版：`backdrop-filter: blur() saturate() brightness()` + `inset 0 1px alpha(white,…)` 高光 + 厚投影。
   - 普通版：只留半透明 `background: alpha(black, …)`，**不要写 `backdrop-filter`**；
     细边 `1px solid alpha(white, 0.08~0.10)`；轻投影。若是按钮/可点击控件，用**不透明实色**
     （如 `#2a2a30`）而非半透明，避免在普通模式下仍像玻璃。
3. **布局/形状/状态规则**（`padding`、`border-radius`、`min-*`、`:hover`/`:active`/`:selected`）
   放进 `BASE_CSS`，两种模式共享。
4. **永远不要在 `BASE_CSS` 里写 `backdrop-filter`**——模糊是模式专属的。
5. 改完后跑 `grid_css` 里的单测（`liquid_mode_keeps_drama_and_shared_parts`、
   `liquid_mode_selector_keeps_original_glass_raised_material`、
   `segmented_glass_style_is_exposed_as_reusable_css_classes`、
   `plain_mode_is_translucent_no_blur`、`both_modes_share_base_and_a11y`）。**新增了选择器就相应
   扩展断言**，确保它同时出现在两种模式里。

视觉验证：`backdrop-filter` 只有在 GNOME 50 运行时（Flatpak）才真正渲染，普通系统 GTK 可能忽略它。
用 `./run-flatpak.sh` 启动，在「设置 → 外观」里来回拨动开关，对比两种模式下你新增的表面。

## 已放弃方案

曾试过 CPU/GSK 自绘折射方案（捕获 ModeSelector 背景纹理，逐像素位移/饱和/亮度后在
`ModeSelector::snapshot` 里画回）。已放弃、不应恢复：滚动与缩略图加载期间需反复捕获背景，
主线程成本高、状态同步复杂，易冻结/错位/延迟。不要重新引入 `src/ui/liquid_glass.rs`
或自定义 `snapshot` 折射绘制。

## 布局约束

ModeSelector 是覆盖在网格上的玻璃层，不应该通过固定底部 padding 预留黑色安全区。
不要再给 `MediaGrid` 的 `ScrolledWindow` 或每个 `FlowBox` 添加类似
`content-safe-bottom { padding-bottom: 128px; }` 的规则；这会在底部或日期分组之间形成深色
空带，削弱 `backdrop-filter` 看到真实内容的效果。

## 相关文件

| 文件 | 作用 |
|---|---|
| `src/ui/grid_css.rs` | 玻璃材质源代码：`BASE_CSS`/`LIQUID_GLASS_MATERIAL_CSS`/`PLAIN_GLASS_MATERIAL_CSS`/`A11Y_CSS` + `.glass-segmented` 通用分段样式 + `build_css`/`install`/`reapply` |
| `src/core/prefs.rs` | `liquid_glass` 偏好读写（`settings.json`，默认开启） |
| `src/ui/window.rs` | 设置页「外观」区块 + 拨动开关时调用 `grid_css::reapply` 实时切换 |
| `data/ui/mode-selector.blp` | ModeSelector 根节点挂 `mode-selector glass-raised` |
| `src/ui/mode_selector.rs` | 交互、焦点、ViewStack 同步；不绘制玻璃材质 |
| `tests/ui_mode_selector.rs` | 断言 ModeSelector 作为 overlay 浮动并携带 `glass-raised` |
| `tests/ui_grid_canvas.rs` | 断言 MediaGrid 不再保留固定底部黑带 |
