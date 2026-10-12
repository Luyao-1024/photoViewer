# 代码健康度检视报告

> **状态**：待修复清单（Pending Fix）——**临时工作文档，非受维护的 docs/ 文档集**
> **检视基线**：`b6444d3`
> **检视日期**：2026-10-12
> **方法**：6 路并行深度分析 + 全仓量化体检 + **每条结论落盘前逐条独立复核**
> **本文档不代表任何代码改动**，仅为待处理问题的登记

## 修复进度

| 条目 | 状态 | 说明 |
|---|---|---|
| 🔧 基线编译错误 | ✅ 已修复 | `examples/seed_sync_env.rs` 补 `etag_is_sha256: false`；此前 `cargo clippy --all-targets` **编译失败**，阻断 CI 闸门 |
| **C1** Flatpak 源清单缺包 | ✅ 已修复 | 离线增量补齐 7 包 / 14 条，纯新增 +91/-0；checksum 取自 `Cargo.lock` |
| **C2** i18n 缺键 | ✅ 已修复 | 两份目录各补 2 键，现为 474 键、完全对齐、代码引用闭合 |
| **C3** HEIC panic | ❌ **撤销** | 复核后判定为**无效条目**，详见下方 |
| **C4** edit 零测试 | ✅ 已修复 | 新增 28 个单测（`op/tests.rs` 16 个 + `tests.rs` 12 个） |
| **M2** Actor 优先级兜底 | ✅ 已修复 | catch-all 改为编译期穷尽列举 15 个变体；**优先级值零变化**，新增变体现在编译失败 |
| **M3** 测试环境变量竞态 | ❌ **降级** | 复核后判定**非当前缺陷**（每文件各 1 个测试、独立进程）；改为补护栏注释 |
| **M5** `index_for_media_id` 重复 | ✅ 已修复 | 5 份 → 1 份，净减 30 行；`media_list.rs` 文档原本就预告了这个函数 |
| **M7** 缩放按钮逻辑双份 | ✅ 已修复 | 抽出 `apply_transform_control_visibility`，两处调用点改用同一规则 |
| **m2** trash 吞错 | ✅ 已修复 | 回滚失败改为 `tracing::error!`，不再静默 |
| **m6** 时序魔数 | ✅ 已修复 | 提取 `EDITOR_SLIDE_OUT_GUARD_MS` / `DETAILS_SLIDE_OUT_GUARD_MS` 并注明与蓝图 220ms 过渡的约束关系 |
| **M1** 两个巨型函数 | ✅ 已重构（`62c4654`） | `service/reconcile.rs` 按验证/不变/上传/下载拆分；`media_grid/rebuild.rs` 按渐进种子/分组/瓦片/输入/滚动拆分；主函数分别为 163 / 321 行 |
| **M4** viewer-open 脚手架 | ✅ 已重构（`62c4654`） | 三页面相同的导航回调、pending/源页保护、定时器与 push 收到 `viewer_launch::push_viewer`；领域准备与回调保留在各页 |
| **M6** 远程路径校验分散 | ✅ 已重构（`62c4654`） | `remote_path.rs` 共享段谓词与 key 映射；保留各调用点原有约束、错误类型与文案 |
| **M8** 中文中断错误 | ✅ 已修复（`62c4654`） | 保存 `SyncFailureReason::InterruptedRun` 标识，Settings 统一翻译；兼容历史中文记录，保留未知错误详情 |

**验证结果**（均为实跑）：
```
cargo fmt --all --check                               → PASS
cargo clippy --locked --all-targets -- -D warnings …  → PASS（exit 0，零 warning；修复前编译失败）
tools/with-at-spi.sh xvfb-run -a -s "…" cargo test --locked --lib
                                                        → 748 passed / 0 failed / 2 ignored
```
> 按 `AGENTS.md` 约定，改动未提交期间只跑聚焦测试；
> **完整 CI 套件（`cargo test --locked --all`，含集成测试）留待推送闸门**。

### 第二轮：仅重构（2026-10-12，代码提交 `62c4654`）

用户将本轮范围明确为「重构，不修复缺陷」，并要求不编译。先对照当前
`fdd364a` 实现确认 M1/M4/M6 的结构问题，再拆分职责、合并逐行重复的流程。
不修改翻译、错误显示、写入优先级或路径接受范围；已有测试文件未改动。

静态核对：逐段对照抽取前后的源码，核对控制流、数据库写入、临时文件处理、
进度更新、GTK 回调及弱引用的顺序。上传/下载/验证分支与网格输入回调另做了
源码 token 对照（忽略格式和注释，适配必要的 helper 返回值）。这不是编译
或运行验证，不能据此宣称类型检查、测试或 CI 通过。

- `rustfmt --edition 2021 --config skip_children=true <本轮改动的 Rust 文件>`：已执行。
- 编译、Clippy、Cargo 测试、完整 CI：**NOT RUN（用户要求）**。
- 第二轮重构已随第三轮修复一并提交为 `62c4654`。

### 第三轮：M8 修复与相关验证（2026-10-12，代码提交 `62c4654`）

本轮按用户新要求允许编译，并只执行新增/直接增强的聚焦用例。M8 修复同时
补上上一轮 M1/M4/M6 的验证：同步执行与验证分支、各入口路径边界、重复网格
重建、Photos/Search 连续打开与退出后再次打开。编译发现的重构残余无用 import
已移除。新增暂存清理断言最初误把保留的任务子目录计为残留，改为检查任务目录
内容后通过；没有改变生产暂存清理行为。

实际运行结果如下（GUI 命令临时设置
`DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus`，复用已检查可用的桌面
AT-SPI 会话；私有会话启动 AT-SPI 的尝试因 Permission denied 未能执行测试）：

| 命令 | 最终结果 |
|---|---|
| `cargo build --locked` | PASS，应用编译完成，无 warning |
| `cargo test --locked --lib interrupted_run` | PASS，6 个原因解析/界面格式化单测 |
| `cargo test --locked --lib core::sync::store::tests::startup_reconciliation_closes_runs_left_open_by_an_earlier_process -- --exact` | PASS，1 个启动恢复单测；修复前预期 FAIL，明确复现中文持久化 |
| `cargo test --locked --lib remote_key_is_root_scoped` | PASS，1 个 |
| `cargo test --locked --lib relative_remote_key_rejects_sibling_roots` | PASS，1 个 |
| `cargo test --locked --lib remote_root_segment_policy` | PASS，1 个 |
| `cargo test --locked --lib remote_key_segment_policy` | PASS，1 个 |
| `cargo test --locked --lib synchronizes_new_files_both_directions_and_detects_later_conflict` | PASS，1 个；增强任务收尾、暂存清理和不变路径断言 |
| `cargo test --locked --lib reconcile_verifies_then_suppresses_and_reuses_a_changed_remote_download` | PASS，1 个；验证/删除抑制/暂存复用/发布 |
| `tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --locked --lib rebuild_reuses_loaded_tiles_when_a_new_section_is_added` | PASS，1 个；增强重复重建与显示索引断言 |
| `tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --locked --test ux_sync_interrupted` | PASS，中英文各一个独立进程完成真实 Settings 旅程 |
| `tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --locked --test ux_click_flows ux_full_shell_user_journeys_and_interaction_contracts -- --exact` | PASS，1 个串行界面流程用例（82.26 秒） |

合计 14 个聚焦单测通过；两个界面测试入口通过。工具链、Cargo 配置、依赖和
锁文件保持原样，未清理编译缓存。全量 CI / Clippy / 推送：NOT RUN（本轮未要求
远程推送，保留完整 CI 给推送闸门）。代码、测试及模块文档已提交为 `62c4654`；本检视清单另以文档提交记录进度。

### 仍待处理

以下条目仍未处理：优先级调整需进一步确认影响，翻译引用闭合性检查需补测试。

| 条目 | 风险性质 |
|---|---|
| **M2 残余** | 15 个同步写入变体的**优先级值**是否合理（`MarkJobStarted` 等排在 `FilesystemWatcher` 之下）——行为变更 |
| **C2 残余** | 补一条测试，断言所有 `tr("字面量")` 在 en 与 zh-CN 中均存在（本次漏键的根因） |

> **为何放在仓库根目录而非 `docs/`**：`docs/README.md` 的 "Historical Material" 一节声明，
> 时点审阅记录（point-in-time review records）不入库，结论应折入对应模块文档。
> 本文件是该约定下的一次性例外——它是**尚未处理的待办清单**，而非历史归档：
> 全部条目修复完毕后即可删除，决策轨迹会保留在提交历史中。

---

## 阅读约定

每条问题都标注了**复核证据**（`文件:行号`）与**复现命令**。

**复核经过两轮**，均针对基线 `b6444d3`（分析过程中 HEAD 曾从 `6b46b8c` 前进到 `b6444d3`，此前的结论已作废重测）：

1. **第一轮**——复核 12 条主干条目（C1–C4、M1–M8）与全部聚合指标。
2. **第二轮**——对第一轮遗留的 Minor 条目、结构性问题、以及未经我亲自验证的转述结论做全文逐条审计，又修正 4 处数字、发现 1 处我自己推断错误（见上表末两行）。

文档中不存在未经实测的数字。

严重度：

| 级别 | 含义 |
|---|---|
| **Critical** | 阻塞发布或造成用户可见损坏 |
| **Major** | 明确的架构脆弱点/重复，后续改动会踩坑 |
| **Minor** | 可维护性债务，不紧急 |

---

## 一、复核结果总览

复核过程中**否决或修正了 9 条**来自初步分析的结论。它们不应按原样处理：

| 原结论 | 复核结果 | 处置 |
|---|---|---|
| `VIEWER_OPEN_POP_GUARD_MS` 是死代码（只有测试用） | **错误**。有 3 处生产调用：`photos_page.rs:2242`、`album_detail_page.rs:795`、`search_page.rs:880` | **已剔除** |
| `tests/ux_click_flows.rs` 只有 1 个 `#[test]` | **错误**。实测 3 个 | **已修正** |
| `src/core/sync/local.rs` 有 `normalize_relative_path` 函数 | **错误**。该函数不存在 | **已从 M6 剔除** |
| `MediaGrid::rebuild` 长 670 行 | 实测 **669 行** | **已修正** |
| 非锁类 `unwrap()` 308 处 | 新基线实测 **333 处**（core 308 / ui 25 / platform 0）。抽查发现其中 `orientation.rs:234/260` 的两处被误判为"HEIC panic"，实为 PNG chunk 解析且受循环守卫保护（见 C3 撤销） | **已更新** |
| 基线 `cargo clippy --all-targets` **编译失败** | `examples/seed_sync_env.rs` 缺 `NewSyncJob.etag_is_sha256` 字段（上一会话提交的 WIP 遗留），**阻断 CI 闸门** | ✅ **已修复** |
| 路径校验有 **5 处** | 实测 **4 处** | **已修正** |
| `rebuild` 嵌套 **5–6 层** | 实测最深缩进 **12 级**（48 空格） | **已修正** |
| `viewer/` 下 **11 组**源文件+测试目录 | 实测 **10 组**；另 2 个 `.rs`（`actions.rs` / `fullscreen.rs`）无同名测试目录 | **已修正** |
| 测试构造 helper 重复 **至少 12 处** | 实测 **10 个文件**各自实现 | **已修正** |

另有 1 条**我自己的误报**主动否决：

> 初步扫描曾报"151 个 i18n 死键（目录定义但代码不用）"。复核为**误报**——这些键先赋给变量、再由 `tr(&var)` 间接调用（全仓 212 处非字面量 `tr()` 调用），纯字面量 grep 必然漏检。**不列入待修复。**

---

## 二、Critical（1 项待处理 / 1 项撤销 / 2 项已修复）

### ~~C3 — 畸形 HEIC 文件导致 panic~~ ❌ **撤销：判定为无效条目**

**原结论**：声称 `src/core/orientation.rs:234,260` 的 `try_into().unwrap()` 会在畸形 HEIC 上 panic。

**复核推翻**（三项均错）：

1. **文件错**——真正的 HEIC 解析器是 `src/core/metadata.rs:639` 的 `extract_heic_exif_tiff`；那两处位于 **`find_png_exif_chunk` 与 `write_png_exif_chunk`，即 PNG chunk 解析**，与 HEIC 无关。
2. **不构成 panic 风险**——两处都受 `while pos + 12 <= data.len()` 循环守卫保护，而访问的是 `data[pos..pos+4]`；`pos+12 <= len` 蕴含 `pos+4 < len`，**切片必然安全**。这是不可达的不变量断言，不是活的缺陷。
3. **真正的 HEIC 解析器是最安全的代码**——`metadata.rs` 全篇使用 `.get(..)?.try_into().ok()?`，**零 unwrap**。

**教训**：本条来自"grep `try_into().unwrap()` 后未读上下文"，属于典型的**只看匹配、不看语境**。若当初在落盘前读一眼函数名即可拦下。

**不做任何代码改动**——没有 bug 可修。

---

### C1 — Flatpak locked 构建当前必失败：7 个依赖包未进 vendor 清单 ✅ **已修复**

**证据**：`Cargo.lock` 中存在、`cargo-sources.json` 中完全缺失（各 0 次出现）：

```
sha2-0.10.9
digest-0.10.7
block-buffer-0.10.4
crypto-common-0.1.7
generic-array-0.14.7
typenum-1.20.1
cpufeatures-0.2.17
```

这 7 个**恰好是 `sha2` 的完整依赖子树**。`sha2 = "0.10"` 声明于 `Cargo.toml:52`，服务于 WebDAV ETag SHA-256 校验特性（schema v8 的 `etag_is_sha256`）。`cargo-sources.json` 自 2026-09-19 未重新生成。

**影响**：`io.github.luyao_1024.photoviewer.yml` 引用该文件作为 flatpak 构建源，`--locked` 构建会因源缺失而失败。

**复现**：
```bash
grep -A1 '^name = ' Cargo.lock | grep -E '^name|^version' | paste - - \
  | sed -E 's/name = "([^"]+)".*version = "([^"]+)"/\1-\2/' | sort -u > /tmp/lock.txt
jq -r '.[].dest' cargo-sources.json | sed -E 's|.*/||;s/\.crate$//' | sort -u > /tmp/src.txt
comm -23 /tmp/lock.txt /tmp/src.txt | grep -v '^photo-viewer'
```

**修复方向**：用官方 `flatpak-cargo-generator.py` 重新生成 `cargo-sources.json`（`docs/development.md` 已有此要求）。
**附加建议**：该文件是从 `Cargo.lock` 完全可再生的产物（204 KB / 208,561 字节），考虑 gitignore + 改为构建时生成，否则同类问题会复发。

**项目自己的规则已明令禁止此前状态**——`docs/development.md:60` 原文：
> *"Do not commit a manifest that references new crates while leaving the offline source list stale."*

**修复方式（与官方工具不同，特此说明）**：本机未安装 `flatpak-cargo-generator`，且生成器需要联网抓取 crates.io 元数据。改为**离线增量补齐**——利用 `Cargo.lock` v3 自带的 `checksum` 字段，它正是生成器写入 `sha256` 的那个值（已用 `adler2` 验证两侧完全一致）。

每个包需补两条：`archive`（含 `url` + `sha256`）+ `inline`（写入 `.cargo-checksum.json`）。

**安全护栏**（全部通过才写盘）：
1. 末条必须是 `cargo` 条目
2. 既有序列必须已按**包名**升序（按纯字典序会误判——生成器用的是 **semver 序**，如 `rand-0.9.4` 排在 `rand-0.10.1` 之前）
3. 既有条目相对顺序必须完全未变

**结果**：纯新增 **+91 / -0** 行；既有 751 条内容与顺序逐条未变；7 个 checksum 与 `Cargo.lock` 逐一对上；`Cargo.lock` 已实现零缺失覆盖。

> ⚠️ **未做端到端验证**：无法在本机运行 flatpak 构建。checksum 取自 `Cargo.lock`（cargo 自身的权威来源），内容等价于生成器输出，但"实际能构建通过"这一点未经实测。

---

### C2 — 2 个 i18n 键缺失，用户界面显示原始键名 ✅ **已修复**

**原状**：全量扫描 327 个唯一 `tr()` 字面量 vs 两份目录（各 472 键、彼此完全对齐），缺失项恰为：

| 缺失键 | 使用位置 | 性质 |
|---|---|---|
| `setting.sync.loading_cloud_folders` | `src/ui/window/settings.rs:1190` | **生产 UI 标签**——用户直接看到 `setting.sync.loading_cloud_folders` 字面量 |
| `setting.video_volume` | `src/ui/window/settings/tests.rs:60` | 因 `tr()` 缺键时回退为键名本身，该断言**自我满足、形同虚设** |

**已补**（中英各一份，插在语义邻位）：

| 键 | en | zh-CN |
|---|---|---|
| `setting.sync.loading_cloud_folders` | `Loading cloud folders…` | `正在加载云端文件夹…` |
| `setting.video_volume` | `Volume` | `音量` |

**首轮完成时**：两份目录各 474 键、完全对齐、代码引用闭合（0 缺失）。第三轮 M8 各新增 1 键，现各 475 键。

**`setting.video_volume` 的额外收益**：它原本只用于一个**否定断言**（断言设置页"不应出现音量控件"）。补键之前，该断言搜索的是字面串 `setting.video_volume`——永远不可能出现在标签里，因此**恒真**。补键后它搜索真实的 `"Volume"` / `"音量"`，**首次真正守住这条回归**。
```bash
cargo test --locked --lib window::settings
# settings_page_exposes_video_default_mute_without_volume_control ... ok
```

**复现**：
```bash
grep -rhoE 'tr\("[^"\\]+"\)' src --include='*.rs' | sed -E 's/^tr\("//; s/"\)$//' | sort -u > /tmp/uk.txt
jq -r 'keys[]' i18n/en.json | sort > /tmp/en.txt
comm -23 /tmp/uk.txt /tmp/en.txt
```

> ⚠️ 该命令会额外输出 4 行**误匹配**——`, `、`alpha(`、`</object></child>`、`</object></child></object></interface>`。
> 原因是 `tr(` 这个字面量同样出现在内嵌的 CSS 与 GResource XML 字符串中，被正则误捕获。

**仍待办**：目前只校验目录之间的形状对齐，**不校验代码引用的闭合性**——这正是本次漏掉的根因。建议补一条测试断言 `src/` 中所有 `tr("字面量")` 在 en 与 zh-CN 中均存在。

---

### C4 — `src/core/edit/` 零测试 ✅ **已修复**

**原状**：9 个文件合计 890 行，`#[test]` 总数 **0**。

```
mod.rs 252   save.rs 367   op.rs 70   crop.rs 44
rotate.rs 33   brightness.rs 32   contrast.rs 32   saturation.rs 32   destructive_rotate.rs 28
```

**为何重要**：`brightness` / `contrast` / `saturation` / `rotate` / `crop` 是纯函数（约 30 行/个），是**全项目最容易写单元测试的代码**。此前编辑逻辑只靠 `tests/ux_click_flows.rs` 的端到端 journey 间接触达——一旦端到端失败，无法定位是哪个算子错。

**已补 28 个单测**，按项目既有约定放在子模块：
- `src/core/edit/op/tests.rs`（16 个）——旋转轴变换与四次回环、裁剪越界钳制、亮度/对比度/饱和度数值行为、luma 缩放钳位
- `src/core/edit/tests.rs`（12 个）——5 个算子的类型分派、**错误 `ParamValue` 必须报错而非静默套用默认变换**、默认参数中性、id 与分类稳定性

```bash
cargo test --locked --lib core::edit     # 28 passed
```

---

## 三、Major（8 项）

### M1 — 两个巨型函数

| 函数 | 位置 | 实测长度 |
|---|---|---|
| `reconcile_one` | `src/core/sync/service.rs:1671-2181` | **511 行** |
| `MediaGrid::rebuild` | `src/ui/media_grid/updates.rs:668-1336` | **669 行** |

（次级：`recover_upload_task` 194 行 `service.rs:1297-1490`；`show_at` 126 行 `viewer_page.rs:906-1031`）

- `reconcile_one` 混杂规划、哈希、验证三分支、6 路动作分发、观测记录、备份归档；其 `match action` 把 planner 的契约**第二次复述**。
- `MediaGrid::rebuild` 内实测 **8 处 `OnceCell.expect()`** 重复；最深缩进达 **12 级**（48 空格），且有 31 行处于 36 空格以上缩进——分段构建 + 事件接线的循环体嵌在多层分支中。

**修复方向**：各自按职责切分为 4–5 个具名函数。

**第二轮已重构（`62c4654`）**：`reconcile_one` 迁入 `service/reconcile.rs`，主函数
163 行，另有抑制删除验证、内容验证、不变基线、上传、下载五个具名 helper。
`rebuild` 迁入 `media_grid/rebuild.rs`，主函数 321 行，渐进种子、分组、瓦片
附加、激活、右键菜单、滚动恢复分别独立。原有 I/O 和 GTK 时序保留；未编译、
未运行测试。

---

### M2 — Actor 写入优先级静默降级 ✅ **已修复**

**原状**：`src/core/db_actor.rs` 的 `priority()` 以 `Self::Sync(_) => DbWritePriority::StartupScan` 收尾。

`SyncWrite` 共 **25 个变体**，其中仅 **10 个**被显式列为 `UserInteractive`，其余 **15 个走 catch-all**：
```
FailScheduledRun  FinishRun  ReconcileInterruptedRuns  FailPendingEdit
MarkJobStarted  MarkJobCompleted  MarkJobFailed  ReplaceInventory
UpsertObservation  SuppressLocalDeletion  CommitBaseline  RecordConflict
PrepareTask  SetTaskState  ResolveConflict
```

**修复**：将 catch-all 改为**显式列举这 15 个变体**，优先级值保持 `StartupScan` 不变。

**关键性质：这是零行为变化的安全加固**——原本走 catch-all 的 15 个变体，现在被逐一写明，映射结果完全相同。

**反证已验证**：故意删掉其中任一变体后，编译立即失败：
```
error[E0004]: non-exhaustive patterns: `&DbCommand::Sync(store::SyncWrite::FinishRun { .. })` not covered
```
即：今后新增 `SyncWrite` 变体，**必须在此显式分类**，无法再静默降级。

**仍待定（未改，需产品判断）**：这些变体的优先级**值**是否合理仍值得复核——`MarkJobStarted` / `MarkJobCompleted` / `FinishRun` 等作业生命周期写入排在 `FilesystemWatcher(70)` 之下，理论上 inotify 风暴期间会被推迟。但改优先级是**行为变更**，且触及同步契约（`docs/modules/sync.md`），不属于低风险范围。

---

### ~~M3 — 4 个测试文件无锁污染全局环境变量~~ ❌ **降级：非当前缺陷**

**原结论**：4 个 `tests/ux_sync_*.rs` 设置 `XDG_*` 环境变量却无锁、无还原，`cargo test` 并行调度会导致顺序相关的偶发失败。

**复核推翻**——风险描述与事实不符：

| 文件 | `#[test]` 数 | 说明 |
|---|---:|---|
| `tests/ux_sync_reset.rs` | **1** | 独立二进制 |
| `tests/ux_sync_album_selection.rs` | **1** | 独立二进制 |
| `tests/ux_sync_strategies.rs` | **1** | 独立二进制 |
| `tests/ux_sync_hash.rs` | **1** | 独立二进制 |
| `tests/locale_pictures_dir.rs`（对照） | **9** | 同一二进制内并发 → **才需要** `ENV_LOCK` |

- 每个 `tests/*.rs` 都是**独立的测试二进制（独立进程）**，环境变量不跨进程泄漏；
- 每个文件**只有 1 个测试**，进程内不存在并发测试；
- `Cargo.toml` 无 `[[test]]` 覆盖，确认划分方式就是"一文件一二进制"。

**且作者当时已推理过这一点**——`tests/ux_sync_album_selection.rs` 原本就带着注释：
> `// This binary owns its process and isolates preferences from the real user.`

**处置**：**不加 `ENV_LOCK`**。那会是为"看起来修了"而 cargo-cult，在没有缺陷的文件里增加无谓的样板。

**改为留下护栏知识**：在另外 3 个文件补上同源注释，写明"当前单测试所以安全；若加入第二个 `#[test]`，则必须照 `locale_pictures_dir.rs` 加 `ENV_LOCK`"——把这条不成文的前置条件显式化。

**真正的风险是潜在的而非当前的**：任何人日后给这些文件加第二个测试，就会踩中一个没有护栏的坑。

---

### M4 — 三页面重复约 100 行 viewer-open 脚手架

**证据**：三处实现同一流程——`viewer_open_pending` 守卫 → `VIEWER_OPEN_POP_GUARD_MS` 定时器 → `set_sensitive(false)` → `connect_navigation` 映射：

```
src/ui/photos_page.rs:2094, 2217, 2235
src/ui/album_detail_page.rs:702, 770, 788, 791
src/ui/search_page.rs:800, 855, 873, 876
```

**风险**：守卫常量定义在 `viewer_page.rs:82`，三份副本容易各自漂移（例如某页改用不同的 index 遍历方式）。

**修复方向**：抽为 `crate::ui::viewer_launch::open(...)` 自由函数，页面只保留各自领域逻辑。

**第二轮已重构（`62c4654`）**：共同生命周期抽为 `viewer_launch::push_viewer`。
三页面保留入口守卫、查询/种子、依赖注入、初始显示及收藏/回收站回调；共享
导航回调、pending 设置、源页面禁用、定时释放与 push。逐页源码对照确认共同
流程相同；未编译、未运行测试。

---

### M5 — `index_for_media_id` 四处重复实现 ✅ **已修复**

**原状**：4 份逐行等价的 `BoxedAnyObject` 降级 + 线性扫描 + id 比较（仅参数名与循环变量名不同）：
```
src/ui/photos_page.rs:2270        src/ui/album_detail_page.rs:1047
src/ui/search_page.rs:1101        src/ui/viewer/navigation.rs:46
```

而 `src/ui/media_list.rs` 正是为集中此类样板而建的，其文档注释**早已预告了这个函数**：
> *"…gives us a single place to add new accessors (e.g. `media_item_by_id`)."*

**修复**：实现上移到 `media_list.rs`；4 处改为调用。`viewer/navigation.rs` 保留 `pub(super) use` 再导出，使 viewer 内调用点无需改动。

**结果**：5 份 → 1 份，**净减 30 行**（+26 / −56）。

---

### M6 — 远程路径校验分散且规则不一致

**证据**（已核实为 4 处，**非**初步分析所称的 5 处）：

```
src/core/sync/store.rs:1545   fn validate_remote_root
src/core/sync/webdav.rs:749   fn validate_key
src/core/sync/service.rs:2459 fn remote_key
src/core/sync/service.rs:2476 fn relative_remote_key
```

**描述需准确**——它们**并非等价复制**，但都做 `.` / `..` 段检查，且规则与错误类型各有差异：

| 函数 | 额外检查 | 错误类型 | 文案 |
|---|---|---|---|
| `validate_remote_root` | 首尾 `/` 裁剪、空段必须非空 | `AppError::Backend` | "remote root is invalid" |
| `validate_key` | 前导 `/`、`\0` | `ProviderError::Protocol` | "escapes the configured root" |

同一类威胁（路径逃逸）由两套规则、两套错误类型、三种文案守护，规则漂移的风险实在。

**修复方向**：抽一个共享的段校验原语，各调用点只保留各自的额外约束。

**第二轮复核修正**：原文「四处都做 `.` / `..` 检查」不准确。
`relative_remote_key` 只做根前缀和 `/` 边界映射，不包含段校验；它不能因本轮
重构而被悄悄加强。WebDAV 允许空 key/空段，与 store/相对上传路径也不同。

**第二轮已重构（`62c4654`）**：`remote_path.rs` 收纳 `remote_key`、
`relative_remote_key` 和两个纯段谓词。store 的远程根/相册校验与 WebDAV key
校验复用谓词，但保留各自的裁剪、空段、NUL、前导 `/` 规则和错误类型/文案。
本轮没有增加或移除可接受路径；未编译、未运行测试。

---

### M7 — 缩放/旋转按钮可见性逻辑双份，会漂移 ✅ **已修复**

**原状**（逐行等价）：
```
src/ui/viewer/transform.rs:137-141
src/ui/viewer/fullscreen_window.rs:161-165
    zoom_in.set_visible(true);
    zoom_out.set_visible(zoomed);
    zoom_reset.set_visible(zoomed);
    rotate_left.set_visible(!zoomed);
    rotate_right.set_visible(!zoomed);
```

**修复**：抽出 `transform::apply_transform_control_visibility`（规则的归属地），两处改为调用。viewer 侧独有的两个 separator 仍留在 `update_zoom_buttons` 内——fullscreen 没有 separator，不应被迫承担。

```rust
cargo test --locked --lib ui::viewer   # 97 passed
```

---

### M8 — 中文错误字符串作为公开常量 ✅ **已修复（`62c4654`）**

**证据**：
```rust
src/core/sync/store.rs:53-54
pub(crate) const INTERRUPTED_RUN_ERROR: &str =
    "上次同步被中断（应用在同步完成前退出），已标记为失败，可重试。";
```

注释声明"deliberately not translated"，理由是设置页概览会自行渲染本地化句子、此串仅为诊断细节。但它仍会进入 per-task error、错误对话框与日志——**英文环境下会出现中文**。

**修复方向**：改为类型化 reason tag，由渲染处本地化。

**第三轮已修复（`62c4654`）**：移除 `INTERRUPTED_RUN_ERROR` 公开常量；新记录
保存 `SyncFailureReason::InterruptedRun.as_str()`（`sync.interrupted_run`）。
`SyncFailureReason::parse` 同时识别旧版本持久化的精确中文句子，不迁移或重写
历史失败记录。Settings 的任务状态、上传选择状态、定时刷新与同步错误对话框
统一经过 `sync_failure_text`，已知原因按当前语言显示，未知服务端/系统错误保持
原文；已知原因经 `AppError::Backend` 包装后也能翻译。

新增 6 个原因解析/界面格式化单测，增强启动恢复用例；新增中英文真实 Settings
旅程，通过阻塞并取消一次真实同步制造未完成运行，再执行正式启动恢复逻辑。
初始回归用例按预期 FAIL，确认数据库保存的是中文句子；修复后该用例 PASS。


---

## 四、Minor（8 项）

| # | 问题 | 证据 |
|---|---|---|
| m1 | 非锁类 `unwrap()` **333 处**（`src/core` 占绝大多数），多数为 `expect("checked above")` 式不变量断言；C3 即其中真实缺陷 | 全仓统计 |
| m2 | `let _ =` 静默吞错 **78 处**，含 `src/core/trash.rs:633` 回滚失败被完全吞掉，可致 DB 与磁盘状态静默不一致 ✅ **已修复**（该项改为 `tracing::error!`，保留原错误返回以免影响调用方；其余 77 处为磁盘清理等 best-effort 场景，未逐一处理） | `cargo test --locked --lib core::trash` 26 passed |
| m3 | `.ok()` 丢弃 Result **72 处**，而 `.ok_or(` **0 处**——错误一律降级为 `None`，丢失类型信息 | 全仓统计 |
| m4 | `anyhow`(64 处) 与 `thiserror::AppError`(2 处) **双轨并存** | 全仓统计 |
| m5 | `gtk::`(1669) 与 `gtk4::`(161) **双拼写**并存（`use gtk4 as gtk` 为逐文件局部别名） | 全仓统计 |
| m6 | 未命名时序魔数：`editor.rs` 400ms 与 `details.rs` 700ms 属同类"等滑动结束"场景却互不相干 ✅ **已修复**（提取为 `EDITOR_SLIDE_OUT_GUARD_MS` / `DETAILS_SLIDE_OUT_GUARD_MS`，并注明二者都是蓝图 `transition-duration: 220` 之上的安全余量，**不可下调至蓝图值**；命名风格对齐既有先例 `IMMERSIVE_TRANSITION_MS`） | `cargo test --locked --lib ui::viewer` 97 passed |
| m7 | ≥1000 行文件 **18 个**（最大 `sync/service.rs` 2531 行、`window/settings.rs` 2502 行） | 文件行数统计 |
| m8 | `cargo-sources.json`(204 KB / 208,561 字节) 已提交且未 gitignore，是从 lockfile 可再生的产物 | `du -h` 实测；`.gitignore` 检索无此条目 |

---

## 五、项目结构评估

### ✅ 经证据支持的健康结论

| 项 | 数据 |
|---|---|
| `src/core` 反向依赖 `crate::ui` | **0 处**——`architecture.md:14` 的分层铁律严格成立（已亲自比对文档原文与代码） |
| TODO / FIXME / HACK / XXX | **0 处** |
| `todo!` / `unimplemented!` | **0 处** |
| i18n 两份目录互差 | **0**（472:472 完全对齐） |
| 文档准确性抽验 | 亲自抽验 4 条声明，**4 条全部与代码一致**：`architecture.md:14`（core 不反向依赖 UI，实测 0 处）、`development.md:60`（要求重新生成 cargo-sources.json）、`AGENTS.md:44`（push 闸门命令与 `ci.yml` 一致）、`sync.md` Key Files 表（所列文件均存在） |

### ⚠️ 结构性问题

**S1 — `viewer_page.rs` + `viewer/` 二元结构**
`src/ui/viewer/` 下 **12 个 `.rs` 文件、10 个同名测试目录**；实测 10 个子模块（crop / details / editor / filmstrip / fullscreen_window / immersive / navigation / stage / stage_input / transform）**全部**是"源文件 + 同名测试目录"成对存在。`tests/inline_test_ownership.rs` 整个文件的存在，就是为了强制这个已成事实的迁移不再回退——**说明项目正处于该迁移的中间态，且中间态已被固化为永久结构**。

**S2 — `ux_click_flows.rs` 巨型化**
实测 4036 行、19 个 journey 函数、仅 **3 个** `#[test]`。单个 journey 失败会连带后续全部报错。这是"GTK 无法并发驱动多 shell"的合理妥协，但缺少分段机制，诊断成本高。

**S3 — `store/` 内部层级不一致**
`store/reset.rs`(210 行) 与 `store/configuration.rs`(328 行) 已抽为独立实现文件，但 `#[test]` 均为 **0**——**抽出来了却没配套测试**，模块内比其他部分低一档。

**S4 — `tests/common/` 复用率不足**
`tests/common/` 提供了 `sample_item`、`tmp_dir`、`write_plain_jpeg` 等规范 helper，但 67 个顶层集成测试文件中仅 **33 个**声明 `mod common;`。其余 **10 个文件各自实现**同名或同类的构造 helper：
```
tests/album_order.rs      tests/albums.rs            tests/albums_find.rs
tests/section_group.rs    tests/section_counts.rs    tests/sidebar_navigation.rs
tests/media_crud.rs       tests/repository.rs        tests/ui_grid_canvas.rs
tests/ui_editor_panel.rs
```

---

## 六、建议处理顺序

> 以下为最初检视时的建议顺序，已被上方进度表取代。C3 已撤销，M3 非当前缺陷，不应按本节旧建议继续实施。

### 立即（阻塞发布）
1. **C1** — 重新生成 `cargo-sources.json`（一条命令，恢复 Flatpak 构建）
2. **C2** — 补 2 个 i18n 键 + 补闭合性测试（永久封死此类回归）
3. **C3** — HEIC 解析器加长度预检

### 短期（高性价比）
4. **M2** — 让编译器参与优先级映射
5. **C4** — `src/core/edit/` 补纯函数单测（性价比最高的测试缺口）
6. **M3** — 4 个 `ux_sync_*` 加 `ENV_LOCK`（照抄同目录既有范式）

### 中期（降低复杂度）
7. **M1** — 拆分两个巨型函数
8. **M4 / M5** — 抽出 viewer-open 脚手架与 `index_for_media_id`

---

## 七、维护本文档的约定

- 修复任一条目后，请在本条目下标注「已修复」并注明提交号，勿直接删除——保留决策轨迹。
- 若基线 `b6444d3` 之后代码变动较大，重新执行各条的「复现」命令确认后再更新结论。
- 本文档中的所有数字均来自实测命令输出，非估算。
- **全部条目处理完毕后删除本文件**，符合 `docs/README.md` 中"时点审阅记录不入库"的约定。