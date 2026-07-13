# GtkGridView 虚拟照片网格实施计划

> **For agentic workers:** 按 Superpowers 的 task-by-task / subagent-driven-development
> 流程执行。每项先写最小测试，再实现，再跑该项 focused verification；不要把
> FlowBox 相册、搜索或回收站一并迁移。

**目标：** 将 Photos 主图库的 Day、Month、Year 视图迁移到可按需加载的
`GtkGridView`，并保留启动期 `runtime.json` 的 FlowBox 回退开关。

**设计依据：**
[`2026-07-13-virtual-gridview-migration-design.md`](../specs/2026-07-13-virtual-gridview-migration-design.md)。

**边界：** 仅 Photos 主图库。相册详情、搜索、回收站继续使用现有 `MediaGrid`。
`photos_grid_backend` 的默认值在本次保持 `flowbox`，以便新后端经由运行时配置
试点；三个模式在 `gridview` 配置下全部可用。

## 任务 0：基线与回退选择

**拥有模块：** `src/core/runtime_config.rs`、`PhotosPage`。

- [ ] 增加 `PhotosGridBackend::{FlowBox, GridView}` 与安全解析：缺失和未知值均回到
  当前编译期默认值。
- [ ] 在 Photos 创建时只读取一次后端配置；不支持运行中热切换或 panic 自动回退。
- [ ] 为 `flowbox` / `gridview` / 未知值 / 缺失值添加单测。
- [ ] 记录后端选择到 browsing trace。

验收：修改 `runtime.json` 并重启即可选择旧路径或新路径，二者不影响数据库和
缩略图缓存。

## 任务 1：纯布局索引

**拥有模块：** `src/ui/virtual_media_grid/layout_index.rs`。

- [ ] 从 authoritative `SectionKey → count` 构建 newest-first `SectionSpan`。
- [ ] 每个 section 的 slot 数向上对齐列数，在末行生成 filler 槽位。
- [ ] 提供 `slot_count`、`slot_at`、`slot_for_media_offset`、`section_for_slot`。
- [ ] 查询复杂度是 `O(log section_count)`；不得构造每媒体一个 slot 的向量。
- [ ] 覆盖空库、排序、filler、映射边界和列数变化的纯单测。

验收：物理 slot 与媒体 offset 的双向关系稳定，日期绝不在同一行混合。

## 任务 2：虚拟模型和范围缓存

**拥有模块：** `src/ui/virtual_media_grid/{model,range_cache}.rs`。

- [ ] 自定义 `gio::ListModel` 的总项数始终等于 layout 的逻辑 slot 数。
- [ ] `get_item` 仅返回 filler、placeholder 或 ready slot；没有 SQL、磁盘 I/O 或
  解码。
- [ ] 用可合并的范围请求和 generation 丢弃陈旧异步结果。
- [ ] 通过 `MediaRepository::items(MediaQuery::LiveAll, start, limit)` 读取后台范围。
- [ ] 只为变化的连续 slot 段发出 `items_changed`；不替换整个 model 或
  `GtkNoSelection`。
- [ ] 限制 range cache 驻留范围，覆盖重叠请求、过期回包和逐出重入的测试。

验收：从顶部拖到任意冷区域立即可见稳定 placeholder，DB 请求不阻塞 GTK 主线程。

## 任务 3：GridView factory 与缩略图安全绑定

**拥有模块：** `src/ui/virtual_media_grid/factory.rs`、`SquareTile` 的最小复用扩展。

- [ ] `setup` 一次性创建可复用 tile 和输入 controller；`bind` / `unbind` 只更新
  状态。
- [ ] `TileBindingKey` 至少包含 layout generation、slot、`MediaId`、mtime cache key。
- [ ] 异步缩略图回调写入前验证绑定键，防止 cell 回收后错图。
- [ ] bind 的同步路径只允许 `try_load_mem_cached`；冷缓存走
  `request_for_media(..., TIER_BOOST)`。
- [ ] filler 不可激活，placeholder 稳定可见；选择通过 tile `.media-selected` class
  表示，而非 FlowBox selected state。
- [ ] Year/Month/Day 均使用各自 tile 尺寸和缩略图桶，Day 保留动态照片、视频时长、
  收藏角标。

验收：GTK factory 回收测试证明旧回调不写入新媒体；所有模式可渲染和点击。

## 任务 4：VirtualMediaGrid 外壳和 Photos 接入

**拥有模块：** `src/ui/virtual_media_grid.rs`、`data/ui/virtual-media-grid.blp`、
`src/ui/photos_page.rs`。

- [ ] 将 `GtkGridView` 直接放入 `GtkScrolledWindow`，固定实际列数为同一 min/max
  columns，并在宽度变化后用 anchor 恢复位置。
- [ ] 用窄 `PhotosGrid` 枚举统一 FlowBox 与 GridView 的 PhotosPage 所需接口。
- [ ] 三个模式仅 active grid 发起 metadata/range/thumbnail 工作。
- [ ] 接入稳定 `MediaId` 的 activate、右键、选择、批量操作和 Viewer 打开。
- [ ] 浮动日期从物理顶部 slot / layout span 解析，filler 时仍显示所属 section。
- [ ] 文件监听或共享窗口数据变化时合并刷新 metadata/layout，保留 anchor 与选择。

验收：`photos_grid_backend=gridview` 下 Day、Month、Year 均能切换、拖动、打开 Viewer、
多选，并保持日期 pill 正确；`flowbox` 回退路径仍可工作。

## 任务 5：资源、文档和验证

- [ ] 更新 Blueprint 编译/资源清单和 UI naming reference（如新增 template child）。
- [ ] 将当前行为和 GridView/FlowBox 回退契约写入 `docs/modules/browsing.md`；若新增
  range/thumbnail contract，同步 `docs/modules/storage.md`。
- [ ] 添加布局、range、factory 回收、后端选择和 Photos 三模式交互测试。
- [ ] 运行 focused checks，然后运行完整 CI：

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments
cargo build --all-targets
xvfb-run -a cargo test --all
tools/visual-check-x11.sh
```

- [ ] 即使有尚未解决的环境/测试问题，也记录准确失败信息并保留可复现修订。

## 任务 6：交付

- [ ] 从默认分支创建 `agent/virtual-gridview-migration` 独立分支。
- [ ] 只暂存本计划及本迁移产生的文件，不吸收无关工作树变更。
- [ ] 提交、推送远端并创建草稿 PR；若验证失败，PR 描述必须包含失败项。
