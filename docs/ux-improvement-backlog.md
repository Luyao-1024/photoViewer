# UX 专项检视与 P0/P1 优化方案

检视日期：2026-10-01
检视基线：`250671a`（工作区干净）
检视范围：浏览（Photos / 虚拟网格 / 模式选择器 / 搜索 / 相册 / 回收站）、查看器与编辑器、窗口与设置、共享玻璃材质与可访问性
状态：检视结论已归档，方案按批次落盘中。**P0 五条已全部实施**并本地提交——P0-1（扫描三态）、P0-2（主网格焦点环）、P0-3（多选入口，照片页 + 虚拟网格 + 相册详情页两半都已落地）、P0-4（快捷键应用内可发现）、P0-5（搜索三态 + 模板去硬编码中文）。落盘过程中发现的新问题已升级为独立条目：P1-15（搜索结果分区没有任何批量动作通路），它也已落地。**P1 全条已落盘**（P1-6 header 选择计数器、P1-7 前两项＝位置计数器 + 到底反馈、P1-8 查看器桌面输入、P1-9 编辑器退出保护、P1-10 图片错误可见化、P1-11 选择态区分、P1-12 查看器控件簇、P1-13 对比度地板、P1-14 减少动画、P1-15 搜索批量通路）。**P2 全条已落盘**（P2-1 toast 撤销、P2-2 toast 让位胶片条、P2-3 模式选择器语义、P2-4 无障碍名称、P2-5 选择查询移出主线程、P2-6 相册选择器四态、P2-7 回收站首帧、P2-8 总览 disclosure + 同步重试、P2-9 心形混合态指示）。至此 P0 / P1 / P2 的**可执行条目全部落地**，只剩两项：P1-7 的「倍率可见」（当时明确留给后续），以及下面的结构性沉浸浏览议题（需产品决策，且依赖本轮已落盘的 P0-2 与 P1-12）。落盘过程中与原方案的偏差记录在各节末尾的「实施结果」里——实测证据优先于草案。

## 总体判断

功能骨架与工程规范是这个项目的强项：虚拟化网格、语义色 token、破坏性操作确认、变换不动布局，都已经达到或超过一般桌面相册应用的水准。当前 UX 短板集中在三类，而不是视觉风格：

1. **状态可见性缺口**——扫描期误报「暂无照片」、主网格键盘焦点不可见、选中集被后台重建静默清空。这三项都会让用户对软件产生错误事实判断。
2. **发现性缺口**——13 个快捷键在应用内完全不可发现、全库总览只靠隐藏手势、查看器控件静止态全裸。多选现已可由右键/长按菜单、Space 与 Ctrl+A 进入，并有显式退出按钮；库主已决定不恢复 header 常驻入口，因此剩余问题是不能假设所有用户都知道这些手势，而不是必须重新加入同一颗按钮。**hover-only 材质是本项目的刻意设计（见「保留项」），但它必须有一个可发现的入口**，否则「克制」变成了「不存在」。
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

### P0-3 多选发现性不足且缺少触控回退（header 常驻入口已按库主决定撤回）

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

**实施结果**（已落盘：照片页 + 相册详情页）：header `[start]` 组新增 `Gtk.Revealer select_mode_revealer → Gtk.Button select_mode_btn`（`photos-page.blp:49-58`，模板默认 `reveal-child: true`），点击对三套分组网格一并 `set_multi_select_mode(true)`，再 `refresh_selection_ui()` 与 `focus_visible_tile()`（`photos_page.rs:747-757`）；reveal 由 `refresh_selection_ui()` 的 `!any_multi` 驱动（`:1286-1288`），与 `exit_multi_select_btn` 形成对称进出。四处偏差：

- 图标用 `selection-mode-symbolic`：草案首选的 `check-select-symbolic` 在本机 Adwaita 图标主题里不存在。
- 图标-only 按钮必须自带 label，tooltip 复用 `photos.batch.multi_select`（`photos_page.rs:419-421`），不新增 i18n key。
- 进入多选后主动把焦点交进网格（`VirtualMediaGrid::focus_visible_tile()`，`virtual_media_grid.rs:724`）：入口按钮正在收起，否则 GTK 自己挑一个 fallback 焦点并把视口滚回顶部一帧。
- 长按回退只落在 `virtual_media_grid/factory.rs:97-111`（`GestureLongPress` + `button(1)` + `touch_only(true)`，走同一个 `show_context_menu`）。

**相册详情页那一半（本轮补完）**：`data/ui/album-detail-page.blp:24-89` 复刻照片页同名同位的 chrome——`[start]` 是 `search_btn`、`select_mode_revealer → select_mode_btn`、`select_all_revealer → select_all_btn`、`exit_multi_select_revealer → exit_multi_select_btn`，`[end]`（edge-first 反序声明）是 `delete_to_trash_revealer → delete_to_trash_btn`（`glass-toolbar-danger`）与 `add_to_album_revealer → add_to_album_btn`。文案全部复用 `photos.batch.*` / `photos.add_to_album` / `viewer.tooltip.move_to_trash`，不新增 key（`wire_selection_chrome()`，`album_detail_page.rs:266`）。`grid.connect_selection_changed` 驱动 `refresh_selection_ui()`（`:408`）：批量动作按「有选择」reveal，退出按「多选态」reveal，入口按「非多选态」reveal，两页语义一致。三点额外落地决定：

- **全选必须按相册而不是按已加载窗口**。实盘探针（截图 + `selected_ids()`）暴露出 `grid.select_all()` 在 ready set 为空的网格上会走 `select_ids(&[])`，那是**静默清空**而不是 no-op。改为 `select_all_in_album()`（`:365`）经 `MediaRepository::items(media_query_for_album(&album), 0, ALBUM_SELECT_ALL_LIMIT)` 取 id，上限 `ALBUM_SELECT_ALL_LIMIT = 2_000` 与照片页 `PHOTOS_SELECT_ALL_LIMIT` 对齐；空结果直接 return，不清选择。测试用 60 张相册只挂载 10 张的夹具断言选中 60，反向变异（改回 store 版）会失败。
- **不加收藏按钮**：该页 `on_set_favorite` 目前是 no-op，放一颗心就是死控件；模板注释里写明了这个刻意的缺失。
- **键盘对齐**：`handle_keyboard_action` 转发 `Ctrl+A`（全选相册）、`Delete`（有选择才进回收站，否则 Ignored）、`Escape`（先清选择/退出多选，再把剩下的交回导航栈）。

**仍在 backlog 的缺口（不再是「刻意留下的例外」）**：旧 `MediaGrid`（FlowBox，仅搜索结果分区在用）以 `enable_context_menu: false` 构造，因此那批瓦片既没有右键菜单、也没有长按回退、更没有进入多选的通路。补它需要在搜索页提供 overlay 宿主并接上真实的 `on_add_to_album` / `on_set_favorite` 回调，作为 P1-15 跟进；现状描述见 `docs/modules/browsing.md`。照片页与相册详情页的右键/长按菜单和 Space / Ctrl+A 入口已可达，退出按钮始终提供明确退路；header 常驻入口已按库主决定撤回，后续方案不得把它当作默认修复。

**风险**：header `[start]` 已挤了 search + 两个 revealer，窄窗口下可能换行或裁切。需在 800×600 与 1280×800 两档目测；必要时把 `select_mode_btn` 放到 `[end]` 的最左位（注意 `[end]` 是 edge-first 反向声明，见 `photos-page.blp:11-15` 的注释）。**已实测**：两页在 800×600 截图下 header 无溢出、无换行，窗口控件仍在右端，`select_mode_btn` 未进入裁切。

**2026-10-02 更新（库主决定，部分回退）**：常驻多选入口按钮整体移除——两页模板的 `select_mode_revealer → select_mode_btn`、Imp 字段、点击 handler、tooltip（`photos.batch.multi_select`）与 `refresh_selection_ui()` 的 `!any_multi` 门控都删掉了；右键/长按菜单项、键盘 Space / Ctrl+A 重新成为进入多选的路径，退出按钮与批量 chrome 不变。理由：库主不喜欢 header 里常驻一枚平时用不上的图标。本条的其余部分（长按回退、退出绑定模式、全选按相册、不加收藏按钮）仍然成立，不要把常驻入口当作「已落地行为」恢复回来。

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

**实施结果**（已落盘，方案被实测证据改写）：新增 `src/ui/keyboard/shortcuts_window.rs`，`GROUPS` 是三 section 的唯一真源（全局 6 / 浏览与选择 8 / 图片查看 14，共 28 行，每行带 `KeyboardScope`），`KeyboardAction::ShowShortcuts` 由 `F1`、`Ctrl+/`、`Ctrl+?` 触发（`binding.rs:105-112`），`MainWindow::show_shortcuts_reference()`（`window.rs:929-936`）先关设置弹窗再开窗，保证同一时刻只有一个模态窗；设置页新增「键盘」分组与一行 activatable `Adw.ActionRow`（`window/settings.rs:523-545`，插在 About 之前）；9 个查看器按钮 + 上一张/下一张 + 侧栏设置按钮的 tooltip 走 `tooltip_with_key()`（`viewer_page.rs:463-527`、`viewer/navigation.rs:390-398`、`window.rs:639`）。偏差与实测结论：

- **窗口用 Builder XML 骨架生成，而不是草案假设的 `ShortcutsWindow::add_section`/`add_group`/`add_shortcut`。** 那套 API 需要 gtk4 的 `v4_14` feature，本项目锁在 `v4_8`；实测把 feature 升到 `v4_14` 会新增约 52 条弃用告警（`Widget::allocation`、`CssProvider::load_from_data`、`StyleContextExt`、`FileChooserNative` 等），CI 的 `clippy -D warnings` 直接红。翻译串一律作为属性写入、不进 XML，因此含 `&` 或 `<` 的文案不会破坏解析（`translated_text_is_attached_as_properties` 守住这条）。
- **草案的 `tr_with_key()` 落地为 `shortcuts_window::tooltip_with_key()`，且不新增 `*.tooltip.key` 键**：键名由 `accelerator_for(action)` 从同一张 `GROUPS` 反查，所以 tooltip 与参考窗口在结构上不可能声明不同的绑定；没有键位的动作（`Restore`）返回 `None`，tooltip 退回纯标签而不是空括号。`src/core/i18n.rs` 未改动。
- **`<Shift>R` 与 `R` 的实测修正了草案的键位表**：`gtk_accelerator_parse("<Shift>R")` 报成小写 `r` + Shift 位，草案的 `binding.rs` 匹配臂会漏掉向左旋转。漂移测试的前向断言把这条抓了出来，最终声明改为 `"<Shift>R"`、匹配臂放宽为 `Key::R | Key::r`。另外 `display_accelerator` 早期把 Shift 折进字形（向左旋转显示成 `R`），与向右旋转的 `R` 撞成同一个提示，已改为与 `gtk_accelerator_get_label` 对齐（字母大写、Shift 显式），并有断言守住两个旋转动作的可区分性。
- **`GtkShortcutsShortcut` 一行只接受一个 accelerator**，所以 `F1` 与 `Ctrl+/` 是两行，不是草案里的一行。
- **草案测试项 `cargo test --test e2e_viewer`（F1 打开且不吞掉 viewer 的 `F`）未做**，覆盖改由 `ui::keyboard::shortcuts_window::tests` 承担（17 项，含「打开即断言 modal / transient_for / view-name / 行高」、「每条 accelerator 真的解析到它声称的动作」、「每个可路由动作都有行」）。
- **表内的 `Ctrl+滚轮` 行不属于本项**：grep 确认查看器至今没有任何 scroll 控制器（`viewer/transform/tests.rs:13-37` 还有一条负向断言禁止 touch 捏合/平移控制器），所以该键位属 P1-8，命名图里改挂 `.pv-p1-8`，只在开启 P1-8 时出现。
- **草案列的两份文档已同步**：`docs/modules/keyboard.md` 新增「Shortcut Discoverability」一节，写明「加/改/删绑定 ⇒ 必须同步 `GROUPS` 行，否则测试红」；`docs/ui-naming-reference/index.html` 里 `shortcut-reference-group` / `shortcut-reference-window` / `shortcut-group-global|browsing|viewer` / `tooltip-key-hint` 六个热点已转为常态 `data-ui` 条目，表内容与 `GROUPS` 逐行一致（脚本核对 28 行），字形按 `gtk_accelerator_get_label` 的实测输出渲染并随语言切换。落盘前的对照不挂在提案开关上，而是显式演示态 `data-pv-demo="shortcut-blind"`（工具条「演示态 → 落盘前：无快捷键入口」）。

**风险**：`F1` 落盘后成为全局键位，`TextInput` 与 `Modal` 作用域按既有契约不回落到全局表，因此搜索框内按 `F1` 不会开窗——这是刻意保留的原生文本编辑行为，若后续要放开需显式加白名单。

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

**实施结果**（已落盘）：三态收进一个 `Gtk.Stack search_state_stack`（页名 `idle`/`results`/`no-results`，`set_search_state`（`search_page.rs:547-567`）依当前输入与命中数选页），两个占位页由 `empty_states::search_idle()`（`:130-140`）与 `empty_states::no_search_results(on_clear)`（`:149-166`）提供，零结果标题每次显示前用 `no_search_results_title(&query)`（`:142-146`）刷新并回显 `trim()` 后的关键词；`remove_media_ids_from_results` 末尾补一次状态刷新，删掉最后一张命中图会落到零结果页。模板侧 `search-page.blp:65-107` 新增 `search_busy_box → search_spinner` 与 `search_state_stack → search_results_box`。与方案的偏差：

1. **进行中指示用 `gtk::Spinner` 而非 `adw::Spinner`**，且只有图标、不带「正在搜索…」文案：这一行只在查询超过 `SEARCH_BUSY_DELAY_MS = 300`（`search_page.rs:35`）后才出现，为一条瞬时提示再造第 9 个键不值当。
2. **栈只负责整页互斥，两个分区各自的 `set_visible` 保留**：否则只命中图片或只命中视频时会露出一个空分区标题。
3. **必须关掉 `hhomogeneous`/`vhomogeneous`**：结果网格与占位页尺寸差很多，隐藏页会决定栈高。
4. **「移除计时器 SourceId」在本机是真崩溃**：`end_search_busy()` 对已触发的 source 调 `SourceId::remove()` 报 `GLib-CRITICAL g_source_remove: Source ID 10 was not found` 后 panic，而 drop `SourceId` 并不取消源（gtk4-rs 0.8.2 无 `Drop` impl）。改为 `busy_generation: Cell<Option<u64>>` 代次失效，测试 `a_settled_query_never_paints_the_busy_row` 锁住这条。
5. **文案在落地时收敛**：草案的「搜索图库」→「搜索你的图库」，「未找到与「zzz」相关的结果」→「没有找到 `{query}`」，描述→「换一个关键词，或将搜索字段切回「全部」。」；新增 8 个键（`search.field.*` 3 + `empty.search_idle.*` 2 + `empty.search_none.*` 3），`i18n/zh-CN.json` 与 `en.json` 各 427 键对齐。
6. **顺带修掉一处公共缺陷**：`AdwStatusPage` 会填满自己的 child，`empty_states::add_action` 建的 pill 按钮被拉成整页宽（截图实测），加 `halign=CENTER`（`empty_states.rs:30`）后 P0-1 那两张照片页占位同时受益。
7. **「模板禁止硬编码文案」已升级为不变量**：`AGENTS.md` 的 UI Invariants 收录，`tests/ui_template_copy.rs` 全仓扫描 `data/ui/*.blp` 的七种文案属性（`label`/`title`/`text`/`placeholder-text`/`tooltip-text`/`subtitle`/`description`），并单独断言 `search-page.blp` 以注释形式留下三个 i18n 键名；用注入字面量的方式验证过该测试不是空跑。
8. **读屏探针改为按 locale 推导**：`tools/assert-at-spi.py` 的 `SEARCH_FIELD_KEYS` + `search_field_labels(locale)` 从 `i18n/<locale>.json` 取期望值，locale 解析顺序与应用一致（config `i18n.json` → `PHOTO_VIEWER_LOCALE` → `LC_ALL`/`LANG`/`LANGUAGE` → `en`），另有 `--locale` 覆盖；`tests/visual_check_script.rs` 把脚本保留的 zh 兜底常量逐字钉在 `i18n/zh-CN.json` 上，标签漂移仍会红。

**实测过的验证**：`ui::search_page::tests` 7 项、`--test ui_search_page` 2 项、`--test ui_template_copy` 2 项、`--test visual_check_script` 7 项、`--test inline_test_ownership` 5 项、`--test ux_click_flows` 1 项（真实点击穿过新栈）、`core::i18n` 2 项，全部在 `tools/with-at-spi.sh xvfb-run -a` 下 PASS。

**无头截图里的一个假缺陷**：`results` 态截图在页面中部残留一行极淡的文字，一度怀疑隐藏的栈子节点仍在 painting。三条证据判定它是 Xvfb 的脏背板而非产品缺陷：(1) 部件树 dump 显示任一时刻只有一个栈子节点 `is_mapped()`；(2) 关掉 crossfade 残留仍在，而让 idle 页从头就不显示则残留消失；(3) 强制窗口 resize 后画面干净。即 headless 下帧时钟在切页后停摆，最后一帧的残影留在 X 背板里——这条只影响截图判读，不影响真机。

---

## P1 检视项与优化方案

### P1-6 选中集被后台重建静默清空，且从不显示已选数量

**现象**：用户选了几十张准备加入相册，文件系统 watcher 落地一次扫描 → 选择全部丢失，无提示。多选期间也永远看不到「选了多少」。

**证据（已验证）**：

- `src/ui/media_grid/loading.rs:562`（`schedule_rebuild` 的 750ms 定时器回调内）与 `:591`（`rebuild_immediately`）都在 `rebuild()` 前无条件 `this.clear_selection()`；两处**都没有先读 `selected_ids()`**，id 集合直接被丢弃。
- 可恢复性已确认：`selected` 是 imp 里的 `RefCell<HashSet<MediaId>>`，`clear_selection()`（`virtual_media_grid.rs:710-715`）之前可读；`select_ids(&[MediaId])`（`:692-708`）能整体重建集合并 `sync_visible_selection`。
- 上限截断不可见：`photos_page.rs:1337` 的 Select All 走 `repo.items(LiveAll, 0, 2000)`，`photos_page.rs:1350` 已算出 `selected_count` 但从不渲染；`selected_reaches_select_all_limit`（`:1360`）只有内部逻辑。
- 计数标签在 Photos/Trash 都不存在：`data/ui/trash-page.blp` 的 action bar（`src/ui/trash_page.rs:91/226`）只有按钮文本。

**证据更正（2026-10-01 实施时实测）**：第一条证据的 `loading.rs:562/591` 属于**旧的 FlowBox `MediaGrid`**，而它在生产代码里只剩搜索结果分区（`search_page.rs:399` 经 `new_for_album` 构造，`enable_context_menu: false`），根本没有多选 UI，也就没有「用户选中几十张被清空」的路径。Photos/相册用的是 `VirtualMediaGrid`，其共享投影刷新路径**不会**清 `imp().selected`。为把这点固定下来，新增探针单测 `src/ui/virtual_media_grid/tests.rs:773 a_shared_projection_refresh_keeps_the_user_selection`：6 张图里选中 2 张 → 向共享 `ListStore` 追加第 7 张触发 debounce reload → 断言 `layout().media_count() == 7`（证明刷新真的落地，否则单测无意义）且选中集合与多选模式都保留。**结论：方案 1/3（捕获-重放 helper）不需要实施**，其余「显示已选数量」按原方案落盘。

**方案**：

1. ~~**重建前捕获、重建后重放**~~ — 见上方更正：受影响的是无多选 UI 的旧网格，`VirtualMediaGrid` 已由单测证明保留选择，故不改。
2. **显示已选数量 + 截断说明**。在 `photos_page.rs:1062 refresh_selection_ui()` 里（它已经算出 `union`）新增/更新一个 header 标签 `selection_count_label`，文案 `trf("photos.selection.count", &[("n", n)])`；命中 2000 上限时追加「（已达上限）」，复用 `select_all_limit_reached`（`:1337-1360` 区域）。标签放 `[start]` revealer 组，遵循同一 reveal 时机。
3. **不要清空选择的替代设计**：如果 `rebuild()` 必然要换 model，则把「按 id 重放」写成 `VirtualMediaGrid::preserve_selection_across_rebuild` 的内部职责，而不是让调用方每处手写，避免第三个调用点将来再犯。

**落点文件**：`src/ui/media_grid/loading.rs`、`src/ui/virtual_media_grid.rs`（重放 helper）、`src/ui/photos_page.rs`、`data/ui/photos-page.blp`、`i18n/*.json`。

**实施结果（2026-10-01 已落盘，方案 2）**：

- `data/ui/photos-page.blp:122-139`、`data/ui/album-detail-page.blp:91-106`：`Gtk.Revealer selection_count_revealer → Gtk.Label selection_count_label`（slide_left，`reveal-child: false`）作为 `[end]` 组最后声明的项，即右组最左侧、紧贴它所要量化的批量动作。
  **落盘偏差（原方案两处都不成立）**：
  1. 草案说「标签放 `[start]` revealer 组」；实施先按 GNOME 习惯试了 header `title-widget`，实测否决——libadwaita 文档明确「放在 `Adw.NavigationPage` 里的 `Adw.HeaderBar` 会显示**页面标题**而不是窗口标题」，本项目 Photos 页 `set_title(page.photos.title)`、相册页 `set_title(album.display_name())`（`album_detail_page.rs:147`），所以占用 `title-widget` 会把「照片」和相册名整块吃掉，无选择时标题区变空白。数量再重要也不该拿页面身份去换。
  2. `[start]` 组同样不理想：全选按钮已经在那里，窄窗口下两个文本控件相邻会互相挤压。最终放 `[end]` 最左，读作「已选择 N 项 ＋ ♡ ⌫」，动词紧跟数字。
  3. 拥挤风险已实测而非推断：`src/ui/photos_page/tests.rs:754 the_selection_counter_does_not_squeeze_the_batch_actions_out` 在 800x600 下选中 3 张后断言计数器宽度 > 0 且 `add_to_album_btn`/`delete_to_trash_btn` 仍 ≥24px。标签 `ellipsize: end` + `max-width-chars: 24`，挤压时先缩自己。
- `src/ui/photos_page.rs:1317-1331`（`refresh_selection_ui`，函数 `:1256`）、`src/ui/album_detail_page.rs:439-447`（`refresh_selection_ui`，函数 `:412`）：文案 `trf("photos.selection.count", …)`，命中上限时追加 `tr("photos.selection.limit")`；reveal 时机与 `has_any` 一致。相册页用 `selected.len() >= ALBUM_SELECT_ALL_LIMIT` 判定上限。
- `i18n/zh-CN.json:176-177`、`i18n/en.json:176-177`：`photos.selection.count` / `photos.selection.limit`（两表键数 429/429 对齐）。
- `data/css/base.css:472-480`：`.selection-count` 用 `@window_fg_color` + 11pt/600 + `margin-right: 4px`，两种玻璃模式下都保持扁平——它是「header 文本」，不是又一个玻璃胶囊，否则与模式选择器抢注意力。
- Blueprint 语法坑（下次改 header 少走弯路）：`title-widget:` 这类属性赋值的内联对象块**必须**以 `};` 收尾，而块内子控件（`Gtk.Label … { }`）不能加分号，否则 `blueprint-compiler` 报 `Expected ';'` / `Unexpected tokens`。用 `blueprint-compiler compile --output /tmp/x.ui <file>.blp` 可以秒级验证，不必等整个 `cargo build`。
- 单测：`src/ui/photos_page/tests.rs:715`、`src/ui/album_detail_page/tests.rs:456`（无选中→不显示；选中 N 张→文案与张数；清空→收起）、`src/ui/virtual_media_grid/tests.rs:773`（刷新保选择）、上面那条窄窗口分配单测。四个都 PASS。
- 已知未覆盖：2000 上限档的文案拼接（`（已达上限）`）在单测里没有真实复现——本地样图达不到该量级，只由 `select_all_limit_reached` / `ALBUM_SELECT_ALL_LIMIT` 的既有逻辑保证；n=0/1/3 档已实测。回收站页的同类计数（`trash-page.blp` action bar）仍未落地，保留为 P1-6 的剩余尾巴。

**测试**：`cargo test --lib -- ui::photos_page::tests::the_header_counter_names_the_selection_the_batch_buttons_act_on ui::album_detail_page::tests::the_album_header_counter_names_the_selection ui::virtual_media_grid::tests::a_shared_projection_refresh_keeps_the_user_selection`（均需 `tools/with-at-spi.sh xvfb-run -a`）。

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

**实施结果**（2026-10-01 已落盘前两项；倍率可见 P1-7b 留给后续工作，本节未覆盖）：

方案被实测量改写两点：

- 方案 1 默认从 `MediaPage.total` 缓存 total 是不适用的：viewer 入参只有窗口化 `gio::ListStore` 与 `MediaQuery`/`MediaId`（`photos_page.rs:1960`、`album_detail_page.rs:720`、`search_page.rs:793` 三个调用点都传 `media_list`，没有任何 `MediaPage` 入参）。`MediaPage.total` 仍存在（`repository.rs:34-39`），但 viewer 的「底」必须自己从查询解析。新增 `db::media_position(pool, id, where, params, trashed) -> Option<(u32, u32)>`（`(1-based rank, total)`，`db.rs`），与共享的 `db::rank_and_total`（0-based，与 `media_neighbor_with_filter_and_order` 的 `MediaNeighbor.index` 保持同一基准以让 rank 永不和 ←/→ 走的顺序不一致）+ `db::media_sort_expr`（`COALESCE(taken_at, file_mtime)` / `trashed_at` 单点）。
- `MediaRepository` 侧抽出私有 `nav_projection(query) -> (filter, params, trashed)`，`neighbor_item` 与新增的 `position(query, id)` 走同一投影；`repository.rs` 的方案 1 那段 `let trashed = matches!(query, MediaQuery::Trash); let (filter, params) = match ... { ... };` 替换为一行 `let (filter, params, trashed) = nav_projection(&query);`。

落点：viewer `imp` 新增 `position_label: TemplateChild<gtk::Label>`（`viewer_page.rs`）、`position_request_token: Cell<u64>`、`prev_exhausted: Cell<bool>`/`next_exhausted: Cell<bool>`；`viewer/navigation.rs` 新增 `update_position_label(&self, item, token)`（`gio::spawn_blocking` 拉 `MediaRepository::position`，`current_token` + `position_request_token` 双重 token 守住，仅在新 `show_at` 仍是当前帧时写入 label，否则静默丢弃——和 `refresh_sync_badge` 同款）、`set_nav_direction_available(delta, available)`/`nav_direction_available(delta)`/`reset_nav_bounds()`、`handle_nav_key(delta)`（编辑态仍 `Handled` 防 ←/→ 泄漏到 grid 焦点；已解析到无方向的端时返回 `Ignored`，键不被吞、dim 后的箭头本身已经是不需要回到键盘号票的反馈）；`navigate_by_delta` 的 `Ok(None) => {}` 改成 `this.set_nav_direction_available(delta, false)`（`navigation.rs`）；`prefetch_neighbors` 异步分支 `Ok(None) => this.set_nav_direction_available(delta, false)`——边界从「每次重新查」提前到「每帧 prefetch 落地后立即可见」，点击即落在生效位置的空洞、`cargo test ui::viewer::navigation` 的现有 `viewer_keyboard_action_navigates_and_closes`（未注入 pool，cells 默认 false=available）回归通过。

`show_at` 在 `refresh_sync_badge` 旁调用 `update_position_label(&item, token)` 并在 `update_date_label` 之后立刻 `reset_nav_bounds()`；label 在 `[start]` 标签的最后一项（`viewer-page.blp`），按 `[start]` 顺序最左的子项其实是 `Gtk.Box` 自己，但其内部子项的分配方向是「向右扩展」——所以位置计数器变长不会推 date_label/`sync_badge`，只往中间空白里长。

视觉：`base.css` 合并 `.viewer-date-label, .viewer-position-label { font-variant-numeric: tabular-nums }`（`base.css`），位置计数器还额外带 libadwaita 的 `.dim-label` 让其退在文件名/日期之后；浮动箭头 `.viewer-overlay-nav-btn` 自身有强制 `color: #ffffff`（`base.css`）使 libadwaita 默认的 insensitive 颜色失效，所以新增 `:disabled { opacity: 0.32 }`—— opacity 是该规则下唯一还能传达状态的通道，并且两种玻璃模式一致。

i18n：`viewer.position.count = "{current} / {total}"` 加进两族 zh-CN（`zh-CN.json`）/ en（`en.json`），两族总长 434/434 平。

偏差：
- **测试缺口部分补上（2026-10-02）**：`ui::viewer_page::tests::switching_media_hides_the_previous_rank_until_the_new_one_resolves` 与 `::a_failed_rank_query_keeps_the_previous_rank_hidden` 经真实 DB 覆盖排名落地、切换隐藏与失败保持隐藏，`ui::viewer_page::navigation::tests::a_video_at_the_query_start_dims_the_previous_arrow_without_a_first_navigation` 覆盖 prefetch 后失活；**键盘在已解析端返回 `Ignored` 的专属断言仍缺**，留给下次补上。
- **未做手动视觉验证**。`base.css` 的 `:disabled opacity` 与 `[start]` 内的子项扩展、`dim-label` 在玻璃材质下的可读性都需要在运行窗口里看一眼才能盖章；「两种材质下的箭头禁用态」与「窄窗口顶栏」两个目测项同样未做。
- 方案 2「两端时禁用 prev_btn/next_btn」的措辞原本是「按 libadwaita 默认（约 0.4 不透明度）即可，无需新 CSS」——实测不行，因为那条规则固定了白色 icon color，insensitive 颜色会被覆盖，所以还是新增了一条 CSS。

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

**实施结果**（2026-10-02 已落盘）：新增 `src/ui/viewer/stage_input.rs`，`EventControllerScroll` 只消费图片可见、非编辑态且 Ctrl 按下的纵向滚轮；滚轮先累计完整 90 度才执行一步，避免未达整格的触控板事件频繁缩放。`GestureDrag` 仅在主按钮且 `zoom_scale > MIN_VIEWER_ZOOM` 时记录起点 pan，后续每帧用 `起点 pan + GTK 累计 offset` 更新，避免草案写法反复累加。`clamp_zoom_pan` 先用 Paintable 固有尺寸与 Picture 分配算出 contain 后的可见矩形（GTK 的 `content-fit` 只看未旋转的图），再按 90°/270° 交换矩形宽高得到旋转后的可见框，平移边界取「可见框 × scale 超出视口的部分 / 2」，因此放大后超出部分之外（含 letterbox 空白）不可平移。契约与测试均已收窄为「禁止 touch 捏合，不禁止桌面滚轮/条件拖拽」。

偏差：草案的 16ms `zoom_provider` 节流未实施——当前每次输入仍只重写一条 scoped CSS，未新增测量/布局路径；是否产生可感知开销需要真实拖拽帧采样后再决定，不能把未经测量的优化算作完成项。`keyboard.md` 也明确该指针手势不进入 `GtkShortcutsWindow`，因为表行只接受可解析 accelerator。

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

**实施结果**（2026-10-02 已落盘）：退出闸门命名为 `EditorPanel::request_close`，四条用户路径（header 关闭、footer 取消、`Esc`、`navigation.pop`）全部经它——草案只列了三条，`viewer/navigation.rs` 的 pop action 同样会无条件 `stop_editing`，一并接进 `ViewerPage::request_editor_close`。干净状态直接 `fire_close`；脏状态弹 `adw::AlertDialog`，`add_response("keep")` 同时是 default 与 close response，只有 Destructive 的 `discard` 才真正退出。`update_pending_edits()`（原 `update_reset_button`）一处驱动 reset 敏感度、标题下 `editor_dirty_label` 与 `compare_btn` 可见性。对比是 `Gtk.ToggleButton` + `render_preview` 里的 `EditState::default()` 分支，复用既有 33ms 单飞预览，不新增渲染路径也不改待保存状态。

偏差：闸门在**收到响应时**就释放，而不是等 `AdwDialog` 的关闭动画（`connect_closed` 再兜一次）。原因是关闭动画在无帧时钟的环境里不会完成，若只依赖它，一次被打断的关闭就会让编辑器永久拒绝退出；`ui_editor_panel` 的 keep 分支正是这样暴露出来的。`ui-design.md` 的 Editor 一节已随其它文档改动移位，故按标题而非行号引用。

---

### P1-10 查看器原图解码失败 → 空白无提示（网格已有不可用占位，需另行验证其辨识度）

**现象**：原图解码失败时（文件损坏/已移动/权限），舞台留白，没有文字。虚拟网格与旧 FlowBox 的缩略图失败路径会收到专门生成的“不可用”占位纹理，因此不能把二者都描述成完全空白；真正确认的缺口是查看器没有任何文本、原因或恢复动作。

**证据（已验证）**：

- `src/ui/viewer/stage.rs:672-682`：原始图解码 Err 分支只 `warn` + 收起 spinner；预览也失败时 `:591-637` 无内容可画，舞台留空。
- 错误 UI 只给视频：`data/ui/viewer-page.blp:128-163` 的 `video_error_box`（样式 `base.css:758-772`），图片无对应物。
- 缩略图侧：图片/视频解码失败由 `src/core/thumbnails/decode.rs:124-137` 转为 `generate_unavailable_placeholder()` 并成功交付，不是空白格子；`base.css:996` 的 `opacity:0` 只属于没有 `.thumb-placeholder` 的普通 `.thumb-loading` 状态。失败占位的辨识度需要真实截图验证，不能据旧文案直接重做状态。

**方案**：

1. **图片错误态**：把 `video_error_box` 泛化为 `media_error_box`（同一 `[overlay]` 兄弟节点、同一 `.viewer-video-error`→`.viewer-media-error` class 家族），在 `stage.rs:672-682` 的 Err 分支显示：图标 `image-missing-symbolic` + 文件名 + 一句原因 + 「在文件管理器中显示」/「重试」按钮。注意 `docs/modules/ui-liquid-glass.md:126` 要求新 selector 两处材质镜像。
   - 文案与 P0-1 的 `scan_error` 用同一措辞风格：说明原因 + 如何修复。
2. **先验证缩略图占位是否已足够可辨**。现有失败纹理会进入正常 `set_paintable()` 路径并清除 loading 类；只有运行截图证明确实无法与加载中/正常缩略图区分时，才增加 `.thumb-broken` 与 icon/text 层。不要预设“格子为空”，也不要先给 core 增加失败回调。
3. **状态来源**：`ThumbnailLoader` 当前只暴露 `set_stats_dirty_callback`（`src/core/thumbnails.rs:348`），无失败回调。方案是给 loader 增加「按 media_id 请求失败」的回调或在 `SquareTile` 上直接置 `.thumb-broken` class（后者不动 core，优先）。

**落点文件**：`data/ui/viewer-page.blp`、`src/ui/viewer/stage.rs`、`data/css/base.css`、`data/css/liquid.css`、`data/css/plain.css`、`src/ui/square_tile.rs`、`src/ui/virtual_media_grid/factory.rs`、`i18n/*.json`。

**测试**：`cargo test --test e2e_viewer`（不存在/损坏文件 → 可见错误框）；`cargo test ui::grid_css`（新 class 在两材质块都存在，沿用 `grid_css/tests.rs:1130` 的镜像断言风格）。

**需同步文档**：`docs/modules/viewer.md`、`docs/modules/storage.md`（缩略图失败语义）、`docs/ui-naming-reference/index.html`。

**实施结果**（2026-10-02 已落盘）：`video_error_box` → `media_error_box`，class 家族 `.viewer-video-error` → `.viewer-media-error`，图标改为具名 `media_error_icon` 并按媒体种类切换，新增 `media_error_retry_btn`（对当前 index 重跑 `show_at`）与 `media_error_reveal_btn`（`gtk::show_uri_full` 打开所在目录，无处理器时 toast）。`show_media_error` 按 `item.is_video()` 选措辞，图片正文点名文件。

偏差三条，都是有意为之：

1. **条件是「无可画内容」而不是「解码 Err」**。原图解码失败时预览缩略图通常仍然成功并在画，此时盖上「无法显示这张图片」与用户眼前的图像自相矛盾；所以 `show_original_decode_error` 只在 `picture.paintable().is_none()` 时接管舞台。
2. **草案的「两处材质镜像」不适用**：错误面是平面主题洗色，和原来一样只存在于 `base.css`，没有进入 liquid/plain 材质层。已把这一点写进 `viewer.md`，避免后续按 `ui-liquid-glass.md:126` 再补一遍。
3. **网格侧不加 `.thumb-broken`**：按方案第 2 点先做验证。`failed_image_placeholder_carries_a_distinguishable_mark` 直接测量 `generate_unavailable_placeholder()` 的像素——图标覆盖比例、与背景的明度分离、红斜杠的色度都过阈值，而加载态是中性的 `alpha(@window_fg_color, 0.05)` 洗色，产不出那条斜杠。故「失败格与加载格都是空白」的原判断不成立，`square_tile.rs`、`factory.rs`、loader 回调均未改动。

**测试**：`cargo test --locked --lib ui::viewer_page::stage`（缺失文件 → 错误面可见并含文件名与重试）、`cargo test --locked --lib thumbnails`（占位可辨性测量）。

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

**实施结果**（2026-10-02 已落盘）：状态分级的落点是 `.thumb-state-glass` 覆盖子节点，不是草案指的 `.glass-thumb-card` background——图片不透明，卡片底色透不过去；先按草案改卡片底色时像素采样两态都是 162.7，才暴露出真正的渲染层。hover 的 veil 由 `linear-gradient(180deg, 0.30, 0.42)` 降到 `(0.14, 0.20)`（径向部分同比减半），选中保持原值；选中组写在 hover 组之后并显式列出 `.media-selected.thumb-pointer-hover`，否则同特异度下会被 hover 规则抢走。对勾常驻：多选模式下未选中格子 `opacity: 0.32`、选中 `1`，规则用 `:not(:selected)` / `:not(.media-selected)` 划作用域而不是靠源码顺序压特异性。模式本身是 grid 级 class `multi-select-active`——FlowBox 侧由既有的 `apply_selection_mode` 顺手挂到每个分区，GridView 侧新增 `set_multi_select_flag` 作为 `is_multi_select_mode` 的唯一写入者（覆盖 set_multi_select_mode / select_ids / clear_selection / 键盘 Space / 右键进入）。

偏差：草案的备选「选中态加 accent 内环」没有实施——实测的强度差加常驻对勾已足够分开两态，多一条环会和 P0-2 的键盘焦点环竞争注意力。方案第 3 点（全局多选提示）确认由已落盘的 `selection_count_label` 与模式绑定的退出按钮承担，本次只补上逐格的常驻对勾，没有新增控件。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::grid_css`（含 hover/selected 采样差与常驻对勾的 CSS 契约）、`ui::media_grid`、`ui::virtual_media_grid`（class 跟随多选模式）。

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

**实施结果**（2026-10-02 已落盘）：落地了方案 1、2 和方案 3 的「容器轮廓」分支。

1. **重排**：`viewer-page.blp` 的 `viewer_zoom_controls` 现按 `zoom_out_btn` → `zoom_in_btn` → `zoom_transform_sep` → `rotate_left_btn` → `rotate_right_btn` → `zoom_state_sep` → `zoom_reset_btn` → `fullscreen_btn` 声明，并在 blp 注释里写明「这是普通 `Gtk.Box`，声明顺序即视觉顺序」，以免和 `photos-page.blp` 的 `[end]` 反向声明混淆。分隔线不是静态装饰：`update_zoom_buttons`（`src/ui/viewer/transform.rs`）让 `zoom_transform_sep` 跟随「放大后整组消失的旋转组」，因此它不会悬在簇尾，而 `zoom_state_sep` 常驻——草案说的「用 `Gtk.Separator` 或增大组间 `margin`」选了前者，因为可见性由按钮组驱动比由 margin 驱动更可验证。
2. **命中区与间距**：`.viewer-overlay-nav-btn` 由 36×32 提到 `min-width: 40px; min-height: 36px`（prev/next 与簇内按钮共用），两个簇的 `spacing` 从 4 到 6。静止态裸图标的材质规则未动。
3. **可发现性**：`.viewer-zoom-controls` 获得 `padding: 4px; border-radius: 14px; background: alpha(black, 0.16); border: 1px solid alpha(black, 0.30)` 的常日内描边，`.viewer-zoom-controls separator` 用 `alpha(black, 0.38)`。它刻意不是玻璃面（无 blur、无主题底色，只是压在照片上的一层黑），因此不存在草案要求的「两处材质镜像」，两种材质下同一套值——这一点写进了 CSS 注释与 `viewer.md`。

偏差：方案 3 的另一分支（首次进入播放 ≤600ms reveal 提示）没有实施，它依赖尚未落盘的 P1-14 reduce-motion 开关，且与 P1-8 的舞台输入提示属同一类一次性引导，留到那条一起做。方案 4（倍率标签计入簇宽）在本机是空谈：P1-7 的 `zoom_level_label` 从未实施，簇内目前不含动态宽度文本，因此没有窄窗口溢出的新风险。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::viewer_page::transform`（四组相邻断言锁住「声明顺序＝视觉顺序」，加两条分隔线可见性随缩放态）、`--lib ui::grid_css`（40×36 命中区与 `.viewer-zoom-controls` 日内描边在两种材质下都成立）、`--test ui_viewer_toolbar --test e2e_viewer --test ux_click_flows`。`tests/ui_viewer_toolbar.rs` 原本也按位置断言簇内顺序，与 `transform/tests.rs` 重复且已随重排失效，改为只断言整组仍住在一个容器里，顺序契约由模块内测试独占。

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

**实施结果**（2026-10-02 已落盘）：地板值按方案 1 落地——媒体错误面 `0.76→0.80`、副标题 `0.58→0.80`、标题 `0.82→0.90`、图标 `0.54→0.72`（大图标走 3:1 档），搜索「更多」瓦片 `0.52→0.78`，库统计与总览同步行 `0.68→0.78`，查看器同步徽记 `0.72→0.78`，侧栏计数 `opacity 0.72→0.78`，关于文本 `opacity 0.56→0.72`（liquid/plain 两份）。方案 2 的断言已进 `render.rs`：新增 `functional_text_holds_a_contrast_floor_over_glass`，把每个站点**真实的表面**（`.viewer-media-error` 面、`.search-more-tile` 面、`@glass_reading_bg` 阅读面）在两种材质 × 两种主题 × 透明度 0/100 下渲染，从**整窗**快照按控件分配取中位色当背景，再把解析出的前景按 source-over 合成上去量 WCAG 比值（文本 4.5:1、大号与图标 3:1）。

三处与草案不同，都是落地时才发现的：

1. **必须量整窗而不是控件自身**。`gtk::WidgetPaintable::new(Some(widget)).snapshot()` 只画该控件，父层透进来的照片不在里面——第一版这样量，深色主题下读到的是「不透明深底上的浅字」，比值 13:1，把 0.52 也判通过。改成整窗快照 + `translate_coordinates` 定位分配区之后，同一组数字立刻落到 4.1:1 一档。
2. **对抗底不能取纯白/纯黑**。透明度拉满时 Liquid 阅读面是 `alpha(@window_bg_color, 0.72)`，其下若真是 255 白，文本要 α≈0.87 才够 4.5:1——那等于取消半透明设计。测试取「与文字对抗但仍是照片」的一侧（深色主题 0.85 白、浅色主题 0.15 黑），比值门槛保持 4.5:1。这条边界写进了测试注释，不是悄悄放宽。
3. **`opacity` 类站点量不到**。GTK 的 opacity 在渲染期合成，`style_context().color()` 读不到它，所以关于文本与侧栏计数改在样式表上断言（`grid_css/tests.rs::opacity_muted_status_text_stays_above_the_floor`，复用既有 `css_block` 解析器）。

方案 3/4 按草案执行：照片叠层的白前景 + 暗光晕未改，模式选择器的 0.72 未动（它由 `ui-liquid-glass.md` 的选择器地板 0.66–0.78 单独管辖）。菜单禁用态保留 0.45，但「同时降敏感度」在 GTK4 里是结构性成立的——`:disabled` 只在 insensitive 时匹配，这一点写进了 CSS 注释。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::grid_css`（53 项）。契约有效性用回退旧值反向验证过：`library-stats` 0.68 → 4.41:1 失败、`search-more-tile` 0.52 → 3.29:1 失败、`viewer-media-error-subtitle` 0.58 → 3.38:1 失败、`settings-about-text` 0.56 被地板测试拦下。

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

**实施结果**（2026-10-02 已落盘）：新增 `src/ui/motion.rs` 作为全项目唯一读取 `GtkSettings:gtk-enable-animations` 的入口（`Settings::default()` 返回 `Option`，没有设置后端时按 GTK 自身默认值 `true` 处理，无头测试不会因此改变行为）。三条通道：

1. **CSS**：`build_css_with_motion(liquid, transparency, reduce_motion)` 在 base/material/a11y 之后追加 `REDUCED_MOTION_CSS`（`* { transition-duration: 0ms; }`）。`install()` 首次注册时连上 `notify::gtk-enable-animations` 并重建 provider，两个方向都是活的。草案担心的「全局 `*` 太宽 / GTK 可能不接受」经实测都不成立：`motion/tests.rs` 用 `connect_parsing_error` 钉住 GTK 确实解析该声明，而只改 duration、不动 `transition-property` 列表，也就不存在波及尺寸动画的副作用——**逐选择器重写 7 个时长的退路没有用上**。`build_css` / `build_css_with_transparency` 保持 motion-on，普通 CSS 字符串测试不会因为跑它的机器开了 reduce-motion 而变红，开关由 `css_for_tests_with_reduced_motion()` 显式注入。
2. **模板级 transition**：`GtkRevealer` / `GtkStack` 的过渡写在 Blueprint 里、CSS 无入口，由 `motion::apply_to` 递归走子树设成 `None`（Stack 另设 duration 0）。调用点是五个页面与主窗口的构造，外加开关转「关」时对当前窗口再走一次。**已知非对称**：转「开」不恢复已经在屏幕上的页面——那需要记住每个控件模板里 authored 的值；新构造的页面会重新按开关取值。这条限制写进了 `motion.rs` 与 `ui-liquid-glass.md`。
3. **Rust 驱动动画**：胶片条的 frame-clock 滚动改由纯函数 `thumb_scroll_should_animate(distance, motion_enabled)` 决定，reduce-motion 时直接跳到目标（复用原有的「小于半像素就不动画」早退路径）。

偏差：草案把 `mode_selector.rs:222-263` 列为代码侧动画，现状不是——滑轨 indicator 自 backdrop 重构后只写 CSS `transform`，其 300ms 过渡已被尾块覆盖，因此该文件未改动。另外没有新增应用内「减少动画」开关：控件就是桌面的辅助功能设置，多一个应用内开关只会与它不一致。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::motion`（3 项：走子树剥离 / 开关为开时一律不动 / GTK 解析尾块）、`--lib ui::grid_css`（尾块只追加一条、排在最后、不改写原表）、`--lib ui::viewer_page::filmstrip`（早退条件三个方向）。

**需同步文档**：`docs/modules/ui-liquid-glass.md`（新增「动效与 reduce-motion」一节，并解释为何不用 `@media`）、`docs/testing.md`（新单测说明）。

---

### P1-15 搜索结果分区里的照片没有任何批量动作通路

**现象**：搜索页的年份/月份/类型预览分区里，瓦片既不能右键、也没有长按回退、更没有任何进入多选的方式。P0-3 补齐照片页与相册详情页之后，这里成了全应用唯一「选中不了」的媒体列表。

**证据（已验证）**：

- 生产代码里旧 `MediaGrid`（FlowBox）只剩一处实例：`search_page.rs:399` 的 `build_result_section()`。
- 它走 `MediaGrid::new_for_album()`（`media_grid.rs:580-589`），第五个参数 `enable_context_menu` 传 `false`（`media_grid.rs:605`），因此 `imp().enable_context_menu` 为假，右键菜单路径整条关闭。
- `search_page.rs:406-409` 的回调里 `on_add_to_album` 与 `on_set_favorite` 都是空闭包，`on_query_favorite_state` 恒返回 `FavoriteMenuState::default()`；也就是说即便打开菜单，收藏与加入相册两项也是死动作。
- 长按回退只在 `virtual_media_grid/factory.rs:97-111`，旧网格没有对应实现；P0-3 期间曾尝试补上，因该网格 `enable_context_menu: false` 而成为不可达代码，已回退。
- 搜索页另一条路径 `VirtualMediaGrid::new_for_query()`（`search_page.rs:733`，「显示更多」后的完整结果页）是虚拟网格，入口与长按都在，不受本条影响。

**方案**：

1. 让预览分区的瓦片可达右键菜单：`build_result_section()` 改用 `MediaGrid::new_for_album_with_context_menu()`（`media_grid.rs:591-598` 已存在，无需新 API）。
2. **顺序前提**：先接上真实回调，再开菜单。`on_add_to_album` 需要搜索页把 `MediaId` 交给现有的加入相册流程（与 `photos_page.rs` 同一批处理函数），`on_set_favorite` 走 `MediaRepository` 的收藏写入，`on_query_favorite_state` 改为查真实状态；否则就是在开一扇通向 no-op 的门。
3. 菜单需要一个页面级 overlay 宿主：`GlassContextMenu` 通过页面 overlay 渲染，`search-page.blp` 目前没有对应容器，参照 `photos-page.blp` 的 `grid_overlay` 补一个。
4. 若产品决定分区保留完整批量路径，应同时接入右键/长按进入多选和页面级批量 chrome；若分区只用于预览，则把「显示更多」后的 `VirtualMediaGrid` 作为唯一批量操作路径并在文档写清。header 常驻入口不属于默认方案（P0-3 已按库主决定撤回）。
5. **收缩方案**（若判断分区不该有批量动作）：反向做法是明确让分区只用于预览、把「显示更多」当作唯一可批量操作的路径，并在文档与命名图里写成契约。这需要产品决策，不能靠沉默实现。

**落点文件**：`src/ui/search_page.rs`、`data/ui/search-page.blp`、`src/ui/media_grid.rs`（`enable_context_menu` 的调用点）、`i18n/*.json`（若新增文案）。

**测试**：`cargo test --test ui_search_page`（新增：分区瓦片右键/入口按钮能产生非空 `selected_ids()`）；`cargo test ui::media_grid`；`tools/assert-at-spi.py` 增加搜索分区的一次可达性检查。

**需同步文档**：`docs/modules/browsing.md`（当前那段「legacy FlowBox grid has neither door」需改为已修复或已明确的契约）、`docs/modules/ui-design.md`（搜索分区行为）、`docs/ui-naming-reference/index.html`（搜索分区的 `data-ui` 条目）。

**实施结果**（2026-10-02 已落盘）：严格按草案定的顺序做——**先接实回调，再开菜单，最后补宿主**。

1. `build_result_section()` 改用已存在的 `MediaGrid::new_for_album_with_context_menu()`，没有新增网格 API。
2. 三个空实现换成真流程：`on_add_to_album` → 共享的 `album_picker::AlbumPickerDialog::present(&nav, pool, db_actor, loader, ids)`（与照片页同一个对话框，写入与缩略图失效由它负责）；`on_set_favorite` → `DbCommand::SetFavorite`，成功后 `apply_favorite_flags` 把标志写回 `image_list`/`video_list` 两个预览 ListStore 与 `detail_grids`，并清掉选中（菜单关掉而瓦片还亮着，读起来像动作没完成）；`on_query_favorite_state` → `MediaRepository::favorite_state(ids)`，所以菜单给「收藏」还是「取消收藏」取决于选中行的真实状态。
3. `search-page.blp` 把原 `search_state_stack` 包进新增的 `Gtk.Overlay search_overlay`；`GlassContextMenu` 运行时 `add_overlay` 挂进去。这一层是必需而非装饰：没有宿主的网格是**静默丢掉菜单**的。

偏差与边界：草案 4 的分区 header 入口按钮没有做，命名图里那枚 `search-section-select-mode-button-proposal` 热点已删除——P0-3 刚按库主决定撤掉照片页与相册页的常驻入口、把右键定为主要路径，分区再放一枚等于当场推翻那条决定。长按回退也没有补到旧 FlowBox 网格：草案自己记过这笔（P0-3 试过，因 `enable_context_menu: false` 成为不可达代码而回退），现在菜单虽可达，旧网格仍只有右键一条路，这一点写进了 `glass-context-menu` 命名条目与 `browsing.md`。`tools/assert-at-spi.py` 的搜索分区可达性检查**没有加**：它要 Flatpak 运行时才能执行，本机跑不了，留一条没跑过的探针比不加更糟；同一契约由进程内 GTK 测试守住。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::search_page` 新增两条——`the_preview_sections_open_the_same_batch_door_as_albums`（分区网格的菜单宿主就是页面的 `search_overlay`，且进入多选后 `selected_ids()` 非空）、`the_favorite_menu_items_ask_the_database_instead_of_defaulting`（未收藏的行只给 `can_favorite`，写进库后翻成 `can_unfavorite`，`apply_favorite_flags` 的写回落到预览 ListStore）。为此 `MediaGrid` 增加 `context_menu_overlay_for_tests()` 读取器：网格是否带菜单此前没有对外可查的口径。

---

## P2 检视项（简表，实施时并入对应批次）

| # | 现象 | 证据 | 建议 |
|---|---|---|---|
| **P2-1（已落盘）** | Toast 一律无撤销，删除只能去回收站找回 | `src/ui/toasts.rs:19-40` 三个工厂都用 `adw::Toast::new(msg)`，全项目无 `set_button_label` | 新增 `success_with_action(overlay, msg, label, f)`；删除/批量收藏接 `viewer/actions.rs:81-93` 已有的回滚 |
| **P2-2（已落盘）** | Toast 可能盖住胶片条 | `data/ui/viewer-page.blp:7` 的 `ToastOverlay` 包裹整块内容 | 限定 overlay 区域到 stage，或为 toast 预留底部 inset |
| **P2-3（已落盘）** | 模式选择器对读屏是三个静态标签 | `data/ui/mode-selector.blp:18-45` 用 `Gtk.Box` + 点击手势，无 role/label | 保留单胶囊视觉（`ui-liquid-glass.md:92-105` 是硬契约），用 `set_accessible_role(Button)`+`set_accessible_label` 补语义；或改 `Gtk.ToggleButton` + `.glass-segment`（`search-page.blp` 已走此路，`ui-liquid-glass.md:108-110`） |
| **P2-4（已落盘）** | 全项目没有任何无障碍角色/名称调用 | 原证据要更正：`set_accessible_label` 在 GTK4 里不是 API，grep 测的是不存在的符号；真实的 0 是模板 0 处 `accessible-role` + Rust 0 处 `update_property`。「26 个图标按钮都缺 tooltip」也不成立——26 枚里 22 枚本来就有，缺槽的是 4 枚（两枚裁剪比例箭头连 Rust 文案都没有） | 见「P2-4 实施结果」：4 枚补槽、2 枚补文案，名称侧给 tile 与全部状态徽标，装饰图标改 presentation；`tools/assert-at-spi.py` 那条被本机 pyatspi 探针替代（见 `docs/testing.md`） |
| **P2-5（已落盘）** | 选择相关的 DB 查询在主线程同步执行 | `photos_page.rs:1155`（每次选择变化 `favorite_state`）、`:1337`（同步取 2000 条）、`:1360`（同步 count） | 见「P2-5 实施结果」：三处都不在选择节拍上了，另加数据层一条被漏掉的 N+1 |
| **P2-6（已落盘）** | 相册选择器加载中是空网格；DB 报错显示成「暂无相册」 | `src/ui/album_picker.rs:174-220`（`:214-218` 把错误渲染为空态标题） | 见「P2-6 实施结果」：四页 Gtk.Stack（loading/empty/albums/error）＋可重试的错误页；顺带纠正草案对相册详情页的一处误判 |
| **P2-7（已落盘）** | 回收站每次打开先闪一下「回收站为空」 | `src/ui/trash_page.rs:122-132` 空态 child 常驻直到数据落地 | 见「P2-7 实施结果」：四页 stack ＋ `first_load_done` 闸门；真正挡住闪烁的是 `refresh()` 里的判断，不是 `constructed` 的初始页 |
| **P2-8（已落盘）** | 全库总览只能靠「顶部再往上滚」发现，明确无 disclosure 按钮 | `photos_page.rs:1243-1257`、`docs/modules/browsing.md:133-141` | 见「P2-8 实施结果」：chevron 逐字复用搜索的材质、状态从 revealer 派生；重试只在 Failed 与读取失败两处出现，且走既有 pull。原「`:1279` 错误仅日志」已不准确：`photos_page.rs:1294-1303` 会将失败写入总览文案。 |
| **P2-9（已落盘）** | 收藏按钮在「直接切换」与「弹层」间隐形变化 | `photos_page.rs:754-770`、契约见 `docs/modules/ui-design.md:140-147` | 见「P2-9 实施结果」：混合态独享一枚 overlay 角标（`favorite_menu_hint`），tooltip 与 accessible name 同步换成 `photos.batch.favorite.mixed`；没有拆分按钮，所以 header 密度契约不被触碰 |

### P2-1 实施结果（2026-10-02 已落盘，草案前提需要更正）

`toasts::success_with_action(overlay, msg, label, f)` 已实现，超时 6 秒（普通
success 的两倍——用户够不着的撤销不算撤销），返回 `adw::Toast` 供测试按名发出
`button-clicked`。两条动作接上了它：

1. **收藏**：`viewer/actions.rs` 的写入抽成 `apply_favorite_state(item_id, next_state, announce)`，
   toast 按钮以 `announce = false` 重入。可回滚的回滚是 toast 链，不是撤销。
2. **移到回收站**：草案说「接 `actions.rs:81-93` 已有的回滚」，但那条
   `DbCommand::RollbackTrashed` **只清 DB 标记，文件仍在回收站里**，直接接上会留下
   指向空文件的瓦片。真正的还原 API 确实存在，只是在别处且回收站页一直在用：
   `core/trash.rs:830 restore_from_trash` 与 `core/repository.rs:469 restore_batch`
   （`prepare_restore` 先把 payload 移回原路径 → 经 actor 写 `RestoreTrashed` → 再
   commit）。所以撤销走 `restore_batch(&ids, Some(&db_actor))`：**经过 actor 才发得出
   DomainEvent**，其它页面才跟得上；直连 pool 只修好当前视图。顺序是「文件先回来、
   列表后动」，行由 `media_list::insert_media_item_sorted` 按 `sort_datetime` 落回时间线
   原位（该 helper 从 `trash_page.rs` 提成共用，回收站页改为调用同一份），完成后
   `show_at` 回到这张照片。

偏差：草案点名的**批量收藏没有撤销 toast**。全应用只有查看器装了
`Adw.ToastOverlay`，照片页与搜索结果页没有任何 toast 宿主；而收藏本身是同一个右键
菜单项可逆的开关，补一个窗口级 overlay 属于结构性改动，不静悄悄塞进一条 P2。i18n
新增 `favorited` / `unfavorited` / `undo` / `restore_failed` 四键（两份 json 同步，parity
446/446），`moved_to_trash` 的「可在回收站找回」在按钮出现后收回成「已移入回收站」。

**测试**：`cargo test --lib ui::toasts`（`an_action_toast_offers_its_button_and_fires_it`：
按钮标签、超时 > 3s、`button-clicked` 确实触发回调）；
`tools/with-at-spi.sh xvfb-run -a cargo test --test ux_click_flows` 新增
`journey_viewer_delete_toast_offers_undo`——真实文件移到回收站 → 在 toast 上找到按钮并
点击 → 断言文件回到原目录、DB `trashed_at` 清空、瓦片回到网格数量、查看器重新显示这张
照片。负向验证：把 `restore_deleted_item` 改成 no-op 后该断言确实变红。

### P2-2 实施结果（2026-10-02 已落盘，取草案第一半）

`Adw.ToastOverlay toast_overlay` 从 `viewer-page.blp` 的页根挪进 `content_box`，只包
`image_overlay`（舞台），`viewer_bottom_stack` 成为它的兄弟节点。libadwaita 把提示贴在
宿主子节点的下边缘，于是宿主下边缘＝胶片条上边缘。实测
（`src/ui/viewer_page/tests.rs::a_toast_lands_above_the_filmstrip`）：改结构前 toast 卡片
画在 `410..456`，而胶片条条带是 `366..468`——整条被压住；改后卡片 `288..334`，舞台底
`358`、条带顶 `366`，让开 32px。两次测量里舞台（`54..358`）与条带（`366..468`）分配完全
一致，说明多插一层容器没有改动布局。

草案第二半「为 toast 预留底部 inset」**实测不可用**：内部子节点 `AdwToastWidget` 没有
Rust 绑定，`toastoverlay .toast` 选不中，而 `toast` 节点上的 `margin-bottom` 只是把节点
撑高（底边仍钉在宿主下边缘），卡片并不上移。因此 base.css 里那条 margin 规则已删除，
位置只由结构决定，不留一条「看起来生效」的 CSS。测量口径也要记一笔：`allocation()` 含
主题外边距（节点 82 对卡片 46），且 `Adw.ToastOverlay` 子节点的 `translate_coordinates`
与实际绘制位置差 32px，所以断言只用 `compute_bounds`。

偏差与未验证：草案担心的「缩小 overlay 影响其它页面」不成立（别的页面没有宿主），但
代价是详情/编辑侧栏触发的 toast 现在画在舞台上而不是窗口底部——提示说的就是这张照片，
可以接受。舞台右下角的上一页/下一页与 toast 在很窄的窗口里可能相叠（卡片居中、实测宽
150px，900px 窗口下与箭头相距约 165px），这一点没有真机目测。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --lib ui::viewer_page`（新
`a_toast_lands_above_the_filmstrip`；负向验证：把 `viewer-page.blp` 还原成页根包裹后该
测试变红）＋ `--test e2e_viewer --test ui_viewer_toolbar --test ux_click_flows` 全绿。

### P2-3 实施结果（2026-10-02 已落盘，取草案第一方案）

没有换成 `Gtk.ToggleButton`：角色写在模板里（`accessible-role` 在 GTK 是可读写属性，但
Blueprint 只认枚举名下划线形式 `radio_group`，写成 `radio-group` 直接编译失败）。整件控件
`radio_group`，三个 `label_cell` 是 `radio`，指示条那一行是 `presentation`——滑动的短条只
是重复「哪一段被选中」，不该在无障碍树里变成第四件要读的东西。名称与状态由 Rust 下发：
`set_labels_i18n` 用**同一个** `tr()` 字符串同时写可见文案与 `accessible::Property::Label`
（两者不可能在不同语言下漂移），整件控件取新键 `photo.mode.group`（两份 json 同步，parity
447/447）并标 `Property::Orientation(Horizontal)`；`apply_state` 给三段各推
`State::Checked(True/False)`，所以永远恰好一段被勾选。

保留原样：整件控件是唯一 tab stop + 左右方向键（含环绕）。草案与原型都给每一段发
`tabIndex`，落盘没有跟——逐段可聚焦会在一个胶囊里画出三个焦点环、把一次 Tab 变成三次，
并直接违反上面那条「单胶囊、内部状态轻量」硬契约。

实测证据（本机、非 CI）：`env -u NO_AT_BRIDGE HOME=<tmp> tools/with-at-spi.sh xvfb-run -a
./target/debug/photo-viewer` 起真实应用（**必须**去掉 `NO_AT_BRIDGE`，开发 shell 里它默认为
1，会静默关掉 GTK 的 AT-SPI 桥——应用照样注册到总线上但树是空的），再用 pyatspi 走树读到
`grouping '照片分组方式'`（FOCUSABLE）下挂 `radio button '年' / '月' / '日'`，`CHECKED` 落在
应用当时记住的那一段。注意角色会改名：`radio_group`→`grouping`、`radio`→`radio button`。
配方与坑都记进了 `docs/testing.md`。

限制记录（不做沉默处理）：三段仍是 `Gtk.Box` + `GestureClick`，**不暴露 AT-SPI action**——
读屏能播报、也能在聚焦的胶囊上用方向键改选，但没法「按下」指定某一段。草案自己点过这条
风险（换 ToggleButton 会触碰材质契约），所以这里记录限制而不是换控件。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --lib ui::mode_selector`（19 项，新增
3 条：角色树、恰好一段被勾选、名称与可见文案同源；负向验证：从 blp 删掉 `accessible-role`
后第一条变红）＋ `--test ui_mode_selector --test ux_click_flows`（段上的点击手势与模式切换
不受 `constructed` 重构影响）。

### P2-4 实施结果（2026-10-02 已落盘，草案的两条测量口径都要更正）

**更正一：`set_accessible_label` 在 GTK4 里不是 API。** 可写的属性只有
`accessible-role`（`data/ui/*.blp` 里可直接声明，Blueprint 用下划线枚举名），名称与状态要走
`gtk_accessible_update_property(Property::Label)` / `update_state(...)`，而这两者都没有 getter。
所以「全项目 `set_accessible_label` 调用为 0」grep 的是一个不存在的符号；结论仍然成立——落盘前
全仓 0 处 `accessible-role`、0 处 `update_property`，落盘后是模板 7 处角色 + Rust 23 处
角色/名称调用。

**更正二：「约 26 个图标按钮先补 tooltip 兜底」不成立。** 按「有 `icon-name` 且没有 `label`」
清点 `data/ui/*.blp`，图标按钮确实是 26 枚，但 22 枚本来就有 tooltip，而且多数与 P0-4 的
`tooltip_with_key` 共用同一份文案——草案 risk 担心的「两套措辞」并没有发生。真正缺槽的是 4 枚：
`crop_ratio_prev_btn`、`crop_ratio_next_btn`、`edit_btn`、`settings_button`。前两枚连 Rust 侧
文案都没有（悬停无提示、读屏只念图标名），新写 `editor.crop.previous.tooltip` /
`editor.crop.next.tooltip`（两份 json 同步，parity 452/452）；后两枚本来分别由
`viewer_page.rs:522`、`window.rs:651` 写入，只补了模板空槽。兜底断言
`tests/ui_template_copy.rs::icon_only_buttons_reserve_a_tooltip_slot` 扫全部 icon-only 按钮，
缺槽即红。**这条断言的首版是假绿**：它按 `depth == 0` 判断根节点，嵌在 box 里的按钮根本没被
检查过，重写成深度跟踪后当场揪出上面这 4 枚。

**名称侧（草案点名的「状态类」）**：`SquareTile` 在 `class_init` 里取类级角色 `Img`，绑定者把
用户看得见的内容推成名字——`set_accessible_name(item.display_name())` 落在
`media_grid/render.rs` 的 `prepare_reused_tile` / `build_photo_picture` 与
`virtual_media_grid/factory.rs::bind_ready_cell` 三处，`clear_for_rebind` 连名字一起清掉，回收的
格子不会继续回答上一张照片。时长/动态/收藏徽标与云徽标各报自己的名字
（`tile.badge.duration` 把可见的 `0:42` 包成「视频时长 0:42」；云徽标与 tooltip 同源）。装饰性
图标一律 `presentation`：`overview_sync_icon`、查看器错误面图标、侧栏行图标与相册分组箭头、相册
选择器的文件夹占位、设置页那枚 chevron。草案把「`overview_sync_icon`（`:949`）与警告图标
（`:1037`）」算成两处，其实是同一个 `Gtk.Image` 的两种状态（`Failed →
dialog-warning-symbolic`，`photos_page.rs:113`），旁边就是 `overview_sync_label`，所以按重复
信息处理成装饰，而不是补 tooltip。

**实测证据（本机、非 CI）**：`docs/testing.md` 的 pyatspi 探针走真实应用的树。第一轮把名字给了
常驻的对勾，结果格子念成「已选中 已收藏」而文件名全部消失——`SquareTile` 当时是容器角色，GTK
直接丢掉推给它的名义；改成「对勾 `presentation` + tile 类级 `Img`」后，`table cell` 的名字变成
`['a.jpg', 'b.gif', '', '']`（后两格是虚拟网格的 filler，本来就没有媒体）。这轮如果没有实测，
按草案原意（给状态角标补 label）落盘会稳定地让网格更难读。

**限制记录（不沉默处理）**：选中目前**不是 accessible state**。虚拟网格的 `GtkListItem` 是
`set_selectable(false)`（选择由 `VirtualMediaGrid` 自己管，不走 `GtkSelectionModel`），对勾又是
装饰，所以读屏听得到文件名、听不到「这张被选了」。补它需要 `SquareTile::set_selected(bool)`
统一现在散在 4 处的 CSS class 增删并推 `State::Selected`，而且要先验 `img` 角色是否接受这个状态；
本轮没有做，写进了 `docs/modules/browsing.md`。`AdwStatusPage` 内部的警告图标同理没动：标题与
描述已经说完这句话，且那是 libadwaita 的内部节点。

**测试**：`cargo test --locked --lib ui::square_tile`（18 项，新增
`the_tile_and_its_state_badges_are_named`：类级角色、名字可回读、`clear_for_rebind` 清名、徽标键
真的解析、对勾是装饰；负向验证：注释掉 `klass.set_accessible_role(Img)` 后该断言变红）＋
`--test ui_template_copy`（3 项）＋
`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::`（403 项）＋
`--test ui_editor_panel --test ui_album_picker --test ui_mode_selector --test ux_click_flows
--test e2e_browsing --test ui_context_menu --test ui_grid_canvas --test inline_test_ownership`
（全绿）＋ 手工 AT-SPI 探针（PASS，需要桌面会话，不进 CI）。

### P2-5 实施结果（2026-10-02 已落盘，草案低估了数据层那一处）

三处同步查库都不在选择节拍上了：

1. **`refresh_selection_ui` 不再问库**。它先 bump 一个 `selection_generation`（只在选中集合
   真的变了才 bump——同一份选择被重复 emit 不该把在途答案判成过期），然后
   `refresh_selection_favorite_state` 起一次 `glib::spawn_future_local` +
   `gtk::gio::spawn_blocking`。答案回来时代次已变就丢弃并立刻重查，所以「停下来的那份选择
   一定拿到自己的答案」，同时最多一条查询在飞。
2. **全选取 2000 条 id** 移进 `select_all_in_current_mode` 的 worker，`select_all_in_flight`
   挡住连点（连点时第二次直接早退，不会出现「选两遍」或「清空一个还没选上的集合」）。
3. **`count(LiveAll)` 不再单独查**：它挂到本来每 2 秒就在后台跑的总览快照
   （`PhotosOverviewSnapshot.live_total`），首帧之前退回「当前可见格是否全选」。

**草案把这三处写成三次查询，真正的瓶颈是其中一次的形状**：`repository.favorite_state` 是
**每个 id 一条 `SELECT`**，2000 项选择＝2000 次 prepare，而不是「一次 favorite_state 查询」。
只把它挪到后台线程等于把掉帧换个地方掉。现在换成每 500 个 id 一条聚合语句
（`db::favorite_state_for_ids`：`SELECT COUNT(*), MAX(is_favorite=1), MAX(is_favorite=0)`），
并保留原来「查不到该 id 记为未收藏」的容错口径（返回行数少于请求数即视为有未收藏）。
20000 库、2000 项选择实测 **15ms → 2ms**（debug 构建；命令与输出字段已加进 opt-in 基准
`tests/library_benchmark.rs`，不再是口头结论）。

**risk 里担心的中间态确实出现过，而且是测试逼出来的**：中途把收藏按钮改成读「画出来的状态」
（省一次查询），`ux_click_flows` 的批量收藏旅程立刻变红——刚换完选择就点，缓存还是上一份
选择的结论（清空＝两个都 false），于是点下去什么都不发生。所以点击路径
`decide_favorite_action` 自己问库再决定分支，只在代次未变时顺手更新绘制。心形是智能开关，
「按下去什么也没发生」不是可接受的中间态。

偏差：瓦片右键菜单的 `on_query_favorite_state` **仍然同步**——它是一次性动作、不在选择节拍
上，异步化要改 `GlassContextMenu` 「先问再建菜单」的契约；它现在也被同一条聚合语句兜住了。
`search_page.rs` 有一份同名同语义的私有 helper，同样留在同步路径，没有一并改。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::photos_page`（18 项，
新增两条：`the_favorite_state_of_a_selection_lands_from_a_worker`（选择节拍里画不出答案 →
worker 落地后心形才红 → 把在途刷新钉住后立刻点击，仍按新选择行动）、
`the_select_all_label_decides_from_the_cached_total`（故意让缓存与库不一致，证明读的是缓存）。
负向验证：把刷新换回同步查询后第一条变红；把点击改回读缓存后它不去收藏而是去弹 popover）＋
`cargo test --locked --test repository`（27 项，新增三条 favorite_state 语义：均匀选择不被报
成混合、跨 500 分块边界、缺失 id 记为未收藏；负向验证：去掉「行数少于请求数」那一支后第三条
变红）＋ `--test ux_click_flows`（四处全选断言改成 `wait_until`，同时抓出上面那个真 bug）＋
opt-in 基准手工测量（不进 CI）。

### P2-6 实施结果（2026-10-02 已落盘，草案对相册详情页的那半句是误判）

相册选择器改成一个四页 `Gtk.Stack`（`loading` / `empty` / `albums` / `error`），弹框一打开
停在 `loading`：

1. `loading` 复用共享的 `empty_states::loading()`：标题 `empty.loading`（加载中…）＋一个**已经
   `start()`** 的 `GtkSpinner`。草案画的「正在读取相册…」没有另起一套文案；未 start 的 spinner
   是冻住的空圈，读起来像渲染坏了——这条是 P0-1 真跑出来的教训，这次直接按它做。
2. `empty` 只在「查询成功且真的没有相册」时出现，标题与描述分别用
   `album_picker.no_albums_yet.title` / `.description`。落盘前是把 description 塞进底部那行小
   标签，而 title 被错误分支拿去当文案用，于是两件事共用一句话。
3. `error` 用新的 `empty_states::load_failed(msg, on_retry)`：标题 `empty.load_failed.title`
   （读取失败），描述 `empty.load_failed.description_with_reason` 带数据库原话，按钮
   `common.retry` 重跑**同一个** listing 闭包（首次加载与重试共用一份，重试会先清空网格、清掉
   选中并把复制/移动重新禁用，而不是留下半活的弹框）。
4. i18n 新增 `empty.load_failed.*` 三键＋`common.retry`（两份 json 同步，parity 456/456）。

risk 那条（错误信息可能含路径）按原样接受：描述显示的就是 rusqlite 的原始错误串。它同时是测试
注入的故障（`DROP TABLE albums`），所以「原因出现在页面上」这条是断言出来的，不是推断的。

**纠正草案的一处事实**：草案与命名图都把「相册详情页空态复用全库文案」列为 P2-6 关联项，但
`AlbumDetailPage` 从很早就用 `empty_states::no_album_photos()`（空相册 / 该相册暂无照片。），
git 历史可查。那条「现状对照」条目已从命名图删除，提案里更强的文案（此相册还没有照片 ＋ 去照片页
按钮）没有落盘，也不需要。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --test ui_album_picker`（同一个
弹框走完 loading→empty，再用坏库起第二个弹框走 loading→error，断言可见页标题、描述含真实原因、
页面上有「重试」）＋ `--lib ui::empty_states`（3 项，新增 loading 会转、load_failed 的标题/原因/
空原因回退/重试按钮真的回调）＋ `--lib core::i18n`（456/456）。两处踩过的坑记进测试注释：
`Gtk.Stack` 的隐藏页仍在 widget 树里，所以断言必须读 `stack.visible_child()`，第一版随便找了一个
`StatusPage` 结果读到的是错误页的标题；负向验证——把 error 分支改成显示 empty 页后该断言超时变红。

### P2-7 实施结果（2026-10-02 已落盘，落点跟草案写的位置不一样）

回收站的 `content_stack` 变成四页：`loading` / `content` / `empty` / `error`，另加一个
`first_load_done` 闸门。risk 说的「区分未加载与已加载且为空」就是实现方式：empty 只由一次
**完成**的读取产生。

**草案把修复写在 `trash_page.rs:122-132`（constructed 的初始页），但只改那里不够**：`build()`
在构造末尾就调 `refresh()`，所以真正挡住闪烁的是 `refresh()` 里「只有首轮才切 loading」那道判断。
负向验证正好量出这件事——只把 `constructed` 改回 `empty` 时新测试仍然是绿的，两处一起回退才变红。
这一点写进了命名图条目与 `albums-trash.md`，免得下一个人只改初始页就以为修完了。

顺带两条草案没写的：之后的 `refresh()` 不再退回 spinner（已经看到的网格不会被一次重算抹掉，
否则只是把闪空态换成闪白）；读取失败不再伪装成空态，改走 P2-6 刚落地的
`empty_states::load_failed`（数据库原话＋「重试」＝再调一次 `refresh()`），且失败时不设
`first_load_done`，所以重试会重新显示 loading。

**测试**：`tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::trash_page`（9 项，新增
`trash_opens_on_loading_and_only_then_decides` 与
`trash_first_read_lands_on_content_and_refresh_keeps_it_there`）＋
`--test e3e_albums_trash --test ux_click_flows`（回收站还原/永久删除旅程不受新页序影响）。

---

### P2-8 实施结果（2026-10-02 已落盘，risk 那一条决定了实现方式）

两个入口、一个动作：`overview_toggle_btn`（photos-page.blp:51）与
`overview_sync_retry_btn`（:215）。草案的 risk 写「按钮不应变成视觉重心」，所以 chevron
不新增任何材质——逐字复用搜索那枚的 `.glass-toolbar-button` + `.round-search-button`，
静止时 bare、hover/聚焦才出胶囊；本轮唯一新增的 CSS 是 `.photos-overview-retry`，它只是把
按钮压回行内 11pt 尺寸，不是新皮肤。

**状态是派生的，不是自存的。** `apply_overview_disclosure_state` 挂在 `overview_revealer` 的
`notify::reveal-child` 上，glyph（pan-down/pan-up）、tooltip 与 accessible name
（`photos.overview.show` / `photos.overview.hide`）全部从 revealer 读。如果按钮自己记一份
「我点过没有」，第一次用下拉手势展开就会说谎——测试里正是这一条最难：
`the_overview_disclosure_button_mirrors_the_revealer` 额外做了一次**没碰按钮**的展开。
图标按钮的状态属于名字，所以同一个串既 `set_tooltip_text` 又推
`gtk::accessible::Property::Label`；`photos.overview.hide` 是仓库里早就存在、一直没人用的死键，
这次才接上，i18n 因此零新增（parity 仍 456/456）。

**重试只在失败出现，且只在它能做事时出现。** `apply_overview_retry_affordance(failed)` 是唯一
写入者：`SyncOverviewStatus::Failed` 与总览读取失败（`apply_overview_error`）两处为真，
Completed/Running/Paused/Ready 只有描述，`Disabled` 连整行都收起。动作复用既有
`trigger_sync_from_home_pull()`，所以草案担心的「per-task Sync Now」没有发生，也没有新增同步
路径。`overview_sync_pull_in_flight` 期间按钮**置灰**而不是吞掉第二次点击；读取失败那条只在
`webdav_sync_enabled()` 时才出按钮——被该 pref 挡下的 pull 是死钮，这条是草案没写的。

顺带改掉一处文案：`photos.overview.sync.failed` 原文「打开设置查看详情或下拉重试」，在有了两个
入口之后「下拉」不再唯一，改为「可点「重试」，或在设置中查看详情」（中英同步）。

**测试**：`--lib ui::photos_page::tests`（20 项，新增 2 项；`the_overview_disclosure_button_mirrors_the_revealer`
与 `a_failed_sync_offers_a_retry_and_other_states_do_not`）＋ `--test ui_photos_toolbar`（5→6 枚
glass 按钮，并断言两枚圆钮同类）＋ `--test ui_template_copy`（新按钮的 tooltip 槽由该测试守住，
删掉 `tooltip-text` 后确认它会红）＋ `--test ux_click_flows`、`--test e2e_browsing`。
`narrow_window_keeps_the_start_header_button_allocated` 从量一枚改成量两枚 `[start]` 圆钮。
两处负向验证各自变红：摘掉 `connect_reveal_child_notify` → disclosure 测试红；把
`apply_overview_retry_affordance` 掏空 → 重试测试红。

---

### P2-9 实施结果（2026-10-02 已落盘，risk 那一条决定了控件形状）

指示物是一枚 12 px `pan-down-symbolic`（`photos-page.blp:115` `favorite_menu_hint`），
挂在心形右下角的 `Gtk.Overlay` `[overlay]` 上（`favorite_btn_host`，:104）。**为什么不是
拆分按钮**：草案的 risk 写「指示图标要与 P0-3 的 header 密度一起看，别挤爆」，而 split button
会为了最稀有的一种状态永久撑宽 header，同时撞上 `ui-design.md` 里「不要把收藏/取消收藏拆成两枚
header 按钮」的既有契约。overlay 的角标不占分配、不改按钮尺寸（38 px 圆钮不变），
`can-target: false` 保证它不吃掉本该落在心形上的按下。

**状态写入收成一个函数。** `apply_selection_favorite_state` 一次画好三样：`favorite-active`
红心、tooltip 与 `accessible::Property::Label`（**同一个串**，两条通道不能各说一套）、以及这枚
角标。混合态另有一键 `photos.batch.favorite.mixed`（中英同步，parity 457/457），因为「承诺收藏、
然后反问一句」在 tooltip 与读屏里是同一个谎——草案只提了视觉那一半。`new()` 里按
`FavoriteMenuState::default()` 先画一次，所以第一次选择之前按钮就已经有名字（这条同时补上了
P2-4 遗留的一处缺口：心形此前只有构造时那句裸 tooltip）。

角标**只属于混合态**：常驻会对着「直接执行」的那两种点击撒谎。红心与文案沿用原有分支，
`all_favorited` 的 `photos.batch.unfavorite` 不变。

**测试**：`--lib ui::photos_page::tests`（21 项，新增
`only_the_mixed_selection_marks_the_heart_as_opening_a_menu`：构造后的初始名字、三种状态各自的
文案/红心/角标、以及角标的 `presentation` 角色）＋ `--test ui_photos_toolbar`、
`--test ux_click_flows`（批量收藏旅程在 overlay 嵌套后仍绿）、`--test ui_context_menu`、
`--test e2e_browsing`。两处负向验证各自变红：删掉 `set_visible(mixed)` → 角标断言红；
把混合文案退回 `photos.batch.favorite` → 名字断言红。

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
| B3 多选与选择 | P0-3（含相册详情页 chrome，已落盘）、P1-6（header 选择计数器，已落盘）、P2-5（已落盘）、P2-9（已落盘）| `photos-page.blp`/`photos_page.rs`/`album-detail-page.blp`/`album_detail_page.rs`/`virtual_media_grid*`/`loading.rs` | B1（焦点环让多选态更易验证） |
| B4 快捷键发现 | P0-4 | `src/ui/keyboard/*`/`window.rs`/`settings.rs`/i18n | 无 |
| B5 搜索 | P0-5、P1-15 | `search_page.rs`/`search-page.blp`/`tools/assert-at-spi.py`/i18n | B2（复用空态工厂改造）；P1-15 还要先接上真实的加入相册/收藏回调 |
| B6 查看器信息层 | P1-7、P1-12、P2-2 | `viewer-page.blp`/`viewer_page.rs`/`navigation.rs`/`transform.rs`/`base.css` | 无 |
| B7 查看器输入 | P1-8 | `transform.rs`/`stage.rs`/`viewer.md` | B1（键盘焦点）、B6（控件尺寸与重排） |
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
| `docs/modules/ui-design.md:105-109` | 多选进入由网格右键/长按、Space 与 Ctrl+A 完成；仅修正选择动作的出现时机描述，不增加 header 常驻入口 | B3 |
| `docs/modules/ui-design.md:151-152` | 空态/loading 需为三态且带下一步动作 | B2 |
| `docs/modules/ui-design.md`（Photos 一节，已改） | 「There is no disclosure button」这条契约已被 P2-8 推翻并改写：概览有两个入口（header 圆钮 + 顶部再滚一格），失败态多一个走既有 pull 的重试；同时新增 `overview_toggle_btn` 的材质说明（逐字复用 `.round-search-button`，不是新玻璃） | B2 |
| `docs/modules/ui-design.md`（Favorite 契约，已改） | 保留「只有一枚心形按钮」，但补上「点之前要可预判」：混合态独享 overlay 角标 + `photos.batch.favorite.mixed` 的 tooltip 与 accessible name，并显式禁止拆分按钮/split arrow | B3 |
| `docs/modules/ui-design.md:175-184` | focus 态可见性落地；hover 与 selected 的强度差决策 | B1、B9 |
| `docs/modules/viewer.md:98-113, 204` | 重新表述控制器禁令（区分 touch-only 与桌面滚轮）；新增位置/倍率契约 | B6、B7 |
| `docs/modules/keyboard.md:60-69` | 声明应用内发现路径与「binding ↔ shortcuts」防漂移断言 | B4 |
| `docs/modules/editor.md` | 退出确认与前后对比契约 | B8 |
| `docs/modules/ui-liquid-glass.md` | 焦点环属 a11y 层且与透明度无关；新增「动效与 reduce-motion」一节 | B1、B9 |
| `docs/modules/browsing.md` | 多选入口、总览披露、空态三态、搜索三态与慢查询指示；B9 已补「Accessible Names For Tiles And Badges」（tile 类级 `Img` 角色、绑定者推名字、装饰图标 presentation、选中尚非 accessible state 的限制）；B2 已补「Overview Disclosure And Sync Retry」（状态从 revealer 派生、重试只属于失败、`photos.overview.hide` 死键接上）；B3 已补「The Batch Heart Predicts Its Own Behavior」（一个写入者画好红心/名字/角标三样，混合态独享 caret） | B2、B3、B5、B9 |
| `docs/modules/storage.md` | `DomainEvent::ScanPhase` 契约、缩略图失败语义 | B2、B8 |
| `docs/testing.md` | `--a11y-smoke` 的搜索字段期望值改为按 locale 从 `i18n/<locale>.json` 取，中文标签不再是常量 | B5 |
| `AGENTS.md`（B5 已加） | UI 不变量已落地：「`data/ui/*.blp` 不得出现硬编码可见文案，一律 `tr()`/`trf()`」，由 `tests/ui_template_copy.rs` 全仓扫描把关 | B5 |
| `AGENTS.md`（B9 已加） | UI 不变量已落地：「图标控件必须可命名——模板留空 `tooltip-text` 槽由 Rust 填，或推 `Property::Label`；纯装饰图标必须 `accessible-role: presentation`。角色写在构造点，名称只能从 Rust 推」 | B9 |

---

## 本次未执行的验证（诚实边界）

- **未运行应用**，也未生成截图或视觉基线。因此以下结论属「代码确认 + 待视觉验证」，落地前必须目测：P1-13 的对比度判断（来自 α 数值与表面叠合推理）、P1-12 的间距/密度拥挤感、P1-11 的两态区分度、P0-3 的 header 宽度是否溢出。
- 上条已在落盘阶段补做完毕：**P0-3**（真实渲染截图，照片页与相册详情页各 800×600 与 1280×800 两档，无换行/裁切，窗口控件仍在右端）、**P1-12**（一次性探针把 ViewerPage 放进 900×600 窗口、舞台铺满合成照片，在 liquid/plain × 深/浅四组下截右上簇并加截放大态；实测按钮分配 42×38、簇内节距 48px 均匀、放大时 `zoom_transform_sep` 随旋转组一起消失）、**P1-11**（`render.rs` 的真实像素采样把 hover 与 selected 的亮度差钉成阈值，比目测更强）、**P1-13**（不再是「待视觉验证」：`functional_text_holds_a_contrast_floor_over_glass` 直接量渲染出来的表面与合成后的前景比值，并用回退旧值反向确认它会红）。
- `ui::grid_css::tests::render` 里的真实像素采样断言自 P1-11 起已是常规聚焦测试的一部分（每次改动都会跑）；`PHOTOVIEWER_GLASS_SCREENSHOTS=...` 的导出分支仍未使用，因为它只向 `target/` 写文件、不参与断言。
- 未做 Flatpak 运行时验证，也未做超大图库压测。
- 未读取 AT-SPI 实际无障碍树（`tools/assert-at-spi.py --dump` 可在真实窗口上验证 P2-3/P2-4 的暴露情况），因此「读屏听到三个静态标签」的推断来自控件类型（`Gtk.Box` + 点击手势）而非实测。
  - 这条已在落盘阶段补做，但换了工具：`tools/assert-at-spi.py` 要 Flatpak 运行时，本机跑不了，所以用本机等价的 pyatspi 探针（`env -u NO_AT_BRIDGE HOME=<tmp> tools/with-at-spi.sh xvfb-run -a ./target/debug/photo-viewer` + python3-gi 走树，配方与三个坑记在 `docs/testing.md`）。**P2-3 的推断成立**（读到 `grouping '照片分组方式'` 下挂三个 `radio button`）；**P2-4 的推断也成立但处方会致病**：按草案给状态角标补名字，实测把格子念成「已选中 已收藏」且文件名全部消失，因为 tile 当时是容器角色、GTK 丢掉推给它的名义。两处更正都记在「P2-4 实施结果」。
- 检视中所有「零调用者」「零命中」结论都用 grep 交叉确认过（`empty_states::loading/scan_error`、`set_accessible_label`、`ShortcutsWindow`、`gtk::Settings`、`@media`、scan 进度布尔状态）。

### 交互评审原型能证明什么、不能证明什么

`docs/ui-naming-reference/index.html` 已升级为可交互评审原型（24 条提案逐条开关、深链、判定与 markdown 导出）。它证明的是**交互与信息架构层面**的取舍，不替代上面任何一条未执行的验证：

- 能看：状态机是否覆盖全（扫描/失败/空、搜索四态、多选进出、脏标记守卫）、控件落点与顺序（P1-12 的两种排列）、提示文案与让位关系（P2-1/P2-2）、i18n 键是否真的两种语言都有（切「语言=en」即刻暴露硬编码中文）。
- 不能看：GTK 真实渲染的材质/圆角/阴影、Flatpak 下的字体度量、以及 P1-13 的**实际**对比度。原型里的「对比度报告」用 WCAG 相对亮度公式量的是浏览器 DOM，只能作为筛选可疑组合的线索；结论仍以 `render.rs` 断言与真实截图为准。
- 静态页只有 18 张样图，所以 P1-6 的 2000 上限、P2-5 的主线程掉帧都只能以标注或演示开关呈现，不是真实复现。
- 标题栏是替身而非提案：窗口控件按桌面 `button-layout`（默认 `:minimize,maximize,close`）画在右端，对话框只有关闭按钮；这些都不受提案开关影响。查看器顶栏的分组照真实结构摆——日期与云状态角标是 header `[start]` 一组（`spacing: 8`），文件名是 `NavigationPage` 的 title 由 HeaderBar 居中——评审时不要把角标读成文件名的一部分。契约文字见 `docs/modules/ui-design.md` 的 Window Shell 与 Viewer Page 两节。
