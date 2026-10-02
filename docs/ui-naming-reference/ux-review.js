/* PhotoViewer UI 视觉参考 · 提案评审层
 *
 * 职责：提案登记表、评审工具条、评审抽屉、采纳/暂缓/否决与导出、
 *       环境开关（语言/材质/透明度/动效/窗宽）、演示触发器、对比度报告。
 * 与命名图的关系：本文件只创建 .ux-bar 与 .pv-inspector 两棵子树，
 *       不读写任何 data-ui / data-name / data-impl / data-props / data-source，
 *       也不改写 tooltip.js 的事件。删除本文件后页面退回纯命名图。
 * 交互能力全部走 window.PV（prototype.js）。
 *
 * 提案内容来源：docs/ux-improvement-backlog.md（现象/证据/方案/落点/测试/批次）。
 */
(function () {
  "use strict";

  var body = document.body;
  var PV = window.PV;
  if (!PV) return;

  var q = PV.q;
  var qa = PV.qa;

  /* ============================================================ 登记表 */

  // 每条提案的可见承载方式不必相同：多数是 index.html 里的 `.pv .pv-<id>` 标记 +
  // `body[data-pv~="<id>"]` 门控，但有 6 条按设计就没有专属标记，改由 JS/CSS 呈现，
  // 做一致性核对时不要把它们当成缺失：
  //   p1-13 对比度报告（ux-review.js 计算并弹检查器）
  //   p1-14 减少动画（styles.css 归零过渡时长，芯片本身即效果）
  //   p2-1  toast 撤销（已落盘为常态，芯片不再改动画面；落盘前形态走 data-pv-demo=toast-no-undo）
  //   p2-2  toast 让位胶片条（已落盘为常态，芯片不再改动画面；落盘前形态走 data-pv-demo=toast-on-strip）
  //   p2-3  模式选择器无障碍语义（已落盘为常态：prototype.js 恒定写 role/aria-checked，焦点环在胶囊上）
  //   p2-4  宫格/徽标/图标的 accessible name（已落盘为常态：prototype.js applyIconLabels 恒定运行，
  //         落盘前形态走 data-pv-demo=tiles-anonymous）
  // 另有 6 条已落盘的提案没有 .pv-<id> 标记，因为它们的常态就是命名图本身：
  //   p0-4 快捷键发现（SettingsPage shortcut_group + tooltip .p4-key 常态渲染）
  //   p0-5 搜索三态（search_state_stack 常态渲染，落盘前形态走 data-pv-demo=search-blank）
  //   p2-5 选择计数查询线程（顶栏 .header-note 常态可见，条目本身即实现说明）
  //   p2-6 相册选择器四态（picker-state--loading/empty/error 常态存在，由演示按钮选看哪页；
  //        落盘前对照走 data-pv-demo=picker-error-as-empty）
  //   p2-8 总览 disclosure 与同步重试（header .overview-disclosure 与行内 .overview-retry
  //        常态渲染；失败态与落盘前对照各走一个演示按钮）
  //   p2-9 心形混合态角标（.favorite-menu-hint 常态渲染，随 body[data-pv-fav] 显隐；
  //        落盘前对照走 data-pv-demo=heart-unpredictable）
  // 半落盘的条目只给未落盘那一半留标记：
  //   p1-6 的照片/相册计数是常态，.pv-p1-6 仅代表回收站计数。
  //   p1-7 的位置计数和边界反馈是常态，.pv-p1-7 仅代表倍率标签。
  var PROPOSALS = [
    {
      id: "p0-1",
      prio: "P0",
      batch: "B2",
      title: "扫描中不再误报「暂无照片」",
      screens: ["photos"],
      problem:
        "新装用户启动后看到「暂无照片」+「在设置中添加文件夹」，而后台扫描正在进行。用户据此判断「配置错了」，转去改设置。同时「扫描失败」与「库为空」两种完全不同的事实被压成同一句文案。",
      evidence: [
        "photos_page.rs:391 is_empty = n_items()==0 → :533-543 立即显示 no_photos()；:547-559 items_changed 同样只按 0 判定",
        "empty_states.rs:57-71 scan_error() 与 loading() 全项目零调用者（grep empty_states:: 仅命中 no_photos/empty_trash/no_album_photos）",
        "进度文案只存在于旧 MediaGrid：media_grid.rs:90、media_grid/loading.rs:450-511（1 秒轮询 core/repository.rs:225 library_stats()），Photos 已迁到 VirtualMediaGrid 拿不到该标签",
        "全 src/ 不存在 scan/scanning/is_scanning/busy 布尔状态"
      ],
      solution: [
        "新增 DomainEvent::ScanPhase { active: bool, error: Option<String> }，由 core/bootstrap.rs:104 scan_and_aggregate_with_actor() 的进入/退出/Err 分支发出（复用 DomainEventSender，events.rs:99）",
        "PhotosPage 空态判定改三态：active → loading_stats()；有 error → scan_error(err) + 重试；n_items==0 → no_photos() + 「打开设置」；否则网格。集中到一个 update_placeholder_child()",
        "进度计数复用现成轮询：ThumbnailLoader::set_stats_dirty_callback（core/thumbnails.rs:348）触发一次 library_stats() 重读，不新建第二个 1 秒定时器",
        "空态必须带动作：adw::StatusPage::set_child(Gtk.Button)，no_photos 复用 KeyboardAction::OpenSettings 的处理函数"
      ],
      files: [
        "src/core/events.rs",
        "src/core/bootstrap.rs",
        "src/ui/empty_states.rs",
        "src/ui/photos_page.rs",
        "i18n/zh-CN.json",
        "i18n/en.json"
      ],
      tests: [
        "cargo test ui::empty_states（scan_error 返回的 StatusPage 带子按钮且描述为传入 msg）",
        "cargo test ui::photos_page（注入 ScanPhase{active:true} 后 visible_child 为 loading）",
        "cargo test --test local_scan（扩一条「扫描中不出现 no_photos」）"
      ],
      docs: ["docs/modules/browsing.md", "docs/modules/ui-design.md:151-152", "docs/modules/storage.md"],
      risk:
        "新增 DomainEvent 变体会让所有 match 分支编译失败——app.rs:131-166 与 refresh_hub 订阅者需一并处理；这是好事，能强制覆盖完整。",
      demo:
        "工具条「扫描状态」选「扫描中 / 失败 / 空库」，照片页直接看三态——三态已经是真实行为，不需要开启本提案；「失败」态的重试按钮会回到扫描中再落回就绪。开启本提案改为显示落盘前的对照：三态退回同一条「暂无照片」（photos-empty-page-before）。",
      landed:
        "B2 已落盘。事件定为 DomainEvent::ScanPhase { active, error }，发射走 DbActorHandle::emit（DomainEventSender::send 用 blocking_send，不能在 GTK 主线程调用）；PhotosPage 新增 PLACEHOLDER_SCANNING / PLACEHOLDER_SCAN_ERROR 与 update_placeholder_child() 单一判定入口（photos_page.rs:965），MainWindow::note_scan_phase 负责存储与页面重建后的回放。偏差三处：扫描中只有 spinner + 固定文案，没有草案里的实时计数；empty.no_photos.description_with_count 未实现（count 为 0 时没有信息量）；重试走 glib::spawn_future_local + tokio::task::spawn_blocking，并用 scan_retry_in_flight 防重入。测试：ui::photos_page::tests::placeholders_separate_indexing_scan_failure_and_an_empty_library、scan_failure_text_names_the_reason_when_there_is_one。"
    },
    {
      id: "p0-2",
      prio: "P0",
      batch: "B1",
      title: "主网格键盘焦点可见",
      screens: ["photos", "album", "trash"],
      problem:
        "方向键能移动焦点，但屏幕上看不出焦点在哪个瓦片上；此时 Space（选中）与 Delete（移入回收站）作用在用户看不见的项目上。",
      evidence: [
        "data/css/a11y.css 只有 9 个 chrome 选择器的 :focus-visible 环，没有任何 gridview 规则",
        "base.css:155-168 主动清掉了主题默认的 gridview > child:selected 边框/背景",
        "FlowBox 有环（base.css:68-71）+ pointer-left/kbd-nav 抑制（:93-95，grid_css.rs:480-525）",
        "关键细节：虚拟网格可聚焦节点是瓦片本身（factory.rs:71 tile.set_can_focus(true)），不是 GridView 的 child —— 照抄 FlowBox 选择器不会命中",
        "与自有契约冲突：ui-design.md:175 要求 hover/selection/focus 三态都可见"
      ],
      solution: [
        "在 a11y.css 末尾追加 gridview.virtual-media-grid-view > child > .glass-thumb-card:focus-visible { outline: 2px solid @accent_color; outline-offset: -2px; }（a11y.css 拼接顺序在最后，grid_css.rs:88-93；base.css:253-266 只重置 border-color/box-shadow，不碰 outline）",
        "环叠在暗罩之上：base.css 补一条同优先级 inset 0 0 0 1px alpha(black,0.55) 内侧描边，保证 accent 在白底照片上仍可辨",
        "焦点环属材质无关层，不需要在 liquid/plain 两处镜像 —— 写进注释，避免后来者误当漏镜像搬进材质块",
        "透明度滑杆不影响焦点环（ui-liquid-glass.md:79 既有规定）",
        "顺带统一 FlowBox 环：base.css:69 的 alpha(@window_fg_color,0.55) 改成 @accent_color，两处一起改，避免同一应用两种焦点语言",
        "回归防线：grid_css/tests.rs:1130 新增断言装配后的 CSS 必须包含 glass-thumb-card:focus-visible"
      ],
      files: [
        "data/css/a11y.css",
        "data/css/base.css",
        "src/ui/grid_css/tests.rs",
        "src/ui/grid_css/tests/render.rs"
      ],
      tests: [
        "cargo test ui::grid_css",
        "tools/assert-at-spi.py 人工确认焦点节点确实落在瓦片上"
      ],
      docs: ["docs/modules/ui-liquid-glass.md", "docs/modules/ui-design.md", "docs/ui-naming-reference/index.html"],
      risk:
        "SquareTile 是自绘控件，需确认 GTK 的 :focus-visible 状态位能到达它；若到不了，退路是把 attach_kbd_nav（grid_css.rs:480-525）从「只接受 FlowBox」泛化为接受容器+焦点节点选择器。",
      demo:
        "焦点环已落盘：照片、相册与回收站瓦片的键盘焦点在默认态可见。用 Tab 和方向键走查，再切换窗宽与透明度确认环不消失；提案芯片不关闭已落地行为。",
      landed:
        "B1 已落盘，方案被实测证据改写。GTK 把 widget 的 inset box-shadow 画在内容之下，缩略图直接盖掉它，所以暗描边由 list-item wrapper 承接：gridview.virtual-media-grid-view > child:focus / :focus-within { outline: 1px solid alpha(black,0.85) }，瓦片节点是 outline: 3px solid @accent_bg_color; outline-offset: -3px。@accent_color 在本机析出为浅橙（它是 accent 底色上的「可读文字色」），饱和色 @accent_bg_color 才压得住照片；FlowBox 网格同步换成它，保持一套焦点语言。GtkGridView 把焦点留在内部 wrapper 上并拒绝瓦片成为 focus widget，所以渲染测试快照取自 wrapper。规则放在 data/css/a11y.css 末尾（材质无关层，两种模式共用，不进 liquid/plain 镜像块）。测试：ui::grid_css::tests::grid_tile_focus_ring_is_assembled 与 tests::render::virtual_grid_tile_focus_ring_renders（断言与具体 accent 色无关：环带与白底通道差 ≥ 60、hairline 近黑、内芯 8px 不变、周边重绘 ≥ 2*(w+h)）。"
    },
    {
      id: "p0-3",
      prio: "P0",
      batch: "B3",
      title: "常驻多选入口 + 长按回退",
      screens: ["photos", "album"],
      problem:
        "左键=打开查看器，进入多选的**唯一**入口是右键菜单里的「多选」；进入后界面不提示点击语义已翻转。触屏/触控板用户没有右键，等于无法批量操作。",
      evidence: [
        "virtual_media_grid.rs:1130-1141 非多选态右键菜单项 photos.batch.multi_select 是唯一入口（GlassMenuItemKind::Suggested → set_multi_select_mode(true)）",
        "virtual_media_grid.rs:494 橡皮筋框选显式关闭",
        "factory.rs:76-88 只有 GestureClick + set_button(3)，没有 GestureLongPress",
        "virtual_media_grid.rs:1909-1913 进入多选后 activate_slot 从「打开」翻转为「切换选中」",
        "可参考的既有模式：photos-page.blp:56-64 exit_multi_select_revealer，由 photos_page.rs:1068、:1094 按 any_multi 控制 reveal"
      ],
      solution: [
        "header [start] 组新增常驻 select_mode_btn（icon check-select-symbolic，退路 selection-mode-symbolic；css-classes glass-toolbar-button + round-search-button），点击调 set_multi_select_mode(true)（virtual_media_grid.rs:680 已是 pub fn）",
        "与 exit_multi_select_btn 形成对称进出；进入后自身收起（复用 any_multi，photos_page.rs:1069-1073）",
        "factory.rs 追加 GestureLongPress + set_touch_only(true)，connect_started 走与右键菜单相同的 show_context_menu(tile,&binding,x,y)，触屏等价可达且不增加可见控件",
        "进入多选时在 header 显示「已选择 N 项」（与 P1-6 合并）",
        "保留右键入口，只是不再让它成为唯一入口",
        "同步检查 album-detail-page.blp 是否需要同一入口，避免同样行为在两个页面位置不同"
      ],
      files: [
        "data/ui/photos-page.blp",
        "data/ui/album-detail-page.blp",
        "src/ui/photos_page.rs",
        "src/ui/virtual_media_grid/factory.rs",
        "src/ui/media_grid.rs",
        "i18n/*.json"
      ],
      tests: [
        "cargo test ui::photos_page（经网格路径进入多选后 exit revealer.reveal_child 为真；2026-10-02 起没有 select_mode_btn 可点）",
        "cargo test --test e2e_browsing"
      ],
      docs: [
        "docs/modules/browsing.md",
        "docs/modules/ui-design.md:105-109（原文把入口条件写成循环依赖，需改为「进入选择有常驻入口；选择动作按钮仅在有选择时出现」）",
        "docs/ui-naming-reference/index.html"
      ],
      risk:
        "header [start] 已挤了 search + 两个 revealer，窄窗口可能换行或裁切；必要时放 [end] 最左位（注意 [end] 是 edge-first 反向声明，photos-page.blp:11-15）。",
      demo:
        "照片页与相册详情页都已是完整的进出多选与批量动作 chrome，但没有常驻入口（2026-10-02 按库主决定移除）：在交互原型里右键瓦片 → 「多选」即进入，入口热区已从两页 header 摘除，进入后退出/全选/批量动作照常 reveal。把「窗口宽度」切到窄，检查两页 header 是否溢出。剩余未落盘的是长按回退覆盖旧 FlowBox 网格（搜索结果分区）——见 browsing.md 记录的独立缺口。",
      landed:
        "B3 已落盘（照片页 + 虚拟网格 + 相册详情页）。照片页：header [start] 新增 select_mode_revealer → select_mode_btn（photos-page.blp:49-58），图标实际用 selection-mode-symbolic——草案写的 check-select-symbolic 在本机 Adwaita 图标主题里不存在；tooltip 复用 photos.batch.multi_select（photos_page.rs:419-421），点击对三套分组网格一并 set_multi_select_mode(true) 后把焦点交进当前可见瓦片（:747-757），reveal 由 refresh_selection_ui() 的 !any_multi 驱动（:1286-1288），与 exit_multi_select_btn 形成对称进出。factory.rs:97-111 追加 GestureLongPress(button=1, touch_only=true) 走与右键同一个 show_context_menu，touch_only 保证慢点击不会与 click-to-open 打架；右键项保留。相册页那一半也已落盘：album-detail-page.blp:24-89 复刻同名同位的五个 revealer/button（含 select_all、exit、add_to_album、delete_to_trash），wire_selection_chrome()（album_detail_page.rs:266）用同一批 photos.batch.* 文案，refresh_selection_ui()（:409）由 grid.connect_selection_changed 驱动；全选走 select_all_in_album()（:365）+ ALBUM_SELECT_ALL_LIMIT=2000 从 MediaRepository 按相册查询取 id，而不是网格已加载窗口——探针截图发现旧写法 select_all() 在未 realize 的 ready set 上会静默清空选择。Ctrl+A / Delete / Escape 在相册页与照片页语义一致。刻意保留：相册页不放假的收藏按钮（该页 on_set_favorite 目前是 no-op）。仍在 backlog 的缺口：旧 FlowBox 网格（搜索结果分区）以 enable_context_menu: false 构造，既无右键也无长按，也没有任何多选入口。测试：ui::photos_page::tests、ui::virtual_media_grid::tests、--test ui_photos_toolbar（6 个 glass-toolbar-button）、ui::album_detail_page::tests（12 个，含 60 张相册只加载 10 张时的全选）。**2026-10-02 更新**：常驻 header 入口（select_mode_revealer → select_mode_btn，两页模板与 Imp 字段、点击 handler、tooltip、refresh_selection_ui 的 !any_multi 门控）按库主决定整体移除；进入多选改回右键/长按菜单项、键盘 Space / Ctrl+A，退出按钮与批量 chrome 不变。测试改为：ui::photos_page::tests::multi_select_entered_without_a_header_entry_hands_over_to_exit_and_back、--test ui_photos_toolbar（5 个 glass-toolbar-button）、ui::album_detail_page::tests。"
    },
    {
      id: "p0-4",
      prio: "P0",
      batch: "B4",
      title: "快捷键应用内可发现",
      screens: ["photos", "viewer", "settings"],
      problem:
        "查看器绑了 13 个键，但应用里没有任何地方能看到它们；tooltip 只有动作名不含按键。用户只能读 docs/modules/keyboard.md 才知道有 F/I/E/H。",
      evidence: [
        "键位表 binding.rs:76-189：全局 Ctrl+F / Ctrl+, / Alt+←；browsing 方向键、Return、Space、Ctrl+A、Delete；viewer ←/→、Esc、Space、+/=/-/0、R/Shift+R、F、I、E、H、Delete",
        "全项目无 GtkShortcutsWindow、无 Ctrl+/（grep ShortcutsWindow 零命中）",
        "i18n/en.json:37-42 tooltip 形如 \"Zoom In\"，不含键名",
        "键位只存在于 docs/modules/keyboard.md:60-69"
      ],
      solution: [
        "新增 src/ui/keyboard/shortcuts_window.rs：gtk::ShortcutsWindow 按三 section（全局/浏览/查看器与编辑）与 binding.rs 的三函数一一对应；每个 ShortcutsShortcut 设 accelerator + title(tr())",
        "触发：F1 与 Ctrl+/（KeyboardAction 加 ShowShortcuts，action.rs:1-28），并在设置对话框加一行「键盘快捷键」；set_transient_for + set_modal",
        "tooltip 附带键名：新增 i18n 键 *.tooltip.key 或集中一个 tr_with_key(label,key) helper（i18n.rs），zh-CN 用全角括号",
        "keyboard.md 升级为「同一张表的文档镜像」，写明新增/修改 binding 必须同步 shortcuts_window.rs",
        "最有价值一步：单测断言 ShortcutsWindow 声明的 accelerator 集合 == binding.rs 三张表实际解析出的 action 集合，加键忘加文档直接红"
      ],
      files: [
        "src/ui/keyboard/shortcuts_window.rs（新增）",
        "src/ui/keyboard/action.rs",
        "src/ui/keyboard/binding.rs",
        "src/ui/keyboard/mod.rs",
        "src/ui/window.rs",
        "src/core/i18n.rs",
        "src/ui/window/settings.rs",
        "i18n/*.json"
      ],
      tests: [
        "cargo test ui::keyboard（含集合相等断言）",
        "cargo test --test e2e_viewer（F1 打开快捷键窗口且不吞掉 viewer 的 F 键）"
      ],
      docs: ["docs/modules/keyboard.md", "docs/ui-naming-reference/index.html"],
      risk:
        "F1 当前未被占用（binding.rs 确认）；Ctrl+/ 在部分布局需要 Shift，考虑同时接受 Ctrl+question。",
      demo:
        "本项已落盘，所以窗口和设置页那一行是常态 chrome：按 F1 或 Ctrl+/（Ctrl+? 同样有效）随时打开，查看器按钮 tooltip 常驻键名后缀「放大 (+)」。想对比落盘前，用工具条「演示态 → 落盘前：无快捷键入口」——它会隐藏设置行、摘掉 tooltip 后缀，并让 F1 只弹一条「应用内没有任何快捷键发现入口」。提案芯片本身不改变画面，因为已落盘的入口不该被开关削弱。",
      landed:
        "B4 已落盘。src/ui/keyboard/shortcuts_window.rs 的 GROUPS 是三 section 唯一的真源：28 行（全局 6 / 浏览与选择 8 / 图片查看 14），KeyboardScope 逐组绑定；tests.rs 双向断言——每条声明的 accelerator 经 gtk::accelerator_parse 后真的解析到它声称的动作，且路由器在该 scope 可达的每个动作都有行（Browsing/Viewer 允许落到 Global 表兜底）。五处偏差：(1) 改用 Builder XML 骨架而非 ShortcutsWindow::add_section/add_group/add_shortcut，那套 API 需要 gtk4 的 v4_14，本项目锁 v4_8，升上去会新增约 52 条弃用告警并被 CI 的 clippy -D warnings 拒掉；翻译串只作属性写入，不进 XML，含 & 或 < 的文案不会破坏解析。(2) 草案的 tr_with_key() 落地为 tooltip_with_key()，键名从同一张 GROUPS 派生而不是新增 *.tooltip.key 键，所以 tooltip 与窗口结构上不可能不一致；没有键位的动作（Restore）自动退回纯标签。(3) 向左旋转草案写 «R»，实测漂移测试直接红：gtk_accelerator_parse(\"<Shift>R\") 报成小写 r + Shift 位，binding.rs 的匹配臂因此同时接受 Key::R 与 Key::r，声明改为 \"<Shift>R\"。(4) GtkShortcutsShortcut 一行一个 accelerator，所以 F1 与 Ctrl+/ 是两行而不是草案里的一行。(5) 草案测试项 cargo test --test e2e_viewer 未做，覆盖放在 ui::keyboard::shortcuts_window::tests（含打开即断言 modal/transient/view-name/行高）。另外表内的 Ctrl+滚轮 行不属于本项：查看器至今没有任何 scroll 控制器（P1-8），已改挂 .pv-p1-8 标记，只在开启 P1-8 时出现，避免冒充已落盘。设置页新增「键盘」分组（settings.rs:523-545），点击先关设置弹窗再开窗，保证同一时刻只有一个模态窗。命名图侧：设置行、窗口、tooltip 提示三个热点已转为常态 data-ui 条目（shortcut-reference-group / shortcut-reference-window / shortcut-group-* / tooltip-key-hint）。"
    },
    {
      id: "p0-5",
      prio: "P0",
      batch: "B5",
      title: "搜索三态 + 模板去硬编码中文",
      screens: ["search"],
      problem:
        "搜索无命中时页面只剩搜索框和三个分段标签，下方完全空白；英文界面用户看到的三个标签是中文「全部/文件名/日期」。",
      evidence: [
        "search_page.rs:469-470 image/video_results_box 仅 set_visible(has_images/has_videos)，皆 false 时无任何提示；:137-138 初始化同样只隐藏",
        "search-page.blp:36/42/47 label: \"全部\"/\"文件名\"/\"日期\" 写死在模板；i18n/en.json 无对应键。同页其余文案都走 tr()（:107/111/123/130/368/564/568）",
        "约束警告：tools/assert-at-spi.py:19 硬编码 SEARCH_FIELD_NAMES=(\"全部\",\"文件名\",\"日期\")，读屏冒烟测试依赖中文标签，i18n 化后必须同步改",
        ":237-245 依赖 GtkSearchEntry 内建 search-changed 延迟，无自研 debounce；:388-447 generation counter 防竞态（做得好，保留）"
      ],
      solution: [
        "补三态：查询前（轻量提示态）、零结果（empty_states::no_search_results(query) + 「清除搜索」子按钮）、查询中（>300ms 在结果区顶部起 adw::Spinner，不整页替换，避免网格闪断）",
        "落点：search_page.rs:460-500 可见性判定改 match (query_empty, has_images||has_videos)，content_box 增加一个 gtk::Stack",
        "三个分段标签 i18n 化：blp 改 label:\"\"，在 :145-152 取得三个 TemplateChild 后统一 set_label(tr(...))；新增 4 键两份 json 同步；set_group 行为（:148-149）与 ui-liquid-glass.md:108-110 的原生 toggle 分组语义不受影响",
        "tools/assert-at-spi.py 把 SEARCH_FIELD_NAMES 换成从 i18n/<locale>.json 读当前 locale 的三段标签（或 --labels 参数），而不是放宽断言",
        "建议把「data/ui/*.blp 不得出现硬编码可见文案」升级为 AGENTS.md 的 UI 不变量"
      ],
      files: [
        "src/ui/search_page.rs",
        "data/ui/search-page.blp",
        "src/ui/empty_states.rs",
        "i18n/*.json",
        "tools/assert-at-spi.py",
        "tests/visual_check_script.rs",
        "AGENTS.md"
      ],
      tests: [
        "cargo test ui::search_page（零结果 → 可见 StatusPage；查询前 → 提示态）",
        "a11y 冒烟脚本相关测试 + 人工双 locale 目测三个标签"
      ],
      docs: ["docs/modules/browsing.md", "docs/testing.md", "docs/ui-naming-reference/index.html", "AGENTS.md"],
      risk:
        "改 assert-at-spi.py 会降低其对真实标签漂移的敏感度；用「从 i18n json 取期望值」而非放宽断言来避免。",
      demo:
        "本项已落盘，所以三态与进行中指示是常态：进入搜索页清空输入即「查询前」态，输入不存在的关键词（如 zzz）落到「没有找到「zzz」」+「清除搜索」，命中则回到结果分区；把语言切到 EN，三个分段标签随之变成 All / File name / Date。提案芯片不再改动画面（已落盘的行为不该被开关削弱）；要看两种对照用工具条：「慢查询：进行中指示（P0-5）」把假延迟抬到 650ms 让转圈出现，「落盘前：搜索页只有搜索框（P0-5）」复现查询前与零结果全空、英文界面仍是中文标签。清除演示态即回到落盘状态。",
      landed:
        "B5 已落盘。三态收进一个 Gtk.Stack search_state_stack（页名 idle/results/no-results，set_search_state 依当前输入与命中数选页），两个占位页由 empty_states::search_idle() 与 no_search_results(on_clear) 提供，零结果标题每次显示前用 trf(\"empty.search_none.title\", {query}) 刷新并回显去空白后的关键词，「清除搜索」清空输入 + grab_focus + 重跑空查询；remove_media_ids_from_results 末尾补一次状态刷新，所以删掉最后一张命中图会落到零结果页。四处与草案不同：(1) 进行中指示用 gtk::Spinner 而不是 adw::Spinner，且只有转圈图标没有「正在搜索…」文案，避免为一条瞬时提示再造第 9 个键；(2) 栈只负责整页互斥，image/video 两个分区各自的 set_visible 保留，否则只命中一类结果时会露出空分区标题；(3) 关掉栈的 hhomogeneous/vhomogeneous——结果网格与占位页尺寸差很多，隐藏页会决定栈高；(4) 草案设想的「移除计时器 SourceId」在本机是真崩溃：end_search_busy() 对已触发的 source 调 SourceId::remove() 直接 GLib-CRITICAL + panic，而 drop SourceId 又不取消源，所以改为 busy_generation 代次失效（新测试 a_settled_query_never_paints_the_busy_row 锁住这条）。落盘时另外修掉一处公共缺陷：AdwStatusPage 会填满自己的 child，所以 empty_states::add_action 建的 pill 按钮被拉成整页宽，加 halign=CENTER 后 B2 那两张照片页占位同时受益。i18n 侧新增 8 个键两份 json 同步（parity 427/427），草案里的文案在落地时收敛为「搜索你的图库 / 没有找到「{query}」 / 换一个关键词，或将搜索字段切回「全部」」。模板去硬编码按建议升级为不变量：blp 里三个 label 留空、标签在 SearchPage::new 用 tr() 填入，tests/ui_template_copy.rs 把「data/ui/*.blp 不得出现可见文案字面量」做成全仓扫描（对 label/title/text/placeholder-text/tooltip-text/subtitle/description 七种属性生效，已用注入字面量的方式验证不是空跑），并把这条写进 AGENTS.md 的 UI Invariants。读屏侧 tools/assert-at-spi.py 不再写死中文：SEARCH_FIELD_KEYS 三个键按 locale 从 i18n/<locale>.json 取期望值，locale 解析顺序与应用一致（config i18n.json → PHOTO_VIEWER_LOCALE → LC_ALL/LANG/LANGUAGE → en），新增 --locale 覆盖；tests/visual_check_script.rs 把脚本里保留的 zh 兜底常量钉在 i18n/zh-CN.json 上，标签漂移仍会红。测试：ui::search_page::tests 7 项（标签跟随 locale、空查询→idle、零结果回显关键词并可清除、命中→结果分区、删最后一张→零结果、慢查询才起指示、已结束的查询永不显示指示）、--test ui_search_page 2 项、--test ui_template_copy 2 项、--test visual_check_script 7 项、--test ux_click_flows 1 项（真实点击穿过新栈）、core::i18n 2 项。命名图侧：三个 .pv-p0-5 提案热点已转为常态 data-ui 条目（search-busy-row / search-state-stack / search-idle-page / search-no-results-page / search-results-box），落盘前对照改挂 body[data-pv-demo=\"search-blank\"]。"
    },
    {
      id: "p1-6",
      prio: "P1",
      batch: "B3",
      title: "重建保留选择 + 显示已选数量",
      screens: ["photos", "album", "trash"],
      problem:
        "用户选了几十张准备加入相册，文件系统 watcher 落地一次扫描 → 选择全部丢失且无提示；多选期间永远看不到「选了多少」。",
      evidence: [
        "media_grid/loading.rs:562（schedule_rebuild 的 750ms 定时器回调）与 :591（rebuild_immediately）都在 rebuild() 前无条件 clear_selection()，两处都没先读 selected_ids()",
        "可恢复：selected 是 RefCell<HashSet<MediaId>>，clear_selection()（virtual_media_grid.rs:710-715）前可读；select_ids(&[MediaId])（:692-708）能整体重建并 sync_visible_selection",
        "上限截断不可见：photos_page.rs:1337 Select All 走 items(LiveAll,0,2000)，:1350 已算出 selected_count 但从不渲染；:1360 selected_reaches_select_all_limit 只有内部逻辑",
        "trash-page.blp 的 action bar（trash_page.rs:91/226）只有按钮文本，无计数"
      ],
      solution: [
        "两处改「捕获 → rebuild → 重放」：let kept=this.selected_ids(); let was_multi=this.is_multi_select_mode(); rebuild(); if kept.is_empty() { if was_multi { set_multi_select_mode(true) } } else { select_ids(&kept) }（注意 select_ids(&[]) 会把 is_multi_select_mode 置 false，:693）",
        "refresh_selection_ui()（photos_page.rs:1062，已算出 union）新增 header 标签 selection_count_label，文案 trf(\"photos.selection.count\")；命中 2000 上限追加「（已达上限）」；放 [start] revealer 组，遵循同一 reveal 时机",
        "不做「调用方每处手写重放」：把按 id 重放写成 VirtualMediaGrid::preserve_selection_across_rebuild 的内部职责，避免第三个调用点将来再犯",
        "Trash 页同样显示计数"
      ],
      files: [
        "src/ui/media_grid/loading.rs",
        "src/ui/virtual_media_grid.rs",
        "src/ui/photos_page.rs",
        "data/ui/photos-page.blp",
        "i18n/*.json"
      ],
      tests: [
        "新增网格单测：选择后触发 schedule_rebuild，选择集合保持不变",
        "cargo test ui::photos_page（计数标签在 n=0/1/2001 三档文案正确）"
      ],
      docs: ["docs/modules/browsing.md", "docs/modules/ui-design.md:105-109"],
      risk: "重放依赖 MediaId 稳定；移出库的 id 自然消失，需确认 select_ids 对不存在 id 静默跳过。",
      demo:
        "照片页与相册页计数已落地：选几项后点「触发一次 rebuild」，无论 P1-6 芯片开关如何，选择与多选模式都保留。芯片只控制尚未落地的回收站计数；本条演示的三步循环依次显示普通计数、全选计数与模拟 2000 上限文案。",
      landed:
        "计数那一半已落盘（照片页 + 相册详情页），「重建清空选择」那一半经实测不成立。落点：photos-page.blp:122-139 / album-detail-page.blp:91-106 的 selection_count_revealer→selection_count_label，photos_page.rs:1317-1331 / album_detail_page.rs:439-447 写文案与 reveal，i18n 两表新增 photos.selection.count / photos.selection.limit（parity 429/429），base.css:472-480 的 .selection-count 保持扁平 header 文本。两处偏差：(1) 草案说放 [start]，实施先试 header title-widget，被 libadwaita 的真实行为否决——放在 Adw.NavigationPage 里的 Adw.HeaderBar 会在 title 槽显示页面标题（「照片」/相册名），占用它等于无选择时删掉页面身份；(2) 最终落 [end] 最左，读作「已选择 N 项 ＋ ♡ ⌫」，并用 photos_page/tests.rs:754 在 800x600 实测计数与批量图标不互相挤出 24px。证据更正：loading.rs:562/591 的 clear_selection 属于旧 FlowBox 网格，而它在生产里只剩搜索结果分区（enable_context_menu: false，无多选 UI），所以没有可丢失的用户选择；VirtualMediaGrid 的共享投影刷新路径不清 selected，已由 virtual_media_grid/tests.rs:773 锁住（追加第 7 项后断言 layout 真的长大且 id 集合与多选模式都保留）。剩余尾巴：回收站 action bar 计数（trash-page.blp / trash_page.rs:91,226）与 2000 上限档的真实复现仍未落地。"
    },
    {
      id: "p1-7",
      prio: "P1",
      batch: "B6",
      title: "查看器补齐定位信息",
      screens: ["viewer"],
      problem:
        "不知道「我在第几/共几张」；按到底时界面毫无反应（看起来像卡住）；放大后不知道当前倍率。",
      evidence: [
        "header 只有文件名标题（viewer_page.rs:824）与日精度日期（:825 → viewer/details.rs:276，标签 viewer-page.blp:26）；全项目无位置计数器",
        "viewer/navigation.rs:162 Ok(None) => {} 空分支；Err 才 fire_nav(delta)（实施后改为 set_nav_direction_available）",
        "viewer_page.rs:66-68 MIN 1.0/MAX 8.0/STEP 1.25，transform.rs:47-88 内部 Cell<f64>，无标签（第三项倍率可见未落盘）",
        "重要陷阱：viewer_page.rs:643 list_n_items() 返回**窗口化 store** 长度（viewer/navigation.rs:289 ensure_media_item_in_window），不是全库总数"
      ],
      solution: [
        "header 日期右侧加 position_label「{current} / {total}」：total 走 MediaRepository::position(query, id) → db::media_position 两 COUNT（一为 total、一为 1-based rank），off-thread（gio::spawn_blocking）异步取，current_token + position_request_token 双重 token 守住；nav_counter 不在导航 critical path——保持 db::seek_media_neighbor 跳过 COUNT 的现有约定",
        "到底反馈：viewer/navigation.rs:162 空分支改为两端 set_sensitive(false) prev/next，并让键盘 ←/→ 在无目标时不再吞事件；禁用态视觉 .viewer-overlay-nav-btn:disabled { opacity: 0.32 }（该选择器原本把 color 钉成 #ffffff，libadwaita 的 insensitive 颜色无法生效，opacity 是唯一可传达状态的通道，且两种玻璃模式一致）",
        "倍率指示：zoom_scale != 1.0 时在 viewer_zoom_controls 右侧显示 trf(\"viewer.zoom.level\")，reset_viewer_transform() 后隐藏；用 tabular 数字避免宽度跳动，% 留在文案侧以便本地化——**本项未落盘，留给 P1-7b**"
      ],
      files: [
        "data/ui/viewer-page.blp",
        "src/ui/viewer_page.rs",
        "src/ui/viewer/navigation.rs",
        "src/core/repository.rs",
        "src/core/db.rs",
        "i18n/*.json"
      ],
      tests: [
        "本批（位置计数器 + 到底反馈）**未新增专属单测**。用户的停止指令在中途下达，留待后续工作补上：DB 单测覆盖 MediaRepository::position 三档排名与 1-based 边界；GTK 单测覆盖 viewer 标签文案 + prefetch 后 prev/next 失活 + 键盘在已解析端返回 Ignored",
        "已跑的回归：cargo check --lib（PASS）、cargo test --lib（611 passed / 0 failed / 2 ignored，含 ui::viewer_page::navigation::tests::* 4 条全部 PASS）、tools/with-at-spi.sh xvfb-run -a cargo test --test ui_viewer_toolbar --test ui_template_copy --test ui_grid_css_install --test ui_viewer_source_structure --test e2e_viewer（7 条全部 PASS）",
        "手动视觉验证 NOT RUN（同上理由）：.viewer-overlay-nav-btn:disabled opacity 在两种玻璃模式下的可读性、[start] 子项扩展、.dim-label 与标题字重的对比仍需开窗口亲眼盖章"
      ],
      docs: ["docs/modules/viewer.md（Header Toolbar 段新增「rank 来源是 db::media_position 而非 list_n_items」陷阱，Navigation Buttons 段新增边界反馈契约）", "docs/modules/keyboard.md（ViewerPrevious/ViewerNext 在已解析端返回 Ignored 而非 Handled）", "docs/modules/storage.md（MediaRepository::position 不可进入导航 critical path）", "docs/modules/ui-design.md（位置计数器 + 边界 disabled 视觉两条 bullets）", "docs/ui-naming-reference/index.html"],
      risk: "viewer 入参只有窗口化 ListStore 与 MediaQuery/MediaId，没有任何 MediaPage 入参（photos_page.rs:1960 / album_detail_page.rs:720 / search_page.rs:793 三个调用点都传 media_list）。所以方案里「打开查看器那次查询的 MediaPage.total」是一次不存在的入口；落盘改用每帧异步 db::media_position 拿 (rank, total)，off-thread 不污染导航 critical path。",
      landed:
        "位置计数器 + 到底反馈两项已落盘（2026-10-01）。落点：viewer-page.blp 在 [start] 末尾新增 Gtk.Label position_label（viewer-date-label 之后），base.css 把 .viewer-date-label 与 .viewer-position-label 合并为 font-variant-numeric: tabular-nums（位置计数器另带 libadwaita .dim-label）；viewer_page.rs imp 新增 position_label TemplateChild + position_request_token + prev_exhausted/next_exhausted；viewer/navigation.rs 新增 update_position_label（spwan_blocking + 双重 token 守卫）、set_nav_direction_available / nav_direction_available / reset_nav_bounds、handle_nav_key（编辑态仍 Handled）；navigate_by_delta 的 Ok(None) 改为 set_nav_direction_available(delta, false)；prefetch_neighbors 的 Ok(None) 同。db 侧：db::media_sort_expr + db::rank_and_total（0-based，与 media_neighbor_with_filter_and_order 共享，避免 rank 与 ←/→ 走序不一致）+ pub fn media_position()(1-based rank, total)；MediaRepository::position 新方法 + 抽出 nav_projection 让 neighbor_item / position 走同一 (filter, params, trashed)。i18n 两族新增 viewer.position.count = \"{current} / {total}\"（parity 434/434）。键盘 ←/→ 在已解析端返回 KeyboardResult::Ignored。",
      demo:
        "位置计数与两端箭头禁用已落地，默认态打开查看器即可看到「N / M」并在首尾看到禁用态。P1-7 芯片只演示未落地的缩放倍率：点放大后显示百分比；真实应用的倍率标签和视频首尾箭头预判仍待处理。"
    },
    {
      id: "p1-8",
      prio: "P1",
      batch: "B7",
      title: "Ctrl+滚轮缩放 + 放大后拖拽平移",
      screens: ["viewer"],
      problem:
        "桌面图片查看器的肌肉记忆是 Ctrl+滚轮缩放 + 拖拽平移；这里只能点 +/- 每次 ×1.25，放大后无法移动画面。",
      evidence: [
        "stage.rs:730 只有 video.add_controller(click)，image stage 上没有任何控制器；ui/viewer/crop.rs:70 的 drag 只挂裁剪 overlay",
        "载体是 Gtk.Picture picture（viewer-page.blp:99，在 image_overlay 内，can-shrink:true，content-fit:contain）",
        "zoom_scale/zoom_pan_x/zoom_pan_y 是 imp 的 Cell（viewer_page.rs:204/209/210）；transform.rs:73 set_viewer_zoom 私有（测试钩子 :91）；transform.rs:136 clamp_zoom_pan 因 pan 恒为 0 而实际不可达",
        "反向约束：transform/tests.rs:13-37 是负向断言，遍历 image_overlay.observe_controllers() 要求不存在 GestureZoom/GestureDrag；viewer.md:204 同样写明不要装 touch-only 捏合/平移/全局滑动控制器"
      ],
      solution: [
        "先收窄契约：既有禁令针对 touch-only 控制器与按钮竞争，桌面 Ctrl+滚轮和 scale>1 后启用的主按钮拖拽不属于该禁令。",
        "EventControllerScroll（VERTICAL，Capture 相位）挂在 image_overlay，以拿到完整舞台；仅在 Ctrl、图片可见、非编辑态且存在纵向 delta 时消费，普通滚轮 Proceed。gtk4 0.8 没有 SMOOTH flag，所以用「累计 90 度才走一步」的累加器把高频触控板事件收敛成离散缩放步。",
        "GestureDrag 限定主按钮，保存手势起点 pan，drag update 使用 GTK 的累计 offset，而不是反复累加单帧 delta；fit 态不启用平移。",
        "clamp_zoom_pan 先按 Paintable 固有尺寸与 Picture 分配算出 contain 可见矩形，再按 viewer rotation 交换宽高，边界只覆盖真正溢出视口的部分。",
        "继续不安装 GestureZoom；触摸捏合如要加入，需要独立手势仲裁设计。"
      ],
      landed:
        "P1-8 已落盘（2026-10-02）：主图片舞台提供 Ctrl+滚轮缩放；仅在倍率大于 1 时以左键拖拽平移。拖拽保存起点 pan 并叠加 GTK 累计偏移，边界使用旋转后的 contain 图像尺寸计算，letterbox 空白不可平移。普通滚轮仍交给页面滚动，未实现 touch 捏合控制器。草案里的 16ms CSS 节流未实施（缺少真实帧采样），舞台输入提示条也未实施——发现性留给后续沉浸浏览项。",
      files: [
        "src/ui/viewer/stage_input.rs",
        "src/ui/viewer/stage_input/tests.rs",
        "src/ui/viewer/transform.rs",
        "src/ui/viewer_page.rs",
        "docs/modules/viewer.md"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::viewer_page::",
        "cargo test --locked --test inline_test_ownership --test ui_viewer_source_structure"
      ],
      docs: ["docs/modules/viewer.md", "docs/modules/keyboard.md", "docs/ux-improvement-backlog.md"],
      risk:
        "桌面滚轮和条件拖拽已替代原 touch-only 禁令；后续若增加触摸捏合，必须另做手势仲裁测试。",
      demo:
        "真实应用里查看器舞台已支持 Ctrl+滚轮缩放与放大后的左键拖拽。原型开关只控制演示层的模拟输入、拖拽光标和提示条：关闭时舞台回到只有 +/- 按钮，提示条不出现。",
    },
    {
      id: "p1-9",
      prio: "P1",
      batch: "B8",
      title: "编辑器退出确认 + 前后对比",
      screens: ["viewer"],
      problem:
        "调完曝光后点关闭/Cancel/按 Esc，图片直接回到原图，没有任何询问。唯一的「脏」信号是一个图标按钮变亮。",
      evidence: [
        "editor_panel.rs:423-439（关闭/Cancel）→ viewer/editor.rs:71-105 stop_editing → :88-90 无条件恢复原纹理；viewer_page.rs:547 的 Esc 同样直达",
        "脏态唯一出口是 reset 按钮敏感度：editor_panel.rs:723-726 has_pending_edits()",
        "无前后对比：editor_panel.rs / editor-panel.blp 没有 hold-to-compare/split 实现",
        "可复用确认模式：viewer/actions.rs:21-30 的 adw::AlertDialog + glass-alert-dialog + 默认/关闭响应为 cancel",
        "保存反馈已具备，无需改：editor_panel.rs:675-685 set_saving 已置不可敏感，:908/:952 有 spinner"
      ],
      solution: [
        "集中退出路径：header 关闭、footer Cancel、Esc 三处改经一个 attempt_close(&self)；!has_pending_edits() 直接关，否则弹 AlertDialog（heading=editor.unsaved.heading、body=editor.unsaved.body），add_response(\"discard\")→Destructive、add_response(\"keep\")→default，仅 discard 时真退出",
        "关键：复用 actions.rs:21-30 的构造顺序并 set_close_response(\"keep\")，否则确认对话框会被 Esc 绕过",
        "可见脏标记：footer Cancel 旁或面板标题下加 editor_dirty_label（「有未保存的修改」），与 reset 敏感度由同一个 has_pending_edits() 驱动，避免漏更新",
        "前后对比：footer 加 compare_btn（Gtk.ToggleButton），切换时把面板参数临时置为单位变换并复用现成 33ms 单飞预览（editor_panel.rs:864-890），不新增渲染路径"
      ],
      landed:
        "P1-9 已落盘（2026-10-02）。四条用户退出路径（header 关闭、footer 取消、Esc、navigation.pop）统一进 EditorPanel::request_close：干净状态直接关，脏状态弹 adw::AlertDialog，default 与 close 响应都是 keep，只有 discard 才 fire_close。同一个 update_pending_edits() 驱动 reset 敏感度、标题下的「有未保存的修改」和对比按钮可见性。compare_btn 是 Gtk.ToggleButton，打开时 render_preview 用 EditState::default() 走既有的 33ms 单飞预览，不新增渲染路径也不改编辑状态。偏差：答案一落地就释放闸门（不等 AdwDialog 关闭动画，测试环境里该动画不完成），connect_closed 再兜一次；保存进行中与对话框已开时拒绝重复请求。落盘前的对照（无脏标记、无对比、取消即静默丢弃）只保留在本说明里，芯片开关已不再改变画面。",
      files: [
        "src/ui/editor_panel.rs",
        "src/ui/viewer/editor.rs",
        "src/ui/viewer_page.rs（Esc 路由）",
        "src/ui/viewer/navigation.rs（navigation.pop 路由）",
        "data/ui/editor-panel.blp",
        "i18n/*.json"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --test ui_editor_panel（干净直接关 / 脏不静默丢 / keep 保留 / discard 退出 / 不叠框 / compare 渲染原图且不动状态）",
        "cargo test --locked --test edit_state --test destructive_rotate --test e2e_editor --test ui_template_copy --test ux_click_flows"
      ],
      docs: ["docs/modules/editor.md", "docs/modules/ui-design.md（Editor Panel And Crop Overlay）"],
      risk: "退出路径容易漏改（尤其 Esc 与对话框自身共用），request_close 必须保持唯一出口；stop_editing 只留给已确认的 discard 与保存成功。",
      demo:
        "进查看器 → 编辑 → 拖任意滑块，面板标题下出现「有未保存的修改」，footer 上方出现「对比」；点关闭/取消/Esc 会弹确认框，Esc 与默认都是「继续编辑」，只有「放弃修改」才真的退出。「对比」按下即还原为原图。本项已落盘，芯片开关不再改变画面。"
    },
    {
      id: "p1-10",
      prio: "P1",
      batch: "B8",
      title: "图片解码失败可见化",
      screens: ["photos", "viewer"],
      problem:
        "原图解码失败（损坏/已移动/权限）时舞台留白、没有文字；缩略图失败在主网格里表现为完全空白的格子，用户以为是渲染 bug。",
      evidence: [
        "viewer/stage.rs 原始图解码 Err 分支只 warn + 收起 spinner；预览也失败时无内容可画，舞台留空",
        "错误 UI 只给视频：viewer-page.blp 的 video_error_box（样式 base.css），图片无对应物",
        "网格侧经实测修正：base.css 的 opacity:0 只作用于没有 .thumb-placeholder 的普通 .thumb-loading；解码失败由 core/thumbnails/decode.rs 转成 generate_unavailable_placeholder() 并作为正常纹理交付，所以失败格子不是空白，而是自带灰框＋红斜杠的纹理",
        "ThumbnailLoader 只暴露 set_stats_dirty_callback，无按 media_id 的失败回调"
      ],
      solution: [
        "把 video_error_box 泛化为 media_error_box（同一 [overlay] 兄弟节点，.viewer-video-error → .viewer-media-error class 家族）：图标按媒体种类切换 image-missing / video-x-generic + 文件名 + 一句原因 + 「重试」/「在文件管理器中显示」",
        "文案与 P0-1 的 scan_error 用同一措辞风格：说明原因 + 如何修复",
        "先用真实像素验证缩略图失败占位是否已足够可辨，只有确实与加载态无法区分时才加 .thumb-broken——不要预设「格子为空」，也不要先给 core 加失败回调",
        "验证结果：占位纹理的图标覆盖、明度分离与红斜杠色度都远超阈值（新增像素测量测试长期守住），故网格侧不加层，本项只落查看器错误面"
      ],
      landed:
        "P1-10 已落盘（2026-10-02）。查看器统一到一个 .viewer-media-error 面（media_error_box），视频流错误与「原图解码失败且舞台无可画内容」共用它；后者是有条件的——预览缩略图还在画着时不加盖错误面，否则文案与用户眼前的图像自相矛盾。show_media_error 按媒体种类选图标与措辞，图片正文点名文件，动作是「重试」（对当前 index 重跑 show_at）与「在文件管理器中显示」（gtk::show_uri_full 打开所在目录，无处理器时 toast）。样式仍在 base.css 单处，它是平面主题洗色而非玻璃材质，所以草案里「两处材质镜像」的要求不适用——这一点写进了 viewer.md。网格侧经像素测量后否决了 .thumb-broken：失败占位纹理本身可辨，SquareTile 与 loader 均未改动。",
      files: [
        "data/ui/viewer-page.blp",
        "src/ui/viewer/stage.rs",
        "src/ui/viewer_page.rs",
        "data/css/base.css",
        "src/core/thumbnails/tests.rs",
        "i18n/*.json"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::viewer_page::stage（缺失文件 → 错误面可见、标题/正文/重试按钮/停 spinner）",
        "cargo test --locked --lib thumbnails（占位纹理可辨性的像素测量）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::grid_css"
      ],
      docs: ["docs/modules/viewer.md", "docs/modules/storage.md", "docs/ui-naming-reference/index.html"],
      risk:
        "「解码失败」与「尚未加载」在 loader 回调里仍不可区分；本项靠占位纹理自身可辨来规避，若将来要在网格里显示可点的重试入口，就必须先给 SquareTile 记一次失败标志。",
      demo:
        "照片网格里失败的那格显示带斜杠的占位纹理（常驻，不再受开关控制），点进查看器舞台即为错误面：图标＋文件名＋原因＋「重试 / 在文件管理器中显示」。查看器错误面同样常驻，草案阶段的「空白格子」假设已被实测否定。"
    },
    {
      id: "p1-11",
      prio: "P1",
      batch: "B9",
      title: "hover 与 selected 视觉分离",
      screens: ["photos", "album", "trash"],
      problem: "鼠标扫过的格子和已勾选的格子用的是同一层暗罩，差别只有右下角对勾的不透明度。",
      evidence: [
        "base.css:231-247 注释即「Pointer emphasis and multi-selection are one interaction language」，把 .media-selected / .thumb-pointer-hover / flowbox hover / gridview hover 归入同一 background+shadow 组",
        ":253-266 进一步把两态的 border-color:transparent; box-shadow:none，抹掉了本可区分二者的环",
        "对勾靠 flowboxchild:selected .thumb-checkmark{opacity:1} 区分（ui-design.md:178-184，实现 base.css:111-140/187）"
      ],
      solution: [
        "让选中态可跨屏识别：对勾常驻——未选中 opacity:0.32，选中 opacity:1；改动最小且不动材质（备选：选中态单独 accent 内环 inset 0 0 0 2px @accent_color）",
        "hover 只保留极轻一层：暗罩强度降到约一半，选中态保持现有强度。这会修改 ui-design.md:176-177 的明文契约，需同步说明「同一材质语言、不同强度」",
        "多选态要有全局提示（与 P1-6 的「已选择 N 项」合并）——比逐格区分更有效地回答「我在不在多选模式里」",
        "用 grid_css/tests/render.rs:112 thumbnail_emphasis_covers_white_image_edges 的像素采样手法新增断言：hover 与 selected 的采样亮度差需大于阈值，防止后来再次把两态压平"
      ],
      landed:
        "P1-11 已落盘（2026-10-02）。真正渲染状态层的是 .thumb-state-glass 覆盖子节点（图片不透明，卡片 background 透不过去——第一次把强度差写进 .glass-thumb-card 时像素采样两态都是 162.7，才定位到这里）：hover 的 veil 从 linear 0.30→0.42 降到 0.14→0.20，选中保持原值，并且选中组排在 hover 组之后、显式列出 .media-selected.thumb-pointer-hover，避免同不透明度下被 hover 抢走。对勾节点常驻，多选模式下未选中格子 opacity 0.32、选中 1；多选模式以 grid 级 class 表达——FlowBox 侧挂在每个分区（apply_selection_mode 顺手切换，它本来就拥有 selection-mode），GridView 侧挂在 GtkGridView（新增 set_multi_select_flag，成为 is_multi_select_mode 的唯一写入者，覆盖 set_multi_select_mode / select_ids / clear_selection / 键盘 Space / 右键进入五条路径）。常驻规则用 :not(:selected) 与 :not(.media-selected) 作用域排除选中格，不靠源码顺序压特异性。草案里的 accent 内环没有实施：实测强度差加常驻对勾已足够区分，多一条环会和 P0-2 的键盘焦点环竞争注意力。",
      files: [
        "data/css/base.css",
        "src/ui/grid_css/tests/render.rs",
        "src/ui/grid_css/tests.rs",
        "src/ui/media_grid.rs",
        "src/ui/media_grid/selection.rs",
        "src/ui/virtual_media_grid.rs",
        "docs/modules/ui-design.md"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::grid_css（含 hover/selected 采样差断言与常驻对勾的 CSS 契约）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::media_grid ui::virtual_media_grid（class 跟随多选模式的两条路径）"
      ],
      docs: ["docs/modules/ui-design.md（Media Grids And Tiles）"],
      risk: "属材质契约改动，必须两主题各目测一次；render.rs 的断言已经在两种材质下分别采样，压平两态会直接失败。",
      demo:
        "常态即已落盘行为：切到「交互原型」，鼠标停在瓦片上是极轻一层，点选后 veil 明显加深且对勾全不透明，多选模式下其它格子带 0.32 的常驻对勾。要看落盘前的同构状态用工具条「落盘前：悬停与选中同构（P1-11）」；提案芯片不再改动画面。"
    },
    {
      id: "p1-12",
      prio: "P1",
      batch: "B6",
      title: "查看器控件簇分组、命中区、间距",
      screens: ["viewer"],
      problem:
        "6 个图标挤在右上角，放大与缩小被旋转/全屏拆在两端；静止态全裸（只有白色符号+阴影），首次进入不知道那里可点；按钮 36×32、间距 4。",
      evidence: [
        "viewer-page.blp:227-271 声明顺序 zoom_reset_btn / zoom_out / rotate_left / rotate_right / fullscreen / zoom_in，spacing:4；viewer_nav_buttons（:202-222）同样 spacing:4",
        "base.css:1114-1125 .viewer-overlay-nav-btn{min-width:36px;min-height:32px;padding:0;color:#ffffff}，注释明确「静止态裸、hover 才上玻璃」并解释白前景+暗光晕动机（这条理由成立，保留）",
        "base.css:394-398 .glass-toolbar-button 是 min-height/width:34px; padding:0 14px"
      ],
      solution: [
        "按语义重排：zoom_out | zoom_in | 分隔 | rotate_left | rotate_right | 分隔 | reset | fullscreen。viewer_zoom_controls 是普通 Gtk.Box，直接改 blp:227-271 声明顺序即可，分组用 Gtk.Separator 或增大组间 margin（不要新增 class）。注意 photos-page.blp:11-15 那条 [end] edge-first 反向声明的坑在普通 Box 不存在，PR 描述里写清以免 review 混淆",
        "命中区与间距：.viewer-overlay-nav-btn 提到 min-width:40px; min-height:36px，组内 spacing:6、组间 margin-start:8。保持静止态裸（材质规则不改），必要时用透明 padding 扩 hit area 而非扩视觉尺寸",
        "可发现性：给整簇一个极淡的静止容器轮廓（复用 .glass-segmented 的轻底，不新增材质语义），或首次进入查看器播放一次 ≤600ms reveal 提示（须尊重 P1-14 的 reduce-motion 开关）",
        "与 P1-7 倍率标签同处布局，新增 label 要计入该簇宽度，避免窄窗口溢出"
      ],
      landed:
        "P1-12 已落盘（2026-10-02），落地内容是方案里的前两条加第三条的容器轮廓分支。viewer-page.blp 的 viewer_zoom_controls 现按 zoom_out → zoom_in ｜ zoom_transform_sep → rotate_left → rotate_right ｜ zoom_state_sep → zoom_reset → fullscreen 声明；该容器是普通 Gtk.Box，声明顺序就是视觉顺序，不像 photos-page.blp 的 [end] HeaderBar 会反向排布，所以 blp 注释里写明了这一点。分隔线不是装饰：update_zoom_buttons 让 zoom_transform_sep 跟随「会整组消失的旋转组」（放大后 rotate 隐藏，分隔线也跟着隐藏，不会悬在簇尾），zoom_state_sep 常驻。命中区提到 40×36、组内 spacing 从 4 到 6；.viewer-zoom-controls 有一层 alpha(black,0.16) + 1px alpha(black,0.30) 的常日内描边，刻意不做成玻璃（因此没有 liquid/plain 双镜像要求，按钮各自仍是 .glass-toolbar-button，hover 才上材质）。草案第三条的「首次进入播放 reveal 提示」没有实施——它依赖尚未落盘的 P1-14 reduce-motion 开关，且与 P1-8 的舞台输入提示是同一类一次性引导，留到那条一起做。草案第四条（倍率标签计入簇宽）在本机是空谈：P1-7 的 zoom_level_label 从未实施，控件簇目前不含动态宽度文本。",
      files: [
        "data/ui/viewer-page.blp",
        "data/css/base.css",
        "src/ui/viewer/transform.rs",
        "src/ui/viewer/transform/tests.rs",
        "src/ui/grid_css/tests.rs",
        "tests/ui_viewer_toolbar.rs",
        "docs/modules/viewer.md",
        "docs/ui-naming-reference/index.html"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::viewer_page::transform（声明顺序＝视觉顺序的四组相邻断言 + 两条分隔线可见性随缩放态）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::grid_css（40×36 命中区与 .viewer-zoom-controls 日内描边在两种材质下都在）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --test ui_viewer_toolbar --test e2e_viewer --test ux_click_flows"
      ],
      docs: ["docs/modules/viewer.md", "docs/ui-naming-reference/index.html"],
      risk: "重排会改变 e2e 里按位置断言的用例，需一起改。",
      demo:
        "常态即已落盘行为：切到「交互原型」并打开查看器，右上簇是 缩小|放大 · 左转|右转 · 还原|全屏，组间有分隔线、命中区更大、整簇带一层日内描边。要看落盘前的乱序形态用工具条「落盘前：控件簇乱序（P1-12）」；提案芯片不再改动画面。"
    },
    {
      id: "p1-13",
      prio: "P1",
      batch: "B9",
      title: "玻璃下的功能性文本对比度地板",
      screens: ["photos", "viewer", "settings", "search"],
      problem:
        "多处功能性文字用了 0.45–0.68 的前景不透明度，叠在半透明玻璃甚至照片内容上；透明度滑杆拉到 100 时更糟。",
      evidence: [
        "实测数值：菜单禁用态 alpha(0.45) base.css:513-515；搜索「更多」瓦片 0.52 叠 0.06 底 :1011-1013；视频错误块副标题/图标 0.58/0.54、标题 0.82 :758-772；关于文本 0.56 liquid.css:233-237 / plain.css:189-193；侧栏计数/分组标题 0.72/0.78 base.css:650-674；库统计与同步行 0.68 :966-983",
        "阅读面 @glass_reading_bg：Liquid 0.72–0.78 / Plain 0.82–0.88（grid_css.rs:104-107 reading_surface_alpha()）；Liquid .glass-base alpha(@window_bg_color,0.42)（liquid.css:4）",
        "ui-liquid-glass.md:75-78 已为阅读面与选择器设了 α 地板（0.72–0.88 / 0.66–0.78），本项只是把同一思路延到前景文本，不冲突"
      ],
      solution: [
        "给「功能性文本」设 α 地板，与装饰性/禁用态区分：错误、状态、计数、进度类文本 α ≥ 0.78（视频错误副标题 0.58→0.80、图标 0.54→0.72；库统计与同步行 0.68→0.78；关于文本 0.56→0.72）",
        "真正的禁用态保持 0.45（符合 0.38–0.5 通用区间），但必须同时降敏感度，避免「看起来像低对比的可用文字」",
        "把对比度断言搬进已有像素测试：grid_css/tests/render.rs:7 已有 luminance(&gdk::RGBA)，:172 已在两主题×两材质×透明度 0/50/100 解析实际颜色；新增对上述 selector 计算 WCAG 比值并断言 ≥4.5:1（功能性文本）/ ≥3:1（大号与图标）。本项最有价值的产出——把对比度从检视意见变成不可回退的契约",
        "白前景+阴影的照片叠层（base.css:934-959、:1206-1212）不在本次范围：暗光晕已提供局部底衬且注释说明了动机（P1-12 保留）",
        "mode_selector.rs:322-331 的 on-light-background 对比采样是已验证的好设计，继续作为照片叠层文本的参照实现"
      ],
      landed:
        "P1-13 已落盘（2026-10-02）。功能性文本的 α 地板：媒体错误面 0.76→0.80、副标题 0.58→0.80、标题 0.82→0.90、图标 0.54→0.72（大图标走 3:1 档），搜索「更多」瓦片 0.52→0.78，库统计与总览同步行 0.68→0.78，查看器同步徽记 0.72→0.78，侧栏计数 opacity 0.72→0.78，关于文本 opacity 0.56→0.72（liquid 与 plain 两份都改）。菜单禁用态保留 0.45：GTK4 的 :disabled 只在 insensitive 时匹配，草案要求的「同时降敏感度」在这条选择器里是结构性成立的，CSS 注释写明了这一点，不会被误读成「能点但颜色淡」。模式选择器的 0.72 没有动，它由 ui-liquid-glass.md 的选择器地板（0.66–0.78）单独管辖。\n\n契约落在 render.rs 新增的 functional_text_holds_a_contrast_floor_over_glass：把每个站点真实的表面（.viewer-media-error 面、.search-more-tile 面、@glass_reading_bg 阅读面）在两种材质 × 两种主题 × 透明度 0/100 下渲染出来，从整窗快照按控件分配取中位色当背景，再把解析出的前景按 source-over 合成上去量 WCAG 比值（文本 4.5:1、大号与图标 3:1）。取整窗而不是控件自身快照是必须的——WidgetPaintable 只画控件本身，父层透进来的照片不在里面，直接量会拿到假的高对比。玻璃背后取「与文字对抗」的一侧（深色主题 0.85 白、浅色主题 0.15 黑）而不是纯黑白：纯白底配透明度拉满时要求 α≈0.87，等于取消半透明设计，这条边界写进了测试注释。回退任一站点都会红（实测 0.68→4.41:1、0.52→3.29:1、副标题 0.58→3.38:1）。opacity 类站点（关于文本、侧栏计数）改在样式表上断言，因为 GTK 的 opacity 是渲染期合成的，style context 读不到它。",
      files: ["data/css/base.css", "data/css/liquid.css", "data/css/plain.css", "src/ui/grid_css/tests/render.rs", "src/ui/grid_css/tests.rs", "docs/modules/ui-liquid-glass.md"],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::grid_css（新增像素级 WCAG 契约 + opacity 地板，53 项全绿；已用回退旧值验证三组站点都会失败）"
      ],
      docs: ["docs/modules/ui-liquid-glass.md"],
      risk:
        "对比不足的判定本身属「待视觉验证」：α 数值是实测的，比值结论需要在 Flatpak GNOME 运行时目测。",
      demo:
        "常态即已落盘行为：功能性文本已在地板上，工具条「对比度报告」量到的比值随之全部通过；要看落盘前的淡字形态用「落盘前：玻璃下文字过淡（P1-13）」。提案芯片不再改动画面。"
    },
    {
      id: "p1-14",
      prio: "P1",
      batch: "B9",
      title: "接入系统「减少动画」",
      screens: ["photos", "viewer", "settings"],
      problem: "GNOME 设置里打开「减少动画」后本应用照常播放全部过渡。",
      evidence: [
        "base.css 有 17 条 transition 声明：120ms（:115 对勾、:272 徽记/侧栏、:322、:905 日期标签、:416 hover 材质、:1016 菜单项）、140ms（:534 菜单入场）、180ms（:678/:739 spinner/箭头）、200ms（:727/:886 淡入）、220ms（:1045 胶片条）、300ms（:363 选择器滑轨）、350ms（:300/:344 色彩交叉淡入）、:378、:1060；另有 liquid.css 2 条、plain.css 1 条",
        "全项目无 @media，且 grid_css.rs:66 明确禁止「web-style @media feature queries here」",
        "全 src/ 无任何 gtk::Settings 读取（grep 零命中），没人读 gtk-enable-animations"
      ],
      solution: [
        "走 GTK 原生通路，不引入 @media：在 grid_css.rs 的 build_css / build_css_with_transparency（:71/:75，拼接顺序 :88-93）追加可选尾块，if !gtk::Settings::default().property_gtk_enable_animations() { css.push_str(REDUCE_MOTION_CSS) }",
        "REDUCE_MOTION_CSS 用全局 * { transition-duration:0ms } 太宽（会波及尺寸动画），退路是显式重写那 7 个时长值——GTK CSS 支持 @define 但不支持自定义 CSS 变量；落地时先验证 GTK 是否接受全局 transition-duration: 0ms",
        "响应式更新：连接 GtkSettings 的 notify::gtk-enable-animations，在 install()（:318）时挂上并触发 reapply()（:336），避免要求重启",
        "代码侧动画同样要收：viewer/filmstrip.rs:837-945（adjustment 动画）、mode_selector.rs:222-263（滑轨 indicator）、photos-page.blp:108-109 scroll_date_revealer crossfade 200ms 与 :184-185 GtkStack crossfade 200ms、header 各 Revealer 的 slide_left/slide_right（:44-104）。统一由一个 motion_enabled() 查询函数供这些点读取（Revealer/Stack 用 set_transition_type(NONE)）",
        "测试：css_for_tests()（:255）注入开关，断言开启后 CSS 不含 120/140/180/200/220/300/350ms 等时长"
      ],
      landed:
        "P1-14 已落盘（2026-10-02）。新增 src/ui/motion.rs 作为全项目唯一读 gtk-enable-animations 的地方（Settings::default() 是 Option，没有后端时按 GTK 自己的默认值 true 处理）。CSS 通道按草案的尾块方案做：build_css_with_motion 在 base/material/a11y 之后追加 `* { transition-duration: 0ms; }`，install() 里连 notify::gtk-enable-animations 重建 provider，所以两个方向都是活的。草案担心的「全局 * 太宽、可能不被 GTK 接受」经实测不成立：GTK 的 CSS 子集接受该声明（motion/tests.rs 里用 connect_parsing_error 钉住），且只改 duration、不改 transition-property 列表，因此没有波及尺寸动画的副作用，逐选择器重写那 7 个时长的退路没有用上。代码通道：Revealer/Stack 由 motion::apply_to 递归走子树把 transition_type 设成 NONE（Stack 另设 duration 0），在页面构造时与开关转「关」时各跑一次；转「开」时不恢复已在屏幕上的页面，因为那需要记住每个控件模板里写的原值——新构造的页面会重新按开关取值，这一非对称写进了 motion.rs 与 ui-liquid-glass.md。胶片条的 frame-clock 滚动改由纯函数 thumb_scroll_should_animate(distance, motion_enabled) 决定，动画直接跳到目标。模式滑轨不需要改：它自 backdrop 重构后就是 CSS 的 transform 300ms 过渡，已被尾块覆盖（草案把它算作代码侧动画，这点与现状不符）。没有新增应用内开关，控件就是桌面「减少动画」辅助功能设置。",
      files: [
        "src/ui/motion.rs",
        "src/ui/motion/tests.rs",
        "src/ui/grid_css.rs",
        "src/ui/grid_css/tests.rs",
        "src/ui/viewer/filmstrip.rs",
        "src/ui/window.rs",
        "docs/modules/ui-liquid-glass.md",
        "docs/testing.md"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::motion（走子树剥离 / 开关为开时不动 / GTK 接受尾块）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::grid_css（尾块只追加一条、排在最后、不改写原表）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::viewer_page::filmstrip（thumb_scroll_should_animate 三个方向）"
      ],
      docs: ["docs/modules/ui-liquid-glass.md（「Motion and reduce-motion」一节）", "docs/testing.md"],
      risk: "GTK CSS 对全局 transition-duration 的支持需实测；不支持时退化为逐选择器重写。",
      demo:
        "常态即已落盘行为：工具条「减少动画」就是系统开关的等价物，切下去所有过渡与动画时长归零，胶片条滚动、模式滑轨、Revealer 全部改为直接切换；再切回来恢复。提案芯片不再改动画面——点亮它什么也不发生才是正确行为。"
    },
    {
      id: "p1-15",
      prio: "P1",
      batch: "B5",
      title: "搜索结果分区的照片选不中",
      screens: ["search"],
      problem:
        "搜索页的分区预览瓦片既不能右键、也没有长按回退、更没有进入多选的方式。P0-3 补齐照片页与相册详情页之后，这里是全应用唯一「选中不了」的媒体列表。",
      evidence: [
        "生产代码里旧 MediaGrid（FlowBox）只剩一处实例：search_page.rs:399 build_result_section()",
        "它走 MediaGrid::new_for_album()（media_grid.rs:580-589），enable_context_menu 传 false（:605 的参数），整条右键路径关闭",
        "search_page.rs:406-409 的 on_add_to_album 与 on_set_favorite 是空闭包，on_query_favorite_state 恒返回 FavoriteMenuState::default()",
        "长按回退只在 virtual_media_grid/factory.rs:97-111；P0-3 期间曾给旧网格补 GestureLongPress，因 enable_context_menu: false 成为不可达代码，已回退",
        "「更多」进去的完整结果页是 VirtualMediaGrid::new_for_query()（search_page.rs:733），入口与长按都在，不受本条影响"
      ],
      solution: [
        "build_result_section() 改用已存在的 MediaGrid::new_for_album_with_context_menu()（media_grid.rs:591-598），无需新 API",
        "顺序前提：先接上真实回调再开菜单——on_add_to_album 交给现有批量加入相册流程，on_set_favorite 走 MediaRepository 收藏写入，on_query_favorite_state 查真实状态；否则是开一扇通向 no-op 的门",
        "菜单需要页面级 overlay 宿主：GlassContextMenu 经页面 overlay 渲染，search-page.blp 目前没这个容器，参照 photos-page.blp 的 grid_overlay 补",
        "若只做分区不做进入多选，则在分区 header（search_page.rs:368-380 的 Gtk.Box）放一枚与 P0-3 同款入口按钮，复用 photos.batch.multi_select 文案，避免「能右键但选不了多个」",
        "收缩方案（需产品决策，不能靠沉默实现）：明确分区只用于预览，把「显示更多」当作唯一可批量操作的路径，并写进文档与命名图"
      ],
      files: [
        "src/ui/search_page.rs",
        "data/ui/search-page.blp",
        "src/ui/media_grid.rs",
        "i18n/*.json（若新增文案）"
      ],
      landed:
        "P1-15 已落盘（2026-10-02），按草案 1→3 的顺序做：先把回调接实，再开菜单，最后补宿主。search_page.rs 的 build_result_section() 改用已存在的 MediaGrid::new_for_album_with_context_menu()（没有新网格 API），on_add_to_album 交给共享的 album_picker::AlbumPickerDialog::present（与照片页同一流程，对话框自己负责写入与缩略图失效），on_set_favorite 走 DbCommand::SetFavorite 并在成功后把标志写回 image_list/video_list 两个预览 ListStore 与 detail_grids，on_query_favorite_state 真查 MediaRepository::favorite_state，所以菜单给的是「收藏」还是「取消收藏」取决于选中行的实际状态。菜单宿主是 search-page.blp 新增的 Gtk.Overlay search_overlay（包住原 search_state_stack），GlassContextMenu 运行时 add_overlay 挂进去——没有宿主的网格是静默丢掉菜单的，所以这一层是必需而不是装饰。\n\n偏差与边界：草案 4 的分区 header 入口按钮没有做，命名图里那枚提案热点已删除——P0-3 刚按库主决定撤掉照片页与相册页的常驻入口、把右键定为主要路径，分区再放一枚就是当场把那条决定推翻。长按回退也没有补到旧 FlowBox 网格：草案自己记过这笔（P0-3 期间试过，因 enable_context_menu: false 成为不可达代码而回退），现在菜单虽然可达，旧网格仍然只有右键一条路，这一点写进了命名图的 glass-context-menu 条目。写入后清选中（apply_favorite_flags 之后 clear_selection），否则菜单关掉、瓦片还亮着，读起来像动作没完成。tools/assert-at-spi.py 的搜索分区可达性检查没有加：它需要 Flatpak 运行时才能跑，本机无法验证，留一条没跑过的探针比不加更糟。",
      files: [
        "src/ui/search_page.rs",
        "src/ui/search_page/tests.rs",
        "data/ui/search-page.blp",
        "src/ui/media_grid.rs",
        "docs/modules/browsing.md",
        "docs/modules/ui-design.md"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::search_page（新增两条：分区网格的菜单宿主就是页面 overlay 且进入多选后 selected_ids() 非空；收藏状态来自数据库而不是 default，并且写回会落到预览 ListStore）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --test ui_search_page --test ui_media_grid_source_structure"
      ],
      docs: [
        "docs/modules/browsing.md（「legacy FlowBox grid has neither door」已改成落盘契约）",
        "docs/modules/ui-design.md（搜索分区行为）",
        "docs/ui-naming-reference/index.html（search-results-box 与 image-results 的现状描述）"
      ],
      risk:
        "在分区里开多选会让「点击=打开查看器」的语义在一个页面内分裂成两种（分区 vs 完整结果页）；若两套网格行为不一致，用户学到的是错误规律。落点顺序必须是回调→菜单→入口，不能反过来。",
      demo:
        "常态即已落盘行为：切到搜索屏搜出结果后，图片/视频分区的瓦片可以进入多选并勾选（原型用 data-act=enter-multi 代表右键菜单里的同一项）。提案芯片不再改动画面；草案里那枚 header 常驻「选择」入口没有落盘，相关热点已从命名图移除。"
    },
    {
      id: "p2-1",
      prio: "P2",
      batch: "B8",
      title: "Toast 支持撤销",
      screens: ["photos", "viewer"],
      problem: "Toast 一律无撤销按钮，删除只能去回收站找回。",
      evidence: [
        "src/ui/toasts.rs:19-40 三个工厂都用 adw::Toast::new(msg)，全项目无 set_button_label",
        "回滚逻辑已存在：viewer/actions.rs:81-93"
      ],
      solution: [
        "新增 success_with_action(overlay, msg, label, f)，删除/批量收藏接上已有回滚"
      ],
      landed:
        "P2-1 已落盘（2026-10-02）。`toasts::success_with_action(overlay, msg, label, f)` 6 秒超时——普通 success 的两倍，用户够不着的撤销不算撤销。收藏把 viewer/actions.rs 里的写入抽成 `apply_favorite_state(item_id, next_state, announce)`，toast 按钮以 `announce = false` 重入：可回滚的回滚是 toast 链，不是撤销。i18n 新增 favorited / unfavorited / undo / restore_failed 四键，两份 json 同步（parity 446/446），`moved_to_trash` 的「可在回收站找回」在按钮出现后收回成「已移入回收站」。\n\n草案的前提需要更正：actions.rs:81-93 那条 `RollbackTrashed` 只清 DB 标记，文件仍躺在回收站里，撤销直接接上去会留下指向空文件的瓦片。真正的还原在别处且早已存在——`core/trash.rs:830 restore_from_trash` / `repository.rs:469 restore_batch`（先 prepare_restore 把文件移回来，再经 DbActor 写 RestoreTrashed，最后 commit），回收站页一直在用。所以撤销走 `restore_batch(&ids, Some(&db_actor))`：经过 actor 才发得出 DomainEvent，其它页面才跟得上；直连 pool 只修好当前视图。顺序是文件先回来、列表后动（`media_list::insert_media_item_sorted`，从回收站页抽成共用 helper，按 sort_datetime 落回时间线原位而不是尾巴上），成功后 `show_at` 回到这张照片。\n\n偏差：草案点名的「批量收藏」没有撤销 toast——全应用只有查看器装了 AdwToastOverlay，照片页与搜索页没有任何 toast 宿主，而收藏本身是同一个菜单项可逆的开关；补一个窗口级 overlay 属于结构性改动，没有静悄悄塞进这条 P2。",
      files: [
        "src/ui/toasts.rs",
        "src/ui/toasts/tests.rs",
        "src/ui/viewer/actions.rs",
        "src/ui/media_list.rs",
        "src/ui/trash_page.rs",
        "i18n/*.json",
        "tests/ux_click_flows.rs"
      ],
      tests: [
        "cargo test --lib ui::toasts（带 action 的 toast 有可点按钮且回调执行）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --test ux_click_flows（journey_viewer_delete_toast_offers_undo：真实文件进回收站 → 点 toast 的撤销 → 文件回到原目录、DB 标记清掉、瓦片回到网格、查看器重新显示这张照片；把 restore_deleted_item 改成 no-op 后该断言确实变红）"
      ],
      docs: [
        "docs/modules/viewer.md（新增「Feedback Toasts And Undo」一节）",
        "docs/modules/albums-trash.md（还原 helper 的位置）",
        "docs/ui-naming-reference/index.html（收藏/删除按钮与 toast 宿主的现状）"
      ],
      risk: "撤销需要事务化（回收站路径 + DB 状态同时回滚）。",
      demo:
        "常态即已落盘行为：在查看器点收藏或移到回收站，toast 右侧就是「撤销」，点了直接回滚（删除那条会真的把文件移回来）。提案芯片不再改动画面；落盘前的纯文本 toast 走「落盘前：toast 没有撤销（P2-1）」演示按钮（data-pv-demo=toast-no-undo）。"
    },
    {
      id: "p2-2",
      prio: "P2",
      batch: "B6",
      title: "Toast 不遮挡胶片条",
      screens: ["viewer"],
      problem: "Toast 出现在查看器底部，正好盖住胶片条。",
      evidence: ["viewer-page.blp:7 的 ToastOverlay 包裹整块内容"],
      solution: ["限定 overlay 区域到 stage，或为 toast 预留底部 inset"],
      landed:
        "P2-2 已落盘（2026-10-02），取草案第一半：`Adw.ToastOverlay toast_overlay` 从页根挪进 content_box，只包 `image_overlay`（舞台），`viewer_bottom_stack` 变成它的兄弟节点，libadwaita 贴宿主下边缘画提示，于是下边缘＝胶片条上边缘。实测（tests.rs::a_toast_lands_above_the_filmstrip）：改结构前卡片在 410..456 而条带是 366..468，整条压住；改后卡片 288..334，舞台底 358、条带顶 366，让开 32px。两次测量的舞台（54..358）与条带（366..468）完全一致，插进一层容器没有改动布局。\n\n草案第二半「为 toast 预留底部 inset」实测不可用：内部子节点 AdwToastWidget 没有 Rust 绑定，`toastoverlay .toast` 选不中，而 `toast` 节点上的 `margin-bottom` 只是把节点撑高（底边仍钉在宿主下边缘），卡片并不上移。所以 base.css 里那条 margin 规则已删除，位置只由结构决定，不留一条「看起来生效」的 CSS。测量口径也要记一笔：`allocation()` 含主题外边距（82 对卡片 46），且 `Adw.ToastOverlay` 子节点的 `translate_coordinates` 与实际绘制位置差 32px，所以断言只用 `compute_bounds`。\n\n偏差：草案担心的「缩小 overlay 影响其他页面」不成立（别的页面没有宿主），但代价是详情/编辑侧栏触发的 toast 现在画在舞台上而不是窗口底部——提示说的就是这张照片，可以接受。舞台右下角的上一页/下一页与 toast 在很窄的窗口里可能相叠（卡片居中、实测宽 150，900px 窗口下与箭头相距约 165px），这一点没有真机目测。",
      files: [
        "data/ui/viewer-page.blp",
        "src/ui/viewer_page/tests.rs",
        "data/css/base.css"
      ],
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --lib ui::viewer_page（新增 a_toast_lands_above_the_filmstrip；负向验证：把 viewer-page.blp 还原成页根包裹后该测试变红）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --test e2e_viewer --test ui_viewer_toolbar --test ux_click_flows"
      ],
      docs: [
        "docs/modules/viewer.md（Layout Contract 之后新增 toast 契约）",
        "docs/ui-naming-reference/index.html（viewer-page 结构描述 + 新增 viewer-toast-overlay 条目）"
      ],
      risk: "缩小 overlay 区域会影响其他页面的 toast 位置，需按页配置。",
      demo:
        "常态即已落盘行为：在查看器触发 toast（收藏、删除、编辑器保存），它停在舞台底部、胶片条之上。提案芯片不再改动画面；落盘前压在胶片条上的位置走「落盘前：toast 盖住胶片条（P2-2）」演示按钮（data-pv-demo=toast-on-strip）。"
    },
    {
      id: "p2-3",
      prio: "P2",
      batch: "B9",
      title: "模式选择器的无障碍语义",
      screens: ["photos"],
      problem: "年/月/日选择器对读屏是三个静态标签。",
      evidence: ["mode-selector.blp:18-45 用 Gtk.Box + 点击手势，无 role/label"],
      solution: [
        "保留单胶囊视觉（ui-liquid-glass.md:92-105 是硬契约），用 set_accessible_role(Button)+set_accessible_label 补语义；或改 Gtk.ToggleButton + .glass-segment（search-page.blp 已走此路，ui-liquid-glass.md:108-110）"
      ],
      files: ["data/ui/mode-selector.blp", "src/ui/mode_selector.rs"],
      landed:
        "P2-3 已落盘（2026-10-02），按草案的第一方案（accessible role），没有换成 ToggleButton。角色写在模板里（`accessible-role` 在 GTK 里是可读写属性，Blueprint 用枚举名下划线形式 `radio_group`，写成 `radio-group` 会编译失败）：整件控件 `radio_group`，三个 label_cell `radio`，指示条那一行 `presentation`——滑动的短条只是重复「哪一段被选中」，不该在无障碍树里变成第四件东西。名称与状态由 Rust 给：`set_labels_i18n` 把每段的可读名与可见文案用同一个 `tr()` 字符串同时下发（`Property::Label`），整件控件另取一个新键 `photo.mode.group`（两份 json 同步，parity 447/447）并标 `Property::Orientation(Horizontal)`；`apply_state` 给三段各推一个 `State::Checked(True/False)`，所以永远恰好一段被勾选。\n\n保留原样：整件控件是唯一 tab stop + 左右方向键（含环绕）。草案和原型都把 tabIndex 发到每一段上，落盘没有跟——逐段可聚焦会把焦点环画在三个 Box 上、把一次 Tab 变成三次，并且和「一个玻璃胶囊、内部状态轻量」这条硬契约相冲。\n\n实测证据（本机，非 CI）：`env -u NO_AT_BRIDGE HOME=<tmp> tools/with-at-spi.sh xvfb-run -a ./target/debug/photo-viewer` 起真实应用，再用 pyatspi 走树，得到 `grouping '照片分组方式'（FOCUSABLE）` 下挂 `radio button '年' / '月' / '日'`，CHECKED 落在应用当时记住的那一段（日）。读到的正是读屏会念的东西，命令已记进 docs/testing.md。\n\n限制记录（不做沉默处理）：三段仍是 `Gtk.Box` + GestureClick，因此**不暴露 AT-SPI action**——读屏能播报、也能用方向键在聚焦的胶囊上改选，但没法「按下」指定某一段。草案自己点过这条风险（改 ToggleButton 会触碰材质契约），所以这里选择记录限制而不是换控件。",
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --lib ui::mode_selector（19 项，新增 3 条：角色树、恰好一段被勾选、名称与可见文案同源。负向验证：从 blp 删掉 accessible-role 后 the_selector_is_a_radio_group_of_three_radios 变红）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --test ui_mode_selector --test ux_click_flows（单元格点击手势与模式切换不受重构影响）",
        "手工 AT-SPI 探针（pyatspi 读真实应用的树）-> PASS，需要桌面会话与手工启动，没有进 CI"
      ],
      docs: [
        "docs/modules/ui-liquid-glass.md（分段控件的无障碍语义）",
        "docs/modules/browsing.md（模式选择器）",
        "docs/testing.md（本机 AT-SPI 探针配方 + NO_AT_BRIDGE 坑）",
        "docs/ui-naming-reference/index.html（mode-selector 条目）"
      ],
      risk: "改 ToggleButton 分组会触碰材质契约，优先用 accessible role 方案。",
      demo:
        "常态即已落盘行为：胶囊是单一 tab stop，Tab 聚焦后左右方向键切换并环绕，焦点环画在胶囊外沿；无障碍语义（radio group + 三个 radio + checked）在浏览器无障碍树里可核对，界面无任何视觉变化。提案芯片不再改动画面。"
    },
    {
      id: "p2-4",
      prio: "P2",
      batch: "B9",
      title: "图标按钮补 tooltip / accessible label",
      screens: ["photos", "viewer", "settings"],
      problem: "全项目 set_accessible_label / set_accessible_role 调用为 0。",
      evidence: [
        "grep 确认；overview_sync_icon（photos_page.rs:949）与警告图标（:1037）连 tooltip 都没有",
        "约 26 个图标按钮"
      ],
      solution: [
        "先给约 26 个图标按钮补 tooltip 兜底，再为状态类（同步状态、时长/云/收藏徽记 square_tile.rs）补 accessible label；用 tools/assert-at-spi.py 模式扩展断言面"
      ],
      files: [
        "src/ui/square_tile.rs",
        "src/ui/media_grid/render.rs",
        "src/ui/virtual_media_grid/factory.rs",
        "data/ui/editor-panel.blp",
        "data/ui/photos-page.blp",
        "data/ui/viewer-page.blp",
        "data/ui/window.blp",
        "src/ui/album_picker.rs",
        "src/ui/window/sidebar.rs",
        "src/ui/window/settings.rs",
        "i18n/zh-CN.json",
        "i18n/en.json"
      ],
      landed:
        "P2-4 已落盘（2026-10-02），但草案的测量口径要先更正：GTK4 里没有 set_accessible_label 这个 API，能写的只有 accessible-role 属性（可读写）与 gtk_accessible_update_property(Property::Label)，所以「调用为 0」grep 的是一个不存在的符号。真正的 0 是全仓 0 处 accessible-role、0 处 update_property——结论成立，落盘后是模板 7 处角色 + Rust 23 处角色/名称调用。\n\n「先给约 26 个图标按钮补 tooltip 兜底」同样要更正：按「有 icon-name 且没有 label」清点 data/ui/*.blp，26 枚图标按钮里 22 枚本来就有 tooltip，而且多数走 P0-4 的 tooltip_with_key（正是 risk 里担心的两套措辞——已经共用同一份，没有第二套）。真正既没提示也没名字的只有裁剪比例那两枚箭头，现在补了 editor.crop.previous.tooltip / editor.crop.next.tooltip（两份 json 同步，parity 452/452）；crop_ratio_prev/next、edit_btn、settings_button 四枚补了模板空槽，另两枚本来分别由 viewer_page.rs:522 与 window.rs:651 写入。兜底由 tests/ui_template_copy.rs::icon_only_buttons_reserve_a_tooltip_slot 扫描全部 icon-only 按钮，缺槽即红。这条断言的首版是假绿：它按 depth==0 判断根节点，嵌在 box 里的按钮根本没被检查，重写深度跟踪后当场揪出两枚真缺槽的按钮。\n\n名称侧（草案点名的「状态类」）：SquareTile 在 class_init 里取类级角色 img，绑定者把用户看得见的内容推成名字——set_accessible_name(item.display_name()) 出现在 media_grid::render 的 prepare_reused_tile / build_photo_picture 与 virtual_media_grid::factory::bind_ready_cell 三处，clear_for_rebind 连名字一起清掉，回收的格子不会继续回答上一张照片。时长/动态/收藏徽标与云徽标各报自己的名字（tile.badge.duration 把可见的 01:23 包成「视频时长 0:42」，云徽标与 tooltip 同源）。装饰性图标一律 presentation：overview_sync_icon（旁边的 label 已经说完）、查看器错误面图标（标题+副标题）、侧栏行图标与相册分组箭头、相册选择器的文件夹占位、设置页那枚 chevron。\n\n实测证据（本机，非 CI，命令在 docs/testing.md）：pyatspi 走真实应用的树。第一轮把名字给了常驻的对勾，格子念成「已选中 已收藏」而文件名全部消失——tile 当时是容器角色，GTK 直接丢掉推给它的名义；改成「对勾 presentation + tile 类级 img 角色」后，table cell 的名字变成 ['a.jpg', 'b.gif', '', '']，后两格是虚拟网格的 filler，本来就没有媒体。\n\n限制记录（不沉默处理）：选中目前不是 accessible state。虚拟网格的 GtkListItem 是 set_selectable(false)（选择由 VirtualMediaGrid 自己管，不是 GtkSelectionModel），对勾又是装饰，所以读屏听得到文件名、听不到「这张被选了」。补它需要 SquareTile::set_selected(bool) 统一那 4 处 CSS class 的增删并推 State::Selected，而且要先验 img 角色是否接受这个状态；本轮没做，写进了 browsing.md。AdwStatusPage 内部的警告图标同理没有动：标题与描述已经说完这句话，且那是 libadwaita 的内部节点。",
      tests: [
        "cargo test --locked --lib ui::square_tile（18 项，新增 the_tile_and_its_state_badges_are_named：类级角色、名字可回读、clear_for_rebind 清名、徽标键真的解析、对勾是装饰。负向验证：注释掉 klass.set_accessible_role(Img) 后该断言变红）",
        "cargo test --locked --test ui_template_copy（3 项，含 icon_only_buttons_reserve_a_tooltip_slot）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::（403 项）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --test ui_editor_panel --test ui_album_picker --test ui_mode_selector --test ux_click_flows --test e2e_browsing --test ui_context_menu --test ui_grid_canvas --test inline_test_ownership（全绿）",
        "手工 AT-SPI 探针（pyatspi 读真实应用的 table cell 名字）-> PASS，需要桌面会话与手工启动，没有进 CI"
      ],
      docs: [
        "docs/modules/browsing.md（Accessible Names For Tiles And Badges）",
        "docs/testing.md（探针补读名字与状态集的方法，以及容器角色丢名字这条坑）",
        "AGENTS.md（UI Invariants：图标控件必须可命名，装饰图标必须 presentation）",
        "docs/ui-naming-reference/index.html（宫格、徽标、同步行、错误面、编辑/设置按钮、新增 crop-ratio-arrows 条目）"
      ],
      risk: "tooltip 文本要与 P0-4 的 tr_with_key 拼接共用，避免两套措辞。",
      demo:
        "常态即已落盘行为：每张格子带自己的文件名（原型写在格子内层一个零尺寸的 role=img 节点上，等价于真实实现里由 tile 本体承担 img 角色），时长/动态/收藏/云徽标各报自己的名字，对勾与状态页图标是 aria-hidden——打开浏览器无障碍树可核对，界面没有任何变化。落盘前形态走工具条「落盘前：宫格对读屏匿名（P2-4）」（data-pv-demo=tiles-anonymous）：那时把读屏实际听到的内容直接画在格子上（「读屏：无名称 · 角标念成「▶」」）。"
    },
    {
      id: "p2-5",
      prio: "P2",
      batch: "B3",
      title: "选择相关 DB 查询移出主线程",
      screens: ["photos"],
      problem: "每次选择变化都在主线程同步查库，大库下多选 header 会掉帧。",
      evidence: [
        "photos_page.rs:1155（每次选择变化 favorite_state）",
        ":1337（同步取 2000 条）",
        ":1360（同步 count）"
      ],
      solution: ["移入 spawn_blocking + generation 回投"],
      files: [
        "src/ui/photos_page.rs",
        "src/core/repository.rs",
        "src/core/db.rs",
        "tests/library_benchmark.rs"
      ],
      landed:
        "P2-5 已落盘（2026-10-02），三处同步查库都不在选择节拍上了：\n\n1. `refresh_selection_ui` 不再问库。它 bump 一个 `selection_generation`（只在选中集合真的变了才 bump——同一份选择被重复 emit 不该把在途答案判成过期），然后 `refresh_selection_favorite_state` 起一次 `glib::spawn_future_local` + `gtk::gio::spawn_blocking`；答案回来时如果代次已经变了就丢弃并立刻重查，所以「停下来的那份选择一定拿到自己的答案」，同时最多一条查询在飞。\n2. 全选取 2000 条 id 移进 `select_all_in_current_mode` 的 worker，`select_all_in_flight` 挡住连点。\n3. 全选/取消全选要的那个总数不再单独 COUNT：它挂到本来每 2 秒就在后台跑的总览快照（`PhotosOverviewSnapshot.live_total`），首帧之前退回「当前可见格是否全选」。\n\n真正的瓶颈其实在数据层，落盘时才量出来：`repository.favorite_state` 是**每个 id 一次 SELECT**，2000 项选择＝2000 次 prepare——草案把它写成一次「favorite_state 查询」，低估了一个数量级。现在换成每 500 个 id 一条聚合语句（`db::favorite_state_for_ids`，`SELECT COUNT(*), MAX(is_favorite=1), MAX(is_favorite=0)`），并且保留原来「查不到该 id 记为未收藏」的容错口径（行数少于请求数即视为有未收藏）。20000 库、2000 项选择实测 15ms → 2ms（debug 构建，opt-in 基准 `PHOTOVIEWER_BENCH_ITEMS=20000 cargo test --test library_benchmark -- --ignored --nocapture`，这一行已加进基准输出）。\n\n一处按原样保留：瓦片右键菜单的 `on_query_favorite_state` 仍然同步——它是一次性动作、不在选择节拍上，而异步化要改 `GlassContextMenu` 的建菜单契约；它现在也被同一条聚合语句兜住了。\n\nrisk 里担心的中间态确实出现过，而且是被测试逼出来的：把收藏按钮改成读「画出来的状态」之后，`ux_click_flows` 的批量收藏旅程红了——刚换选择就点，缓存还是上一份选择的（清空＝两边都 false），于是点下去什么都不发生。所以点击路径 `decide_favorite_action` 自己问库再决定（心形是智能开关，答案决定它是直接切换还是弹两选菜单），只在代次未变时顺手更新绘制。",
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::photos_page（18 项。新增 the_favorite_state_of_a_selection_lands_from_a_worker：选择节拍里画不出答案、worker 落地后心形才红、连点选择后点心形按新选择行动；the_select_all_label_decides_from_the_cached_total：故意让缓存与库不一致，证明读的是缓存。负向验证：把 refresh 换回同步查询后第一条变红；把点击改回读缓存状态后第三条不收藏而是去弹 popover）",
        "cargo test --locked --test repository（27 项，新增三条 favorite_state 语义：均匀选择不再报成混合、跨 500 分块边界、缺失 id 记为未收藏。负向验证：去掉「行数少于请求数」那一支后第三条变红）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --test ux_click_flows（全选改异步后四处断言改成 wait_until；这一步同时抓出了点击读缓存的真 bug）",
        "PHOTOVIEWER_BENCH_ITEMS=20000 cargo test --test library_benchmark -- --ignored --nocapture -> 15ms（逐条）/ 2ms（聚合），手工测量，不进 CI"
      ],
      docs: [
        "docs/modules/browsing.md（选择计数与收藏状态改为异步）",
        "docs/modules/storage.md（favorite_state 的聚合查询与缺失 id 口径）",
        "docs/ui-naming-reference/index.html（selection-query-thread 条目转为已落盘说明）"
      ],
      risk: "generation 处理不当会出现计数与选择不一致的中间态。",
      demo:
        "常态即已落盘行为：顶栏那条「计数查询：后台线程」标注现在是实现说明，不再受提案开关控制（点亮芯片不会改动画面，静态页本来也复现不了掉帧）。可核对的是联动本身：进入多选、全选、再点一次取消全选，计数与按钮文案的切换不再夹一次同步查库。15ms→2ms 的数字来自 opt-in 基准 tests/library_benchmark.rs，不是这一页能演示的东西。"
    },
    {
      id: "p2-6",
      prio: "P2",
      batch: "B2",
      title: "相册选择器 loading / error 分离",
      screens: ["picker"],
      problem: "相册选择器加载中是空网格；DB 报错显示成「暂无相册」。",
      evidence: ["src/ui/album_picker.rs:174-220，其中 :214-218 把错误渲染为空态标题"],
      solution: ["复用 P0-1 的 loading/error 分离结论"],
      files: [
        "src/ui/album_picker.rs",
        "src/ui/empty_states.rs",
        "i18n/zh-CN.json",
        "i18n/en.json"
      ],
      landed:
        "P2-6 已落盘（2026-10-02）。相册选择器现在是一个四页 Gtk.Stack（loading / empty / albums / error），弹框一打开停在加载页：\n\n1. loading 复用共享的 empty.loading（加载中…）＋一个已经 start() 的 GtkSpinner——草案画的「正在读取相册…」没有另起一套文案；未 start 的 spinner 是冻住的空圈，这条是 P0-1 真跑出来的教训。\n2. empty 只在「查询成功且真的没有相册」时出现，标题与描述分别用 album_picker.no_albums_yet.title / .description。落盘前是把 description 塞进底部那行小标签，而标题被错误分支拿去当文案用。\n3. error 用新的 empty_states::load_failed(msg, on_retry)：标题 empty.load_failed.title（读取失败），描述 empty.load_failed.description_with_reason 带数据库原话，按钮 common.retry 重跑同一条列表查询（首次加载与重试共用一个闭包，重试会先清空网格、清掉选中并把复制/移动重新禁用）。\n\nrisk 那条（错误信息可能含路径）按原样接受：描述里显示的就是 rusqlite 的原始错误串，例如 no such table: albums。它同时是测试注入的故障（DROP TABLE albums），所以断言能直接看到原因出现在页面上。\n\n顺带纠正草案与命名图的一处事实：两者都把「相册详情页空态复用全库文案」当成 P2-6 关联项，但 AlbumDetailPage 从很早就用 empty_states::no_album_photos()（空相册 / 该相册暂无照片。），那条「现状对照」条目已删；提案里更强的文案（此相册还没有照片 ＋ 去照片页按钮）没有落盘，也不需要。",
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --test ui_album_picker（同一个弹框走完 loading→empty，再用 DROP TABLE albums 起第二个弹框走 loading→error：断言可见页的标题、描述含真实原因、页面上有「重试」按钮。注意 Gtk.Stack 的隐藏页仍在树里，所以断言读的是 stack.visible_child() 而不是随便找一个 StatusPage——第一版就是这样读到了错误页的标题。负向验证：把 error 分支改成显示「empty」页后该断言超时变红）",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::empty_states（3 项，新增 loading 会转、load_failed 的标题/原因/空原因回退/重试按钮真的回调）",
        "cargo test --locked --lib core::i18n -> PASS（两份 json 各 456 键，parity 保持）"
      ],
      docs: [
        "docs/modules/albums-trash.md（相册选择器四态与 load_failed 契约）",
        "docs/ui-naming-reference/index.html（picker 三页 + 落盘前对照；删掉失实的 album-empty-current）"
      ],
      risk: "错误信息可能含路径，展示时需本地化与截断。",
      demo:
        "常态即已落盘行为：弹框的四种事实各占一页，演示按钮按 正常 → 加载中 → 真的没有相册 → 读取失败 → 落盘前对照 → 空相册详情页 循环（静态页没有真查询，能核对的是页面本身与文案）。落盘前对照（picker-error-as-empty）就是报错与空态共用同一句话的那一屏。"
    },
    {
      id: "p2-7",
      prio: "P2",
      batch: "B2",
      title: "回收站不再闪「回收站为空」",
      screens: ["trash"],
      problem: "每次打开回收站先闪一下「回收站为空」。",
      evidence: ["src/ui/trash_page.rs:122-132 空态 child 常驻直到数据落地"],
      solution: ["首轮加载完成前不切空态"],
      files: ["src/ui/trash_page.rs", "src/ui/empty_states.rs"],
      landed:
        "P2-7 已落盘（2026-10-02）。回收站的 content_stack 现在是四页：loading / content / empty / error，外加一个 `first_load_done` 闸门。\n\n关键的一点跟草案的写法不同：**光把 `constructed` 的初始页改成 loading 不够**——`build()` 在构造末尾就调 `refresh()`，所以真正挡住闪烁的是 `refresh()` 里那道「只有首轮才切 loading」的判断。负向验证正好证明这件事：只回退 `constructed` 那一行时测试仍然是绿的，两处一起回退才变红。这条已经写进命名图的条目说明，免得下一个人只改初始页。\n\n之后的 refresh 不再退回 spinner：已经看到的网格不会因为一次重算被抹掉（草案没提这点，但如果每轮都切 loading，等于把闪空态换成闪白）。读取失败也不再伪装成空态——走 P2-6 刚落地的 `empty_states::load_failed`，带数据库原话与「重试」（重试就是再调一次 `refresh()`），并且失败时不设 `first_load_done`，所以重试会重新显示 loading。\n\nrisk 那条（要区分「未加载」与「已加载且为空」）就是这次的实现方式：两个状态两页，empty 只由一次完成的读取产生。",
      tests: [
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --lib ui::trash_page（9 项，新增两条：trash_opens_on_loading_and_only_then_decides（构造后立刻断言可见页是 loading 且 first_load_done 为假，pump 之后才落到 empty）、trash_first_read_lands_on_content_and_refresh_keeps_it_there（有一件回收项时首轮落 content，再 refresh 不回退到 loading）",
        "负向验证：只把 constructed 改回 empty 时测试仍绿（因为 build() 会调 refresh()），两处一起回退才红——这条差异已记进条目说明",
        "tools/with-at-spi.sh xvfb-run -a cargo test --locked --test e3e_albums_trash --test ux_click_flows（回收站还原/永久删除旅程不受新页序影响）"
      ],
      docs: [
        "docs/modules/albums-trash.md（回收站四页与首轮加载闸门）",
        "docs/ui-naming-reference/index.html（trash-first-load 转为已落盘说明；现状对照改名落盘前对照）"
      ],
      risk: "需要区分「未加载」与「已加载且为空」两个状态。",
      demo:
        "常态即已落盘行为：进回收站停在 loading，读取完成才决定出图还是出空态。工具条「落盘前：回收站首帧闪空态（P2-7）」（data-pv-demo=trash-flash）调出的那 0.9 秒才是修复前的样子；点提案芯片会直接播一遍这一闪并说明常态。"
    },
    {
      id: "p2-8",
      prio: "P2",
      batch: "B2",
      title: "总览可点 disclosure + 同步失败重试",
      screens: ["photos"],
      problem: "全库总览只能靠「顶部再往上滚」发现，明确无 disclosure 按钮；同步状态失败也无重试按钮。",
      evidence: [
        "photos_page.rs:1243-1257、docs/modules/browsing.md:133-141",
        "photos_page.rs:1024-1040，:1279 错误仅日志"
      ],
      solution: ["加一个可点 chevron（不改材质）；同步失败行补重试动作"],
      files: ["data/ui/photos-page.blp", "src/ui/photos_page.rs", "i18n/*.json"],
      tests: ["cargo test ui::photos_page（点 chevron 展开/收起总览）"],
      docs: ["docs/modules/browsing.md"],
      risk: "总览默认隐藏是刻意的，按钮不应变成视觉重心。",
      demo:
        "本项已落盘，所以 header 的 ⌄/⌃ 圆钮与概览行里的「重试」都是常态 chrome：点 chevron 真的开合那一行，glyph、tooltip 与读屏名字一起翻。提案芯片不再改动画面（已落盘的行为不该被开关削弱）。两个演示按钮：「全库总览：展开 + 同步失败重试（P2-8 常态）」把那一行展开并切到 Failed，重试才出现在句子末尾；「落盘前：总览没有 disclosure（P2-8）」摘掉 chevron 与重试，回到只能靠「顶部再往上滚」发现的旧样子。",
      landed:
        "B2 已落盘（2026-10-02）。chevron：photos-page.blp:51 overview_toggle_btn，[start] 第二个 child，紧挨 search_btn 声明（选择 chrome slide 进来不会挤走它），材质逐字复用 .glass-toolbar-button + .round-search-button，没有新增玻璃类，也没有 per-segment 底色块——它是一枚普通按钮，不是分段控件。状态是派生的：photos_page.rs apply_overview_disclosure_state 挂在 overview_revealer 的 notify::reveal-child 上，glyph（pan-down/pan-up）、tooltip 与 accessible name（photos.overview.show / photos.overview.hide）都从 revealer 读，所以下拉手势、切换分组时的自动收起、点按钮三条路径不可能各说一套。图标按钮的状态属于名字，因此同一串既 set_tooltip_text 又推 accessible::Property::Label。重试：overview_sync_retry_btn（photos-page.blp:215）走 .flat + base.css .photos-overview-retry（11pt、去掉主题最小按钮框），只在 SyncOverviewStatus::Failed 与总览读取失败（apply_overview_error）两处出现，apply_overview_retry_affordance 是唯一写入者；Completed/Running/Paused/Ready 只有描述，Disabled 连整行收起。点击复用既有 trigger_sync_from_home_pull()，所以没有新增同步路径、也没有 per-task Sync Now（草案担心的那一条）；overview_sync_pull_in_flight 期间置灰而不是吞点击；读取失败那条只在 webdav_sync_enabled() 时才出按钮——被该 pref 挡下的 pull 是死钮。落盘时顺带改掉一处文案：photos.overview.sync.failed 原文是「打开设置查看详情或下拉重试」，「下拉重试」在有两个入口之后不再唯一，改为「可点「重试」，或在设置中查看详情」。偏差一处：草案说「加一个可点 chevron」暗示新样式，实际连 CSS 都没新增选择器（复用现成两枚 class），新增的 .photos-overview-retry 只是把按钮压回行内尺寸。i18n 没有新增键（photos.overview.hide 早已存在但一直是死键，这次才接上）。测试：ui::photos_page::tests::the_overview_disclosure_button_mirrors_the_revealer（两个方向、外加一次没碰按钮的展开）、a_failed_sync_offers_a_retry_and_other_states_do_not（四种非失败态都隐藏、in-flight 置灰、读取失败按 pref 决定出不出钮）；--test ui_photos_toolbar 从 5 枚 glass 按钮变 6 枚并断言两枚圆钮同类；narrow_window_keeps_the_start_header_button_allocated 现在量两枚 [start] 圆钮。--test ui_template_copy 的 tooltip 槽断言已用「删掉 overview_toggle_btn 的 tooltip-text」验证不是空跑。"
    },
    {
      id: "p2-9",
      prio: "P2",
      batch: "B3",
      title: "收藏按钮混合态下拉指示",
      screens: ["photos"],
      problem: "收藏按钮在「直接切换」与「弹层」之间隐形变化，用户无法预判会不会弹菜单。",
      evidence: ["photos_page.rs:754-770、契约见 docs/modules/ui-design.md:140-147"],
      solution: ["混合态给按钮加下拉指示，让「会弹菜单」可预判"],
      files: ["src/ui/photos_page.rs", "data/ui/photos-page.blp"],
      tests: ["cargo test ui::photos_page（混合态显示指示、全同态不显示）"],
      docs: ["docs/modules/ui-design.md:140-147"],
      risk: "指示图标要与 P0-3 的 header 密度一起看，别挤爆。",
      demo:
        "本项已落盘，所以角标是常态 chrome 而不是提案开关：进入多选后用心形按钮演示三种点击——混合态才有右下角那枚 ⌄，全同态直接执行且红心/文案各自对应。演示按钮「心形：混合态（P2-9 常态）」把选择切到混合态；「落盘前：心形按钮行为不可预判（P2-9）」摘掉角标并让三种状态长得一模一样，回到只能靠经验预判的旧样子。",
      landed:
        "B3 已落盘（2026-10-02）。指示物是一枚 12 px pan-down-symbolic（photos-page.blp:115 favorite_menu_hint），挂在心形右下角的 Gtk.Overlay [overlay] 上（favorite_btn_host，:104），can-target: false 所以不吃按下，accessible-role: presentation 所以不重复播报——它复述的是按钮自己的名字。为什么用 overlay 而不是拆分按钮：ui-design.md 的契约禁止把收藏/取消收藏拆成两枚 header 按钮，而 split button 会为了最稀有的一种状态把 header 撑宽（草案 risk 担心的正是密度）。状态写入收成一个函数：apply_selection_favorite_state 一次画好三样——favorite-active 红心、tooltip 与 accessible name（同一个串，两条通道不能各说一套）、以及这枚角标；混合态另有一键 photos.batch.favorite.mixed（中英同步，parity 457/457），因为「承诺收藏、然后反问」在 tooltip 与读屏里是同一个谎。new() 里按 FavoriteMenuState::default() 先画一次，第一次选择之前按钮就已经有名字。角标只在混合态出现：常驻会对着「直接执行」的那两种点击撒谎。测试：ui::photos_page::tests::only_the_mixed_selection_marks_the_heart_as_opening_a_menu（三种状态各自的名字、红心与角标；构造后的初始态；presentation 角色）。两处负向验证：删掉 set_visible(mixed) → 测试红；把 mixed 文案退回 photos.batch.favorite → 测试红。--test ui_photos_toolbar 与 --test ux_click_flows 的批量收藏旅程在 overlay 嵌套后仍绿。"
    }
  ];

  var BY_ID = {};
  PROPOSALS.forEach(function (p) { BY_ID[p.id] = p; });

  var BATCHES = {
    B1: { name: "B1 焦点环", ids: ["p0-2"] },
    B2: { name: "B2 扫描态", ids: ["p0-1", "p2-6", "p2-7", "p2-8"] },
    B3: { name: "B3 多选与选择", ids: ["p0-3", "p1-6", "p2-5", "p2-9"] },
    B4: { name: "B4 快捷键发现", ids: ["p0-4"] },
    B5: { name: "B5 搜索", ids: ["p0-5", "p1-15"] },
    B6: { name: "B6 查看器信息层", ids: ["p1-7", "p1-12", "p2-2"] },
    B7: { name: "B7 查看器输入", ids: ["p1-8"] },
    B8: { name: "B8 编辑与错误反馈", ids: ["p1-9", "p1-10", "p2-1"] },
    B9: { name: "B9 材质与动效", ids: ["p1-11", "p1-13", "p1-14", "p2-3", "p2-4"] }
  };

  var VERDICT_LABELS = { accept: "采纳", hold: "暂缓", reject: "否决" };

  /* ==================================================== 持久化 / 深链 */

  var LS_KEY = "photoviewer.ui-review.v1";

  function loadState() {
    try {
      return JSON.parse(localStorage.getItem(LS_KEY) || "{}");
    } catch (err) {
      return {};
    }
  }
  function saveState(patch) {
    var s = loadState();
    Object.keys(patch).forEach(function (k) { s[k] = patch[k]; });
    try {
      localStorage.setItem(LS_KEY, JSON.stringify(s));
    } catch (err) {
      /* 隐私模式下写入失败：不阻塞评审 */
    }
  }

  function readHash() {
    var out = {};
    var raw = String(location.hash || "").replace(/^#/, "");
    if (!raw) return out;
    raw.split("&").forEach(function (pair) {
      var kv = pair.split("=");
      if (kv.length === 2) out[decodeURIComponent(kv[0])] = decodeURIComponent(kv[1]);
    });
    return out;
  }

  function writeHash() {
    var parts = [];
    parts.push("mode=" + (body.dataset.pvMode || "naming"));
    parts.push("screen=" + (body.dataset.pvScreen || "photos"));
    var t = PV.tokens();
    if (t.length) parts.push("pv=" + t.join(","));
    var scan = body.dataset.pvScan;
    if (scan && scan !== "ready") parts.push("scan=" + scan);
    var loc = body.dataset.pvLocale;
    if (loc && loc !== "zh") parts.push("locale=" + loc);
    var mat = body.dataset.pvMaterial;
    if (mat && mat !== "liquid") parts.push("material=" + mat);
    var tr = body.dataset.pvTransparency;
    if (tr && tr !== "0") parts.push("t=" + tr);
    if (body.dataset.pvMotion === "off") parts.push("motion=off");
    var w = body.dataset.pvWidth;
    if (w && w !== "wide") parts.push("width=" + w);
    if (body.dataset.pvDemo) parts.push("demo=" + body.dataset.pvDemo);
    var next = "#" + parts.join("&");
    if (next === location.hash) return;
    try {
      history.replaceState(null, "", next);
    } catch (err) {
      /* file:// 下某些浏览器禁止 replaceState：深链只读不写，不影响评审 */
    }
  }

  /* ============================================================ 工具条 */

  function el(tag, cls, text) {
    var node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text != null) node.textContent = text;
    return node;
  }

  function group(label) {
    var g = el("div", "ux-group");
    if (label) g.appendChild(el("span", "ux-label", label));
    return g;
  }

  function chipBtn(text, onClick, extraCls) {
    var b = el("button", "ux-btn" + (extraCls ? " " + extraCls : ""), text);
    b.type = "button";
    b.addEventListener("click", onClick);
    return b;
  }

  var barRefs = {};

  function buildBar() {
    var bar = q("#ux-bar");
    if (!bar) return;
    // 保留 <noscript>
    qa(".ux-row", bar).forEach(function (r) { r.remove() });

    /* ---- 行 0：收纳开关。收起后整条评审工具条只剩这一行，不再遮挡
       下方原型内容（也方便对页面截图）。状态存 localStorage。---- */
    var rowToggle = el("div", "ux-row ux-toggle-row");
    var uxBarCollapsed = false;
    try { uxBarCollapsed = localStorage.getItem("pv-uxbar-collapsed") === "1"; } catch (e) { }
    var collapseBtn = chipBtn("收起检视 ▴", function () {
      uxBarCollapsed = !uxBarCollapsed;
      try { localStorage.setItem("pv-uxbar-collapsed", uxBarCollapsed ? "1" : "0"); } catch (e) { }
      applyUxBarCollapse();
    }, "ux-collapse-btn");
    collapseBtn.setAttribute("aria-expanded", "true");
    rowToggle.appendChild(collapseBtn);
    bar.appendChild(rowToggle);

    function applyUxBarCollapse() {
      bar.classList.toggle("ux-collapsed", uxBarCollapsed);
      collapseBtn.textContent = uxBarCollapsed ? "展开检视 ▾" : "收起检视 ▴";
      collapseBtn.setAttribute("aria-expanded", uxBarCollapsed ? "false" : "true");
    }
    applyUxBarCollapse();

    /* ---- 行 1：模式 + 路由 ---- */
    var row1 = el("div", "ux-row");
    var gMode = group("模式");
    barRefs.modeNaming = chipBtn("命名总览", function () { PV.setMode("naming"); }, "mode-naming");
    barRefs.modeRun = chipBtn("交互原型", function () { PV.setMode("run"); }, "mode-run");
    gMode.appendChild(barRefs.modeNaming);
    gMode.appendChild(barRefs.modeRun);
    row1.appendChild(gMode);

    var gRoute = el("div", "ux-group ux-routes");
    var ROUTES = [
      ["photos", "照片页"], ["album", "相册详情"], ["trash", "回收站"],
      ["viewer", "查看器"], ["search", "搜索"], ["collapsed", "窄窗折叠态"],
      ["settings", "设置"], ["picker", "相册选择"]
    ];
    ROUTES.forEach(function (pair) {
      var b = chipBtn(pair[1], function () { needScreen(pair[0]); }, "ux-route");
      b.dataset.route = pair[0];
      gRoute.appendChild(b);
    });
    gRoute.appendChild(chipBtn("← 返回", function () {
      if (body.dataset.pvMode !== "run") PV.setMode("run");
      PV.back();
    }));
    barRefs.crumb = el("span", "ux-crumb");
    gRoute.appendChild(barRefs.crumb);
    row1.appendChild(gRoute);
    row1.appendChild(el("div", "ux-sep"));
    bar.appendChild(row1);

    /* ---- 行 2：批次预设 + 提案开关 ---- */
    var row2 = el("div", "ux-row");
    var gBatch = group("实施批次");
    Object.keys(BATCHES).forEach(function (key) {
      var b = BATCHES[key];
      gBatch.appendChild(chipBtn(b.name, function () {
        clearTokens();
        b.ids.forEach(function (id) { PV.setEnabled(id, true); });
        jumpToProposal(b.ids[0]);
        syncBar();
      }));
    });
    gBatch.appendChild(chipBtn("全部 P0+P1", function () {
      clearTokens();
      PROPOSALS.filter(function (p) { return p.prio !== "P2"; }).forEach(function (p) { PV.setEnabled(p.id, true); });
      syncBar();
    }));
    gBatch.appendChild(chipBtn("全部提案", function () {
      PROPOSALS.forEach(function (p) { PV.setEnabled(p.id, true); });
      syncBar();
    }));
    barRefs.clear = chipBtn("清空提案", function () { clearTokens(); syncBar(); });
    gBatch.appendChild(barRefs.clear);
    row2.appendChild(gBatch);

    /* ---- 行 3：环境 ---- */
    var row3 = el("div", "ux-row");
    var gEnv = group("环境");
    barRefs.locale = chipBtn("语言 EN", function () {
      PV.setLocale(body.dataset.pvLocale === "zh" ? "en" : "zh");
      syncBar();
    });
    barRefs.material = chipBtn("材质 Plain", function () {
      PV.setMaterial(body.dataset.pvMaterial === "liquid" ? "plain" : "liquid");
      syncBar();
    });
    barRefs.motion = chipBtn("减少动画", function () {
      PV.setMotion(body.dataset.pvMotion !== "off");
      PV.emit("motionUI");
      syncBar();
    });
    barRefs.width = chipBtn("窗口宽度", function () {
      var order = ["wide", "medium", "narrow"];
      var i = order.indexOf(body.dataset.pvWidth || "wide");
      PV.setWidth(order[(i + 1) % order.length]);
      syncBar();
    });
    [barRefs.locale, barRefs.material, barRefs.motion, barRefs.width].forEach(function (b) { gEnv.appendChild(b); });

    var gTrans = group("透明度");
    barRefs.trans = el("input", "range");
    barRefs.trans.type = "range";
    barRefs.trans.min = "0";
    barRefs.trans.max = "100";
    barRefs.trans.step = "5";
    barRefs.trans.value = "0";
    barRefs.trans.setAttribute("aria-label", "玻璃透明度");
    barRefs.trans.addEventListener("input", function () { PV.setTransparency(barRefs.trans.value); });
    gTrans.appendChild(barRefs.trans);
    row3.appendChild(gEnv);
    row3.appendChild(gTrans);

    var gScan = group("扫描状态");
    barRefs.scan = el("select", "ux-btn");
    barRefs.scan.style.padding = "3px 8px";
    [["ready", "就绪"], ["scanning", "扫描中"], ["failed", "扫描失败"], ["empty", "空库"]].forEach(function (pair) {
      var o = document.createElement("option");
      o.value = pair[0];
      o.textContent = pair[1];
      barRefs.scan.appendChild(o);
    });
    barRefs.scan.addEventListener("change", function () { PV.setScan(barRefs.scan.value); syncBar(); });
    gScan.appendChild(barRefs.scan);
    row3.appendChild(gScan);

    /* ---- 行 4：演示触发器 ---- */
    var row4 = el("div", "ux-row");
    var gDemo = group("演示触发");
    function demoBtn(text, run) { gDemo.appendChild(chipBtn(text, run)); }
    demoBtn("触发一次 rebuild（P1-6）", function () { needScreen("photos"); PV.runAction("rebuild"); });
    demoBtn("全选（2000 上限）", function () { needScreen("photos"); PV.runAction("select-all"); });
    demoBtn("进入多选（P0-3）", function () { needScreen("photos"); PV.setMulti(true); });
    demoBtn("打开查看器（P1-7/8/12）", function () {
      needScreen("photos", function () {
        var tiles = qa(".screen[data-screen='photos'] .tile[data-media]");
        if (tiles.length) PV.openViewer(tiles, 0);
      });
    });
    demoBtn("进入编辑（P1-9）", function () { needScreen("viewer", function () { PV.toggleEditor(); }); });
    demoBtn("标注已改动（脏）", function () { PV.setEditorDirty(); });
    demoBtn("快捷键窗口（P0-4）", function () { PV.openShortcuts(true); });
    demoBtn("落盘前：无快捷键入口（P0-4）", function () {
      needScreen("settings");
      body.dataset.pvDemo = "shortcut-blind";
      PV.applyLocale();
      syncBar();
      PV.toast("落盘前对照：设置里没有「键盘」分组，查看器 tooltip 只剩动作名，按 F1 无反应。清除演示态即回到已落盘状态。", { kind: "info", ms: 5200 });
    });
    demoBtn("相册选择器报错（P2-6）", function () { needScreen("picker"); body.dataset.pvDemo = "picker-error"; syncBar(); });
    demoBtn("慢查询：进行中指示（P0-5）", function () {
      needScreen("search", function () {
        var input = PV.q("#search-input");
        if (input && !input.value) input.value = "zzz";
        PV.setSearchLatency(650);
        PV.runSearch(input ? input.value : "zzz");
      });
      PV.toast("已把这次搜索抬到 650ms：超过 300ms 才在顶部起转圈，结果不会被整页替换。清除演示态回到瞬时搜索。", { kind: "info", ms: 5200 });
    });
    demoBtn("落盘前：搜索页只有搜索框（P0-5）", function () {
      needScreen("search");
      body.dataset.pvDemo = "search-blank";
      PV.applyLocale();
      PV.setSearchLatency(200);
      syncBar();
      PV.toast("落盘前对照：查询前与零结果下方全空，英文界面三个分段标签仍是中文。清除演示态即回到已落盘状态。", { kind: "info", ms: 5200 });
    });    demoBtn("落盘前：悬停与选中同构（P1-11）", function () {
      needScreen("photos");
      body.dataset.pvDemo = "tile-states-flattened";
      syncBar();
      PV.toast("落盘前对照：hover 用满强度 veil，未选中的格子没有对勾，指针扫过读起来就像在选片。清除演示态回到已落盘状态。", { kind: "info", ms: 5200 });
    });
    demoBtn("落盘前：控件簇乱序（P1-12）", function () {
      openViewerDemo();
      body.dataset.pvDemo = "cluster-legacy";
      syncBar();
      PV.toast("落盘前对照：还原在最左、放大在最右，旋转与全屏把缩小/放大拆在两端，36×32、间距 4、无分隔线、簇身无描边。清除演示态回到已落盘顺序。", { kind: "info", ms: 5200 });
    });
    demoBtn("落盘前：玻璃下文字过淡（P1-13）", function () {
      needScreen("photos");
      body.dataset.pvDemo = "low-contrast";
      syncBar();
      PV.toast("落盘前对照：库统计、侧栏计数、空态副标题与查看器错误面都掉在地板之下；把「透明度」拉高会更糟。清除演示态回到已落盘的地板。", { kind: "info", ms: 5200 });
    });
    demoBtn("落盘前：toast 盖住胶片条（P2-2）", function () {
      openViewerDemo();
      body.dataset.pvDemo = "toast-on-strip";
      syncBar();
      PV.toast("落盘前对照：提示贴在窗口底部，正好压在胶片条上——报的就是刚才在条带上做的那一步。清除演示态回到已落盘的舞台锚点。", { kind: "info", ms: 5200 });
      PV.runAction("viewer-delete");
    });
    demoBtn("落盘前：toast 没有撤销（P2-1）", function () {
      openViewerDemo();
      body.dataset.pvDemo = "toast-no-undo";
      syncBar();
      PV.toast("落盘前对照：可撤销的动作也只报成功，撤销只能自己去回收站找。清除演示态即回到带「撤销」按钮的 toast。", { kind: "info", ms: 5200 });
      PV.runAction("viewer-delete");
    });
    demoBtn("落盘前：宫格对读屏匿名（P2-4）", function () {
      needScreen("photos");
      body.dataset.pvDemo = "tiles-anonymous";
      PV.applyIconLabels();
      syncBar();
      PV.toast("落盘前对照：格子没有名字，角标只念出「▶」「♥」「0:42」，常驻的对勾反而被当成状态朗读——第一轮尝试就翻在这里。清除演示态回到已落盘的名称。", { kind: "info", ms: 5200 });
    });
    demoBtn("相册选择器加载中（P2-6）", function () { needScreen("picker"); body.dataset.pvDemo = "picker-loading"; syncBar(); });
    demoBtn("相册选择器空态（P2-6）", function () { needScreen("picker"); body.dataset.pvDemo = "picker-empty"; syncBar(); });
    demoBtn("落盘前：相册报错读起来像没有相册（P2-6）", function () {
      needScreen("picker");
      body.dataset.pvDemo = "picker-error-as-empty";
      syncBar();
      PV.toast("落盘前对照：查询失败与「真的没有相册」共用同一句话，既没有原因也没有重试。清除演示态后回到四页分开的事实。", { kind: "info", ms: 5200 });
    });
    demoBtn("空相册详情页（P2-6 关联，已是常态）", function () { needScreen("album"); body.dataset.pvDemo = "album-empty"; syncBar(); });
    demoBtn("落盘前：回收站首帧闪空态（P2-7）", function () { jumpToProposal("p2-7"); syncBar(); });
    demoBtn("全库总览：展开 + 同步失败重试（P2-8 常态）", function () {
      needScreen("photos");
      PV.setOverview("failed");
      syncBar();
      PV.toast("已落盘的常态：概览展开，同步切到 Failed，句子末尾才出现「重试」。换成别的状态（清除演示态即回到收起 + 已暂停）就不会有这颗按钮。", { kind: "info", ms: 5200 });
    });
    demoBtn("落盘前：总览没有 disclosure（P2-8）", function () {
      needScreen("photos");
      body.dataset.pvDemo = "overview-no-disclosure";
      PV.setOverview("closed");
      syncBar();
      PV.toast("落盘前对照：header 里没有那枚圆钮，同步失败也只有一句话——总览只能靠「顶部再往上滚」发现。清除演示态即回到已落盘的两个入口与重试。", { kind: "info", ms: 5200 });
    });
    demoBtn("心形：混合态角标（P2-9 常态）", function () {
      needScreen("photos");
      PV.setMulti(true);
      PV.setFavoriteState("mixed");
      syncBar();
      PV.toast("已落盘的常态：只有混合态在心形右下角带一枚 ⌄，tooltip 与读屏名字同时换成 photos.batch.favorite.mixed；换成全收藏或全未收藏，角标就消失。", { kind: "info", ms: 5200 });
    });
    demoBtn("落盘前：心形按钮行为不可预判（P2-9）", function () {
      needScreen("photos");
      PV.setMulti(true);
      // 顺序要紧：applyFavoriteState 会读演示态，先挂标记再算名字。
      body.dataset.pvDemo = "heart-unpredictable";
      PV.setFavoriteState("mixed");
      syncBar();
      PV.toast("落盘前对照：混合态与另外两种状态长得完全一样，名字也只承诺「收藏」——只有点下去才知道会弹菜单。清除演示态回到带角标的常态。", { kind: "info", ms: 5200 });
    });
    demoBtn("清除演示态", function () { body.dataset.pvDemo = ""; PV.applyLocale(); PV.setSearchLatency(200); syncBar(); });
    row4.appendChild(gDemo);

    var gOut = group("输出");
    barRefs.contrast = chipBtn("对比度报告（P1-13）", function () { openInspector("p1-13", "contrast"); });
    gOut.appendChild(barRefs.contrast);
    barRefs.export = chipBtn("导出评审结论", function () { exportMarkdown(); });
    gOut.appendChild(barRefs.export);
    barRefs.reset = chipBtn("清空评审记录", function () {
      saveState({ verdicts: {}, notes: {} });
      syncBar();
      if (currentInspection) renderInspector(currentInspection);
    });
    gOut.appendChild(barRefs.reset);
    row4.appendChild(gOut);

    /* ---- 提案 chips ---- */
    var rowChips = el("div", "ux-row");
    ["P0", "P1", "P2"].forEach(function (prio) {
      var g = group(prio);
      PROPOSALS.filter(function (p) { return p.prio === prio; }).forEach(function (p) {
        var chip = el("span", "ux-chip prio-" + prio.toLowerCase());
        chip.dataset.proposal = p.id;
        if (p.landed) {
          chip.classList.add("landed");
          chip.title = "已落盘：" + p.landed;
        }
        chip.appendChild(el("span", "dot"));
        var toggle = el("button", "ux-chip-name", (p.landed ? "✓ " : "") + p.id.toUpperCase() + " " + p.title);
        toggle.type = "button";
        toggle.addEventListener("click", function () { PV.toggle(p.id); syncBar(); if (currentInspection === p.id) renderInspector(p.id); });
        chip.appendChild(toggle);
        var info = el("button", "info", "ℹ");
        info.type = "button";
        info.setAttribute("aria-label", "查看 " + p.id + " 的检视证据与方案");
        info.addEventListener("click", function (ev) { ev.stopPropagation(); openInspector(p.id); });
        chip.appendChild(info);
        g.appendChild(chip);
      });
      rowChips.appendChild(g);
      rowChips.appendChild(el("div", "ux-sep"));
    });
    row2.appendChild(rowChips);

    bar.appendChild(row2);
    bar.appendChild(row3);
    bar.appendChild(row4);
    bar.appendChild(el("div", "ux-hint",
      "提示：命名总览＝所有界面平铺 + 悬停看名称/实现/属性；交互原型＝单窗口路由，侧栏、宫格、查看器、搜索、弹框全部可点。提案开关默认全关，全关且切回命名总览后与纯命名图完全一致。"));
  }

  function clearTokens() {
    PV.tokens().slice().forEach(function (id) { PV.setEnabled(id, false); });
  }

  function needScreen(screen, then) {
    if (body.dataset.pvMode !== "run") PV.setMode("run");
    if ((body.dataset.pvScreen || "photos") !== screen) {
      PV.go(screen);
    }
    if (then) setTimeout(then, 30);
  }

  /* ======================================================== 同步工具条 */

  var SCREEN_LABELS = {
    photos: "照片页", album: "相册详情", trash: "回收站", viewer: "查看器",
    search: "搜索", collapsed: "窄窗折叠态", settings: "设置", picker: "相册选择"
  };

  function syncBar() {
    if (!barRefs.modeNaming) return;
    var isRun = body.dataset.pvMode === "run";
    barRefs.modeNaming.classList.toggle("on", !isRun);
    barRefs.modeRun.classList.toggle("on", isRun);
    barRefs.locale.textContent = "语言 " + (body.dataset.pvLocale === "en" ? "中文" : "EN");
    barRefs.locale.classList.toggle("on", body.dataset.pvLocale === "en");
    barRefs.material.textContent = "材质 " + (body.dataset.pvMaterial === "plain" ? "Liquid" : "Plain");
    barRefs.material.classList.toggle("on", body.dataset.pvMaterial === "plain");
    barRefs.motion.classList.toggle("on", body.dataset.pvMotion === "off");
    barRefs.width.textContent = "窗口宽度 " + (body.dataset.pvWidth || "wide");
    barRefs.width.classList.toggle("on", (body.dataset.pvWidth || "wide") !== "wide");
    barRefs.scan.value = body.dataset.pvScan || "ready";
    if (String(barRefs.trans.value) !== String(body.dataset.pvTransparency || "0")) {
      barRefs.trans.value = body.dataset.pvTransparency || "0";
    }
    var demo = body.dataset.pvDemo || "";
    var v = verdicts();
    qa(".ux-chip", q("#ux-bar")).forEach(function (chip) {
      var id = chip.dataset.proposal;
      if (!id) return;
      chip.classList.toggle("on", PV.enabled(id));
      if (v[id] && v[id].v) chip.dataset.verdict = v[id].v; else delete chip.dataset.verdict;
    });
    var screen = body.dataset.pvScreen || "photos";
    barRefs.crumb.innerHTML = "";
    barRefs.crumb.appendChild(document.createTextNode("路径 "));
    var trail = crumbStack.slice(-3).concat([SCREEN_LABELS[screen] || screen]);
    var dedup = trail.filter(function (name, i) { return i === 0 || name !== trail[i - 1]; });
    dedup.forEach(function (name, i, arr) {
      barRefs.crumb.appendChild(el("b", null, name));
      if (i < arr.length - 1) barRefs.crumb.appendChild(document.createTextNode(" › "));
    });
    if (demo) barRefs.crumb.appendChild(el("span", null, " · 演示态 " + demo));
    qa(".ux-route", q("#ux-bar")).forEach(function (b) {
      b.classList.toggle("on", b.dataset.route === screen);
    });
    var on = PROPOSALS.filter(function (p) { return PV.enabled(p.id); }).length;
    barRefs.clear.textContent = on ? "清空提案（" + on + "）" : "清空提案";
  }

  /* 面包屑用本地记录近似：prototype.js 内部的路由栈不导出，
     这里只关心「用户怎么走到这一屏」的可读轨迹。 */
  var crumbStack = [];

  function verdicts() { return loadState().verdicts || {}; }
  function notes() { return loadState().notes || {}; }

  /* ============================================================ 抽屉 */

  var currentInspection = null;
  var lastInspectionTab = "detail";

  function ensureInspector() {
    var ins = q(".pv-inspector");
    if (ins) return ins;
    ins = el("aside", "pv-inspector pv-ins");
    ins.setAttribute("aria-label", "提案评审抽屉");
    ins.innerHTML =
      '<div class="pv-ins-head">' +
      '<span class="pv-ins-badge" data-role="badge">P0</span>' +
      "<strong data-role=\"title\">提案</strong>" +
      '<button class="ux-btn" data-role="prev" type="button" title="上一项">‹</button>' +
      '<button class="ux-btn" data-role="next" type="button" title="下一项">›</button>' +
      '<button class="ux-btn" data-role="close" type="button">关闭</button>' +
      "</div>" +
      '<div data-role="body"></div>';
    body.appendChild(ins);
    q('[data-role="close"]', ins).addEventListener("click", closeInspector);
    q('[data-role="prev"]', ins).addEventListener("click", function () { stepInspector(-1); });
    q('[data-role="next"]', ins).addEventListener("click", function () { stepInspector(1); });
    return ins;
  }

  function section(title, value) {
    var s = el("div", "pv-ins-section");
    s.appendChild(el("h5", null, title));
    if (Array.isArray(value)) {
      var ul = el("ul");
      value.forEach(function (item) {
        var li = el("li");
        // 只把 ASCII 写成「路径:行号」的片段包成 code，中文前缀留在文本里。
        var PATH = /[A-Za-z0-9_./-]+\.(?:rs|css|blp|json|md|py|sh)(?::\d[\d\-/,]*)?/g;
        var last = 0;
        var m;
        var hit = false;
        while ((m = PATH.exec(item)) !== null) {
          hit = true;
          if (m.index > last) li.appendChild(document.createTextNode(item.slice(last, m.index)));
          li.appendChild(el("code", null, m[0]));
          last = m.index + m[0].length;
        }
        if (hit) {
          if (last < item.length) li.appendChild(document.createTextNode(item.slice(last)));
        } else {
          li.textContent = item;
        }
        ul.appendChild(li);
      });
      s.appendChild(ul);
    } else {
      s.appendChild(el("p", null, value));
    }
    return s;
  }

  function renderInspector(id, tab) {
    var p = BY_ID[id];
    if (!p) return;
    currentInspection = id;
    lastInspectionTab = tab || "detail";
    var ins = ensureInspector();
    ins.classList.add("open");
    q('[data-role="badge"]', ins).textContent = p.prio + " · " + p.batch;
    q('[data-role="title"]', ins).textContent = p.id.toUpperCase() + " " + p.title;
    var host = q('[data-role="body"]', ins);
    host.innerHTML = "";

    host.appendChild(section("现象", p.problem));
    host.appendChild(section("证据（已验证）", p.evidence));
    host.appendChild(section("方案", p.solution));
    host.appendChild(section("落点文件", p.files));
    host.appendChild(section("测试", p.tests));
    host.appendChild(section("需同步文档", p.docs));
    if (p.risk) host.appendChild(section("风险 / 边界", p.risk));
    if (p.landed) host.appendChild(section("落地状态（已实施，含偏差）", p.landed));

    var demoSec = el("div", "pv-ins-section");
    demoSec.appendChild(el("h5", null, "在本页怎么验"));
    var demo = el("div", "pv-demo");
    demo.appendChild(el("b", null, (PV.enabled(p.id) ? "提案已开启 · " : "提案未开启（现在是现状）· ")));
    demo.appendChild(document.createTextNode(p.demo));
    var act = el("button", "ux-btn", PV.enabled(p.id) ? "关闭本提案对比现状" : "开启本提案");
    act.type = "button";
    act.style.marginLeft = "6px";
    act.addEventListener("click", function () { PV.toggle(p.id); renderInspector(p.id, lastInspectionTab); syncBar(); });
    demo.appendChild(act);
    var jump = el("button", "ux-btn", "跳到演示界面");
    jump.type = "button";
    jump.style.marginLeft = "6px";
    jump.addEventListener("click", function () { jumpToProposal(p.id); });
    demo.appendChild(jump);
    demoSec.appendChild(demo);
    host.appendChild(demoSec);

    /* 评审结论 */
    var vSec = el("div", "pv-ins-section");
    vSec.appendChild(el("h5", null, "评审结论"));
    var row = el("div", "pv-verdicts");
    var rec = verdicts()[id] || {};
    Object.keys(VERDICT_LABELS).forEach(function (key) {
      var b = el("button", "ux-btn", VERDICT_LABELS[key]);
      b.type = "button";
      b.dataset.v = key;
      b.setAttribute("aria-pressed", rec.v === key ? "true" : "false");
      b.addEventListener("click", function () {
        var all = verdicts();
        all[id] = { v: rec.v === key ? null : key, note: (all[id] || {}).note || "" };
        if (!all[id].v) delete all[id].v;
        saveState({ verdicts: all });
        renderInspector(id, lastInspectionTab);
        syncBar();
      });
      row.appendChild(b);
    });
    vSec.appendChild(row);

    var ta = el("textarea");
    ta.placeholder = "备注（例如：需先在 Flatpak 运行时目测 / 与 P1-11 同批做 / 需要先推翻 viewer.md:204）";
    ta.value = (notes()[id] || rec.note || "");
    ta.addEventListener("input", function () {
      var all = notes();
      all[id] = ta.value;
      saveState({ notes: all });
    });
    vSec.appendChild(ta);
    host.appendChild(vSec);

    /* 对比度报告 */
    if (id === "p1-13" || tab === "contrast") {
      var cSec = el("div", "pv-ins-section");
      cSec.appendChild(el("h5", null, "实测对比度（WCAG 比值）"));
      var hostTable = el("div", "pv-contrast-host");
      hostTable.appendChild(contrastTable());
      cSec.appendChild(hostTable);
      var re = el("button", "ux-btn", "重新测量");
      re.type = "button";
      re.addEventListener("click", function () {
        hostTable.innerHTML = "";
        hostTable.appendChild(contrastTable());
      });
      cSec.appendChild(re);
      cSec.appendChild(el("p", null,
        "只量当前真正渲染在屏幕上的节点，没渲染的目标标成「未渲染」而不是猜一个比值：切到对应界面后点「重新测量」。" +
        "应用侧的同一契约已落盘：src/ui/grid_css/tests/render.rs 的 functional_text_holds_a_contrast_floor_over_glass 渲染真实表面、按控件分配取中位色当背景，再量合成后的前景比值（文本 4.5:1、大号与图标 3:1），回退任一站点都会让 CI 变红。" +
        "先切「透明度」到 100 再测，最能看出差距；要看落盘前的淡字用工具条「落盘前：玻璃下文字过淡（P1-13）」。"));
      host.appendChild(cSec);
    }
  }

  function contrastTable() {
    var rows = PV.contrastReport() || [];
    var t = el("table", "pv-contrast-table");
    t.innerHTML = "<thead><tr><th>位置</th><th>界面</th><th>前景 α</th><th>比值</th><th>阈值</th><th>结论</th></tr></thead>";
    var tb = el("tbody");
    if (!rows.length) {
      var tr0 = el("tr");
      tr0.appendChild(el("td", null, "当前界面没有可测量的目标元素，切到照片页或设置页后再测。"));
      tb.appendChild(tr0);
    }
    rows.forEach(function (r) {
      var tr = el("tr");
      tr.appendChild(el("td", null, r.label));
      tr.appendChild(el("td", null, r.screen || "-"));
      if (r.skipped) {
        /* 这个目标在当前界面上没渲染出来：不猜比值，让评审自己切界面重测 */
        tr.appendChild(el("td", "num", "-"));
        tr.appendChild(el("td", "num", "-"));
        tr.appendChild(el("td", "num", r.target.toFixed(1)));
        tr.appendChild(el("td", "pv-warn", "未渲染，切到对应界面后「重新测量」"));
        tb.appendChild(tr);
        return;
      }
      tr.appendChild(el("td", "num", String(r.alpha)));
      tr.appendChild(el("td", "num", r.ratio.toFixed(2) + ":1"));
      tr.appendChild(el("td", "num", r.target.toFixed(1)));
      tr.appendChild(el("td", r.pass ? "pv-ok" : "pv-bad", r.pass ? "通过" : "不足"));
      tb.appendChild(tr);
    });
    t.appendChild(tb);
    return t;
  }

  function openInspector(id, tab) {
    if (!BY_ID[id]) return;
    renderInspector(id, tab);
  }
  function closeInspector() {
    var ins = q(".pv-inspector");
    if (ins) ins.classList.remove("open");
    currentInspection = null;
  }
  function stepInspector(delta) {
    var ids = PROPOSALS.map(function (p) { return p.id; });
    var i = ids.indexOf(currentInspection);
    if (i < 0) i = 0;
    var next = ids[(i + delta + ids.length) % ids.length];
    if (delta && !PV.enabled(next)) PV.setEnabled(next, true);
    jumpToProposal(next);
    renderInspector(next, lastInspectionTab);
    syncBar();
  }

  /* P1-6 演示走三步：普通计数 → 全选 → 命中 2000 上限的文案变体。
     静态页只有 18 张样图，上限变体只能显式模拟，不能让「全选」谎报已达上限。 */
  var p16Step = 0;
  function demoP16() {
    needScreen("photos");
    var tiles = qa(".screen[data-screen='photos'] .tile[data-media]");
    p16Step = (p16Step + 1) % 4;
    body.dataset.pvDemo = "";
    if (p16Step === 0) {
      PV.setMulti(false);
      PV.toast("P1-6 演示结束：退出多选并清空选择", { kind: "info" });
      return;
    }
    PV.setMulti(false);
    PV.setMulti(true);
    if (p16Step === 1) {
      tiles.slice(0, 3).forEach(function (t) { PV.toggleTile(t); });
      PV.toast("P1-6 1/3：选 3 项 → 顶栏出现「已选择 3 项」", { kind: "info" });
    } else if (p16Step === 2) {
      PV.selectAll();
      PV.toast("P1-6 2/3：全选 → 计数跟随；真实上限 2000，这里不应显示已达上限", { kind: "info" });
    } else {
      PV.selectAll();
      if (PV.selectedIds().length === 0) PV.selectAll();
      body.dataset.pvDemo = "selection-limit";
      PV.updateCount();
      PV.toast("P1-6 3/3：模拟命中 2000 上限 → 文案追加「（已达上限）」", { kind: "info" });
    }
  }

  /* P2-6 演示：报错 / 加载中 / 相册为空三种状态轮着看，每步都对照现状与提案。 */
  var p26Step = 0;
  var P26_STATES = [
    { state: "", screen: "picker", name: "正常（相册宫格）", note: "读到相册就显示封面网格；这是 Gtk.Stack 的 albums 页。" },
    { state: "picker-loading", screen: "picker", name: "加载中", note: "已落盘：弹框一打开就停在 empty.loading ＋一个已经 start() 的 spinner，不再是一片空网格。" },
    { state: "picker-empty", screen: "picker", name: "真的没有相册", note: "已落盘：只有查询成功且结果为空才说「还没有相册」，标题与描述各归各位。" },
    { state: "picker-error", screen: "picker", name: "读取失败", note: "已落盘：empty.load_failed ＋数据库真实原因 ＋「重试」重跑同一条列表查询。" },
    { state: "picker-error-as-empty", screen: "picker", name: "落盘前对照", note: "报错与空态共用同一句话，既没有原因也没有出口——用户会以为相册丢了。" },
    { state: "album-empty", screen: "album", name: "空相册详情页", note: "相册详情页一直用的就是专用空态 empty.no_album_photos.*，草案说的「复用全库文案」不成立。" }
  ];
  function demoP26() {
    p26Step = (p26Step + 1) % P26_STATES.length;
    var step = P26_STATES[p26Step];
    body.dataset.pvDemo = step.state;
    needScreen(step.screen);
    syncBar();
    PV.toast("P2-6 " + (p26Step + 1) + "/" + P26_STATES.length + " · " + step.name + "：" + step.note,
      { kind: "info", ms: 4600 });
  }

  function jumpToProposal(id) {
    var p = BY_ID[id];
    if (!p) return;
    var screen = (p.screens || ["photos"])[0];
    var map = {
      "p0-1": function () { needScreen("photos"); PV.setScan("scanning"); },
      "p0-2": function () { needScreen("photos"); },
      "p0-3": function () { needScreen("photos"); },
      "p0-4": function () { needScreen("settings", function () { PV.openShortcuts(true); }); },
      "p0-5": function () { needScreen("search"); },
      "p1-6": demoP16,
      "p1-7": function () { openViewerDemo(); },
      "p1-8": function () { openViewerDemo(); PV.revealStageHint(true); },
      "p1-9": function () { needScreen("viewer", function () { PV.toggleEditor(); PV.setEditorDirty(); }); },
      "p1-10": function () { needScreen("photos"); },
      "p1-11": function () { needScreen("photos"); PV.setMulti(true); },
      "p1-12": function () { openViewerDemo(); },
      "p1-13": function () { needScreen("photos"); },
      "p1-14": function () {
        needScreen("photos");
        PV.toast("P1-14 已落盘：应用直接读桌面「减少动画」设置。原型里代表这个系统开关的是工具条「减少动画」按钮，切下去全部过渡归零。", { kind: "info", ms: 5200 });
      },
      "p1-15": function () {
        needScreen("search");
        var input = q("#search-input");
        if (input && !input.value) input.value = "IMG";
        PV.runSearch(input ? input.value : "IMG");
      },
      "p2-1": function () { needScreen("photos"); PV.setMulti(true); PV.runAction("batch-fav"); },
      "p2-2": function () { openViewerDemo(); setTimeout(function () { PV.toast("P2-2：提示让开底部胶片条", { kind: "info" }); }, 220); },
      "p2-3": function () {
        needScreen("photos");
        var bar = q(".screen[data-screen='photos'] .mode-selector");
        if (bar) bar.focus();
        PV.toast("P2-3 已落盘：胶囊是唯一 tab stop，焦点环在 capsule 外沿；左右方向键切换并环绕，读屏听到的是 radio group + 三个 radio（打开浏览器无障碍树可核对）。", { kind: "info", ms: 5200 });
      },
      "p2-4": function () {
        needScreen("photos");
        var tile = q(".screen[data-screen='photos'] .tile[data-media]");
        if (tile) {
          var face = PV.q(".pv-a11y-face", tile);
          PV.toast("P2-4 已落盘：这张格子读屏念作「" + (face ? face.getAttribute("aria-label") : "(无名)") + "」；角标与云状态各报自己的名字，对勾是装饰。浏览器无障碍树可核对，落盘前形态见工具条演示按钮。", { kind: "info", ms: 5200 });
        }
      },
      "p2-5": function () { needScreen("photos"); },
      "p2-6": demoP26,
      "p2-7": function () {
        needScreen("trash");
        // 落盘前：先渲染「回收站为空」，等首轮列表回来再顶掉，肉眼看到的是闪一下。
        body.dataset.pvDemo = "trash-flash";
        syncBar();
        setTimeout(function () {
          body.dataset.pvDemo = "";
          syncBar();
          PV.toast("P2-7 已落盘：常态停在 loading，读取完成才决定出图还是出空态；刚才那 0.9 秒是修复前的一闪。", { kind: "info", ms: 4200 });
        }, 900);
      },
      "p2-8": function () {
        needScreen("photos");
        // 已落盘，所以芯片展示常态而不是打开提案开关：展开概览、把同步切到 Failed，
        // 让「chevron 是真的可点 + 失败才有重试」一次演完，再收起回到常态。
        PV.setOverview("failed");
        syncBar();
        var chevron = PV.q(".overview-disclosure");
        PV.toast("P2-8 已落盘：概览现在有 header 那枚圆钮（点它开合，glyph 与读屏名字随状态翻），同步失败时句子末尾多出「重试」。演示里 1.6 秒后收回折叠常态。", { kind: "info", ms: 5200 });
        setTimeout(function () {
          if (chevron) chevron.click();
          syncBar();
        }, 1600);
      },
      "p2-9": function () {
        needScreen("photos");
        PV.setMulti(true);
        // 已落盘：三种状态轮一遍，让「点之前就知道」这件事自己演出来。
        var states = ["none", "all", "mixed"];
        var i = 0;
        PV.setFavoriteState(states[0]);
        var timer = setInterval(function () {
          i += 1;
          if (i >= states.length) { clearInterval(timer); return; }
          PV.setFavoriteState(states[i]);
          PV.toast("心形状态：" + { none: "全未收藏 → 点击直接收藏", all: "全已收藏 → 点击直接取消（红心）", mixed: "混合 → 点击弹出菜单，右下角那枚 ⌄ 说的就是它" }[states[i]], { kind: "info", ms: 2200 });
        }, 2400);
      }
    };
    if (map[id]) map[id]();
    else needScreen(screen);
    /* 命名总览模式下把对应界面滚到视口内 */
    setTimeout(function () {
      var sel = ".screen[data-screen='" + screen + "'] .pv-p" + id.replace("-", "-") + "";
      var node = q(sel) || q(".screen[data-screen='" + screen + "']");
      if (body.dataset.pvMode !== "run" && node && node.scrollIntoView) node.scrollIntoView({ block: "center" });
    }, 60);
  }

  function openViewerDemo() {
    needScreen("photos", function () {
      var tiles = qa(".screen[data-screen='photos'] .tile[data-media]");
      if (tiles.length) PV.openViewer(tiles, 4);
    });
  }

  /* ============================================================ 导出 */

  function exportMarkdown() {
    var v = verdicts();
    var n = notes();
    var lines = [];
    lines.push("# PhotoViewer UI 提案评审结论");
    lines.push("");
    lines.push("导出于 " + new Date().toLocaleString("zh-CN") + "。来源评审页：docs/ui-naming-reference/index.html；方案细节见 docs/ux-improvement-backlog.md。");
    lines.push("");
    lines.push("| 提案 | 标题 | 优先级 | 批次 | 结论 | 备注 |");
    lines.push("|---|---|---|---|---|---|");
    var counted = {};
    PROPOSALS.forEach(function (p) {
      var rec = v[p.id] || {};
      var verdict = rec.v ? VERDICT_LABELS[rec.v] : "未评审";
      var note = (n[p.id] || rec.note || "").replace(/\|/g, "\\|").replace(/\n/g, " ");
      lines.push("| " + p.id.toUpperCase() + " | " + p.title + (p.landed ? "（已实施）" : "") + " | " + p.prio + " | " + p.batch + " | " + verdict + " | " + note + " |");
      if (rec.v) counted[rec.v] = (counted[rec.v] || 0) + 1;
    });
    lines.push("");
    lines.push("统计：采纳 " + (counted.accept || 0) + " · 暂缓 " + (counted.hold || 0) + " · 否决 " + (counted.reject || 0) +
      " · 未评审 " + (PROPOSALS.length - (counted.accept || 0) - (counted.hold || 0) - (counted.reject || 0)) + "。");
    lines.push("");
    lines.push("## 建议实施顺序（按采纳项归入批次）");
    lines.push("");
    Object.keys(BATCHES).forEach(function (key) {
      var b = BATCHES[key];
      var acc = b.ids.filter(function (id) { return v[id] && v[id].v === "accept"; });
      if (!acc.length) return;
      lines.push("- " + b.name + "：" + acc.map(function (id) { return id.toUpperCase(); }).join("、"));
    });
    lines.push("");
    lines.push("## 采纳项的验证要求");
    lines.push("");
    lines.push("- 未提交期间只跑新增/直接被改的窄测试；每条 commit message 带 `Tests:` 段（PASS/FAIL/NOT RUN+原因）。");
    lines.push("- 推送前门禁跑 `.github/workflows/ci.yml` 等价全套：fmt / clippy / build --all-targets / tools/with-at-spi.sh xvfb-run -a cargo test --locked --all。");
    lines.push("- 触碰 CSS 材质或对比度的批次（B1、B6、B8、B9）需在 Flatpak GNOME 运行时目测。");
    lines.push("- 新增/改名 UI 控件都要同步本页命名图与 docs/modules/*.md。");

    var text = lines.join("\n");
    var stamp = new Date().toISOString().slice(0, 10);
    var name = "ui-review-" + stamp + ".md";

    /* 优先下载；被浏览器拦截时退回可复制文本框。 */
    try {
      var blob = new Blob([text], { type: "text/markdown;charset=utf-8" });
      var url = URL.createObjectURL(blob);
      var a = el("a");
      a.href = url;
      a.download = name;
      body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(function () { URL.revokeObjectURL(url); }, 4000);
      PV.toast("已导出 " + name, { kind: "success" });
    } catch (err) {
      var win = window.open("", "_blank");
      if (win) {
        win.document.title = name;
        var pre = win.document.createElement("pre");
        pre.textContent = text;
        pre.style.cssText = "white-space:pre-wrap;font:12px/1.6 ui-monospace,monospace;padding:16px";
        win.document.body.appendChild(pre);
      }
      PV.toast("下载被拦截，已在新窗口给出可复制文本", { kind: "info" });
    }
    return text;
  }

  /* ==================================================== 面包屑 / 事件 */

  function trackCrumb() {
    crumbStack = [];
    PV.on("screen", function (screen) {
      if (body.dataset.pvMode !== "run") return;
      var label = SCREEN_LABELS[screen] || screen;
      if (crumbStack[crumbStack.length - 1] !== label) crumbStack.push(label);
      if (crumbStack.length > 6) crumbStack.shift();
    });
  }

  /* ============================================================ 启动 */

  function applyHash() {
    var h = readHash();
    if (!Object.keys(h).length) return false;
    if (h.pv) {
      clearTokens();
      h.pv.split(",").filter(Boolean).forEach(function (id) { if (BY_ID[id]) PV.setEnabled(id, true); });
    }
    // 顺序要紧：先定模式，再定界面，否则 needScreen 会把 naming 深链强制拉到 run。
    if (h.mode) PV.setMode(h.mode);
    if (h.screen && (h.mode || body.dataset.pvMode) === "run") PV.go(h.screen);
    else if (h.screen) body.dataset.pvScreen = h.screen;
    if (h.scan) PV.setScan(h.scan);
    if (h.locale) PV.setLocale(h.locale);
    if (h.material) PV.setMaterial(h.material);
    if (h.t) PV.setTransparency(h.t);
    if (h.motion === "off") PV.setMotion(true);
    if (h.width) PV.setWidth(h.width);
    if (h.demo) body.dataset.pvDemo = h.demo;
    return true;
  }

  function applyStored() {
    var s = loadState();
    if (s.tokens && s.tokens.length) s.tokens.forEach(function (id) { if (BY_ID[id]) PV.setEnabled(id, true); });
    if (s.mode === "run") PV.setMode("run");
    if (s.locale) PV.setLocale(s.locale);
    if (s.material) PV.setMaterial(s.material);
    if (s.transparency) PV.setTransparency(s.transparency);
    if (s.motion === "off") PV.setMotion(true);
    if (s.width) PV.setWidth(s.width);
  }

  function boot() {
    buildBar();
    trackCrumb();
    ["proposal", "proposalChange", "screen", "routes", "multi", "selection", "scan",
      "mode", "width", "locale", "transparency", "motionUI", "materialUI"].forEach(function (evt) {
      PV.on(evt, function () {
        syncBar();
        writeHash();
        var s = loadState();
        saveState({
          tokens: PV.tokens(),
          mode: body.dataset.pvMode,
          locale: body.dataset.pvLocale,
          material: body.dataset.pvMaterial,
          transparency: body.dataset.pvTransparency,
          motion: body.dataset.pvMotion,
          width: body.dataset.pvWidth,
          verdicts: s.verdicts || {},
          notes: s.notes || {}
        });
      });
    });

    document.addEventListener("keydown", function (ev) {
      if (ev.key === "Escape" && q(".pv-inspector.open")) { closeInspector(); ev.stopPropagation(); }
    }, true);

    window.addEventListener("hashchange", function () {
      applyHash();
      syncBar();
    });

    if (!applyHash()) applyStored();
    syncBar();
    writeHash();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }

  window.PVReview = {
    proposals: PROPOSALS,
    batches: BATCHES,
    open: openInspector,
    close: closeInspector,
    jump: jumpToProposal,
    exportMarkdown: exportMarkdown,
    contrastReport: function () { return PV.contrastReport(); }
  };
})();
