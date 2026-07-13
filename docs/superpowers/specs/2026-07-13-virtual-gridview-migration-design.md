# GtkGridView 虚拟照片网格迁移设计

**状态：** 已定稿，待实施

**日期：** 2026-07-13

**范围：** Photos 主图库的 Year / Month / Day 网格；先以 Day 视图试点。

**不在本次范围：** 相册详情、搜索预览/结果页、回收站和侧边栏封面。

## 决策摘要

将 Photos 主图库从“`GtkFlowBox` 分组 + 500 项虚拟页替换 + 整页重建”迁移为：

```text
GtkGridView
  → VirtualMediaModel（全库逻辑槽位）
  → RangeCache / RangeCoordinator（仅缓存视口附近媒体）
  → MediaRepository::items(...)（后台范围查询）
  → ThumbnailLoader（可见区优先、定向预取）
```

日期不再作为网格内的全宽 header，而是复用 Photos 页面现有的浮动日期 pill。为保持现有按日期分组的视觉语义，每个日期的末行会用不可交互的 filler 槽位补齐；不同日期不会出现在同一图像行。

迁移期间保留现有 `MediaGrid`（FlowBox）作为**启动时可选的内部回退后端**。它不是用户设置，不自动捕获异常后切换，也不作为长期双实现维护。GridView 后端稳定并完成推广后删除该开关和旧的 Photos 主图库路径。

`GtkGridView` 的列表模型和 factory 模型能够按需创建和复用可见 item widget；这是消除整页 FlowBox child 构造的基础，但不能替代数据范围缓存和缩略图优先级调度。[GTK GridView 文档](https://docs.gtk.org/gtk4/class.GridView.html)

## 问题与目标

当前主线的 Photos 网格在大图库中有两个独立瓶颈：

1. 网格加载：滚动条跳转会把全库位置映射到一个 DB page，然后替换 `ListStore` 并重建该页的 FlowBox、日期 header 和 tile。即使 DB 查询很快，GTK 主线程仍要创建、测量和连接大量 widget。
2. 缩略图加载：页面重建会同时制造一批冷 tile；若请求顺序、缓存命中或异步回调处理不当，用户会看到空白、延迟或错误缩略图。

目标如下：

- 拖动原生滚动条到任意位置时，滚动位置立即反映全库逻辑位置；目标区域先显示稳定骨架，再由后台范围加载和缩略图请求填充。
- 常驻 GTK tile 数量与“视口 + overscan”成正比，而不是与 500 项虚拟页或图库总量成正比。
- GTK 主线程不得在滚动、模型 `get_item`、factory bind 或 tile 构造路径中执行同步 SQLite 查询、磁盘缩略图读取或图像解码。
- Year、Month、Day 的点击、右键菜单、键盘导航、多选、查看器和文件监听更新继续以 `MediaId` 为身份，不依赖渲染位置。
- 浮动日期在冷区域、快速拖动和列数变化时仍指向正确日期。
- 迁移任何阶段均可通过重启选择 FlowBox 后端恢复当前稳定实现；数据表和缩略图缓存格式不受后端选择影响。

## 非目标

- 不实现自定义滚动条、拖动 scrubber 或 Android 风格手势系统；继续使用 GTK 原生 `GtkScrolledWindow`。
- 不在首次迁移中改造 SQLite 为全量内存索引，也不预先创建全库的 `GObject` / `SquareTile`。
- 不直接合并 `dev` 分支的行虚拟化 `GtkListView` 实验。该实验验证了稳定 ID、请求代际和滚动反馈防护的必要性，但它依赖手工 row/spacer 模型，目标与本设计的扁平 `GtkGridView` 不同。
- 不在本设计中改变相册、搜索和回收站的页面边界；它们保持现有有界 `MediaGrid`，待主图库稳定后再单独评估。
- 不把回退开关暴露到偏好设置，不承诺永久兼容两个渲染后端。

## 现有约束与可复用能力

- `PhotosPage` 当前持有 Year、Month、Day 三个 `MediaGrid`，仅可见模式应处于 active 状态。
- `MediaRepository::items(query, start, limit)` 已能提供不附带 `COUNT(*)` 的范围查询；主图库总数和 section counts 应在后台单独刷新，而不是每次 range 请求调用 `page()`。
- `SectionKey`、`section_counts` 和 `section_for_global_offset` 已能把全库 offset 投影到日期；`PhotosPage` 已有 `scroll_date_revealer` / `scroll_date_label` 浮层。
- `SquareTile`、`ThumbnailLoader` 和现有缩略图内存缓存可复用；bulk bind 只能调用内存缓存路径，磁盘缓存读取与解码必须留在 worker 中。
- `ViewerPage::new_for_query(query, current_id, initial_items)` 已支持查询范围内的邻居导航。虚拟网格只需为打开项构造一个很小的初始 `ListStore`，后续相邻项由 Viewer 的 repository 查询补入。
- 当前 GTK 运行时和 `gtk4-rs` 依赖已支持基础 `GtkGridView`、`GtkNoSelection` 和 `SignalListItemFactory`；首个版本不依赖更高版本 GTK 的额外滚动 API。

## 回退设计

### 后端选择

新增内部运行时配置项：

```json
{
  "photos_grid_backend": "flowbox"
}
```

合法值为：

- `flowbox`：现有 `MediaGrid` 路径。
- `gridview`：新的 `VirtualMediaGrid` 路径，仅作用于 Photos 主图库。

`runtime_config` 提供一个强类型 `PhotosGridBackend` 枚举和 `photos_grid_backend()` accessor。迁移期缺失或非法值回退到编译期默认值；第一阶段默认值为 `flowbox`。Day 试点达到验收标准后，默认值切换为 `gridview`，但 `flowbox` 仍保留一个稳定观察周期。

后端只在 `PhotosPage::new` 时读取，改动 `runtime.json` 后必须重启应用。运行中不热切换 widget：这会同时改变滚动模型、factory 生命周期和选中视觉状态，反而增加故障面。

### 回退语义

- 回退是诊断和止损手段，不是正常产品功能；不出现在设置页、菜单或本地化资源中。
- 不做“捕获 panic 后自动改用 FlowBox”。GTK/GObject 状态部分初始化后继续运行并不安全，自动切换也会掩盖故障。
- 两个后端都读同一个数据库、同一 `ThumbnailLoader`、同一 `MediaQuery` 排序和同一 `MediaId` 选择集合；回退不涉及数据库迁移、缓存清理或重扫。
- 诊断日志必须记录启动选择，例如 `photos_grid_backend=gridview`，并在 range 请求、布局重算、缩略图绑定 trace 中带上后端字段。
- 如果 GridView 在试点库上发生错误图片、滚动失稳、不可恢复的输入问题或性能回归，测试/支持人员将配置改为 `flowbox` 并重启，保留 trace 和测试数据库用于复现。

### 代码边界

`PhotosPage` 不应散布 `if backend == ...`。新增一个小型适配层：

```rust
enum PhotosGrid {
    FlowBox(MediaGrid),
    GridView(VirtualMediaGrid),
}
```

该枚举只委派 Photos 页面真正需要的共同能力：`widget`、`set_active`、`mode`、`connect_view_changed`、`scroll_fraction`、`current_scroll_section_key`、背景亮度查询、选择读写、`clear_selection` 和激活/右键回调。不要用 `dyn Trait<Widget>`；GTK widget 所有权和模板类型通过枚举保持静态、可检查的分派。

第一版保留 Photos 页面现有的三视图结构：每种 `GroupBy` 创建一个 `PhotosGrid`，但 GridView 后端只有 active view 可以启动元数据加载、range 请求和缩略图预取。这样回退不会同时变更模式切换架构。

### 删除条件

满足以下所有条件后，才删除 FlowBox 主图库后端、运行时键及适配枚举分支：

- Day、Month、Year 均以 GridView 为默认后端通过大图库和冷缓存验证。
- 至少一个完整扫描/文件监听周期内，新增、删除、移动、收藏和批量操作没有后端特有回归。
- 回归测试和 Flatpak X11 视觉检查覆盖两次连续稳定构建。
- 性能 trace 显示不存在旧的全页重建热点，且没有新的主线程 I/O 或高频 `items_changed` 抖动。
- 明确记录不再支持通过 `runtime.json` 选择 FlowBox。

## 目标架构

### 模块划分

建议新增以下 UI 层模块；纯布局和缓存算法不应依赖 GTK widget。

| 模块 | 职责 |
|---|---|
| `src/ui/virtual_media_grid.rs` | `VirtualMediaGrid` 外壳、公共 PhotosGrid 接口、模板绑定和生命周期。 |
| `src/ui/virtual_media_grid/layout_index.rs` | 纯 Rust 的 `VirtualGridLayoutIndex`、日期 section span、slot/媒体 offset 双向映射。 |
| `src/ui/virtual_media_grid/model.rs` | 自定义 `gio::ListModel`、轻量 `GridSlotObject`、局部 `items_changed` 通知。 |
| `src/ui/virtual_media_grid/range_cache.rs` | 视口范围计算、请求去重/合并、异步 DB 查询、LRU 驻留和 generation 校验。 |
| `src/ui/virtual_media_grid/factory.rs` | `SignalListItemFactory` 的 setup/bind/unbind、filler、tile 与交互绑定。 |
| `src/ui/virtual_media_grid/tests.rs` 及子测试 | GTK 生命周期和跨模块行为测试。 |

现有 `src/ui/media_grid.rs` 在迁移期不重构为新后端的内部实现。它继续作为 FlowBox 回退和非 Photos 页的稳定组件，避免一次提交同时破坏主图库、相册和搜索。

### 布局索引：不构造全库 slot 向量

`VirtualGridLayoutIndex` 以每个 section 的累积 span 表示全库，而不是为每张媒体创建一项 `Vec`：

```text
SectionSpan {
  key: SectionKey,
  media_start: u32,
  media_len: u32,
  slot_start: u32,
  slot_len: u32,       // media_len + 0..columns-1 个 filler
}
```

输入是按图库排序（最新在前）的 authoritative `section_counts`、总媒体数和当前列数。每个 section 的 `slot_len` 向上取整到列数；最后不足一行的格子是 `Filler`。通过二分查找实现：

- `slot_at(position) -> MediaOffset(offset) | Filler { section }`。
- `slot_for_media_offset(offset) -> position`。
- `section_for_slot(position) -> SectionKey`。
- `slot_count()`，作为虚拟 GListModel 的 `n_items`。

这让内存复杂度为 `O(section_count)`，而非 `O(media_count)`。列数变更时只重新计算 spans，不查询数据库；在变更前保存最靠近视口顶部的 `MediaId` / global media offset，重算后滚动到对应新 slot，避免窗口 resize 将用户送回顶部。

默认保留 filler 的理由是：浮动日期替代 header 后，若直接把所有媒体拼为一个连续流，日期会在同一行的两个 tile 之间切换。filler 保持原有日期边界，代价仅是少量逻辑空格和列宽变化时的 span 重算。若未来产品决定接受完全连续的 Google Photos 风格流，可让 layout index 禁用 filler；它是一个独立产品决策，不应与第一版迁移混在一起。

### 虚拟模型与范围缓存

`VirtualMediaModel` 实现 `gio::ListModel`：

- `n_items` 始终等于 `VirtualGridLayoutIndex::slot_count()`，因此 GTK 的调整值和滚动条天然代表全库逻辑高度，不再依赖顶部/底部 spacer 近似高度。
- `get_item(position)` 是同步且常数时间的：立刻返回 `Filler`、`Placeholder` 或已缓存的 `Ready(MediaItem)` slot object；它绝不发起 SQL、文件 `metadata()`、磁盘缓存读取或图像解码。
- `GridSlotObject` 只表达当前绑定所需的 slot 状态。不可见、已逐出的 item 不由模型强持有；GTK 对已实现 list item 的引用自然限制在当前可见区域附近。
- `RangeCoordinator` 从可见物理 slot 区间转换为连续 media offset 区间，合并重叠/相邻请求，并追加一屏前后 overscan。滚动方向决定优先预取前方还是后方。
- DB worker 使用 `MediaRepository::items(MediaQuery::LiveAll, start, limit)`；总数和 section counts 由单独元数据请求提供。每个 range 结果必须携带 `backend_generation`、`layout_generation`、query 和请求 token，过期结果直接丢弃。
- range 结果落地后，只对变化的连续 slot 段发出 `items_changed(position, removed=1, added=1)`，或更新对应 slot revision；不得替换整个 GListModel、`GtkNoSelection` 或所有可见 item。
- 缓存按连续 media range 驻留并设定上限，目标是当前视口、前后 overscan 和正在请求的相邻 range。缓存逐出只影响未来再进入该区间时的状态，不清除仍由 GTK 绑定的 tile。

滚动/数据读取路径如下：

```text
Adjustment changed
  → 由固定 tile metrics 推导可见 slot 行
  → layout index 转为 media offsets
  → RangeCoordinator 合并、去重并后台查询
  → VirtualMediaModel 局部 items_changed
  → GtkGridView factory 仅重绑受影响可见 cell
```

初版仍使用现有 OFFSET 查询。必须通过 1 万和 10 万级数据库 trace 证明深页 OFFSET 是瓶颈后，才单独增加锚点/键集分页；不能在 UI 迁移期间同时修改排序语义、数据库索引和 viewer 邻居查询。

### GtkGridView 和 factory

`GtkGridView` 必须直接放在 `GtkScrolledWindow` 中，不再通过 `GtkViewport` 包一层 FlowBox 内容。列数由当前可用宽度、tile 尺寸和 gap 计算，并把 GridView 的 min/max columns 固定为同一值，保证 `VirtualGridLayoutIndex` 与实际行数一致。

factory 生命周期：

1. `setup`：创建一个可重用 `SquareTile`，安装一次点击、右键和键盘相关 controller；不要在 bind 中重复安装 controller。
2. `bind`：读取 `GridSlotObject`。filler 绑定为不可见、不可选的固定正方形占位；placeholder 显示稳定 loading surface；ready media 绑定内容、覆盖层和选择 CSS。
3. `unbind`：断开该绑定的回调/观察器，清除视觉状态和弱引用，但不进行磁盘 I/O 或同步 thumbnail 取消等待。

每次 ready media bind 生成不可复用的 `TileBindingKey`：至少包括 backend/layout generation、slot position、`MediaId` 和 thumbnail cache identity（含 mtime）。缩略图回调在写入 paintable 前必须验证当前 tile 的 key 完全相等。这样旧 range、旧 mtime 或 tile 被回收给另一媒体后的回调都不能覆盖新内容。

bulk bind 的即时路径只调用 `ThumbnailLoader::try_load_mem_cached`。未命中时 tile 保持 loading state，并使用 `request_for_media(..., TIER_BOOST)` 走后台 worker；worker 内部才可访问磁盘缓存和解码。可见项优先级高于 range 预取和背景 prewarm，且当前全库 media offset 继续传给预热重定向。

### 日期 pill

现有 `PhotosPage` 的 `scroll_date_revealer` 保持 UI 和 700ms 隐藏行为。GridView 后端的 `current_scroll_section_key()` 不再按“滚动比例 × media 总数”直接推断，因为 filler 使物理 slot 和媒体 offset 不再一一对应。

改为：

1. 由 adjustment value、固定行高、列数求当前顶部物理 slot。
2. 由 `VirtualGridLayoutIndex::slot_at` 找到该 slot；若是 filler，选择同 section 最后一个媒体 offset。
3. 由该 offset / section span 得到 `SectionKey`，再复用现有无数量日期格式化。

该过程只读取内存布局元数据，在冷 range 和快速拖动时同样可用。布局重算期间 pill 可暂时隐藏一个 idle 周期，重算并恢复 anchor 后再显示；不能显示上一列数的错误日期。

### 选择、菜单、键盘与 Viewer

GridView 使用 `GtkNoSelection`；多选继续由应用维护 `HashSet<MediaId>`。这避免在 range 更新、插入或列数变化后把不稳定的 position 当作身份。factory 在 bind 时按集合添加/移除 tile 级 `.media-selected` CSS 类，替代 FlowBox 的 `flowboxchild:selected`。

所有事件从 slot 解析出 `MediaId` 后再调用现有 `MediaGridCallbacks` 语义：

- 单击：普通模式打开 Viewer；多选模式切换该 `MediaId`。
- 右键：普通模式作用于该项；多选模式先确保右键项进入集合，再对稳定 ID 集合执行菜单操作。
- 键盘：保留 GridView 原生方向移动，Enter/Space 与当前模式语义一致。
- Viewer：以 `MediaQuery::LiveAll + MediaId` 创建 `ViewerPage::new_for_query`。初始 `ListStore` 至少包含被点击的 ready item；Viewer 的现有 repository 邻居查询负责后续前后导航。不得把虚拟网格 position 或当前 range 下标传给 Viewer 当作全局身份。

### 扫描器和文件监听事件

数据库和 watcher 仍是 source of truth。GridView 后端收到媒体 upsert/remove 批次时：

1. 合并短时间内的事件，保存当前视口 anchor `MediaId` 与其相对像素位置。
2. 后台刷新总数和 section counts，建立新的 layout index generation。
3. 失效受影响的 range 和 thumbnail binding；不把整个图库重新放进 GTK 模型。
4. 对模型发送必要的结构变化，恢复 anchor；若 anchor 已删除，使用相邻的全局 media offset。
5. 保留 `HashSet<MediaId>` 中仍存在的项，并重新绑定可见 tile 的选择外观。

初始扫描可能在排序头部连续插入大量项，因此必须批处理，不能逐条触发全模型重建。若 section counts 尚未返回，保持上一份一致的 layout，等待新 generation 的完整元数据后一次切换；宁可短暂显示旧布局，也不要在中间状态让 adjustment 上下跳动。

## 分阶段实施与默认值策略

| 阶段 | 交付内容 | 默认后端 | 回退与推进门槛 |
|---|---|---|---|
| 0. 基线 | 建立大图库测试库、现有 FlowBox trace、GridView backend runtime key 与 `PhotosGrid` 适配壳。 | FlowBox | 配置可选择 GridView，但尚未接入真实页面。 |
| 1. 纯逻辑 | `VirtualGridLayoutIndex`、slot/span 映射、filler、列数/anchor 算法和纯单测。 | FlowBox | 不碰页面；所有边界映射测试通过。 |
| 2. 数据虚拟化 | `VirtualMediaModel`、range coalescing、generation、范围缓存和 repository 测试。 | FlowBox | `get_item` 无 I/O；深跳结果正确；缓存有界。 |
| 3. 渲染 | `VirtualMediaGrid`、factory、缩略图 token、选择和菜单视觉。 | FlowBox | GTK 回收测试无错图/泄漏；主线程 trace 无同步 I/O。 |
| 4. Day 试点 | Photos Day 由 `gridview` 配置启用；浮动日期、Viewer、扫描事件接入。 | FlowBox（默认） | 仅测试/试点环境 opt-in；任何问题可重启回 FlowBox。 |
| 5. 默认切换 | Day 通过性能和交互验收后默认 GridView；再依次启用 Month、Year。 | GridView（Photos） | FlowBox 配置继续保留，观察至少一个完整稳定周期。 |
| 6. 收尾 | 删除 FlowBox 主图库分支、旧虚拟 pager/spacer 路径和运行时键；更新维护文档。 | GridView | 仅在“删除条件”全部满足时执行。 |

每一阶段都是可独立提交、可独立测试的。阶段 4 之前不允许为了“复用”而改动相册、搜索或回收站构造器。

## 验收标准与可观测性

### 必须满足的行为标准

- 100k 项图库中，将滚动条从顶部拖到中部、底部或反复来回时，GTK 不创建或销毁与整个 DB page 等量的 tile；目标视口立即显示 placeholder 或内存缓存结果。
- 热缓存时可见 tile 直接显示；冷缓存时只有目标视口和 overscan 排在 `TIER_BOOST` 前列，背景预热不得抢占。
- 快速滚动、模式切换、列数调整、range 返回乱序时，不出现错误媒体、错误日期、错位选择、高亮残留或空白卡死 tile。
- 选中项可跨出/进入缓存窗口；批量菜单返回的 ID 集合与用户选择一致。
- Viewer 从任意冷 range 打开后，当前项正确，前后导航仍保持 `LiveAll` 的数据库排序。
- 新增、删除、外部移动和缩略图失效不会把用户无故送回顶部。

### 自动测试

- 纯单元测试：section span、filler 数量、slot↔media offset、边界/空库、列数变化、anchor 恢复和日期投影。
- range cache 测试：请求合并、重叠去重、最新 generation 胜出、驱逐后重入、`items_changed` 的最小连续段。
- repository 集成测试：临界 offset、最后一页、查询排序和大范围查询不触发额外 count。
- `#[gtk::test]`：factory 回收后旧 thumbnail 回调不能写入新 tile；选择跨 range 保持；filler 不可激活；`GtkNoSelection`/模型对象不在 page 变更时整体替换。
- UX 测试：Day 后端下打开 Viewer、右键多选、键盘导航、模式切换与滚动日期 pill。
- 回退测试：同一小型 fixture 分别以 `flowbox` 和 `gridview` 创建 PhotosPage，验证共同回调契约；未知配置值走当前阶段的安全默认值。

### Trace 和人工验证

保留/扩展现有 tracing，至少包含：

- `gridview:layout_rebuild`：section 数、列数、slot 总数、anchor。
- `gridview:range_request` / `gridview:range_apply`：请求范围、合并数、generation、stale/drop、耗时。
- `gridview:factory_bind`：ready/placeholder/filler 数量和当前绑定数（采样，不能逐帧刷日志）。
- `gridview:thumbnail_request`：visible/overscan/directional prefetch 分类和队列长度。

在 Flatpak GNOME 50 X11 环境中使用 `tools/visual-check-x11.sh` 做至少以下人工路径：冷启动、滚动条两端跳转、快速连续拖动、窗口缩放、Year/Month/Day 切换、缩略图冷缓存、扫描期间新增/删除和 Viewer 往返。

## 风险与缓解措施

| 风险 | 缓解措施 |
|---|---|
| GTK adjustment 因模型替换或不稳定高度产生跳动 | 全库逻辑 slot 模型保持同一 `GtkNoSelection`；固定列数/固定 tile metrics；局部 item 更新，不使用旧 spacer/page swap 方案。 |
| tile 回收导致旧异步结果写错图 | `TileBindingKey` 必须包含 generation、slot、MediaId 和 cache identity；unbind 清理观察器；回调写入前二次验证。 |
| 日期 filler 使滚动日期换算失真 | 使用物理 slot→section span 映射，不再把 fraction 直接乘媒体总数。 |
| 窗口 resize 改列数后内容跳跃 | 在 layout 重算前保存稳定 ID anchor，重算后映射到新 slot 并恢复相对位置。 |
| 深 OFFSET 在极大库变慢 | 先以 trace 和 repository 基准确认；若成立，再单独引入索引锚点/键集分页，不混入 UI 迁移。 |
| 文件监听批次造成全模型抖动 | 合并事件、完整元数据 generation 后一次提交、保留当前 anchor 和稳定 ID 选中集合。 |
| 双后端长期漂移 | 回退仅限 Photos，适配接口窄，设置明确删除条件；每次共同交互改动由双后端契约测试覆盖。 |

## 预期修改文件

| 文件 | 预期变化 |
|---|---|
| `src/core/runtime_config.rs` | `PhotosGridBackend`、`photos_grid_backend` 解析、默认值和测试。 |
| `src/ui/photos_page.rs` | 后端选择、`PhotosGrid` 适配使用、浮动日期改为通用接口。 |
| `src/ui/virtual_media_grid*.rs` | 新的虚拟 GridView 实现及其测试模块。 |
| `src/ui/square_tile.rs` | 增加/收紧可验证的绑定 key 和安全的复用清理。 |
| `data/ui/virtual-media-grid.blp`（如需要） | GridView 直接嵌入 `GtkScrolledWindow` 的模板；不得编辑生成的 `.ui`。 |
| `data/css/base.css` | tile 级 `.media-selected`、filler/placeholder 的最小样式，兼容 Liquid Glass 与普通半透明模式。 |
| `src/core/repository.rs` / `src/core/db.rs` | 仅在 range 查询或索引基准确有缺口时扩展；不为 UI 便利把 GTK 依赖带入 core。 |
| `docs/modules/browsing.md` | GridView 成为默认后更新当前行为、不变量和旧路径移除说明。 |
| `docs/modules/storage.md` | 如 range 获取或 thumbnail 优先级契约改变，更新存储/缩略图不变量。 |
| `docs/ui-naming-reference/index.html` | 仅在新增/改名模板 widget、child-id 或交互 affordance 时同步更新。 |

## 最终验证

每个小阶段先运行拥有该逻辑的 focused test。准备提交或切换默认后端前，执行与 CI 一致的完整验证：

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments
xvfb-run -a cargo test --all
tools/visual-check-x11.sh
```

若主线在实施前已经存在无关失败，必须在交付记录中明确说明；不得把其归因于这次 GridView 迁移。
