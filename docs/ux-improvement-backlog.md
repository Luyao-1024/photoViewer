# UX 专项检视与 P0/P1 优化方案

检视日期：2026-10-01
检视基线：`250671a`（工作区干净）
检视范围：浏览（Photos / 虚拟网格 / 模式选择器 / 搜索 / 相册 / 回收站）、查看器与编辑器、窗口与设置、共享玻璃材质与可访问性
状态：检视结论已归档，方案按批次落盘中。P0-1（扫描三态）与 P0-2（主网格焦点环）已实施并本地提交，未实施的条目仍为**草案**。落盘过程中与原方案的偏差记录在各节末尾的「实施结果」里——实测证据优先于草案。

## 总体判断

功能骨架与工程规范是这个项目的强项：虚拟化网格、语义色 token、破坏性操作确认、变换不动布局，都已经达到或超过一般桌面相册应用的水准。当前 UX 短板集中在三类，而不是视觉风格：

1. **状态可见性缺口**——扫描期误报「暂无照片」、主网格键盘焦点不可见、选中集被后台重建静默清空。这三项都会让用户对软件产生错误事实判断。
2. **发现性缺口**——多选只能右键进入且无入口提示、13 个快捷键在应用内完全不可发现、全库总览只靠隐藏手势、查看器控件静止态全裸。**hover-only 材质是本项目的刻意设计（见「保留项」），但它必须有一个可发现的入口**，否则「克制」变成了「不存在」。
3. **反馈缺口**——搜索零结果、图片解码失败、编辑器退出丢改动，都是「用户做了动作但界面什么都不说」。

优先级定义（沿用 [`improvement-backlog.md`](improvement-backlog.md) 的分级）：

- **P0**：误导用户或使核心功能不可用，应在下个版本前阻断。
- **P1**：主路径上的可用性损耗，建议在 P0 之后连续解决。
- **P2**：细节打磨与语义补全。

证据标记：

- **代码确认**：实现中存在对应行为，已通过读码与 grep 定位到具体行。
- **已验证**：本次通过 grep/读取交叉确认，且排除了其他实现路径（例如确认某函数零调用者）。
- **待视觉验证**：结论来自 CSS 数值与设计文档推理，未在运行界面中确认（本次**未运行应用**，见文末「本次未执行的验证」）。

---

## 保留项：不要为了「规范」去动

以下几处是有意为之且质量较高，改动前必须先证明当前设计不成立，避免被通用清单牵着走：

| 保留项 | 证据 | 保留理由 |
|---|---|---|
| 虚拟化与滚动预算 | `src/ui/virtual_media_grid/range_cache.rs:224`（overscan 前 2 后 1）、`virtual_media_grid.rs:1691`（DB 分页 `spawn_blocking`）、`virtual_media_grid.rs:107-139`（按帧批量提交缩略图）、`factory.rs:445`（取消陈旧请求） | 已经是正确的 GTK4 虚拟化模型，不要引入第二套列表机制 |
| 语义色 token 化 | 三份 CSS 共 229 处 `alpha()` 组合、**0 个 `rgba()` 字面量**，前景走 `@window_fg_color`/`@accent_*`/`@error_*` | 主题跟随与对比度整改都能在同一层完成，成本低 |
| 破坏性操作确认 | `src/ui/viewer/actions.rs:21-30`、`src/ui/editor_panel.rs:585-605`，均 `ResponseAppearance::Destructive` + 默认取消，覆盖前说明 `.bak` | 已达到桌面端最佳实践，P1-9 只需补齐「编辑器退出」这一条路径 |
| 变换不动布局 | `src/ui/viewer/transform.rs:102-106`（CSS transform）、`filmstrip.rs:837-945` | 符合 `docs/modules/viewer.md:98-113` 的自有契约 |
| hover-only 玻璃静止态 | `docs/modules/ui-liquid-glass.md:141-148`、`docs/modules/ui-design.md:429-464` | 「叠在照片上的主窗口控件用 hover-only」是刻意的内容优先策略，问题不在材质本身，而在缺少可发现入口与命中区尺寸（见 P0-3、P1-12） |
| 模式选择器的单胶囊模型 | `docs/modules/ui-liquid-glass.md:92-105` | canonical Liquid Glass 控件，任何整改都不得引入 per-segment 底色块 |
| 两种材质模式并行 | `liquid.css` / `plain.css` 双份材质 | 任何新增 selector 必须两处镜像，本文所有 CSS 草案都遵守此约束 |

---

## P0 检视项与优化方案

### P0-1 首次扫描期间误报「暂无照片」

**现象**：新装用户启动后看到 `empty.no_photos.title`（「暂无照片」）+「在设置中添加文件夹」的引导文案，而后台扫描正在进行。用户据此判断「配置错了」，转去改设置。

**证据（已验证）**：

- `src/ui/photos_page.rs:391` `let is_empty = media_list.n_items() == 0;` → `:533-543` 立即显示 `empty_states::no_photos()`；`:547-559` 的 `connect_items_changed` 只要 `n_items() == 0` 就切回空态。
- `src/ui/empty_states.rs:57-71` 的 `scan_error()` 与 `loading()` **全项目零调用者**（grep `empty_states::` 仅命中 `no_photos`/`empty_trash`/`no_album_photos` 三处）。`no_albums()` 同样未被使用。
- 进度文案「媒体 X · 缩略图 Y/X」只存在于旧版 `MediaGrid`：`src/ui/media_grid.rs:90 library_stats_text()`、`src/ui/media_grid/loading.rs:450-511 start_stats_refresh()`（1 秒 `timeout_add_local` 轮询 `MediaRepository::library_stats()`，`src/core/repository.rs:225`），触发点在 `src/ui/media_grid/updates.rs:834-843` 且要求 `full_library_context && mode == Day`。Photos 主网格已迁到 `VirtualMediaGrid`，拿不到这个标签。
- 全 `src/` 不存在 `scan/scanning/is_scanning/busy` 布尔状态（grep 确认）。

**影响**：首次使用体验的最大障碍；同时把「扫描失败」和「库为空」两种完全不同的事实压成同一句文案。

**优化方案**：

1. **引入可查询的扫描阶段状态**（最小侵入路线）。`DomainEvent`（`src/core/events.rs:54`）目前只有 `MediaUpserted { source, items }` / `ThumbnailStatsDirty` / `LiveCountDirty` / `SyncStateDirty`，`ChangeSource::StartupScan` 已存在（`events.rs:6`）。
   - 新增变体：`DomainEvent::ScanPhase { active: bool, error: Option<String> }`，由 `src/core/bootstrap.rs:104 scan_and_aggregate_with_actor()` 的进入/退出/Err 分支发出（该函数已持有 `DbActorHandle`，发送端走现成 `DomainEventSender`，`events.rs:99`，容量 2048 + `blocking_send`）。
   - 落点：`src/core/events.rs`、`src/core/bootstrap.rs`、`src/app.rs:131-166`（`UiRefreshHub` 分发链已经存在，`src/ui/refresh_hub.rs:26-57`，无需新建通道）。
2. **PhotosPage 三态化空态判定**。当前是「有数据 → 网格 / 无数据 → no_photos」二态。改为：

   草案（逻辑，非落盘代码）：

   ```text
   if scan_phase.active            -> empty_states::loading_stats()   // 骨架 + 计数
   else if let Some(err) = error   -> empty_states::scan_error(&err)  // + 重试按钮
   else if n_items() == 0          -> empty_states::no_photos()       // + 「打开设置」按钮
   else                            -> grid
   ```

   判定函数集中到一个 `update_placeholder_child()`，取代 `photos_page.rs:547-559` 内联的 `items_changed` 逻辑；`is_empty` 初始分支（`:539-543`）同样调用它。
3. **进度计数复用现有轮询**。把 `start_stats_refresh()` 的模式从 `MediaGrid` 抽出为共享 helper（`src/core/refresh.rs` 的 `LibraryStats { live_total, thumbnails_generated }` 已是公共类型），VirtualMediaGrid 路径只需 `ThumbnailLoader::set_stats_dirty_callback`（`src/core/thumbnails.rs:348`）触发一次 `library_stats()` 重读即可，**不要**新建第二个 1 秒定时器。
4. **空态必须带动作**。`empty_states.rs` 目前返回裸 `adw::StatusPage`，全部无子按钮。`adw::StatusPage` 支持 `set_child()`，放一个 `gtk::Button` 即可：no_photos → 「打开设置」（复用 `KeyboardAction::OpenSettings` 的处理函数，避免重复实现）；scan_error → 「重试扫描」；loading → 无按钮但显示计数。

   草案 i18n（`trf()` 见 `src/core/i18n.rs:151`）：

   ```json
   "empty.scanning.title":       "正在建立索引…",
   "empty.scanning.description": "已发现 {found} 项 · 缩略图 {done}/{total}",
   "empty.scan_failed.retry":    "重试扫描",
   "empty.no_photos.action":     "打开设置"
   ```

**落点文件**：`src/core/events.rs`、`src/core/bootstrap.rs`、`src/ui/empty_states.rs`、`src/ui/photos_page.rs`、`i18n/zh-CN.json`、`i18n/en.json`（两份各 379 键，必须同步）。

**测试**：

- 新增 `src/ui/empty_states.rs` 单测：`scan_error` 返回的 StatusPage 带子按钮且描述为传入 `msg`（对齐现有工厂的纯函数特性）。
- 新增 PhotosPage 三态判定单测：注入 `ScanPhase{active:true}` 后 `visible_child` 为 loading，`active:false + n_items=0` 才是 no_photos。
- 集成：`cargo test --test local_scan`（已存在）扩一条「扫描中不出现 no_photos」断言。
- 建议命令：`cargo test ui::empty_states ui::photos_page`。

**需同步文档**：`docs/modules/browsing.md`（空态契约）、`docs/modules/ui-design.md:151-152`（把「loading 用 status-page 风格」写成硬要求并列出三态）、`docs/modules/storage.md`（scanner 事件契约）。

**实施结果**（已落盘）：按方案实现，事件名定为 `DomainEvent::ScanPhase { active, error }`，发射走 `DbActorHandle::emit`（`DomainEventSender::send` 用 `blocking_send`，不能在 GTK 主线程调用）。页面侧新增 `PLACEHOLDER_SCANNING` / `PLACEHOLDER_SCAN_ERROR` 与 `update_placeholder_child()` 单一判定入口，`MainWindow::note_scan_phase` 负责存储与页面重建后的回放。偏差三处：

- loading 态没有「显示计数」，只有 spinner——计数在扫描期本身就是不稳定输入，改成文案说明「找到照片后会自动显示」。
- 草案里的 `empty.no_photos.description_with_count`（`{count}` 变体）未实现：count 为 0 时它没有信息量。
- 重试走 `glib::spawn_future_local` + `tokio::task::spawn_blocking`（进程已 enter 多线程 runtime），并用 `scan_retry_in_flight` 防重入。
- 测试落点：`ui::photos_page::tests::placeholders_separate_indexing_scan_failure_and_an_empty_library`、`scan_failure_text_names_the_reason_when_there_is_one`。

**风险**：新增 `DomainEvent` 变体会让所有 `match` 分支编译失败——`apply_domain_event_to_legacy_ui`（`src/app.rs:131-166` 附近）与 `refresh_hub` 订阅者需一并处理；这是好事，能强制覆盖完整。

---

### P0-2 主照片网格键盘焦点完全不可见

**现象**：Photos 网格里方向键能移动焦点，但屏幕上看不出焦点在哪个瓦片上；此时 `Space`（选中）与 `Delete`（移入回收站）会作用在用户看不见的项目上。

**证据（已验证）**：

- `data/css/a11y.css` 只声明 9 个 chrome 选择器的 `:focus-visible` 环（`outline: 2px solid @accent_color`），**没有任何 `gridview` 规则**。
- `data/css/base.css:155-168` 主动清掉了主题默认的 `> child` 边框/背景（`gridview.virtual-media-grid-view > child:selected { background: transparent; border: 0; box-shadow: none; }`）。
- FlowBox（旧网格/搜索结果）有环：`base.css:68-71`，并有 `pointer-left`/`kbd-nav` 抑制逻辑（`:93-95`，Rust 侧 `src/ui/grid_css.rs:480-525 attach_kbd_nav`）。
- **关键细节**：虚拟网格中可聚焦的节点是**瓦片本身**而不是 GridView 的 `child` 包装——`src/ui/virtual_media_grid/factory.rs:71 tile.set_can_focus(true)`，注释说明这是为自定义右键菜单归还焦点。因此照抄 FlowBox 的 `> child:focus > .glass-thumb-card` 选择器**不会命中**。
- 与项目自有契约冲突：`docs/modules/ui-design.md:175` 要求「Tile hover, selection, and **focus** states should be visible but restrained」。

**影响**：这是「不可见焦点 + 破坏性快捷键」的组合，属于最高风险的可用性缺陷；纯键盘用户与读屏用户都无法安全使用主网格。

**优化方案**：

在 `a11y.css` 末尾追加（该文件按 `src/ui/grid_css.rs:88-93` 的拼接顺序**排在最后**，同特异性下天然覆盖 base.css；且 base.css:253-266 那两个高特异性块只重置 `border-color/box-shadow`，不碰 `outline`，所以用 `outline` 表达焦点最安全）：

```css
/* 草案：虚拟网格的可聚焦节点是瓦片本身（factory.rs:71），不是 GridView child。
   用 :focus-visible 复用 GTK 自身的键盘判定，避免指针移动时出现残影环，
   因而无需为 GridView 复制 FlowBox 的 kbd-nav/pointer-left 机制。 */
gridview.virtual-media-grid-view > child > .glass-thumb-card:focus-visible {
  outline: 2px solid @accent_color;
  outline-offset: -2px;
}
```

配套决策与加固：

1. **环必须叠在暗罩之上**。焦点态常常同时是 hover/selected 态（`base.css:231-247` 的暗罩），accent 色叠在照片上可能对比不足。方案：`outline-offset: -2px` 让环画在瓦片内侧，并在 base.css 增加一条同优先级的 `box-shadow` 内侧暗描边（`inset 0 0 0 1px alpha(black, 0.55)`）以保证 accent 在白底照片上仍可辨。该描边属于状态层，不违反「一交互语言」约束。
2. **Plain 模式覆盖检查**。`outline` 定义在 `a11y.css`（材质无关层，见 `docs/modules/ui-liquid-glass.md:16`），两种模式共用，**不需要**在 liquid/plain 两处镜像——这一点要写进注释，否则后来者会误以为漏了镜像而把它移进材质块。
3. **不要用透明度滑杆影响焦点环**。`docs/modules/ui-liquid-glass.md:79` 已规定「Text, icons, and focus outlines are independent of the transparency slider」，此规则必须维持。
4. **顺带补齐 FlowBox 的一致性**：搜索页仍用 FlowBox（`src/ui/media_grid`），其环为 `alpha(@window_fg_color, 0.55)`（`base.css:69`）而 chrome 用 `@accent_color`。统一为 accent 色，两处选择器一起改，避免同一应用两种焦点语言。
5. **回归防线**：`src/ui/grid_css/tests.rs:1130 both_modes_share_base_and_a11y` 已经在守护 base/a11y 共享性，新增断言：装配后的 CSS 字符串必须包含 `glass-thumb-card:focus-visible`。这是最便宜的防回归手段。

**落点文件**：`data/css/a11y.css`、`data/css/base.css`（内侧描边）、`src/ui/grid_css/tests.rs`、`src/ui/grid_css/tests/render.rs`。

**测试**：`cargo test ui::grid_css`；再用 `tools/assert-at-spi.py` 思路人工确认（见 P2-8）。

**需同步文档**：`docs/modules/ui-liquid-glass.md`（焦点环属 a11y 层、与透明度无关）、`docs/modules/ui-design.md`（Tile focus 契约落地）、`docs/ui-naming-reference/index.html`（新增 selector 与状态需登记）。

**风险**：`SquareTile` 是自绘控件，需确认 GTK 的 `:focus-visible` 状态位能到达它（`can-focus` 已为真，通常可用）；若 GTK 未置 `focus-visible` 位，退路是复用 `attach_kbd_nav` 的 class 机制，把该 helper 从「只接受 FlowBox」泛化为接受任意容器 + 焦点节点选择器参数（`src/ui/grid_css.rs:480-525`）。

**实施结果**（已落盘，方案被实测证据改写）：草案的「`outline` + `inset box-shadow` 暗描边 + `@accent_color`」组合实测无效，两条都是错的：

- GTK 把 widget 的 `box-shadow: inset …` 画在内容**之下**，缩略图直接盖掉它。探针实测 `shadow-only` / `two-layer` 两组像素全白，只有 `outline` 能到达照片。因此暗描边改由 list-item wrapper 承接：`gridview.virtual-media-grid-view > child:focus / :focus-within { outline: 1px solid alpha(black, 0.85) }`，瓦片节点上是 `outline: 3px solid @accent_bg_color; outline-offset: -3px`。
- `@accent_color` 在本机会析出成 `[255,208,129]` 的浅橙，压在亮照片上等于消失；它是 libadwaita 的「accent 上的可读文字色」，饱和色是 `@accent_bg_color`。FlowBox 网格同步改用后者，保持一套焦点语言。
- `GtkGridView` 把焦点据在它内部的 list-item wrapper 上，并**拒绝**瓦片成为 focus widget（`tile.grab_focus()` 返回 false，`set_focus(window, tile)` 后 `window.focus()` 为 None）。所以渲染测试快照取自 wrapper（它同时包含 hairline 与 accent 环），瓦片的 `:focus` 选择器只服务 `glass_context_menu.rs` 里直接 `grab_focus()` 的退路。
- 测试改为色彩无关断言（accent 可被用户换掉）：环带与白底的最大通道差 ≥ 60、hairline 落在近黑区、内芯 8px 不变、周边重绘像素 ≥ `2*(w+h)`。落点 `ui::grid_css::tests::render::virtual_grid_tile_focus_ring_renders`。

---

### P0-3 多选只能靠右键进入，无入口提示、无触控回退

**现象**：左键 = 打开查看器。进入多选的**唯一**入口是右键菜单里的「Multi-select」；进入后界面没有任何提示说明「点击语义已从打开变为选中」。触屏/触控板用户没有右键，等于无法批量操作。

**证据（已验证）**：

- `src/ui/virtual_media_grid.rs:1130-1141`：非多选态的右键菜单项 `photos.batch.multi_select` 是唯一入口（`GlassMenuItemKind::Suggested` → `set_multi_select_mode(true)`）。
- `src/ui/virtual_media_grid.rs:494`：橡皮筋框选显式关闭。
- `src/ui/virtual_media_grid/factory.rs:76-88`：只有 `GestureClick::new()` + `set_button(3)`，**没有 `GestureLongPress`**。
- `src/ui/virtual_media_grid.rs:1909-1913`：进入多选后 `activate_slot` 从「打开」翻转为「切换选中」。
- header 侧已有多选态退出按钮的完整模式可参考：`data/ui/photos-page.blp:56-64` `exit_multi_select_revealer` / `exit_multi_select_btn`，由 `src/ui/photos_page.rs:1068、:1094` 按 `any_multi` 控制 reveal。

**影响**：桌面相册应用的核心能力（批量加入相册/收藏/删除）发现率接近零；且语义翻转无提示会造成「我点了怎么没打开」的困惑。

**优化方案**：

1. **常态可见的「选择」入口按钮**（主方案）。复用 header 既有 revealer 模式，在 `[start]` 组新增一个与 `search_btn` 同级的按钮：

   草案（`data/ui/photos-page.blp`，插在 `search_btn` 之后、`select_all_revealer` 之前，保持 `[start]` 声明顺序即视觉顺序）：

   ```text
   [start]
   Gtk.Button select_mode_btn {
     icon-name: "check-select-symbolic";   // 若图标主题缺该符号，退路：selection-mode-symbolic
     tooltip-text: "";                     // 运行时按仓库惯例 set_tooltip_text(tr(...))
     css-classes: ["glass-toolbar-button", "round-search-button"];
   }
   ```

   - 点击 → `grid.set_multi_select_mode(true)`（`virtual_media_grid.rs:680` 已是 `pub fn`，无需新 API）。
   - 与 `exit_multi_select_btn` 形成显式进入/退出的对称；进入后按钮自身收起（复用 `any_multi` 判定，`photos_page.rs:1069-1073`）。
   - 尺寸/材质沿用 `round-search-button`，符合 `docs/modules/ui-liquid-glass.md:145-148` 的 hover-only 作用域规则，不需要新 selector；若需要新 class，则必须在 `LIQUID_GLASS_MATERIAL_CSS` 与 `PLAIN_GLASS_MATERIAL_CSS` 两处镜像。
   - 注意 `Adw.HeaderBar` 的 `[start]` 打包顺序注释（`photos-page.blp:11-15, 34-35`）：AlbumDetailPage 的搜索按钮位置是刻意对齐的，新增按钮要同步检查 `data/ui/album-detail-page.blp` 是否需要同一入口，否则「同样的行为在两个页面不同位置」。
2. **补长按回退**：在 `factory.rs` 的 tile 上追加 `gtk::GestureLongPress`，`set_touch_only(true)`（只影响触屏，不与鼠标左键双击冲突），`connect_started` 走与右键菜单相同的 `show_context_menu(tile, &binding, x, y)`。这样多选入口在触屏上等价可达，且不引入新的可见控件。
3. **多选态要有身份**。进入多选时在 header 标题区显示「已选择 N 项」（与 P1-6 合并实施，见下），并让模式选择器/总览等次要 chrome 保持但不争抢注意力。
4. **保留右键入口**（不删），它现在是唯一的「快速入口」，只是不能再是**唯一**入口。

**落点文件**：`data/ui/photos-page.blp`、（很可能）`data/ui/album-detail-page.blp`、`src/ui/photos_page.rs`、`src/ui/virtual_media_grid/factory.rs`、`src/ui/media_grid.rs`（旧网格同样需要长按回退）、`i18n/*.json`。

**测试**：`cargo test ui::photos_page`（新增：点 `select_mode_btn` 后三个 grid 的 `is_multi_select_mode()` 为真、`exit_multi_select_revealer.reveal_child` 为真）；`cargo test --test e2e_browsing`。

**需同步文档**：`docs/modules/browsing.md`（多选进入路径）、`docs/modules/ui-design.md:105-109`（原文「selection actions appear only when the user has selected media」把入口条件写成了循环依赖，需改写为「进入选择有一个常驻入口；选择动作按钮仅在有选择时出现」）、`docs/ui-naming-reference/index.html`。

**风险**：header `[start]` 已挤了 search + 两个 revealer，窄窗口下可能换行或裁切。需在 800×600 与 1280×800 两档目测；必要时把 `select_mode_btn` 放到 `[end]` 的最左位（注意 `[end]` 是 edge-first 反向声明，见 `photos-page.blp:11-15` 的注释）。

---

### P0-4 快捷键体系完整，但应用内 100% 不可发现

**现象**：查看器绑了 13 个键，但应用里没有任何地方能看到它们；tooltip 也只有动作名不含按键。用户只能靠读 `docs/modules/keyboard.md` 才知道有 `F`/`I`/`E`/`H`。

**证据（已验证）**：

- 键位表：`src/ui/keyboard/binding.rs:76-189`。全局 `Ctrl+F` 搜索、`Ctrl+,` 设置、`Alt+←` 返回；browsing 作用域方向键/`Return`/`Space`/`Ctrl+A`/`Delete`；viewer 作用域 `←/→`、`Esc`、`Space`、`+`/`=`/`-`/`0`、`R`/`Shift+R`、`F`、`I`、`E`、`H`、`Delete`。
- 全项目无 `GtkShortcutsWindow`、无 `Ctrl+/`（grep `ShortcutsWindow` 零命中）。
- tooltip 不带键名：`i18n/en.json:37-42` 形如 "Zoom In"。
- 键位只存在于 `docs/modules/keyboard.md:60-69`。

**影响**：一个专为键盘设计的桌面应用，却把它的键盘能力藏起来了；新功能的可发现性也随之归零。

**优化方案**：

1. **新增 `gtk::ShortcutsWindow`**（GTK4 原生，零自定义 chrome，不与 Liquid Glass 契约冲突）。在代码里构造（项目 blp 模板不含该窗口，代码构造也更符合 `KeyboardAction` 的单一事实源）：
   - 新文件 `src/ui/keyboard/shortcuts_window.rs`，按三个 section 分组：全局 / 浏览 / 查看器与编辑，与 `binding.rs` 的三个函数（`global_binding`/`browsing_binding`/`viewer_binding`）**一一对应**，注释里写明对应关系，避免两处漂移。
   - 每个 `gtk::ShortcutsShortcut` 设 `accelerator`（如 `"<Ctrl>F"`、`"plus"`）与 `title`（走 `tr()`）。
   - 触发：`F1` 与 `Ctrl+/`（需在 `KeyboardAction` 枚举加 `ShowShortcuts`，`src/ui/keyboard/action.rs:1-28`），并在设置对话框 Appearance/About 附近加一行「键盘快捷键」按钮打开它。
   - 挂载：`ShortcutsWindow` 需 `set_transient_for(window)` + `set_modal(true)`。
2. **tooltip 附带键名**（低成本，立刻提升发现性）。仓库已有惯例：GNOME 系应用把键名写进 tooltip。方案是新增 i18n 键 `*.tooltip.key`，或用现有 `tr()` 结果与一个键名常量拼接（集中一处函数，避免散落）：

   草案：`viewer_page.rs:467-500` 的 11 个 tooltip 从 `tr("viewer.tooltip.zoom_in")` 改为 `tr_with_key("viewer.tooltip.zoom_in", "+")`，其中 `tr_with_key` 是新增的 `src/core/i18n.rs` 小 helper（`format!("{label} ({key})")`，本地化时注意 `zh-CN` 用全角括号或中括号）。

   注意：`docs/modules/keyboard.md` 应升级为「同一张表的文档镜像」，并加一句「新增/修改 binding 必须同步 shortcuts_window.rs」——真正的单一事实源应让 `shortcuts_window.rs` **由 `binding.rs` 的表生成**（可在测试里断言两者集合相等，见下）。
3. **防漂移测试**（最有价值的一步）：新增单测断言 `ShortcutsWindow` 声明的 accelerator 集合 == `binding.rs` 三张表实际可解析出的 action 集合。这样将来加键忘加文档会直接红。

**落点文件**：`src/ui/keyboard/shortcuts_window.rs`（新增）、`src/ui/keyboard/action.rs`、`src/ui/keyboard/binding.rs`、`src/ui/keyboard/mod.rs`、`src/ui/window.rs`（挂载与 transient）、`src/core/i18n.rs`、`src/ui/window/settings.rs`、`i18n/*.json`。

**测试**：`cargo test ui::keyboard`（含新的集合相等断言）；`cargo test --test e2e_viewer` 补一条「F1 打开快捷键窗口且不吞掉 viewer 的 F 键」。

**需同步文档**：`docs/modules/keyboard.md`（增加「应用内发现」一节并声明防漂移测试）、`docs/ui-naming-reference/index.html`。

**风险**：`F1` 当前是否已被占用需确认（`binding.rs` 未见 F1，安全）；`Ctrl+/` 在部分布局需要 Shift，考虑同时接受 `Ctrl+question`。另外 `ShortcutsWindow` 在 `GtkSettings:gtk-shell-shows-app-menu` 等设置下的行为差异属已知，不影响 transient 用法。

---

### P0-5 搜索页：零结果不留任何痕迹 + 模板硬编码中文

**现象**：搜索无命中时，页面只剩搜索框和三个分段标签，下方完全空白。英文界面用户看到的三个分段标签是中文「全部/文件名/日期」。

**证据（已验证）**：

- `src/ui/search_page.rs:469-470`：`image_results_box`/`video_results_box` 仅 `set_visible(has_images/has_videos)`，两者皆 false 时无任何提示；`:137-138` 初始化同样只隐藏。
- `data/ui/search-page.blp:36/42/47`：`label: "全部"`、`"文件名"`、`"日期"` 写死在模板里；`i18n/en.json` 无对应键（已 grep）。同页其余文案都走 `tr()`：`search_page.rs:107/111/123/130/368/564/568`。
- **约束警告**：`tools/assert-at-spi.py:19` 硬编码 `SEARCH_FIELD_NAMES = ("全部", "文件名", "日期")`，即读屏冒烟测试**依赖中文标签**。i18n 化后该脚本必须改，否则 a11y 冒烟在英文 locale 下失败。
- 搜索无 spinner：`search_page.rs:237-245` 依赖 `GtkSearchEntry` 内建 `search-changed` 延迟，无自研 debounce；`:388-447` 用 generation counter 防竞态（这块做得好，保留）。

**影响**：「什么都没找到」是搜索产品最基本的反馈契约；硬编码中文则直接破坏双语产品的语言一致性。

**优化方案**：

1. **补三态**（查询前 / 零结果 / 有结果）：
   - 零结果：新增 `empty_states::no_search_results(query: &str)`，图标 `system-search-symbolic`，标题用 `trf("empty.search_none.title", &[("query", q)])`，子按钮「清除搜索」（`search_entry.set_text("")` 并回焦）。
   - 查询前（空串）：显示一个轻量提示态而非空白（复用同一 StatusPage，文案「输入文件名或拍摄日期」）。
   - 查询中：`>300ms` 起一个 `adw::Spinner`，放在结果区顶部而非整页替换，避免网格闪断。
   - 落点：`src/ui/search_page.rs:460-500` 的结果可见性判定改为 `match (query_empty, has_images||has_videos) { … }`，在 `content_box` 里增加一个 `gtk::Stack`（`search-page.blp` 现有 `content_box` 结构可直接插入）。
2. **i18n 化三个分段标签**：`search-page.blp` 改 `label: ""`，在 `search_page.rs:145-152`（已取得三个 `TemplateChild`）之后统一 `set_label(&tr("search.field.all" / ".name" / ".date"))`。新增 4 个键（含结果态文案），两份 json 同步。
   - 分段控件的 `set_group` 行为（`:148-149`）不受影响，`ui-liquid-glass.md:108-110` 记录的原生 toggle 分组语义保留。
3. **改 `tools/assert-at-spi.py`**：把 `SEARCH_FIELD_NAMES` 换成从 `i18n/<locale>.json` 读取当前 locale 的三段标签（脚本已 `import json`? 需补），或接受 `--labels` 参数；同时在 `tests/visual_check_script.rs:73-96` 的 `--a11y-smoke` 断言处确认无中文硬编码。
4. **顺带**：结果分段标题（`search.images`/`search.videos`）已 i18n，无需改动。

**落点文件**：`src/ui/search_page.rs`、`data/ui/search-page.blp`、`src/ui/empty_states.rs`、`i18n/*.json`、`tools/assert-at-spi.py`、（可能）`tests/visual_check_script.rs`。

**测试**：`cargo test ui::search_page`（零结果 → 可见 StatusPage；查询前 → 提示态）；`cargo test --test flatpak_a11y`/a11y 冒烟脚本相关测试；人工双 locale 目测三个标签。

**需同步文档**：`docs/modules/browsing.md`（搜索三态）、`docs/ui-naming-reference/index.html`（新 stack/StatusPage 命名）、`AGENTS.md`（若把「模板禁止硬编码文案」升级为规则——建议升级，见文末建议）。

**风险**：`tools/assert-at-spi.py` 是黑盒 AT-SPI 探针，改动会降低其对真实标签漂移的敏感度；用「从 i18n json 取期望值」而不是「放宽断言」来避免这一点。

---

## P1 检视项与优化方案

### P1-6 选中集被后台重建静默清空，且从不显示已选数量

**现象**：用户选了几十张准备加入相册，文件系统 watcher 落地一次扫描 → 选择全部丢失，无提示。多选期间也永远看不到「选了多少」。

**证据（已验证）**：

- `src/ui/media_grid/loading.rs:562`（`schedule_rebuild` 的 750ms 定时器回调内）与 `:591`（`rebuild_immediately`）都在 `rebuild()` 前无条件 `this.clear_selection()`；两处**都没有先读 `selected_ids()`**，id 集合直接被丢弃。
- 可恢复性已确认：`selected` 是 imp 里的 `RefCell<HashSet<MediaId>>`，`clear_selection()`（`virtual_media_grid.rs:710-715`）之前可读；`select_ids(&[MediaId])`（`:692-708`）能整体重建集合并 `sync_visible_selection`。
- 上限截断不可见：`photos_page.rs:1337` 的 Select All 走 `repo.items(LiveAll, 0, 2000)`，`photos_page.rs:1350` 已算出 `selected_count` 但从不渲染；`selected_reaches_select_all_limit`（`:1360`）只有内部逻辑。
- 计数标签在 Photos/Trash 都不存在：`data/ui/trash-page.blp` 的 action bar（`src/ui/trash_page.rs:91/226`）只有按钮文本。

**方案**：

1. **重建前捕获、重建后重放**。在 `loading.rs:562/591` 两处改为「捕获 → rebuild → 重放」，并注意 `select_ids(&[])` 会把 `is_multi_select_mode` 置 false（`virtual_media_grid.rs:693`）——空集合时必须显式保留多选模式：

   草案：

   ```text
   let kept = this.selected_ids();
   let was_multi = this.is_multi_select_mode();
   this.rebuild(media_list, this.mode());
   if kept.is_empty() { if was_multi { this.set_multi_select_mode(true) } }
   else { this.select_ids(&kept) }
   ```

2. **显示已选数量 + 截断说明**。在 `photos_page.rs:1062 refresh_selection_ui()` 里（它已经算出 `union`）新增/更新一个 header 标签 `selection_count_label`，文案 `trf("photos.selection.count", &[("n", n)])`；命中 2000 上限时追加「（已达上限）」，复用 `select_all_limit_reached`（`:1337-1360` 区域）。标签放 `[start]` revealer 组，遵循同一 reveal 时机。
3. **不要清空选择的替代设计**：如果 `rebuild()` 必然要换 model，则把「按 id 重放」写成 `VirtualMediaGrid::preserve_selection_across_rebuild` 的内部职责，而不是让调用方每处手写，避免第三个调用点将来再犯。

**落点文件**：`src/ui/media_grid/loading.rs`、`src/ui/virtual_media_grid.rs`（重放 helper）、`src/ui/photos_page.rs`、`data/ui/photos-page.blp`、`i18n/*.json`。

**测试**：新增「选择后触发 schedule_rebuild，选择集合保持不变」的网格单测；`cargo test ui::photos_page`（计数标签在 n=0/1/2001 三档文案正确）。

**需同步文档**：`docs/modules/browsing.md`（选择生命周期）、`docs/modules/ui-design.md:105-109`。

---

### P1-7 查看器缺少三项基本定位信息

**现象**：查看器里不知道「我在第几/共几张」；按到底时界面毫无反应（看起来像卡住）；放大后不知道当前倍率。

**证据（已验证）**：

- header 只有文件名标题（`viewer_page.rs:824 set_title(item.display_name())`）与日精度日期（`:825` → `src/ui/viewer/details.rs:276 update_date_label()`，标签 `date_label` 见 `data/ui/viewer-page.blp:26`）；全项目无位置计数器。
- 末尾无反馈：`src/ui/viewer/navigation.rs:162` `Ok(None) => {}` 空分支；`Err` 才 `this.fire_nav(delta)`。
- 倍率不可见：`viewer_page.rs:66-68` MIN 1.0/MAX 8.0/STEP 1.25，`transform.rs:47-88` 内部 `Cell<f64>`，无标签。
- **重要陷阱**：`viewer_page.rs:643 list_n_items()` 返回的是**窗口化 store** 的长度（`viewer/navigation.rs:289 ensure_media_item_in_window`），不是全库总数，不能直接当总数用。可用真源：`src/core/repository.rs:136 count(query)` 与 `:165 page(query, start, limit) -> MediaPage`，而 `MediaPage` **已带 `total: u32`**（`src/core/repository.rs:34-39`）。

**方案**：

1. **位置计数器**：在 header 日期标签右侧加 `position_label`（`viewer-page.blp:26` 附近的 `[start]` box），格式「{current} / {total}」。
   - `total` 来自打开查看器时那次查询的 `MediaPage.total`（`viewer_page.rs` 已持有 `db_actor` 与 `MediaQuery`，见 `new_for_query(MediaQuery::LiveAll, …)` 调用点 `photos_page.rs:1743`），异步取一次并缓存，**不要**每次 `show_at` 重查。
   - `current` 优先用同一 `MediaPage`/窗口窗口的局部序号 + 窗口起始 offset；若 offset 不可靠，用一次 `items(query, start, limit)` 定位。取不到确切全局序号时，**宁可退化为窗口内「1 / 128」也不要显示错误数字**，并在代码注释写明。
2. **到底反馈**：把 `viewer/navigation.rs:162` 的空分支改为可见反馈——两端时禁用 `prev_btn`/`next_btn`（`set_sensitive(false)`）并让键盘 `←/→` 在无目标时不再吞事件。禁用态视觉按 libadwaita 默认（约 0.4 不透明度）即可，无需新 CSS。
3. **倍率指示**：`zoom_scale != 1.0` 时在 `viewer_zoom_controls` 右侧显示 `trf("viewer.zoom.level", &[("percent", …)])`；`reset_viewer_transform()`（`transform.rs:60`）后隐藏。用 tabular 数字，避免宽度跳动（`format!` 前把 `%` 留在文案侧以便本地化）。
4. **视觉成本控制**：三者都是 label/sensitive 变更，符合 `docs/modules/viewer.md:98-113`「只用 transform/outline/shadow，不动布局」的约束；计数器 label 需要固定最小宽度以免翻页时 header 抖动。

**落点文件**：`data/ui/viewer-page.blp`、`src/ui/viewer_page.rs`、`src/ui/viewer/navigation.rs`、`src/ui/viewer/transform.rs`、`i18n/*.json`。

**测试**：`cargo test --test e2e_viewer`（首/末张的按钮敏感度、计数与窗口长度一致性）；`cargo test ui::viewer`（倍率标签在 1.0/1.25/8.0 的文本）。

**需同步文档**：`docs/modules/viewer.md`（新增 header 信息契约与「total 来源是 MediaPage.total 而非窗口 store」这条陷阱）、`docs/ui-naming-reference/index.html`。

---

### P1-8 图片只能点按钮缩放，无滚轮/捏合缩放与拖拽平移

**现象**：桌面图片查看器的肌肉记忆是 `Ctrl+滚轮` 缩放 + 拖拽平移；这里只能点 `+`/`-` 每次 ×1.25，放大后无法移动画面。

**证据（已验证）**：

- image stage 上**当前没有任何控制器**：`src/ui/viewer/stage.rs:730` 只有 `video.add_controller(click)`；`ui/viewer/crop.rs:70` 的 drag 只挂在裁剪 overlay。
- 载体是 `Gtk.Picture picture`（`data/ui/viewer-page.blp:99`，在 `Gtk.Overlay image_overlay` 内，`can-shrink: true`，`content-fit: contain`）。
- 缩放状态是 imp 的 `Cell`：`viewer_page.rs:204 zoom_scale`、`:209/:210 zoom_pan_x/zoom_pan_y`、`:207 viewer_rotation_degrees`；`transform.rs:73 set_viewer_zoom(scale, pan_x, pan_y)` 是私有（测试钩子 `:91 set_viewer_zoom_for_tests`）；`transform.rs:136 clamp_zoom_pan(...)` 因 pan 恒为 0 而实际不可达（`:56-63` 只重置）。
- **反向约束**：`src/ui/viewer/transform/tests.rs:13-37` 是负向断言，遍历 `image_overlay.observe_controllers()`，要求不存在 `gtk::GestureZoom`/`gtk::GestureDrag`，消息为「image overlay should not install touch pinch zoom while buttons own zoom actions」。`docs/modules/viewer.md:204` 同样写明「Do not install touch-only pinch, pan, or global swipe controllers on the viewer image stage, because they compete with overlay buttons and keyboard-driven actions」。

**方案（需要一次设计决策，不是纯实现）**：

1. **先改契约**。文档与那条负向测试的理由是「**touch-only** 控制器与 overlay 按钮竞争」。`Ctrl+滚轮`（精确指针）与「仅在 `scale > 1.0` 时启用的拖拽平移」并不属于 touch-only，与理由不冲突。因此把 `viewer.md:204` 与测试注释收窄为「禁止 touch-only 的捏合/全局滑动；桌面滚轮缩放与放大后的拖拽平移是允许且推荐的」，并把测试从「禁止任何 drag/zoom 控制器」改为「禁止**无按钮修饰条件**的 touch 捏合控制器」+ 保留一条「fit 态（scale==1.0）拖拽不改变 pan」的断言。
2. **加缩放控制器**：`gtk::EventControllerScroll`（`ScrollFlags::SMOOTH | VERTICAL`）挂在 `image_overlay`（不是 `picture`，以拿到整块 stage 区域），仅当 `combo` 含 Ctrl 或触摸板捏合相位时才步进；步长复用 `step_zoom(current, direction)`（`transform.rs:127`），避免两套倍率逻辑。滚轮事件在无 Ctrl 时应 `Propagation::Proceed`，让普通滚轮仍走页面滚动，防止劫持。
3. **加平移**：`gtk::GestureDrag` 同挂 `image_overlay`，`connect_drag_update` 里 `set_viewer_zoom(scale, pan_x + dx, pan_y + dy)` 并走既有 `clamp_zoom_pan`（这条路径终于可达，需要单测覆盖）。`scale == 1.0` 时直接 `Proceed` 不消费，保持「fit 态无平移」的原设计意图，同时避免与单击打开/关闭手势冲突（当前图片上根本没有 click 控制器，冲突面极小）。
4. **加触摸捏合（可选，后置）**：`GestureZoom` 只在有触摸输入的窗口启用；若维持原契约拒绝它，就跳过本步——把决策记录进 `viewer.md`。
5. **连续性**：缩放/平移全部经现有 CSS `transform` 通道（`transform.rs:102-106`），不引入 reflow；顺带把 `zoom_provider`（`viewer_page.rs:216`）的每帧 CSS 重装改为节流（跟随指针 16ms 预算），否则连续拖拽会高频重建 provider。

**落点文件**：`src/ui/viewer/transform.rs`、`src/ui/viewer/transform/tests.rs`、`src/ui/viewer/stage.rs`（或新 `viewer/input.rs`）、`src/ui/viewer_page.rs`、`docs/modules/viewer.md`。

**测试**：改写 `transform/tests.rs`（正/负断言各一条）+ 新增「pan 被 clamp 到视口边界」的单测；`cargo test --test e2e_viewer`。

**风险**：这是本文唯一**需要先推翻既有文档决策**的项。若决定不做，请在 `viewer.md` 里显式写出「有意不提供滚轮缩放」的理由，以免后续检视反复提出同一问题。

---

### P1-9 编辑器退出既不提示脏、也不确认，改动静默丢失

**现象**：调完曝光后点关闭/Cancel/按 `Esc`，图片直接回到原图，没有任何询问。唯一的「脏」信号是一个图标按钮变亮。

**证据（已验证）**：

- `src/ui/editor_panel.rs:423-439`（关闭/Cancel 路径）→ `src/ui/viewer/editor.rs:71-105 stop_editing` → `:88-90` 无条件恢复原纹理。`viewer_page.rs:547` 的 `Esc` 同样直达。
- 脏态唯一出口是 reset 按钮敏感度：`editor_panel.rs:723-726`（`has_pending_edits()`）。
- 无前后对比：`editor_panel.rs`/`editor-panel.blp` 里没有 hold-to-compare/split 的实现。
- 已有可复用确认模式：`viewer/actions.rs:21-30` 的 `adw::AlertDialog` + `glass-alert-dialog` + 默认/关闭响应为 cancel。

**方案**：

1. **集中退出路径**。当前三处（header 关闭、footer Cancel、`Esc`）各自调 `fire_close`。改为经一个 `attempt_close(&self)`：

   草案：

   ```text
   attempt_close:
     if !has_pending_edits() { do_close(); return }
     AlertDialog(heading = tr("editor.unsaved.heading"),
                 body    = tr("editor.unsaved.body"))
       add_response("discard")   -> Destructive
       add_response("keep")      -> default + close_response
     仅在 response == "discard" 时 do_close()
   ```

   复用 `viewer/actions.rs:21-30` 的构造顺序与 `set_close_response("keep")`（关键：`Esc` 关闭对话框默认必须是「留在编辑」，否则确认对话框会被 `Esc` 绕过）。
2. **可见脏标记**：footer Cancel 旁或面板标题下加一行 `editor_dirty_label`（「有未保存的修改」），`has_pending_edits()` 变化时同步（同一个函数驱动 reset 敏感度与标签，避免漏更新）。
3. **前后对比**：footer 加一个 `compare_btn`（`Gtk.ToggleButton`，图标 `image-x-generic-symbolic` 或 `view-split-symbolic`），按住/切换时把面板参数临时置为「单位变换」并复用现成 33ms 单飞预览（`editor_panel.rs:864-890`），不新增渲染路径。
4. **保存反馈已具备**，无需改：`editor_panel.rs:675-685 set_saving` 已把 save/cancel/close 置不可敏感，`:908/:952` 有 spinner。

**落点文件**：`src/ui/editor_panel.rs`、`src/ui/viewer/editor.rs`、`src/ui/viewer_page.rs`（`Esc` 路由）、`data/ui/editor-panel.blp`、`i18n/*.json`。

**测试**：`cargo test --test edit_state`（已存在）补「有 pending edits 时 attempt_close 不退出」；`cargo test --test destructive_rotate`（已存在）确认没有回归；`cargo test --test e2e_editor`。

**需同步文档**：`docs/modules/editor.md`（退出确认契约 + 对比按钮）、`docs/modules/ui-design.md:282-303`（Editor 一节）。

---

### P1-10 大图解码失败 → 空白无提示

**现象**：原图解码失败时（文件损坏/已移动/权限），舞台留白，没有文字。缩略图失败在主网格里表现为「完全空白的格子」。

**证据（已验证）**：

- `src/ui/viewer/stage.rs:672-682`：原始图解码 Err 分支只 `warn` + 收起 spinner；预览也失败时 `:591-637` 无内容可画，舞台留空。
- 错误 UI 只给视频：`data/ui/viewer-page.blp:128-163` 的 `video_error_box`（样式 `base.css:758-772`），图片无对应物。
- 网格侧：`data/css/base.css:996` `.glass-thumb-card.thumb-loading:not(.thumb-placeholder){opacity:0}`（镜像逻辑 `src/ui/media_grid/render.rs:41-48`）——失败瓦片不可见。

**方案**：

1. **图片错误态**：把 `video_error_box` 泛化为 `media_error_box`（同一 `[overlay]` 兄弟节点、同一 `.viewer-video-error`→`.viewer-media-error` class 家族），在 `stage.rs:672-682` 的 Err 分支显示：图标 `image-missing-symbolic` + 文件名 + 一句原因 + 「在文件管理器中显示」/「重试」按钮。注意 `docs/modules/ui-liquid-glass.md:126` 要求新 selector 两处材质镜像。
   - 文案与 P0-1 的 `scan_error` 用同一措辞风格：说明原因 + 如何修复。
2. **网格失败瓦片可见化**：`thumb-loading`/失败态改为显示 `image-x-generic-symbolic` 占位（去掉 `opacity: 0`），保留 `.thumb-placeholder` 骨架路径不变（`factory.rs:275-306` 的骨架是有效的）。需要一个新 class（如 `.thumb-broken`）区分「还在加载」与「失败」，否则把 spinner 常驻会误导。
3. **状态来源**：`ThumbnailLoader` 当前只暴露 `set_stats_dirty_callback`（`src/core/thumbnails.rs:348`），无失败回调。方案是给 loader 增加「按 media_id 请求失败」的回调或在 `SquareTile` 上直接置 `.thumb-broken` class（后者不动 core，优先）。

**落点文件**：`data/ui/viewer-page.blp`、`src/ui/viewer/stage.rs`、`data/css/base.css`、`data/css/liquid.css`、`data/css/plain.css`、`src/ui/square_tile.rs`、`src/ui/virtual_media_grid/factory.rs`、`i18n/*.json`。

**测试**：`cargo test --test e2e_viewer`（不存在/损坏文件 → 可见错误框）；`cargo test ui::grid_css`（新 class 在两材质块都存在，沿用 `grid_css/tests.rs:1130` 的镜像断言风格）。

**需同步文档**：`docs/modules/viewer.md`、`docs/modules/storage.md`（缩略图失败语义）、`docs/ui-naming-reference/index.html`。

---

### P1-11 悬停态与选中态视觉同构，指针用户无法区分

**现象**：鼠标扫过的格子和已勾选的格子用的是同一层暗罩，差别只有右下角对勾的不透明度。

**证据（已验证）**：

- `data/css/base.css:231-247` 注释即「Pointer emphasis and multi-selection are one interaction language」，把 `.media-selected` / `.thumb-pointer-hover` / flowbox hover / gridview hover 归入同一 background+shadow 组。
- `:253-266` 进一步把两态的 `border-color: transparent; box-shadow: none`，抹掉了本可区分二者的环。
- 对勾靠 `flowboxchild:selected .thumb-checkmark { opacity: 1 }`（`docs/modules/ui-design.md:178-184`，实现 `base.css:111-140/187`）区分。

**方案**（保持材质一致，只加状态层）：

1. **让选中态可跨屏识别**：对勾常驻——未选中 `opacity: 0.32`，选中 `opacity: 1`；或者给选中态单独一条 accent 内环（`inset 0 0 0 2px @accent_color`）。前者改动最小且不动材质。
2. **hover 只保留极轻一层**：把 hover 的暗罩强度降到当前值约一半，选中态保持现有强度。注意这会修改 `ui-design.md:176-177` 的明文契约，需要同步更新文档说明「同一材质语言、不同强度」。
3. **多选态要有全局提示**（与 P1-6 的「已选择 N 项」合并）——比逐格区分更有效地解决「我在不在多选模式里」。
4. 用 `src/ui/grid_css/tests/render.rs:112 thumbnail_emphasis_covers_white_image_edges` 的像素采样手法新增一条断言：hover 与 selected 的采样亮度差需大于阈值，防止后来再次把两态压平。

**落点文件**：`data/css/base.css`、`src/ui/grid_css/tests/render.rs`、`docs/modules/ui-design.md`。

---

### P1-12 查看器右上控件簇：分组、命中区、间距三处问题

**现象**：6 个图标挤在右上角，`放大` 与 `缩小` 被旋转/全屏拆在两端；静止态全裸（只有白色符号+阴影），首次进入不知道那里可点；按钮 36×32、间距 4。

**证据（已验证）**：

- `data/ui/viewer-page.blp:227-271` 声明顺序：`zoom_reset_btn` `zoom_out_btn` `rotate_left_btn` `rotate_right_btn` `fullscreen_btn` `zoom_in_btn`，`spacing: 4`；`viewer_nav_buttons`（`:202-222`）同样 `spacing: 4`。
- `.viewer-overlay-nav-btn { min-width: 36px; min-height: 32px; padding: 0; color: #ffffff; }`（`base.css:1114-1125`）；注释明确「静止态裸、hover 才上玻璃」，并解释了白前景 + 暗光晕的用意（这条理由成立，保留）。
- `base.css:394-398` 的 `.glass-toolbar-button` 是 `min-height/width: 34px; padding: 0 14px`。

**方案**：

1. **按语义重排**：`zoom_out | zoom_in | 分隔 | rotate_left | rotate_right | 分隔 | reset | fullscreen`。`Adw.HeaderBar` 不适用（这是 overlay box），直接改 `viewer-page.blp:227-271` 声明顺序即可；分组分隔用 `Gtk.Separator` 或增大组间 `margin`（不要用新 class）。注意 `photos-page.blp:11-15` 里那条「`[end]` edge-first 反向声明」的坑在 `Gtk.Box` 不存在，`viewer_zoom_controls` 是普通 Box，顺序即视觉顺序——在 PR 描述里写清以免 review 混淆。
2. **命中区与间距**：`.viewer-overlay-nav-btn` 提到 `min-width: 40px; min-height: 36px`，组内 `spacing: 6`，组间 `margin-start: 8`。保持静止态裸（材质规则不改），只放大可点区域；必要时用透明 `padding` 扩 hit area 而非扩视觉尺寸。
3. **可发现性**：给整簇一个极淡的静止容器轮廓（复用 `.glass-segmented` 的轻底，不新增材质语义），或首次进入查看器时播放一次 ≤600ms 的 reveal 提示（须尊重 P1-14 的 reduce-motion 开关）。
4. **与 P1-7 倍率标签同处布局**：新增的 zoom level label 要计入该簇宽度，避免窄窗口溢出。

**落点文件**：`data/ui/viewer-page.blp`、`data/css/base.css`（`1114-1125`）、`docs/ui-naming-reference/index.html`。

**测试**：`cargo test --test e2e_viewer`（按钮 id→动作映射随重排仍正确，tooltip 断言不变）；`cargo test ui::grid_css`（尺寸变更不破坏 hover 采样）。

---

### P1-13 玻璃/透明材质下的对比度风险

**现象**：多处功能性文字用了 0.45–0.68 的前景不透明度，叠在半透明玻璃甚至照片内容上；透明度滑杆拉到 100 时更糟。

**证据（已验证）**——下列数值为代码实测；对比不足的判定本身属「待视觉验证」：

| 位置 | 前景 | 证据 |
|---|---|---|
| 菜单项禁用态 | `alpha(currentColor, 0.45)` | `base.css:513-515` |
| 搜索「更多」瓦片 | `alpha(@window_fg_color, 0.52)` 叠 `0.06` 底 | `base.css:1011-1013` |
| 视频错误块副标题/图标 | `0.58` / `0.54`，标题 `0.82` | `base.css:758-772` |
| 关于文本 | `0.56` | `liquid.css:233-237`、`plain.css:189-193` |
| 侧栏计数/分组标题 | `0.72` / `0.78` | `base.css:650-674` |
| 库统计 / 总览同步行 | `0.68` | `base.css:966-983` |
| 阅读面自身 | `@glass_reading_bg`：Liquid 0.72–0.78 / Plain 0.82–0.88（`reading_surface_alpha()`） | `src/ui/grid_css.rs:104-107` |
| Liquid `.glass-base` | `alpha(@window_bg_color, 0.42)` | `liquid.css:4` |

**方案**：

1. **给「功能性文本」设 α 地板**，与「装饰性/禁用态」区分开：
   - 错误、状态、计数、进度类文本：α ≥ **0.78**（视频错误块副标题 0.58→0.80、图标 0.54→0.72；库统计与同步行 0.68→0.78；关于文本 0.56→0.72）。
   - 真正的禁用态保持 0.45（符合禁用态 0.38–0.5 的通用区间），但**必须同时降敏感度**，避免「看起来像低对比的可用文字」。
   - `docs/modules/ui-liquid-glass.md:75-78` 已为阅读面与选择器设了 α 地板（0.72–0.88 / 0.66–0.78），本方案只是把同一思路延到前景文本，不与之冲突。
2. **把对比度断言搬进已有的像素测试**。`src/ui/grid_css/tests/render.rs:7` 已有 `luminance(&gdk::RGBA) -> f64`，且 `:172 materials_resolve_in_both_themes_and_at_transparency_endpoints` 已在两主题 × 两材质 × 透明度 0/50/100 解析实际颜色。新增一条：对上述 selector 解析出的 foreground 与解析出的 surface 计算 WCAG 比值，断言 ≥ 4.5:1（功能性文本）/ ≥ 3:1（大号与图标）。这条测试是本项最有价值的产出——它把「对比度」从检视意见变成不可回退的契约。
3. **白前景 + 阴影的照片叠层**（`base.css:934-959`、`:1206-1212`）不在本次调整范围：其暗光晕已提供局部底衬，且注释说明了动机（P1-12 保留）。
4. **模式选择器的对比采样**（`src/ui/mode_selector.rs:322-331` 的 `on-light-background`）是已验证的好设计，继续作为照片叠层文本的参照实现。

**落点文件**：`data/css/base.css`、`data/css/liquid.css`、`data/css/plain.css`、`src/ui/grid_css/tests/render.rs`。

**测试**：`cargo test ui::grid_css::tests::render`（需 `tools/with-at-spi.sh xvfb-run`，见 `docs/modules/ui-liquid-glass.md:155-159`）。

---

### P1-14 完全没有接入「减少动画」

**现象**：GNOME 设置里打开「减少动画」后本应用照常播放全部过渡。

**证据（已验证）**：

- `data/css/base.css` 有 **17 条 `transition` 声明**（含注释共 23 处提及）：120ms（对勾 `:115`、徽记/侧栏 `:272`、`:322`、日期标签 `:905`、hover 材质 `:416`、菜单项 `:1016`）、140ms（菜单入场 `:534`）、180ms（spinner/箭头 `:678/:739`）、200ms（淡入 `:727/:886`）、220ms（胶片条 `:1045`）、300ms（选择器滑轨 `:363`）、350ms（色彩交叉淡入 `:300/:344`）、`:378`、`:1060`（多属性长声明）。另有 `liquid.css` 2 条、`plain.css` 1 条，尾块必须同时覆盖三者。
- 全项目无 `@media`，且 `src/ui/grid_css.rs:66` 明确禁止「web-style @media feature queries here」。
- 全 `src/` **无任何** `gtk::Settings` 读取（grep 零命中），因此也没人读 `gtk-enable-animations`。

**方案**（走 GTK 原生通路，不引入 `@media`）：

1. 在 `grid_css.rs` 的 CSS 装配处（`build_css` / `build_css_with_transparency`，`:71/:75`，拼接顺序 `:88-93`）追加一个可选尾块：

   草案：

   ```text
   // 读取 GtkSettings:gtk-enable-animations；为 false 时输出全局压制块。
   // 用尾块而不是 @media：见 grid_css.rs:66 的禁令与 GTK CSS 子集限制。
   if !gtk::Settings::default().property_gtk_enable_animations() {
       css.push_str(REDUCE_MOTION_CSS);   // 把 transition-duration 归零
   }
   ```

   `REDUCE_MOTION_CSS` 用 `* { transition-duration: 0ms; }` 太宽（会波及尺寸动画），建议逐条覆盖已知的 7 个时长值或改为给需要动画的选择器统一挂 `--motion: 0ms` 变量风格（GTK CSS 支持 `@define`，但**不支持自定义 CSS 变量**，所以退路是显式重写那 7 个选择器）。落地时先验证 GTK 是否接受全局 `transition-duration: 0ms`。
2. **响应式更新**：`GtkSettings` 的 `gtk-enable-animations` 可 notify；在 `install()`（`:318`）时连接并触发 `reapply()`（`:336`），避免要求重启。
3. **代码侧动画同样要收**：Rust 驱动的动画不在 CSS 里——`src/ui/viewer/filmstrip.rs:837-945`（adjustment 动画）、`src/ui/mode_selector.rs:222-263`（滑轨 indicator 动画）、`data/ui/photos-page.blp:108-109`（`scroll_date_revealer` crossfade 200ms）与 `:184-185`（`GtkStack` crossfade 200ms）、以及 header 各 `Revealer` 的 `slide_left`/`slide_right`（`photos-page.blp:44-104`）。统一由一个 `motion_enabled()` 查询函数供这些点读取（Revealer 可用 `set_transition_type(NONE)`，Stack 同理）。
4. **测试**：`css_for_tests()`（`:255`）注入 reduce-motion 开关，断言开启后的 CSS 不含 `transition-duration: 1[284]`/`200`/`220`/`300`/`350` 等时长。

**落点文件**：`src/ui/grid_css.rs`、`src/ui/grid_css/tests.rs`、`data/css/a11y.css`（或新尾块常量）、`src/ui/viewer/filmstrip.rs`、`src/ui/mode_selector.rs`、相关 `Revealer` 设置点。

**需同步文档**：`docs/modules/ui-liquid-glass.md`（新增「动效与 reduce-motion」一节，并解释为何不用 `@media`）、`docs/testing.md`（新单测说明）。

---

## P2 检视项（简表，实施时并入对应批次）

| # | 现象 | 证据 | 建议 |
|---|---|---|---|
| P2-1 | Toast 一律无撤销，删除只能去回收站找回 | `src/ui/toasts.rs:19-40` 三个工厂都用 `adw::Toast::new(msg)`，全项目无 `set_button_label` | 新增 `success_with_action(overlay, msg, label, f)`；删除/批量收藏接 `viewer/actions.rs:81-93` 已有的回滚 |
| P2-2 | Toast 可能盖住胶片条 | `data/ui/viewer-page.blp:7` 的 `ToastOverlay` 包裹整块内容 | 限定 overlay 区域到 stage，或为 toast 预留底部 inset |
| P2-3 | 模式选择器对读屏是三个静态标签 | `data/ui/mode-selector.blp:18-45` 用 `Gtk.Box` + 点击手势，无 role/label | 保留单胶囊视觉（`ui-liquid-glass.md:92-105` 是硬契约），用 `set_accessible_role(Button)`+`set_accessible_label` 补语义；或改 `Gtk.ToggleButton` + `.glass-segment`（`search-page.blp` 已走此路，`ui-liquid-glass.md:108-110`） |
| P2-4 | 全项目 `set_accessible_label`/`set_accessible_role` 调用为 **0** | grep 确认；`overview_sync_icon`（`photos_page.rs:949`）与警告图标（`:1037`）连 tooltip 都没有 | 约 26 个图标按钮先补 tooltip 兜底，再为「状态类」（同步状态、时长/云/收藏徽记 `src/ui/square_tile.rs`）补 accessible label；用 `tools/assert-at-spi.py` 模式扩展断言面 |
| P2-5 | 选择相关的 DB 查询在主线程同步执行 | `photos_page.rs:1155`（每次选择变化 `favorite_state`）、`:1337`（同步取 2000 条）、`:1360`（同步 count） | 移入 `spawn_blocking` + generation 回投；大库下多选 header 会掉帧 |
| P2-6 | 相册选择器加载中是空网格；DB 报错显示成「暂无相册」 | `src/ui/album_picker.rs:174-220`（`:214-218` 把错误渲染为空态标题） | 复用 P0-1 的 loading/error 分离结论 |
| P2-7 | 回收站每次打开先闪一下「回收站为空」 | `src/ui/trash_page.rs:122-132` 空态 child 常驻直到数据落地 | 首轮加载完成前不切空态 |
| P2-8 | 全库总览只能靠「顶部再往上滚」发现，明确无 disclosure 按钮 | `photos_page.rs:1243-1257`、`docs/modules/browsing.md:133-141` | 加一个可点 chevron（不改材质）；否则同步状态入口等于不存在。同步状态失败也无重试按钮（`photos_page.rs:1024-1040`，`:1279` 错误仅日志） |
| P2-9 | 收藏按钮在「直接切换」与「弹层」间隐形变化 | `photos_page.rs:754-770`、契约见 `docs/modules/ui-design.md:140-147` | 混合态给按钮加下拉指示，让「会弹菜单」可预判 |

---

## 结构性议题：沉浸浏览与「F 是另一个窗口」

**观察**：查看器 header 常驻（`data/ui/viewer-page.blp:11-69`，全项目 viewer 侧无 `Revealer`/autohide），沉浸态靠 `F` 打开一个**独立无边框顶层窗口**（`src/ui/keyboard/binding.rs:183` → `viewer_page.rs:583` → `src/ui/viewer/fullscreen_window.rs:52-58` `decorated(false).fullscreened(true)`，`Escape` 关闭 `:256-267`）。用户很难预期「F 之后 Esc 回到的是另一个窗口实例」。同时 header + 右上 6 按钮 + 右下 2 按钮 + 胶片条，与 `docs/modules/ui-design.md:40`「内容占据最大连续区域」存在张力。

**建议方向（需产品决策，不在本轮 P0/P1 内）**：把 `F` 改为**原地**进入沉浸态——header 与浮层控件用 `Revealer` + 鼠标静止 ~2.5s 收起、移动即现（150–300ms，尊重 P1-14）；`fullscreen_window.rs` 保留为真正的系统级全屏预览。

**必须联动的前提**：P0-2（焦点环）、P1-12（静止裸图标 + 命中区）。若「控件自动收起 + 静止态无底 + 焦点不可见」三者叠加，结果是完全找不到入口。**顺序要求：P0-2 与 P1-12 先落地，才允许做自动收起。**

---

## 建议实施批次

按「同一批只碰同一组文件、且测试可分别执行」切分：

| 批次 | 内容 | 主要文件面 | 依赖 |
|---|---|---|---|
| B1 焦点环 | P0-2 | `a11y.css`/`base.css`/`grid_css` 测试 | 无（最小、最高价值，可独立先做） |
| B2 扫描态 | P0-1、P2-6、P2-7、P2-8 | `events.rs`/`bootstrap.rs`/`empty_states.rs`/`photos_page.rs`/`album_picker.rs`/`trash_page.rs`/i18n | 无 |
| B3 多选与选择 | P0-3、P1-6、P2-5、P2-9 | `photos-page.blp`/`photos_page.rs`/`virtual_media_grid*`/`loading.rs` | B1（焦点环让多选态更易验证） |
| B4 快捷键发现 | P0-4 | `src/ui/keyboard/*`/`window.rs`/`settings.rs`/i18n | 无 |
| B5 搜索 | P0-5 | `search_page.rs`/`search-page.blp`/`tools/assert-at-spi.py`/i18n | B2（复用空态工厂改造） |
| B6 查看器信息层 | P1-7、P1-12、P2-2 | `viewer-page.blp`/`viewer_page.rs`/`navigation.rs`/`transform.rs`/`base.css` | 无 |
| B7 查看器输入 | P1-8 | `transform.rs`/`stage.rs`/`viewer.md` | B6（同文件，顺序执行避免冲突） |
| B8 编辑与错误反馈 | P1-9、P1-10、P2-1 | `editor_panel.rs`/`viewer/editor.rs`/`viewer/stage.rs`/`square_tile.rs`/三材质 CSS | 无 |
| B9 材质与动效 | P1-11、P1-13、P1-14、P2-3、P2-4 | 三份 CSS/`grid_css.rs`/`render.rs`/`filmstrip.rs`/`mode_selector.rs` | 建议最后做（触碰材质契约最广） |

---

## 验证策略（遵循 `AGENTS.md`）

- 未提交期间**只跑新增/直接被改的窄测试**，不因为改动而跑全量套件；本地提交可基于窄测证据。
- **每条 commit message 必须带 `Tests:` 段**，列出实际运行命令与 `PASS/FAIL/NOT RUN(原因)`；不得把未运行的检查报为通过。
- 全量 CI 等价套件只在**推送前门禁**运行，命令与 `.github/workflows/ci.yml` 一致：

  ```bash
  cargo fmt --all --check
  cargo clippy --locked --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments
  cargo build --locked --all-targets
  tools/with-at-spi.sh xvfb-run -a cargo test --locked --all
  ```

- 涉及 CSS 材质/对比度的批次（B1、B6、B8、B9）需在 Flatpak GNOME 运行时做目测（`run-flatpak.sh`），理由见 `docs/modules/ui-liquid-glass.md:152`（旧宿主 GTK 会对 `backdrop-filter` 打解析警告并回落）。
- 各批次涉及的 UI 控件新增/改名/模板 `child-id` 变更，都要同步 `docs/ui-naming-reference/index.html`（`AGENTS.md` 的硬性规则）。

---

## 需要更新的设计契约清单（集中列出，避免遗漏）

| 文档 | 需改动的结论 | 触发批次 |
|---|---|---|
| `docs/modules/ui-design.md:105-109` | 「选择动作只在有选择时出现」需补充「进入选择有常驻入口」，消除循环依赖 | B3 |
| `docs/modules/ui-design.md:151-152` | 空态/loading 需为三态且带下一步动作 | B2 |
| `docs/modules/ui-design.md:175-184` | focus 态可见性落地；hover 与 selected 的强度差决策 | B1、B9 |
| `docs/modules/viewer.md:98-113, 204` | 重新表述控制器禁令（区分 touch-only 与桌面滚轮）；新增位置/倍率契约 | B6、B7 |
| `docs/modules/keyboard.md:60-69` | 声明应用内发现路径与「binding ↔ shortcuts」防漂移断言 | B4 |
| `docs/modules/editor.md` | 退出确认与前后对比契约 | B8 |
| `docs/modules/ui-liquid-glass.md` | 焦点环属 a11y 层且与透明度无关；新增「动效与 reduce-motion」一节 | B1、B9 |
| `docs/modules/browsing.md` | 多选入口、总览披露、空态三态 | B2、B3、B9 |
| `docs/modules/storage.md` | `DomainEvent::ScanPhase` 契约、缩略图失败语义 | B2、B8 |
| `AGENTS.md`（建议） | 新增一条 UI 不变量：「`data/ui/*.blp` 中不得出现硬编码可见文案，一律 `tr()`/`trf()`」 | B5 |

---

## 本次未执行的验证（诚实边界）

- **未运行应用**，也未生成截图或视觉基线。因此以下结论属「代码确认 + 待视觉验证」，落地前必须目测：P1-13 的对比度判断（来自 α 数值与表面叠合推理）、P1-12 的间距/密度拥挤感、P1-11 的两态区分度、P0-3 的 header 宽度是否溢出。
- 未运行 `PHOTOVIEWER_GLASS_SCREENSHOTS=... tools/with-at-spi.sh xvfb-run -a cargo test --lib ui::grid_css::tests::render`——它会向 `target/` 写文件，本文档阶段未执行。
- 未做 Flatpak 运行时验证，也未做超大图库压测。
- 未读取 AT-SPI 实际无障碍树（`tools/assert-at-spi.py --dump` 可在真实窗口上验证 P2-3/P2-4 的暴露情况），因此「读屏听到三个静态标签」的推断来自控件类型（`Gtk.Box` + 点击手势）而非实测。
- 检视中所有「零调用者」「零命中」结论都用 grep 交叉确认过（`empty_states::loading/scan_error`、`set_accessible_label`、`ShortcutsWindow`、`gtk::Settings`、`@media`、scan 进度布尔状态）。

### 交互评审原型能证明什么、不能证明什么

`docs/ui-naming-reference/index.html` 已升级为可交互评审原型（23 条提案逐条开关、深链、判定与 markdown 导出）。它证明的是**交互与信息架构层面**的取舍，不替代上面任何一条未执行的验证：

- 能看：状态机是否覆盖全（扫描/失败/空、搜索四态、多选进出、脏标记守卫）、控件落点与顺序（P1-12 的两种排列）、提示文案与让位关系（P2-1/P2-2）、i18n 键是否真的两种语言都有（切「语言=en」即刻暴露硬编码中文）。
- 不能看：GTK 真实渲染的材质/圆角/阴影、Flatpak 下的字体度量、以及 P1-13 的**实际**对比度。原型里的「对比度报告」用 WCAG 相对亮度公式量的是浏览器 DOM，只能作为筛选可疑组合的线索；结论仍以 `render.rs` 断言与真实截图为准。
- 静态页只有 18 张样图，所以 P1-6 的 2000 上限、P2-5 的主线程掉帧都只能以标注或演示开关呈现，不是真实复现。
- 标题栏是替身而非提案：窗口控件按桌面 `button-layout`（默认 `:minimize,maximize,close`）画在右端，对话框只有关闭按钮；这些都不受提案开关影响。查看器顶栏的分组照真实结构摆——日期与云状态角标是 header `[start]` 一组（`spacing: 8`），文件名是 `NavigationPage` 的 title 由 HeaderBar 居中——评审时不要把角标读成文件名的一部分。契约文字见 `docs/modules/ui-design.md` 的 Window Shell 与 Viewer Page 两节。
