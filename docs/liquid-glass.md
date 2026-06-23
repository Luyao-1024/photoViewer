# 液态玻璃 ModeSelector（Liquid Glass）/ Liquid-Glass ModeSelector

> 年/月/日 切换器的背景折射效果说明文档。
> Design doc for the refracting "liquid glass" treatment on the Year/Month/Day switcher.

参考 / Reference: [`shuding/liquid-glass`](https://github.com/shuding/liquid-glass)

---

## 1. 是什么 / What it is

`PhotosPage` 底部居中悬浮的 **年 / 月 / 日 三段式切换器**（`ModeSelector`）现在呈现"液态玻璃"质感：透过这块半透明的胶囊，可以看到其背后的照片网格被**凸透镜式地折射/放大**，并带有镜面高光边缘、玻璃淡色 tint 和投影。

The floating Year/Month/Day switcher at the bottom-center of `PhotosPage` now looks like a piece of liquid glass: through the translucent pill you see the photo grid behind it **refracted/magnified like a convex lens**, with a specular rim, a faint glass tint, and a drop shadow.

## 2. 为什么不能照搬网页方案 / Why the web approach doesn't port

参考实现的招牌效果是**背景折射**：用 SVG `feDisplacementMap` 把胶囊*背后*的内容做位移，再通过 CSS `backdrop-filter` 应用到已合成的背景上。

The reference's signature effect is **backdrop refraction**: an SVG `feDisplacementMap` warps the content *behind* the element, applied to the already-composited backdrop via CSS `backdrop-filter`.

在 GTK4 里这条路走不通 / This doesn't work in GTK4:

| 能力 / Capability | Web | GTK4 |
|---|---|---|
| `backdrop-filter`（对已合成背景做模糊/位移） | ✅ 浏览器合成器，GPU，近乎免费 | ❌ CSS 不支持 |
| GL 着色器采样背后内容 | ✅（`backdrop-filter`） | ❌ `GskGLShader` 只能采样显式喂进去的子纹理，采样不到 widget 背后 |
| 读取某区域已渲染的像素 | ✅（合成器层） | ❌ 无公开 API（Mutter 合成器层才有，应用拿不到） |

关键结论 / Key conclusions:
- **GTK4 没有 `backdrop-filter` 等价物**，也没有公开的"把 widget 背后已渲染内容采样成纹理"的 API。
- `GskGLShader` 的子纹理**必须是预先栅格化好的 `GdkTexture`**（通过 `gtk_snapshot_append_texture` 添加），不能直接把一个实时 widget 树当子纹理——所以即便用 GL 着色器，仍得先用 `render_texture` 把网格栅格化成纹理，并不能省掉这次主线程捕获。

## 3. 实际实现 / What we actually do

唯一可行的真实折射路径：**捕获 → 位移 → 回填**。

The only viable real-refraction path: **capture → displace → paint**.

1. **捕获 / Capture** —— 用 `gsk::Renderer::render_texture` 把当前可见网格（`ViewStack` 的 visible child，即 `MediaGrid`）的 `WidgetPaintable` 渲染成一张离屏纹理，按胶囊背后的矩形裁剪（`src/ui/liquid_glass.rs::refract_region`）。
2. **位移 / Displace**（CPU）—— 逐像素做凸透镜位移：每个输出像素采样一个被向中心拉近的位置（`factor = 1 - strength * smoothstep(0, 0.5, r)`），再叠加亮度 +6% / 饱和度 +12%（对应参考实现的 `brightness(1.05)/saturate(1.1)`）。
3. **回填 / Paint** —— `ModeSelector::snapshot`（`WidgetImpl::snapshot` 重写）把折射后的 `gdk::MemoryTexture` 画进胶囊的圆角裁剪里，再叠加 tint、高光边缘、投影，最后画 label/dot 子节点。

整个胶囊（投影 + 折射 + tint + 高光边缘）**只在 `snapshot` 一处统一绘制**，圆角描边不再走 CSS `box-shadow`——否则 CSS 描边与 snapshot 的折射圆角会是两个略微错位的矩形，看起来像两个没对齐的胶囊。

### 性能 / Performance

单次捕获实测（284×66 区域，debug 构建）/ Per-capture cost (284×66 region, debug build):

```
node 构建 ~50µs   render_texture 1–2ms   读回 0.2–0.5ms   CPU 位移 1–2ms   合计 ~3–5ms
```

`render_texture` 同步跑在主线程，是真正的大头；CPU 位移在 release 构建下 <0.5ms。

`render_texture` runs synchronously on the main thread and is the real cost; the CPU displace is <0.5ms in a release build.

**节流策略 / Throttling**（`mode_selector.rs::tick_check`，每帧 tick 回调里）：
- **滚动时不捕获**：检测到背景在动（滚动位置 / 胶囊几何 / 可见网格变化）就只记录"最后运动时刻"，**等滚动停下 ~150ms** 才做一次捕获——保证滚动本身永远不被这次主线程捕获卡住。
- **首次立即捕获**：还没有纹理时（`backdrop_tex` 为 `None`）不等 settle，立刻捕获（失败则下一帧重试），所以打开页面后折射尽快出现，而不是等启动加载全部平静。
- **陈旧上限**：连续不停滚动时，最多每 ~500ms 强制刷新一次，避免玻璃长时间冻结。

实测这样在普通图片库下滚动流畅、停下后折射很快更新。

### 涉及文件 / Files

| 文件 / File | 作用 / Role |
|---|---|
| `src/ui/liquid_glass.rs` | 折射捕获 + 位移管线（`refract_region`）。 |
| `src/ui/mode_selector.rs` | `WidgetImpl::snapshot` 统一画整个胶囊；tick 回调按 settle 节流捕获并缓存纹理。 |
| `src/ui/grid_css.rs` | `box.mode-selector` 仅保留 `padding`（形状/边缘都在 snapshot 里画）。明暗对比仍由 `on-light-background` 类 + label/dot 颜色规则驱动。 |
| `data/ui/mode-selector.blp` | 结构未改 / unchanged。 |

## 4. 调参 / Tuning

| 参数 / Param | 位置 / Where | 含义 / Meaning |
|---|---|---|
| `GLASS_STRENGTH` (≈0.35) | `mode_selector.rs` | 透镜位移强度，0–1。调大→更强的放大扭曲，调小→更柔和。 |
| `SAT` / `BRIGHT` (1.12 / 1.06) | `liquid_glass.rs` | 折射区域的饱和度 / 亮度提升。 |
| settle (150ms) / stale (500ms) | `tick_check` | 捕获节流：停下多久后捕获 / 连续滚动时最长多久刷新一次。 |
| tint / rim alpha | `ModeSelector::snapshot` | 玻璃淡色与高光边缘的不透明度。 |
| 圆角 radius (22px) | `snapshot` + snapshot clip | 胶囊圆角。 |

## 5. 局限与后续 / Limitations & future work

- **滚动期间折射会暂时停顿**（约 150ms 的 settle 窗口内不更新），停下后立即刷新——这是为了把主线程捕获成本挪出滚动交互的权衡。
- 缩略图异步加载时，胶囊背后的内容在变化但 `tick` 的签名只跟踪滚动/几何/可见网格，不跟踪单张缩略图加载完成——因此加载期间折射可能略有滞后，直到下一次滚动/settle 触发刷新。
- 非 GL（cairo）渲染器下 `render_texture` 仍可用（折射正常）；真正的 GL 着色器位移因 GTK4 限制无法直接采样背景，未采用。
- 若未来 GTK 提供背景采样能力（如公开的 backdrop 节点），可把位移搬到 GPU 并每帧实时，彻底消除主线程捕获。
