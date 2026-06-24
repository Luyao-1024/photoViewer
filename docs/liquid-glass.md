# 液态玻璃 ModeSelector（Liquid Glass）

> 年/月/日 切换器的背景玻璃效果说明。

参考实现：[`shuding/liquid-glass`](https://github.com/shuding/liquid-glass)

## 当前实现

`PhotosPage` 底部居中悬浮的 **年 / 月 / 日** 切换器通过 GTK CSS 原生
`backdrop-filter` 实现液态玻璃材质。实现路径是：

1. `data/ui/mode-selector.blp` 给根节点同时挂 `mode-selector` 和 `glass-raised`。
2. `src/ui/grid_css.rs` 中的 `.glass-raised` 提供半透明填充、边框、内高光、投影和：

```css
backdrop-filter: blur(28px) saturate(1.22) brightness(1.06);
```

3. `box.mode-selector` 只保留尺寸、圆角和内边距；不再自己绘制材质。
4. `ModeSelector` 不重写 `snapshot`。背景采样、模糊和增强交给 GTK/GSK 的
   `backdrop-filter` 节点处理。

这意味着玻璃效果依赖支持 `backdrop-filter` 的 GTK 运行时。普通系统 GTK 如果还不支持
该属性，会退化为半透明填充、边框和阴影；Flatpak/GNOME 新运行时才是目标验证环境。

GTK 的 `CssProvider` 支持 `prefers-reduced-motion` / `prefers-contrast` /
`prefers-color-scheme`，不支持 Web 草案里的 `prefers-reduced-transparency`。因此无障碍
回退使用 `@media (prefers-reduced-motion: reduce)` 禁用玻璃模糊/透明效果，并使用
`@media (prefers-contrast: more)` 增强边框和文字对比。

## 已放弃方案

之前试过一套 CPU/GSK 自绘折射方案：

- 捕获 ModeSelector 背后的网格区域；
- 在 CPU 上做逐像素位移、饱和度和亮度处理；
- 在 `ModeSelector::snapshot` 里把处理后的纹理画回胶囊区域。

这套方案已经放弃，不应恢复。原因是它需要在滚动和缩略图加载过程中反复捕获背景纹理，
主线程成本高，状态同步复杂，也容易出现冻结、错位或延迟更新。当前代码里不应重新引入
`src/ui/liquid_glass.rs`、自定义 `ModeSelector::snapshot` 折射绘制，或 tick 捕获节流。

## 布局约束

ModeSelector 是覆盖在网格上的玻璃层，不应该通过固定底部 padding 预留黑色安全区。
不要再给 `MediaGrid` 的 `ScrolledWindow` 或每个 `FlowBox` 添加类似
`content-safe-bottom { padding-bottom: 128px; }` 的规则；这会在底部或日期分组之间形成深色
空带，削弱 `backdrop-filter` 看到真实内容的效果。

## 相关文件

| 文件 | 作用 |
|---|---|
| `data/ui/mode-selector.blp` | ModeSelector 根节点挂 `mode-selector glass-raised` |
| `src/ui/mode_selector.rs` | 交互、焦点、ViewStack 同步；不绘制玻璃材质 |
| `src/ui/grid_css.rs` | `.glass-raised` / `.glass-base` 等 `backdrop-filter` 材质 |
| `tests/ui_mode_selector.rs` | 断言 ModeSelector 作为 overlay 浮动并携带 `glass-raised` |
| `tests/ui_grid_canvas.rs` | 断言 MediaGrid 不再保留固定底部黑带 |
