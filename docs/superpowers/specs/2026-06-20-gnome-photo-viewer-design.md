# GNOME Photo Viewer — Design Spec

- **Date**：2026-06-20
- **Status**：Approved (pending implementation)
- **Author**：Brainstorming session with user

---

## 1. 目标

构建一款使用标准 GNOME 技术栈（GTK4 + Libadwaita）开发的高性能相册工具，体验对标 Android 图库：

- 年/月/日三级视图浏览所有照片（统一数据源，仅缩略图大小与分组粒度不同）
- 文件夹即相册
- 系统回收站集成
- 1–10 万张照片规模下流畅运行
- 图片按拍摄时间排序
- 基础编辑能力（旋转 / 裁剪 / 调色），可扩展架构预留后期添加滤镜、贴纸等

## 2. 技术栈

| 用途 | 选型 |
|---|---|
| 语言 | Rust（edition 2021） |
| GUI | GTK4 + Libadwaita（`gtk4`, `libadwaita`） |
| 图像 | `gdk-pixbuf`, `image` crate |
| 异步 | `tokio`（rt-multi-thread） |
| 数据库 | `rusqlite`（bundled）+ `r2d2` 连接池 |
| EXIF | `exif` |
| HEIC/HEIF | `libheif-sys`（链接系统 `libheif`） |
| 文件监听 | `notify` |
| 序列化 | `serde`, `serde_json` |
| 错误 | `thiserror`, `anyhow` |
| 日志 | `tracing`, `tracing-subscriber` |
| 构建 | `cargo` + `meson`（打包） |
| UI 描述 | Blueprint（`.blp` 文件）→ GTK 模板 |
| 国际化 | `gettext` |

## 3. 架构总览

### 模块划分

```
photoViewer/
├── src/
│   ├── main.rs                    # 应用入口
│   ├── app.rs                     # AdwApplication 生命周期
│   ├── config.rs                  # 配置加载（XDG 配置目录）
│   │
│   ├── core/                      # 数据层（与 UI 解耦）
│   │   ├── mod.rs
│   │   ├── media.rs               # MediaItem, MediaBackend trait
│   │   ├── backend/
│   │   │   ├── local.rs           # LocalBackend: 文件系统扫描
│   │   │   └── tracker.rs         # TrackerBackend: SPARQL（V2）
│   │   ├── db.rs                  # SQLite 连接池 + 迁移
│   │   ├── schema.sql             # 表结构
│   │   ├── metadata.rs            # EXIF 提取
│   │   ├── thumbnails.rs          # 缩略图生成与缓存
│   │   ├── trash.rs               # gio trash:// 包装
│   │   └── edit/
│   │       ├── mod.rs
│   │       ├── op.rs              # EditOperation trait + 注册表
│   │       ├── rotate.rs
│   │       ├── crop.rs
│   │       ├── brightness.rs
│   │       ├── contrast.rs
│   │       └── saturation.rs
│   │
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── window.rs              # AdwApplicationWindow
│   │   ├── photos_page.rs         # PhotosPage + Year/Month/Day 模式
│   │   ├── albums_page.rs         # AlbumsPage + AlbumDetailPage
│   │   ├── trash_page.rs          # TrashPage
│   │   ├── viewer_page.rs         # ViewerPage（单图查看）
│   │   ├── editor_page.rs         # EditorPage
│   │   ├── media_grid.rs          # 复用的 Gtk.GridView 组件
│   │   ├── photo_tile.rs          # 单个缩略图单元
│   │   ├── thumbnail_loader.rs    # 异步缩略图加载
│   │   └── dialogs.rs             # Adw.AlertDialog 工具
│   │
│   └── platform/
│       ├── mod.rs
│       └── portals.rs             # XDG Desktop Portal
│
├── data/
│   ├── icons/                     # Adwaita 图标
│   └── ui/                        # Blueprint UI 文件
│       ├── window.blp
│       ├── photos-page.blp
│       ├── albums-page.blp
│       ├── trash-page.blp
│       ├── viewer-page.blp
│       └── editor-page.blp
│
├── Cargo.toml
├── Cargo.lock
├── meson.build                    # GNOME 打包
└── docs/superpowers/specs/
```

### 核心 trait

```rust
#[async_trait]
pub trait MediaBackend: Send + Sync {
    async fn scan(&self, tx: mpsc::Sender<ScanEvent>) -> Result<()>;
    async fn list_all(&self) -> Result<Vec<MediaItem>>;
    async fn list_trashed(&self) -> Result<Vec<MediaItem>>;
    fn watch_changes(&self) -> BoxStream<FileChangeEvent>;
}
```

启动时由 `app.rs` 决定使用 `LocalBackend`（V1）或 `TrackerBackend`（V2），通过 `Arc<dyn MediaBackend>` 注入。

### 启动数据流

```
main()
  │
  ├─→ AdwApplication::new()
  │
  └─→ activate()
        ├─→ db::init()             // SQLite + migrations
        ├─→ LocalBackend::spawn()  // 后台扫描线程
        ├─→ ThumbnailWorkerPool    // 4 tokio tasks
        ├─→ Window::present()
        │     ├─→ 加载所有 media_items 到 GListStore
        │     └─→ 三个 Page 共享同一 GListStore
        └─→ 设置 notify watcher（监听文件变化）
```

## 4. 数据模型

### SQLite Schema

```sql
CREATE TABLE media_items (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uri             TEXT    UNIQUE NOT NULL,
    path            TEXT    NOT NULL,
    folder_path     TEXT    NOT NULL,
    mime_type       TEXT    NOT NULL,
    width           INTEGER,
    height          INTEGER,
    taken_at        INTEGER,                          -- EXIF DateTimeOriginal (UTC)
    file_mtime      INTEGER NOT NULL,
    file_size       INTEGER NOT NULL,
    blake3_hash     TEXT    NOT NULL,
    trashed_at      INTEGER,                          -- NULL = 正常
    indexed_at      INTEGER NOT NULL
);
CREATE INDEX idx_media_taken_at   ON media_items(taken_at DESC) WHERE trashed_at IS NULL;
CREATE INDEX idx_media_folder     ON media_items(folder_path)    WHERE trashed_at IS NULL;
CREATE INDEX idx_media_trashed    ON media_items(trashed_at)     WHERE trashed_at IS NOT NULL;
CREATE INDEX idx_media_blake3     ON media_items(blake3_hash);

CREATE TABLE albums (
    folder_path     TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    cover_uri       TEXT,
    photo_count     INTEGER NOT NULL DEFAULT 0,
    last_modified   INTEGER NOT NULL
);

CREATE TABLE edits (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id        INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    edit_type       TEXT    NOT NULL,                 -- 'rotate' | 'crop' | 'brightness' | ...
    params          TEXT    NOT NULL,                 -- JSON
    created_at      INTEGER NOT NULL
);
CREATE INDEX idx_edits_media ON edits(media_id);

CREATE TABLE settings (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);
-- 键：'root_paths' (JSON), 'backend' ('local'), 'edit_default' ('copy'|'overwrite'),
--     'thumbnail_cache_max_bytes', 'window_geometry'
```

### 设计取舍

| 决策 | 理由 |
|---|---|
| `taken_at` 可空 | 部分图片无 EXIF（PNG 截图、扫描件），降级到 `file_mtime` |
| `folder_path` 冗余 | 按相册过滤是热路径，避免运行时 `dirname()` |
| `trashed_at` 用 NULL 标记 | 单一表 + 部分索引比双表同步更简单 |
| `blake3_hash` | 比 SHA-256 快 3–5x，碰撞概率足够 |
| `albums` 物化 | 10w 张 GROUP BY folder 仍 < 50ms，但首页进入要"瞬开"所以物化 |
| `edits` 用 JSON params | 编辑类型扩展灵活，无需频繁迁移 schema |

### 缩略图分桶策略

```
$XDG_CACHE_HOME/photoViewer/thumbnails/
├── small/256/      → {hash[0:2]}/{hash}.jpg    (网格用)
├── medium/512/                                      (预览用)
└── large/1024/                                      (查看器首屏用)
```

- **命名**：16×16 = 256 个子目录，单目录不超过 ~1000 文件，ext4 友好
- **格式**：JPEG quality 82 / 85 / 88
- **生成**：`PixbufLoader` 异步，tokio task pool（4 workers），不重复（pending 队列）
- **失效**：mtime 或 hash 变化即失效；启动扫描大小，超过限额（默认 2GB）按 LRU 清理
- **占位**：加载期间显示 `Adw.Spinner` + 灰色占位块

### 回收站映射

```rust
async fn trash_item(item: &MediaItem, pool: &DbPool) -> Result<()> {
    let file = gio::File::for_uri(&item.uri);
    // 1. gio 自动处理：移到 ~/.local/share/Trash/files/
    //    并在 ~/.local/share/Trash/info/ 写 .trashinfo（含原始路径）
    file.trash_future().await?;

    // 2. DB 标记（rusqlite 同步 API，spawn_blocking 包裹）
    let id = item.id;
    spawn_blocking(move || {
        let conn = pool.get()?;
        conn.execute(
            "UPDATE media_items SET trashed_at = unixepoch() WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok::<_, AppError>(())
    }).await??;
    Ok(())
}

async fn restore_item(item: &MediaItem, pool: &DbPool) -> Result<()> {
    let file = gio::File::for_uri(&item.uri);
    file.restore_from_trash_future().await?;     // 依赖 trash::orig-path

    let id = item.id;
    spawn_blocking(move || {
        let conn = pool.get()?;
        conn.execute(
            "UPDATE media_items SET trashed_at = NULL WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok::<_, AppError>(())
    }).await??;
    Ok(())
}
```

## 5. UI 架构

### 顶层结构

```
Adw.ApplicationWindow
└── Adw.OverlaySplitView (侧边栏 + 内容)
    ├── [侧边栏] Library（Photos / Albums / Trash）
    └── [内容] Adw.NavigationView
        ├── PhotosPage
        ├── AlbumsPage → AlbumDetailPage
        ├── TrashPage
        └── ViewerPage → EditorPage
```

### 5.1 PhotosPage（年 / 月 / 日）

**核心设计**：三种视图共用同一份 `GListStore<MediaItem>` 和同一个 `MediaGrid` 控件，仅切换分组粒度和缩略图大小。

| 视图 | Section header | 缩略图大小 | 每行列数 |
|---|---|---|---|
| 年 | `2025 · 3,247 张` | 96 px | 10–15 |
| 月 | `2025年4月 · 234 张` | 192 px | 5–7 |
| 日 | `2025年4月1日 周二 · 12 张` | 320 px | 3–4 |

**视图切换**：HeaderBar 中 `Adw.ViewSwitcher`（三个 stack page）。切换 = 重建 SectionModel + 调整 `item-size`，不重新查 DB。

**复用组件**：`MediaGrid` widget 接受参数：
```rust
pub struct MediaGrid {
    model: GListStore<MediaItem>,
    group_by: Option<GroupBy>,
    item_size: u32,
    max_columns: u32,
}
```

被 PhotosPage（月/日/年）、AlbumDetailPage、TrashPage 共用。AlbumsPage 本身是相册瓦片的网格（非图片网格），用自己的布局。

**无 EXIF 的图片**：归入"未知日期"虚拟 section，按 `file_mtime DESC` 排序，显示在列表末尾。

### 5.2 AlbumsPage（文件夹即相册）

**边界行为**：方案 B —— 相册只包含直接放在该文件夹的图片，不递归子文件夹。与 Android 图库行为一致。

**布局**：
```
┌──────────────────────────────────┐
│ Albums                  [+ Folder]│
├──────────────────────────────────┤
│ ┌─────────┐ ┌─────────┐ ┌─────┐ │
│ │ 封面    │ │ 封面    │ │封面 │ │
│ │ Camera  │ │ DCIM    │ │ ... │ │
│ │ 1,234   │ │ 567     │ │     │ │
│ └─────────┘ └─────────┘ └─────┘ │
└──────────────────────────────────┘
```

**数据**：启动时聚合 `GROUP BY folder_path`，封面取 `MAX(file_mtime)` 对应 URI。

**添加文件夹**：点击 `[+ Folder]` 弹 `Gtk.FileDialog`（XDG Portal），追加到 `settings.root_paths`，触发增量扫描。

### 5.3 TrashPage（回收站）

**布局**：网格 + `Adw.Banner`（"30 天后永久删除"）+ 选择模式 `Adw.ActionBar`（取消 / 还原 / 永久删除）。

**数据源（V1）**：DB 驱动，仅显示本应用移入回收站的项目。

```sql
SELECT * FROM media_items
WHERE trashed_at IS NOT NULL
ORDER BY trashed_at DESC;
```

**未来增强（V2）**：扫描 `~/.local/share/Trash/files/` 显示所有系统回收站项目，标注"来自 Files"。

**操作**：
- **还原**：`gio::File::restore_from_trash()` + UPDATE trashed_at = NULL
- **永久删除**：`gio::File::delete_from_trash()` + DELETE FROM media_items（CASCADE 清理 edits）
- **Empty All**：弹 `Adw.AlertDialog` 确认后批量永久删除

### 5.4 ViewerPage（单图全屏查看）

**布局**：HeaderBar（← 返回、文件名 + 拍摄时间、[✎]、[🗑]、[⋯]）+ 中央图片区 + 底部进度。

**手势**：

| 操作 | 实现 |
|---|---|
| 双指缩放（1.0×–8.0×） | `Gtk.GestureZoom` |
| 拖拽平移（仅缩放时） | `Gtk.GestureDrag`（axis = BOTH） |
| 双击切换 fit ↔ 100% | `Gtk.GestureClick`（n-press = 2） |
| 左右滑切换 | `Gtk.GestureSwipe` |
| 键盘 ← → | `Gtk.EventControllerKey` |
| ESC 退出 | `Gtk.EventControllerKey` |
| CTRL + 滚轮缩放 | `Gtk.EventControllerScroll` |

**预加载**：`LruCache<i64, Gdk.Texture>`（容量 = 5），进入 viewer 时立即解码当前 + 后台预加载前后各 2 张。

**删除后导航**：自动 `next()` 到下一张（避免跳到不存在的索引）；最后一张则退出回 PhotosPage。

### 5.5 EditorPage（编辑 / 可扩展）

**布局**：HeaderBar（Cancel / [Save Copy] / [Save ▼]）+ 中央预览 + 底部 `Adw.PreferencesGroup` 分组控件。

**状态**：
```rust
#[derive(Default)]
pub struct EditState {
    pub rotation: Rotation,             // None | 90 | 180 | 270
    pub brightness: i32,               // -100..+100
    pub contrast: i32,
    pub saturation: i32,
    pub crop: Option<CropRect>,
}
```

**实时预览流水线**：slider 拖动 → 30fps 节流（`glib::timeout_add_local_once`）→ 后台线程 `spawn_blocking` 重算 → 主线程更新 `Gdk.Texture`。

**大图预览**：>8MP 用 Triangle filter 降采样到 8MP；保存时使用原始全分辨率。

**可扩展架构**：
```rust
pub trait EditOperation: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn icon_name(&self) -> &'static str;
    fn category(&self) -> EditCategory;
    fn apply(&self, img: &DynamicImage, params: &Value) -> Result<DynamicImage>;
    fn default_params(&self) -> Value;
    fn validate_params(&self, params: &Value) -> Result<()>;
    fn build_controls(&self, state: Rc<RefCell<EditState>>,
                      on_change: impl Fn() + 'static) -> gtk::Widget;
}

pub enum EditCategory {
    Transform, Color, Crop, Filter, Effect,
}

pub struct EditRegistry {
    ops: Vec<Arc<dyn EditOperation>>,
}
```

V1 内置：RotateOp、CropOp、BrightnessOp、ContrastOp、SaturationOp。
V2+ 注册新操作只需 `registry.ops.push(Arc::new(MyNewOp))`。

**保存策略（混合）**：

| 按钮 | 行为 |
|---|---|
| **Save Copy**（默认） | 渲染到新文件 `{原名}_edited.jpg`，INSERT 新 media_items 行 + 写 edits 表 |
| **Save (Overwrite)** | 备份 `原.jpg.bak` → 覆盖原图 → 更新 DB 元数据 + 失效缩略图缓存。弹 `Adw.AlertDialog` 二次确认 |

**旋转操作特殊性**：旋转是破坏性 + 即时（不进入 Save 流程），弹 5 秒可撤销 toast（反向旋转）。

## 6. 性能策略

| 优化点 | 做法 |
|---|---|
| 元数据加载 | 启动时 `SELECT *` 到 `GListStore<MediaItem>`；10w × 200B ≈ 20MB |
| 增量更新 | 索引线程写入新图片 → `g_list_store_append()` → 视图自动 diff |
| 缩略图按需加载 | `bind`/`unbind` 生命周期管理；仅加载可见行 |
| 虚拟化 | 所有 `Gtk.GridView` 用 `Gtk.NoSelection` model + 自定义 `Gtk.ListItemFactory` |
| 缩略图分桶 | small/medium/large，PNG/JPEG 二次压缩节省空间 |
| 后台索引 | `LocalBackend` 独立线程 + `mpsc::Sender<ScanEvent>` 推送进度 |
| 预加载 | ViewerPage LRU 缓存当前 + 前后各 2 张 |
| 大图解码 | `PixbufLoader` 流式解码，不阻塞 UI |
| 缩放/平移 GPU 加速 | `Gsk.Transform` 合成 |
| 编辑预览 | 30fps 节流，slider 拖动不卡顿 |
| 大图预览降采样 | >8MP 用 Triangle filter 缩到 8MP |
| 批量 trash/restore | 并发 N=4 worker |

## 7. 错误处理

### 错误分类

```rust
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("database error: {0}")]           Db(#[from] rusqlite::Error),
    #[error("io error: {0}")]                 Io(#[from] std::io::Error),
    #[error("gio error: {0}")]                Gio(#[from] gio::Error),
    #[error("image decode failed: {0}")]      Decode(String),
    #[error("exif parse failed: {0}")]        Exif(String),
    #[error("backend unavailable: {0}")]      Backend(String),
}
```

### 用户可见错误处理

| 场景 | 表现 |
|---|---|
| 启动扫描失败（根目录权限不足） | 启动仍能完成，列表显示已索引部分 + 警告 toast |
| 单张图片解码失败 | ViewerPage 显示错误图标 + 文件名 + Retry 按钮；其他图片正常浏览 |
| 缩略图生成失败 | 显示占位灰块 + 鼠标悬停提示 |
| Trash 还原失败（原位置不存在） | gio 自动退到 `~/`，UI 提示 |
| 删除失败（跨设备） | gio 退化为直接删除，提示用户 |
| 磁盘空间不足 | 保存/索引时弹错误对话框，应用其他功能正常 |
| 数据库迁移失败 | 弹严重错误对话框，提示备份数据后重试 |

### 日志策略

- 启动信息 → `tracing::info!`
- 单项失败（解码、单图操作） → `tracing::warn!`（不阻塞）
- 关键错误（DB、迁移、配置） → `tracing::error!` + 用户对话框
- 用户操作日志不记录（隐私）

## 8. 测试策略

### 单元测试（`#[cfg(test)]`）

- `core/db.rs`：schema 迁移、CRUD 查询
- `core/metadata.rs`：EXIF 解析（含各类畸形文件）
- `core/thumbnails.rs`：缓存命中、失效、LRU
- `core/trash.rs`：mock gio 文件操作
- `core/edit/*.rs`：每个 EditOperation 的 apply 函数

### 集成测试（`tests/`）

- `tests/scan_local.rs`：扫描测试目录 → 验证 DB 内容
- `tests/trash_flow.rs`：标记 + 还原 + 永久删除全流程
- `tests/edit_pipeline.rs`：编辑 → 保存 → 重新加载 → 校验

### UI 测试

- **V1**：手动测试 + 截图
- **V2**：考虑 `gtk-test` 框架或录制/回放

### CI

- GitHub Actions：cargo build + cargo test on stable Rust
- 跨发行版测试：Fedora、Ubuntu（可选 V2）

## 9. 扩展性预留

| 扩展点 | 当前架构支持 |
|---|---|
| 添加新图片格式 | `MediaBackend` trait + 独立解码器选择表 |
| 添加新编辑操作 | `EditOperation` trait + 注册到 `EditRegistry` |
| 切换数据源 | `MediaBackend` trait，已预留 `TrackerBackend` |
| 添加搜索/筛选 | 复用 `MediaGrid`，加 `SearchOverlay` |
| 视频支持 | 新增 `VideoItem` 变体 + `MediaGrid` 多态 |
| 多窗口/平板模式 | GTK4 `Adw.Breakpoint` 已内置响应式 |
| 国际化 | `gettext` + `.po` 文件 |
| 主题 | Libadwaita 自动 light/dark |

## 10. 不在 V1 范围

明确不做（避免范围蔓延）：

- 用户拖拽创建相册（仅文件夹相册）
- 视频播放
- 云同步（Google Photos / iCloud）
- 人脸识别 / 地理位置聚类
- 共享菜单（导出/发送至其他应用）
- AVIF / RAW 格式支持
- 撤销栈（仅旋转操作有 5s toast 撤销）
- 多窗口/平板专属布局
- 国际化（V1 仅英文，V2 添加）
- 自动 30 天清理回收站（V1 仅显示提示）

## 11. 里程碑（建议实现顺序）

1. **M1 — 基础浏览**：项目脚手架 + DB + LocalBackend 扫描 + PhotosPage 年/月/日 + MediaGrid 复用
2. **M2 — 缩略图与查看**：ThumbnailWorkerPool + ViewerPage + 预加载 + 手势
3. **M3 — 相册与回收站**：AlbumsPage + AlbumDetailPage + TrashPage + gio trash 集成
4. **M4 — 编辑**：EditorPage + EditOperation 架构 + 5 个内置 op + Save Copy/Overwrite
5. **M5 — 打磨**：空状态、错误处理、性能优化、图标/主题

每个 M 完成后即一个可发布版本。

## 12. 开放问题

暂无。后续在实现过程中如遇到未预见的技术决策，再补充到此 spec。