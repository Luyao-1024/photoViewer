# GNOME Photo Viewer — M1: Foundation & Browsing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 搭建可运行的 GTK4 + Libadwaita 应用骨架；建立 SQLite 元数据存储与本地文件系统扫描后端；实现 PhotosPage 年/月/日三级视图（共用一份 `GListStore<MediaItem>`，仅切换分组粒度与缩略图大小）；交付一个可浏览本地照片库的可用版本（缩略图为占位，M2 实现真实缩略图流水线）。

**Architecture:**
- 数据层 (`core/`) 与 UI 层 (`ui/`) 解耦，通过 `MediaBackend` trait 抽象
- 启动一次性把 `media_items` 加载到 `GListStore<MediaItem>`，三个视图（年/月/日）共用此 store
- `MediaGrid` 复用组件 + 自定义 `SectionModel` 实现按年/月/日分组
- ViewSwitcher 切换 = 重建 SectionModel + 调整 item-size，不重新查 DB

**Tech Stack:** Rust (edition 2021) + GTK4 + Libadwaita + rusqlite + tokio + kamadak-exif + notify + Blueprint (`.blp`)

## Global Constraints

- 缩略图占位（M1 用灰色块），真实缩略图流水线在 M2
- 不实现编辑（旋转/裁剪/调色）→ M4
- 不实现 Albums / Trash → M3
- 不实现 ViewerPage → M2
- 单个 user-visible 行为变更一个 commit
- 测试优先于实现（TDD：先写失败测试，再写最小实现通过）
- 所有代码含中文注释（与 spec 一致）
- 模块路径 `src/core/`, `src/ui/`, `src/platform/`；UI 模板 `data/ui/*.blp`
- Database 文件路径：`$XDG_DATA_HOME/photoViewer/photos.db`
- Cargo features 启用：`gtk4`（v4_8）, `libadwaita`（v1_4）

---

## File Structure

```
photoViewer/
├── Cargo.toml                              # 依赖 + features
├── meson.build                             # GNOME 打包（最小骨架）
├── src/
│   ├── main.rs                             # 入口
│   ├── app.rs                              # AdwApplication 生命周期
│   ├── config.rs                           # XDG 路径解析
│   ├── core/
│   │   ├── mod.rs
│   │   ├── error.rs                        # AppError 枚举
│   │   ├── media.rs                        # MediaItem, MediaBackend trait
│   │   ├── backend/
│   │   │   ├── mod.rs                      # MediaBackend trait 定义
│   │   │   └── local.rs                    # LocalBackend 实现
│   │   ├── db.rs                           # 连接池 + 迁移
│   │   ├── schema.sql                      # 表结构
│   │   ├── metadata.rs                     # EXIF 提取
│   │   └── section_model.rs                # GtkSectionModel 按年/月/日分组
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── window.rs                       # 主窗口
│   │   ├── photos_page.rs                  # PhotosPage + 3 modes
│   │   ├── media_grid.rs                   # 复用的 GridView 组件
│   │   ├── photo_tile.rs                   # 单格 widget
│   │   └── section_header.rs               # section header
│   └── platform/
│       ├── mod.rs
│       └── xdg.rs                          # XDG 路径
├── data/
│   └── ui/
│       ├── window.blp
│       ├── photos-page.blp
│       └── media-grid.blp
├── tests/
│   ├── common/mod.rs                       # 共享 fixtures
│   ├── db_migrations.rs
│   ├── media_crud.rs
│   ├── metadata_extract.rs
│   ├── local_scan.rs
│   └── section_group.rs
└── docs/superpowers/{specs,plans}/
```

---

## Task 1: 项目脚手架（Cargo + 最小可运行 GTK 应用）

**Files:**
- Create: `Cargo.toml`
- Create: `meson.build`
- Create: `src/main.rs`
- Create: `src/app.rs`
- Create: `src/config.rs`
- Create: `src/platform/mod.rs`
- Create: `src/platform/xdg.rs`
- Create: `data/ui/window.blp`
- Create: `tests/smoke.rs`

**Interfaces:**
- Produces: `photo_viewer::app::build_app() -> adw::Application`
- Produces: `photo_viewer::config::data_dir() -> PathBuf`
- Produces: `photo_viewer::config::cache_dir() -> PathBuf`

- [ ] **Step 1: 写失败测试 — 应用启动并退出**

`tests/smoke.rs`:
```rust
// Smoke 测试：应用能创建并立刻退出
#[test]
fn app_builds_without_panic() {
    // 初始化 GTK 测试模式（无需显示）
    gtk::init().expect("GTK init failed");
    let _app = photo_viewer::app::build_app();
    // 不调用 run()，仅验证 build 不 panic
}
```

- [ ] **Step 2: 运行测试，验证失败**

Run: `cargo test --test smoke`
Expected: 编译失败（`photo_viewer` crate 不存在 / `app` 模块缺失）

- [ ] **Step 3: 创建 Cargo.toml**

`Cargo.toml`:
```toml
[package]
name = "photo-viewer"
version = "0.1.0"
edition = "2021"

[lib]
name = "photo_viewer"
path = "src/lib.rs"

[[bin]]
name = "photo-viewer"
path = "src/main.rs"

[dependencies]
gtk4 = { version = "0.8", features = ["v4_8"] }
libadwaita = { version = "0.6", features = ["v1_4"] }
gdk-pixbuf = "0.4"
tokio = { version = "1", features = ["rt-multi-thread", "fs", "macros", "sync"] }
rusqlite = { version = "0.31", features = ["bundled"] }
r2d2 = "0.8"
r2d2_sqlite = "0.24"
kamadak-exif = "0.6"
notify = "6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
glib = "0.20"
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
blake3 = "1"

[dev-dependencies]
tempfile = "3"
image = "0.25"
```

- [ ] **Step 4: 创建 `src/lib.rs`**

```rust
// 库根：暴露所有公共模块
pub mod app;
pub mod config;
pub mod core;
pub mod platform;
pub mod ui;

pub use core::error::AppError;
```

- [ ] **Step 5: 实现 `src/config.rs`（XDG 路径）**

```rust
//! 应用配置路径解析（XDG Base Directory 规范）
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").expect("HOME not set");
            PathBuf::from(home).join(".local/share")
        });
    base.join("photoViewer")
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").expect("HOME not set");
            PathBuf::from(home).join(".cache")
        });
    base.join("photoViewer")
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").expect("HOME not set");
            PathBuf::from(home).join(".config")
        });
    base.join("photoViewer")
}
```

- [ ] **Step 6: 实现 `src/platform/mod.rs` 和 `src/platform/xdg.rs`**

`src/platform/mod.rs`:
```rust
pub mod xdg;

pub use xdg::*;
```

`src/platform/xdg.rs`:
```rust
//! XDG Desktop Portal 集成（V1 仅占位，M3 完整实现）
use gtk::glib;

pub async fn pick_folder() -> anyhow::Result<Option<std::path::PathBuf>> {
    // V1 占位：返回 None
    Ok(None)
}

pub fn init() {
    // 后续 portal 初始化预留
    let _ = glib::user_config_dir();
}
```

- [ ] **Step 7: 实现 `src/app.rs`（AdwApplication 构造）**

```rust
//! AdwApplication 生命周期管理
use libadwaita as adw;
use libadwaita::prelude::*;

pub fn build_app() -> adw::Application {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer")
        .build();

    app.connect_activate(|_app| {
        // M1 占位：仅打印
        tracing::info!("Photo Viewer activated");
    });

    app
}
```

- [ ] **Step 8: 实现 `src/main.rs`（入口）**

```rust
use photo_viewer::app;
use photo_viewer::config;

fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 确保 XDG 目录存在
    std::fs::create_dir_all(config::data_dir())?;
    std::fs::create_dir_all(config::cache_dir())?;

    let app = app::build_app();
    let empty: Vec<String> = vec![];
    app.run_with_args(&empty);

    Ok(())
}
```

- [ ] **Step 9: 添加 `meson.build`（最小骨架）**

```meson
project('photo-viewer', 'rust',
  version : '0.1.0',
  meson_version : '>= 0.59.0',
)

gnome = import('gnome')

# V1: 仅 schema 和 desktop 占位
# 完整 Blueprint 集成在 M2 加入

install_data(
  'data/org.gnome.PhotoViewer.desktop',
  install_dir : get_option('datadir') / 'applications',
)
```

- [ ] **Step 10: 创建 `data/org.gnome.PhotoViewer.desktop`**

```ini
[Desktop Entry]
Type=Application
Name=Photo Viewer
GenericName=Image Viewer
Comment=Browse and view photos
Exec=photo-viewer
Icon=org.gnome.PhotoViewer
Terminal=false
Categories=Graphics;Photography;Viewer;
StartupNotify=true
```

- [ ] **Step 11: 运行 smoke 测试，验证通过**

Run: `cargo test --test smoke`
Expected: PASS（GTK init 成功，build_app 不 panic）

- [ ] **Step 12: Commit**

```bash
git add Cargo.toml src/ data/ meson.build tests/smoke.rs
git commit -m "feat: scaffold Rust+GTK4+Libadwaita app shell

建立项目脚手架：cargo + meson 双构建系统；XDG 路径解析；
AdwApplication 构造（占位 activate handler）；smoke 测试验证
GTK 能成功初始化。"
```

---

## Task 2: 错误类型与 AppError

**Files:**
- Create: `src/core/mod.rs`
- Create: `src/core/error.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `photo_viewer::core::error::AppError`

- [ ] **Step 1: 写失败测试 — AppError 枚举构造与转换**

`src/core/error.rs`（含测试模块）：
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("gio error: {0}")]
    Gio(#[from] gtk::glib::Error),

    #[error("image decode failed: {0}")]
    Decode(String),

    #[error("exif parse failed: {0}")]
    Exif(String),

    #[error("backend unavailable: {0}")]
    Backend(String),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),
}

pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: AppError = io_err.into();
        assert!(matches!(err, AppError::Io(_)));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn from_db_error() {
        let db_err = rusqlite::Error::QueryReturnedNoRows;
        let err: AppError = db_err.into();
        assert!(matches!(err, AppError::Db(_)));
    }
}
```

- [ ] **Step 2: 创建 `src/core/mod.rs`**

```rust
pub mod error;

pub use error::{AppError, Result};
```

- [ ] **Step 3: 修改 `src/lib.rs`（lib.rs 已经声明了 core 模块，验证）

确认 `src/lib.rs` 已有：
```rust
pub mod core;
```
（Task 1 已添加，无需修改）

- [ ] **Step 4: 运行测试，验证通过**

Run: `cargo test core::error`
Expected: PASS（2 个测试）

- [ ] **Step 5: Commit**

```bash
git add src/core/
git commit -m "feat(core): add AppError enum with thiserror

统一错误类型：DB / IO / Gio / Decode / Exif / Backend / Pool
七个变体；实现 From 转换；类型别名 Result<T>。"
```

---

## Task 3: MediaItem 结构体

**Files:**
- Create: `src/core/media.rs`
- Modify: `src/core/mod.rs`

**Interfaces:**
- Produces: `photo_viewer::core::media::MediaItem`

- [ ] **Step 1: 写失败测试 — MediaItem 构造与字段访问**

`src/core/media.rs`（含测试模块）：
```rust
use chrono::{DateTime, Utc};
use std::path::PathBuf;

/// 单张媒体项的完整元数据
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub id: i64,
    pub uri: String,
    pub path: PathBuf,
    pub folder_path: PathBuf,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub file_mtime: DateTime<Utc>,
    pub file_size: u64,
    pub blake3_hash: String,
    pub trashed_at: Option<DateTime<Utc>>,
}

impl MediaItem {
    pub fn display_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("(unnamed)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item() -> MediaItem {
        MediaItem {
            id: 1,
            uri: "file:///tmp/IMG_001.jpg".into(),
            path: PathBuf::from("/tmp/IMG_001.jpg"),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            width: Some(1920),
            height: Some(1080),
            taken_at: Some(Utc::now()),
            file_mtime: Utc::now(),
            file_size: 123_456,
            blake3_hash: "abc123".into(),
            trashed_at: None,
        }
    }

    #[test]
    fn display_name_from_path() {
        let item = sample_item();
        assert_eq!(item.display_name(), "IMG_001.jpg");
    }

    #[test]
    fn trashed_flag() {
        let mut item = sample_item();
        assert!(item.trashed_at.is_none());
        item.trashed_at = Some(Utc::now());
        assert!(item.trashed_at.is_some());
    }
}
```

- [ ] **Step 2: 修改 `src/core/mod.rs`**

```rust
pub mod error;
pub mod media;

pub use error::{AppError, Result};
pub use media::MediaItem;
```

- [ ] **Step 3: 运行测试，验证通过**

Run: `cargo test core::media`
Expected: PASS（2 个测试）

- [ ] **Step 4: Commit**

```bash
git add src/core/media.rs src/core/mod.rs
git commit -m "feat(core): add MediaItem struct

媒体项完整元数据：URI/路径/EXIF 维度/拍摄时间/mtime/
blake3 hash/回收站标记；display_name() 取 basename。"
```

---

## Task 4: SQLite Schema 与迁移

**Files:**
- Create: `src/core/schema.sql`
- Create: `src/core/db.rs`
- Modify: `src/core/mod.rs`
- Create: `tests/common/mod.rs`
- Create: `tests/db_migrations.rs`

**Interfaces:**
- Produces: `photo_viewer::core::db::DbPool` (type alias for `r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>`)
- Produces: `photo_viewer::core::db::init_pool(path: &Path) -> Result<DbPool>`
- Produces: `photo_viewer::core::db::run_migrations(pool: &DbPool) -> Result<()>`

- [ ] **Step 1: 写失败测试 — 数据库迁移创建所有表**

`tests/db_migrations.rs`:
```rust
use photo_viewer::core::db;
use tempfile::tempdir;

#[test]
fn migrations_create_all_tables() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();

    // 验证所有表存在
    let conn = pool.get().unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    assert!(tables.contains(&"media_items".to_string()));
    assert!(tables.contains(&"albums".to_string()));
    assert!(tables.contains(&"edits".to_string()));
    assert!(tables.contains(&"settings".to_string()));
}

#[test]
fn migrations_are_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    // 再次运行迁移不应失败
    db::run_migrations(&pool).unwrap();
    db::run_migrations(&pool).unwrap();
}

#[test]
fn indexes_created() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::init_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();

    let indexes: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    assert!(indexes.iter().any(|n| n.contains("taken_at")));
    assert!(indexes.iter().any(|n| n.contains("folder")));
    assert!(indexes.iter().any(|n| n.contains("trashed")));
}
```

- [ ] **Step 2: 创建 `src/core/schema.sql`**

```sql
-- media_items 主表
CREATE TABLE IF NOT EXISTS media_items (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uri             TEXT    UNIQUE NOT NULL,
    path            TEXT    NOT NULL,
    folder_path     TEXT    NOT NULL,
    mime_type       TEXT    NOT NULL,
    width           INTEGER,
    height          INTEGER,
    taken_at        INTEGER,
    file_mtime      INTEGER NOT NULL,
    file_size       INTEGER NOT NULL,
    blake3_hash     TEXT    NOT NULL,
    trashed_at      INTEGER,
    indexed_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_media_taken_at
    ON media_items(taken_at DESC) WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_folder
    ON media_items(folder_path)    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_trashed
    ON media_items(trashed_at)     WHERE trashed_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_media_blake3
    ON media_items(blake3_hash);

-- albums 物化视图
CREATE TABLE IF NOT EXISTS albums (
    folder_path     TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    cover_uri       TEXT,
    photo_count     INTEGER NOT NULL DEFAULT 0,
    last_modified   INTEGER NOT NULL
);

-- edits 非破坏性编辑记录
CREATE TABLE IF NOT EXISTS edits (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id        INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    edit_type       TEXT    NOT NULL,
    params          TEXT    NOT NULL,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_edits_media ON edits(media_id);

-- settings
CREATE TABLE IF NOT EXISTS settings (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);

-- schema 版本表
CREATE TABLE IF NOT EXISTS schema_version (
    version         INTEGER PRIMARY KEY,
    applied_at      INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_version (version, applied_at)
VALUES (1, unixepoch());
```

- [ ] **Step 3: 实现 `src/core/db.rs`**

```rust
//! SQLite 连接池与迁移管理
use crate::core::error::{AppError, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

pub type DbPool = Pool<SqliteConnectionManager>;

const SCHEMA_SQL: &str = include_str!("schema.sql");

/// 初始化数据库连接池；如不存在则创建并运行迁移
pub fn init_pool(path: &Path) -> Result<DbPool> {
    let manager = SqliteConnectionManager::file(path)
        .with_init(|c| {
            c.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA synchronous = NORMAL;",
            )
        });
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(AppError::from)?;
    run_migrations(&pool)?;
    Ok(pool)
}

/// 执行 schema.sql 迁移（幂等）
pub fn run_migrations(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(())
}
```

- [ ] **Step 4: 修改 `src/core/mod.rs`**

```rust
pub mod db;
pub mod error;
pub mod media;

pub use db::{init_pool, run_migrations, DbPool};
pub use error::{AppError, Result};
pub use media::MediaItem;
```

- [ ] **Step 5: 运行测试，验证通过**

Run: `cargo test --test db_migrations`
Expected: PASS（3 个测试）

- [ ] **Step 6: Commit**

```bash
git add src/core/schema.sql src/core/db.rs src/core/mod.rs tests/db_migrations.rs
git commit -m "feat(core): SQLite schema and connection pool

表结构（media_items / albums / edits / settings / schema_version）
+ 索引；r2d2 连接池（max=8）；WAL 模式 + foreign_keys ON；
幂等迁移。"
```

---

## Task 5: MediaItem CRUD（DB 层基础操作）

**Files:**
- Modify: `src/core/db.rs`
- Modify: `src/core/media.rs`
- Create: `tests/media_crud.rs`

**Interfaces:**
- Produces: `db::insert_media_item(pool: &DbPool, item: &NewMediaItem) -> Result<i64>`
- Produces: `db::get_media_item(pool: &DbPool, id: i64) -> Result<MediaItem>`
- Produces: `db::list_all_media(pool: &DbPool) -> Result<Vec<MediaItem>>`
- Produces: `db::delete_media_item(pool: &DbPool, id: i64) -> Result<()>`
- Produces: `media::NewMediaItem` (用于 INSERT，不含 id)

- [ ] **Step 1: 写失败测试 — CRUD 全流程**

`tests/media_crud.rs`:
```rust
use photo_viewer::core::db;
use photo_viewer::core::media::{MediaItem, NewMediaItem};
use chrono::Utc;
use tempfile::tempdir;

fn fresh_pool() -> db::DbPool {
    let dir = tempdir().unwrap();
    db::init_pool(&dir.path().join("test.db")).unwrap()
}

fn sample_new_item() -> NewMediaItem {
    let now = Utc::now();
    NewMediaItem {
        uri: "file:///test/IMG_001.jpg".into(),
        path: "/test/IMG_001.jpg".into(),
        folder_path: "/test".into(),
        mime_type: "image/jpeg".into(),
        width: Some(1920),
        height: Some(1080),
        taken_at: Some(now),
        file_mtime: now,
        file_size: 100_000,
        blake3_hash: "hash001".into(),
    }
}

#[test]
fn insert_and_get() {
    let pool = fresh_pool();
    let id = db::insert_media_item(&pool, &sample_new_item()).unwrap();
    assert!(id > 0);

    let item = db::get_media_item(&pool, id).unwrap();
    assert_eq!(item.uri, "file:///test/IMG_001.jpg");
    assert_eq!(item.width, Some(1920));
    assert_eq!(item.blake3_hash, "hash001");
}

#[test]
fn list_all_returns_inserted() {
    let pool = fresh_pool();
    db::insert_media_item(&pool, &sample_new_item()).unwrap();

    let mut item2 = sample_new_item();
    item2.uri = "file:///test/IMG_002.jpg".into();
    item2.path = "/test/IMG_002.jpg".into();
    item2.blake3_hash = "hash002".into();
    db::insert_media_item(&pool, &item2).unwrap();

    let all = db::list_all_media(&pool).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn delete_removes_row() {
    let pool = fresh_pool();
    let id = db::insert_media_item(&pool, &sample_new_item()).unwrap();
    db::delete_media_item(&pool, id).unwrap();
    let result = db::get_media_item(&pool, id);
    assert!(result.is_err());
}

#[test]
fn unique_uri_constraint() {
    let pool = fresh_pool();
    db::insert_media_item(&pool, &sample_new_item()).unwrap();
    let result = db::insert_media_item(&pool, &sample_new_item());
    assert!(result.is_err());
}
```

- [ ] **Step 2: 修改 `src/core/media.rs` — 添加 `NewMediaItem`**

在 `MediaItem` 后添加：
```rust
/// 用于 INSERT 的新项（不含 id 和 trashed_at）
#[derive(Debug, Clone)]
pub struct NewMediaItem {
    pub uri: String,
    pub path: PathBuf,
    pub folder_path: PathBuf,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub file_mtime: DateTime<Utc>,
    pub file_size: u64,
    pub blake3_hash: String,
}

impl From<&MediaItem> for NewMediaItem {
    fn from(item: &MediaItem) -> Self {
        Self {
            uri: item.uri.clone(),
            path: item.path.clone(),
            folder_path: item.folder_path.clone(),
            mime_type: item.mime_type.clone(),
            width: item.width,
            height: item.height,
            taken_at: item.taken_at,
            file_mtime: item.file_mtime,
            file_size: item.file_size,
            blake3_hash: item.blake3_hash.clone(),
        }
    }
}
```

- [ ] **Step 3: 修改 `src/core/db.rs` — 添加 CRUD 函数**

在文件末尾添加：
```rust
use crate::core::media::{MediaItem, NewMediaItem};
use chrono::{DateTime, TimeZone, Utc};

fn ts(dt: DateTime<Utc>) -> i64 { dt.timestamp() }
fn from_ts(ts: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(ts, 0).single()
}

/// 插入新项，返回自增 id
pub fn insert_media_item(pool: &DbPool, item: &NewMediaItem) -> Result<i64> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO media_items
            (uri, path, folder_path, mime_type, width, height,
             taken_at, file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, unixepoch())",
        rusqlite::params![
            item.uri,
            item.path.to_string_lossy(),
            item.folder_path.to_string_lossy(),
            item.mime_type,
            item.width,
            item.height,
            item.taken_at.map(ts),
            ts(item.file_mtime),
            item.file_size as i64,
            item.blake3_hash,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 根据 id 查询
pub fn get_media_item(pool: &DbPool, id: i64) -> Result<MediaItem> {
    let conn = pool.get()?;
    let item = conn.query_row(
        "SELECT id, uri, path, folder_path, mime_type, width, height,
                taken_at, file_mtime, file_size, blake3_hash, trashed_at
         FROM media_items WHERE id = ?1",
        [id],
        row_to_media_item,
    )?;
    Ok(item)
}

/// 列出所有非回收站项，按 taken_at DESC 排序
pub fn list_all_media(pool: &DbPool) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, width, height,
                taken_at, file_mtime, file_size, blake3_hash, trashed_at
         FROM media_items
         WHERE trashed_at IS NULL
         ORDER BY taken_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], row_to_media_item)?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// 删除单行
pub fn delete_media_item(pool: &DbPool, id: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute("DELETE FROM media_items WHERE id = ?1", [id])?;
    Ok(())
}

fn row_to_media_item(row: &rusqlite::Row) -> rusqlite::Result<MediaItem> {
    let taken_at: Option<i64> = row.get(7)?;
    let file_mtime: i64 = row.get(8)?;
    let trashed_at: Option<i64> = row.get(11)?;

    Ok(MediaItem {
        id: row.get(0)?,
        uri: row.get(1)?,
        path: std::path::PathBuf::from(row.get::<_, String>(2)?),
        folder_path: std::path::PathBuf::from(row.get::<_, String>(3)?),
        mime_type: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        taken_at: taken_at.and_then(from_ts),
        file_mtime: from_ts(file_mtime).unwrap_or_else(Utc::now),
        file_size: row.get::<_, i64>(9)? as u64,
        blake3_hash: row.get(10)?,
        trashed_at: trashed_at.and_then(from_ts),
    })
}
```

- [ ] **Step 4: 运行测试，验证通过**

Run: `cargo test --test media_crud`
Expected: PASS（4 个测试）

- [ ] **Step 5: Commit**

```bash
git add src/core/db.rs src/core/media.rs tests/media_crud.rs
git commit -m "feat(core): MediaItem CRUD operations

insert/get/list_all/delete + 唯一 URI 约束触发；
按 taken_at DESC 排序（无 EXIF 的图排在末尾）。"
```

---

## Task 6: EXIF 元数据提取

**Files:**
- Create: `src/core/metadata.rs`
- Modify: `src/core/mod.rs`
- Create: `tests/metadata_extract.rs`
- Create: `tests/common/mod.rs`

**Interfaces:**
- Produces: `photo_viewer::core::metadata::extract(path: &Path) -> Result<RawMetadata>`
- Produces: `photo_viewer::core::metadata::RawMetadata { width, height, taken_at, mime_type }`

- [ ] **Step 1: 创建 `tests/common/mod.rs`（共享 fixtures）**

```rust
//! 共享测试 fixtures：生成带 EXIF 的测试图片
use image::{ImageBuffer, Rgb};
use std::path::PathBuf;

pub fn tmp_dir() -> tempdir::TempDir { tempdir::tempdir().unwrap() }

pub use tempfile;

/// 生成一张纯色 JPEG 测试图（无 EXIF）
pub fn write_plain_jpeg(dir: &std::path::Path, name: &str) -> PathBuf {
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| {
        Rgb([128, 128, 128])
    });
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}
```

`tests/common/mod.rs` 需要 `tempfile` 和 `image` 作为 dev-dependencies（已在 Cargo.toml 添加）

- [ ] **Step 2: 写失败测试 — EXIF 提取**

`tests/metadata_extract.rs`:
```rust
mod common;
use common::*;
use photo_viewer::core::metadata;
use std::path::Path;

#[test]
fn plain_jpeg_returns_no_taken_at() {
    let dir = tmp_dir();
    let path = write_plain_jpeg(dir.path(), "plain.jpg");

    let meta = metadata::extract(&path).unwrap();
    assert_eq!(meta.mime_type, "image/jpeg");
    assert_eq!(meta.width, Some(64));
    assert_eq!(meta.height, Some(48));
    assert!(meta.taken_at.is_none(), "无 EXIF 数据应返回 None");
}

#[test]
fn unknown_extension_returns_error() {
    let dir = tmp_dir();
    let path = dir.path().join("garbage.xyz");
    std::fs::write(&path, b"not an image").unwrap();

    let result = metadata::extract(&path);
    assert!(result.is_err());
}

#[test]
fn mime_type_inferred_from_extension() {
    let dir = tmp_dir();
    let png_path = dir.path().join("test.png");
    image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(10, 10, |_, _| {
        image::Rgb([0, 0, 0])
    }).save(&png_path).unwrap();

    let meta = metadata::extract(&png_path).unwrap();
    assert_eq!(meta.mime_type, "image/png");
}
```

- [ ] **Step 3: 实现 `src/core/metadata.rs`**

```rust
//! 图像元数据提取：尺寸、EXIF DateTimeOriginal、MIME 类型
use crate::core::error::{AppError, Result};
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct RawMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<DateTime<Utc>>,
    pub mime_type: String,
}

/// 从文件提取元数据
pub fn extract(path: &Path) -> Result<RawMetadata> {
    let mime_type = mime_from_extension(path);
    if mime_type.is_empty() {
        return Err(AppError::Decode(format!(
            "unknown extension: {}",
            path.display()
        )));
    }

    let mut meta = RawMetadata {
        mime_type,
        ..Default::default()
    };

    // 1. 读取文件头获取尺寸（gdk-pixbuf 处理 JPEG/PNG/WebP）
    if let Ok(buf) = gdk_pixbuf::Pixbuf::from_file_at_size(path, 4096, 4096) {
        meta.width = Some(buf.width() as u32);
        meta.height = Some(buf.height() as u32);
    } else if let Ok(dim) = image::image_dimensions(path) {
        meta.width = Some(dim.0);
        meta.height = Some(dim.1);
    } else {
        return Err(AppError::Decode(format!(
            "cannot decode dimensions: {}",
            path.display()
        )));
    }

    // 2. 尝试读取 EXIF DateTimeOriginal
    if let Ok(exif) = exif_reader(path) {
        meta.taken_at = exif;
    }

    Ok(meta)
}

fn exif_reader(path: &Path) -> Result<DateTime<Utc>> {
    let file = std::fs::File::open(path)?;
    let mut bufreader = std::io::BufReader::new(&file);
    let exif = exif::Reader::new().read_from_container(&mut bufreader)
        .map_err(|e| AppError::Exif(e.to_string()))?;

    // 优先 DateTimeOriginal > DateTime > DateTimeDigitized
    for field in [exif::Tag::DateTimeOriginal, exif::Tag::DateTime,
                  exif::Tag::DateTimeDigitized] {
        if let Some(v) = exif.get_field(field, exif::In::PRIMARY) {
            if let exif::Value::Ascii(ref vec) = v.value {
                if let Some(s) = vec.first() {
                    if let Ok(s) = std::str::from_utf8(s) {
                        if let Some(dt) = parse_exif_datetime(s.trim()) {
                            return Ok(dt);
                        }
                    }
                }
            }
        }
    }
    Err(AppError::Exif("no datetime field".into()))
}

/// EXIF DateTime 格式 "YYYY:MM:DD HH:MM:SS"
fn parse_exif_datetime(s: &str) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = s.splitn(2, ' ').collect();
    if parts.len() != 2 { return None; }
    let date: Vec<&str> = parts[0].split(':').collect();
    let time: Vec<&str> = parts[1].split(':').collect();
    if date.len() != 3 || time.len() != 3 { return None; }

    let y: i32 = date[0].parse().ok()?;
    let m: u32 = date[1].parse().ok()?;
    let d: u32 = date[2].parse().ok()?;
    let h: u32 = time[0].parse().ok()?;
    let mi: u32 = time[1].parse().ok()?;
    let s: u32 = time[2].parse().ok()?;

    // EXIF 无时区信息，按本地时间解释后转 UTC
    use chrono::Local;
    let naive = chrono::NaiveDate::from_ymd_opt(y, m, d)?
        .and_hms_opt(h, mi, s)?;
    let local_dt = Local.from_local_datetime(&naive).single()?;
    Some(local_dt.with_timezone(&Utc))
}

fn mime_from_extension(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => "image/jpeg".into(),
            "png" => "image/png".into(),
            "webp" => "image/webp".into(),
            "heic" | "heif" => "image/heic".into(),
            _ => String::new(),
        },
        None => String::new(),
    }
}
```

> 注：需添加 `exif = "2"` 到 Cargo.toml dependencies（移除 `kamadak-exif`，改用 `exif`）

- [ ] **Step 4: 修改 Cargo.toml — 替换 exif crate**

```toml
# 移除：kamadak-exif = "0.6"
# 添加：
exif = "2"
image = "0.25"
```

> 注：`image` crate 之前是 dev-dependency，提到 dependencies（因为 metadata.rs 也使用）

- [ ] **Step 5: 修改 `src/core/mod.rs`**

```rust
pub mod db;
pub mod error;
pub mod media;
pub mod metadata;

pub use db::{init_pool, run_migrations, DbPool};
pub use error::{AppError, Result};
pub use media::{MediaItem, NewMediaItem};
pub use metadata::{extract as extract_metadata, RawMetadata};
```

- [ ] **Step 6: 运行测试，验证通过**

Run: `cargo test --test metadata_extract`
Expected: PASS（3 个测试）

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml src/core/metadata.rs src/core/mod.rs tests/metadata_extract.rs tests/common/
git commit -m "feat(core): EXIF and image metadata extraction

提取 width/height（MIME-aware 解码）+ EXIF DateTimeOriginal
（优先顺序：Original > DateTime > Digitized）；EXIF 无时区
按本地时间解释后转 UTC。"
```

---

## Task 7: LocalBackend — 文件系统扫描

**Files:**
- Create: `src/core/backend/mod.rs`
- Create: `src/core/backend/local.rs`
- Modify: `src/core/mod.rs`
- Create: `tests/local_scan.rs`

**Interfaces:**
- Produces: `photo_viewer::core::backend::local::LocalBackend`
- Produces: `LocalBackend::new(pool: DbPool) -> Self`
- Produces: `LocalBackend::scan_dir(&self, dir: &Path) -> Result<Vec<NewMediaItem>>` (同步版)
- Produces: `LocalBackend::upsert(&self, item: &NewMediaItem) -> Result<i64>`

- [ ] **Step 1: 写失败测试 — 扫描测试目录**

`tests/local_scan.rs`:
```rust
mod common;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::backend::local::LocalBackend;
use std::path::Path;

#[test]
fn scan_finds_jpeg_png() {
    let dir = tmp_dir();
    let root = dir.path();

    // 创建测试图片：3 张 JPEG + 1 张 PNG + 1 个非图片文件
    for name in &["a.jpg", "b.jpg", "c.jpeg"] {
        write_plain_jpeg(root, name);
    }
    let png_path = root.join("d.png");
    image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(10, 10, |_, _| {
        image::Rgb([255, 0, 0])
    }).save(&png_path).unwrap();
    std::fs::write(root.join("readme.txt"), b"text").unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 4, "应识别 4 张图片（JPEG×3 + PNG×1），忽略 .txt");

    // 验证每项都有 hash 和 mime
    for item in &items {
        assert!(!item.blake3_hash.is_empty());
        assert!(item.mime_type.starts_with("image/"));
    }
}

#[test]
fn upsert_inserts_and_updates_by_uri() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "x.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 1);

    let id1 = backend.upsert(&items[0]).unwrap();
    let id2 = backend.upsert(&items[0]).unwrap();
    assert_eq!(id1, id2, "同 URI 应返回相同 id（INSERT OR REPLACE）");
}

#[test]
fn scan_recursive_subdirs() {
    let dir = tmp_dir();
    let root = dir.path();
    let sub = root.join("sub");
    std::fs::create_dir(&sub).unwrap();
    write_plain_jpeg(root, "top.jpg");
    write_plain_jpeg(&sub, "nested.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 2);
}
```

- [ ] **Step 2: 创建 `src/core/backend/mod.rs`**

```rust
//! 数据源后端抽象（V1 仅 LocalBackend，V2 加入 TrackerBackend）
pub mod local;
```

- [ ] **Step 3: 实现 `src/core/backend/local.rs`**

```rust
//! 本地文件系统扫描后端
use crate::core::db::{self, DbPool};
use crate::core::error::Result;
use crate::core::media::NewMediaItem;
use crate::core::metadata;
use chrono::Utc;
use std::path::Path;
use walkdir::WalkDir;

const SUPPORTED_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "heic", "heif"];

pub struct LocalBackend {
    pool: DbPool,
}

impl LocalBackend {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// 递归扫描目录，返回所有支持的图片项
    pub fn scan_dir(&self, root: &Path) -> Result<Vec<NewMediaItem>> {
        let mut items = Vec::new();

        for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() { continue; }

            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_ascii_lowercase(),
                None => continue,
            };
            if !SUPPORTED_EXT.contains(&ext.as_str()) { continue; }

            match self.process_file(path) {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {},  // 不支持的 MIME
                Err(e) => {
                    tracing::warn!("跳过文件 {}: {}", path.display(), e);
                }
            }
        }
        Ok(items)
    }

    fn process_file(&self, path: &Path) -> Result<Option<NewMediaItem>> {
        let meta = metadata::extract(path)?;

        let file_meta = std::fs::metadata(path)?;
        let mtime = file_meta.modified().unwrap_or_else(|_| std::time::SystemTime::now());
        let mtime_utc: chrono::DateTime<Utc> = mtime.into();

        let uri = format!("file://{}", path.display());
        let folder = path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new("/").to_path_buf());

        let hash = blake3::hash(&std::fs::read(path)?).to_hex().to_string();

        Ok(Some(NewMediaItem {
            uri,
            path: path.to_path_buf(),
            folder_path: folder,
            mime_type: meta.mime_type,
            width: meta.width,
            height: meta.height,
            taken_at: meta.taken_at,
            file_mtime: mtime_utc,
            file_size: file_meta.len(),
            blake3_hash: hash,
        }))
    }

    /// 插入或更新（URI 冲突则 UPDATE）
    pub fn upsert(&self, item: &NewMediaItem) -> Result<i64> {
        let conn = self.pool.get()?;

        // 检查是否存在
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM media_items WHERE uri = ?1",
                [&item.uri],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            conn.execute(
                "UPDATE media_items
                 SET path=?2, folder_path=?3, mime_type=?4, width=?5,
                     height=?6, taken_at=?7, file_mtime=?8, file_size=?9,
                     blake3_hash=?10, indexed_at=unixepoch()
                 WHERE id=?1",
                rusqlite::params![
                    id,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
                    item.width,
                    item.height,
                    item.taken_at.map(|t| t.timestamp()),
                    item.file_mtime.timestamp(),
                    item.file_size as i64,
                    item.blake3_hash,
                ],
            )?;
            Ok(id)
        } else {
            Ok(db::insert_media_item(&self.pool, item)?)
        }
    }
}
```

- [ ] **Step 4: 修改 `src/core/mod.rs`**

```rust
pub mod backend;
pub mod db;
pub mod error;
pub mod media;
pub mod metadata;

pub use backend::local::LocalBackend;
pub use db::{init_pool, run_migrations, DbPool};
pub use error::{AppError, Result};
pub use media::{MediaItem, NewMediaItem};
pub use metadata::{extract as extract_metadata, RawMetadata};
```

- [ ] **Step 5: 添加 `walkdir` 依赖**

修改 `Cargo.toml`：
```toml
walkdir = "2"
```

- [ ] **Step 6: 运行测试，验证通过**

Run: `cargo test --test local_scan`
Expected: PASS（3 个测试）

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml src/core/backend/ src/core/mod.rs tests/local_scan.rs
git commit -m "feat(core): LocalBackend filesystem scanner

walkdir 递归扫描 + 扩展名白名单（jpg/jpeg/png/webp/heic/heif）；
提取元数据 + blake3 hash；upsert 按 URI 处理 INSERT/UPDATE。"
```

---

## Task 8: SectionModel — 按年/月/日分组

**Files:**
- Create: `src/core/section_model.rs`
- Create: `tests/section_group.rs`
- Modify: `src/core/mod.rs`

**Interfaces:**
- Produces: `photo_viewer::core::section_model::GroupBy` (enum: Year, Month, Day)
- Produces: `section_model::group_items(items: &[MediaItem], mode: GroupBy) -> Vec<MediaSection>`
- Produces: `MediaSection { key: SectionKey, label: String, items: Vec<MediaItem> }`

- [ ] **Step 1: 写失败测试 — 分组逻辑**

`tests/section_group.rs`:
```rust
use chrono::{TimeZone, Utc};
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::section_model::{group_items, GroupBy, MediaSection};
use std::path::PathBuf;

fn item(id: i64, year: i32, month: u32, day: u32) -> MediaItem {
    let dt = Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap();
    MediaItem {
        id,
        uri: format!("file:///test/{id}.jpg"),
        path: PathBuf::from(format!("/test/{id}.jpg")),
        folder_path: PathBuf::from("/test"),
        mime_type: "image/jpeg".into(),
        width: Some(100),
        height: Some(100),
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 1000,
        blake3_hash: format!("h{id}"),
        trashed_at: None,
    }
}

#[test]
fn group_by_year() {
    let items = vec![
        item(1, 2025, 3, 1),
        item(2, 2025, 8, 1),
        item(3, 2024, 1, 1),
    ];
    let sections = group_items(&items, GroupBy::Year);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].label, "2025 · 2 张");
    assert_eq!(sections[1].label, "2024 · 1 张");
    assert_eq!(sections[0].items.len(), 2);
}

#[test]
fn group_by_month() {
    let items = vec![
        item(1, 2025, 3, 1),
        item(2, 2025, 3, 15),
        item(3, 2025, 4, 1),
        item(4, 2024, 12, 31),
    ];
    let sections = group_items(&items, GroupBy::Month);
    assert_eq!(sections.len(), 3);
    assert_eq!(sections[0].label, "2025年3月 · 2 张");
    assert_eq!(sections[1].label, "2025年4月 · 1 张");
    assert_eq!(sections[2].label, "2024年12月 · 1 张");
}

#[test]
fn group_by_day() {
    let items = vec![
        item(1, 2025, 3, 1),
        item(2, 2025, 3, 1),
        item(3, 2025, 3, 15),
    ];
    let sections = group_items(&items, GroupBy::Day);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].label, "2025年3月1日 周日 · 2 张");
    assert_eq!(sections[1].label, "2025年3月15日 周六 · 1 张");
}

#[test]
fn unknown_date_grouped_separately() {
    let mut a = item(1, 2025, 3, 1);
    a.taken_at = None;
    let b = item(2, 2025, 3, 2);
    let sections = group_items(&[a, b], GroupBy::Year);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[1].label, "未知日期 · 1 张");
}
```

- [ ] **Step 2: 实现 `src/core/section_model.rs`**

```rust
//! 按年/月/日对 MediaItem 分组（用于 PhotosPage 三种视图）
use crate::core::media::MediaItem;
use chrono::{Datelike, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy { Year, Month, Day }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionKey {
    pub year: Option<i32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct MediaSection {
    pub key: SectionKey,
    pub label: String,
    pub items: Vec<MediaItem>,
}

pub fn group_items(items: &[MediaItem], mode: GroupBy) -> Vec<MediaSection> {
    let mut sections: Vec<MediaSection> = Vec::new();

    for item in items {
        let key = make_key(item, mode);
        let label = make_label(&key, 0);  // 计数后修正

        // 查找或创建 section
        let pos = sections.iter().position(|s| s.key == key);
        match pos {
            Some(idx) => sections[idx].items.push(item.clone()),
            None => sections.push(MediaSection {
                key,
                label,
                items: vec![item.clone()],
            }),
        }
    }

    // 更新每个 section 的 label 中的计数
    for sec in &mut sections {
        let count = sec.items.len() as u32;
        sec.label = make_label(&sec.key, count);
    }

    sections
}

fn make_key(item: &MediaItem, mode: GroupBy) -> SectionKey {
    match item.taken_at {
        Some(dt) => match mode {
            GroupBy::Year => SectionKey { year: Some(dt.year()), month: None, day: None },
            GroupBy::Month => SectionKey { year: Some(dt.year()), month: Some(dt.month()), day: None },
            GroupBy::Day => SectionKey { year: Some(dt.year()), month: Some(dt.month()), day: Some(dt.day()) },
        },
        None => SectionKey { year: None, month: None, day: None },
    }
}

fn make_label(key: &SectionKey, count: u32) -> String {
    match (key.year, key.month, key.day) {
        (Some(y), Some(m), Some(d)) => {
            let dt = chrono::NaiveDate::from_ymd_opt(y, m, d)
                .and_then(|d| chrono::NaiveDate::weekday(&d)
                    .num_days_from_sunday().into());
            let weekday_cn = match dt {
                Some(0) | Some(7) => "周日",
                Some(1) => "周一",
                Some(2) => "周二",
                Some(3) => "周三",
                Some(4) => "周四",
                Some(5) => "周五",
                Some(6) => "周六",
                _ => "",
            };
            format!("{}年{}月{}日 {} · {} 张", y, m, d, weekday_cn, count)
        }
        (Some(y), Some(m), None) => format!("{}年{}月 · {} 张", y, m, count),
        (Some(y), None, None) => format!("{} · {} 张", y, count),
        _ => format!("未知日期 · {} 张", count),
    }
}
```

- [ ] **Step 3: 修改 `src/core/mod.rs`**

```rust
pub mod backend;
pub mod db;
pub mod error;
pub mod media;
pub mod metadata;
pub mod section_model;

pub use backend::local::LocalBackend;
pub use db::{init_pool, run_migrations, DbPool};
pub use error::{AppError, Result};
pub use media::{MediaItem, NewMediaItem};
pub use metadata::{extract as extract_metadata, RawMetadata};
pub use section_model::{group_items, GroupBy, MediaSection, SectionKey};
```

- [ ] **Step 4: 运行测试，验证通过**

Run: `cargo test --test section_group`
Expected: PASS（4 个测试）

- [ ] **Step 5: Commit**

```bash
git add src/core/section_model.rs src/core/mod.rs tests/section_group.rs
git commit -m "feat(core): section grouping by year/month/day

group_items() 按 GroupBy 模式切分；中文 label 包含计数；
无 EXIF 项归入'未知日期'section。"
```

---

## Task 9: 主窗口骨架（AdwApplicationWindow + OverlaySplitView）

**Files:**
- Create: `src/ui/mod.rs`
- Create: `src/ui/window.rs`
- Create: `data/ui/window.blp`
- Modify: `src/lib.rs`
- Modify: `src/app.rs`

**Interfaces:**
- Produces: `photo_viewer::ui::window::MainWindow`
- Produces: `MainWindow::new(app: &adw::Application) -> Self`
- Produces: `MainWindow::present()`

- [ ] **Step 1: 创建 `src/ui/mod.rs`**

```rust
pub mod window;

pub use window::MainWindow;
```

- [ ] **Step 2: 修改 `src/lib.rs`**

确认已有：
```rust
pub mod ui;
```
（Task 1 已添加）

- [ ] **Step 3: 创建 `data/ui/window.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="PhotoViewerWindow" parent="AdwApplicationWindow">
    <property name="title" translatable="yes">Photo Viewer</property>
    <property name="default-width">1200</property>
    <property name="default-height">800</property>

    <child>
      <object class="AdwOverlaySplitView" id="split_view">
        <property name="sidebar-width-fraction">0.2</property>
        <property name="min-sidebar-width">200</property>

        <!-- 侧边栏 -->
        <child type="sidebar">
          <object class="AdwNavigationPage" id="sidebar_page">
            <property name="title">Library</property>
            <child>
              <object class="GtkListBox" id="sidebar_list">
                <property name="selection-mode">single</property>
              </object>
            </child>
          </object>
        </child>

        <!-- 内容区 -->
        <child type="content">
          <object class="AdwNavigationView" id="nav_view" />
        </child>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 4: 实现 `src/ui/window.rs`**

```rust
//! 主窗口：侧边栏 + 内容区
use gtk::prelude::*;
use gtk::{glib, ListBoxRow};
use libadwaita as adw;
use libadwaita::prelude::*;

mod imp {
    use super::*;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::MainWindow)]
    pub struct MainWindow {}

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "PhotoViewerWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for MainWindow {}
    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}
    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}
}

glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends adw::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    pub fn populate_sidebar(&self) {
        let list: gtk::ListBox = self.imp().sidebar_list.get();
        for (label, _target) in &[
            ("Photos", "photos"),
            ("Albums", "albums"),
            ("Trash", "trash"),
        ] {
            let row = ListBoxRow::new();
            let lbl = gtk::Label::builder()
                .label(*label)
                .halign(gtk::Align::Start)
                .margin_start(12)
                .margin_end(12)
                .margin_top(8)
                .margin_bottom(8)
                .build();
            row.set_child(Some(&lbl));
            list.append(&row);
        }
    }
}
```

- [ ] **Step 5: 修改 `src/app.rs`（构建并展示主窗口）**

```rust
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::ui::MainWindow;

pub fn build_app() -> adw::Application {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer")
        .build();

    app.connect_activate(|app| {
        let window = MainWindow::new(app);
        window.populate_sidebar();
        window.present();
    });

    app
}
```

- [ ] **Step 6: 添加 Blueprint 编译步骤到 `meson.build`**

修改 `meson.build`：
```meson
project('photo-viewer', 'rust',
  version : '0.1.0',
  meson_version : '>= 0.59.0',
)

gnome = import('gnome')
blueprint_compiler = find_program('blueprint-compiler', required: false)

if blueprint_compiler.found()
  blueprint_files = files(
    'data/ui/window.blp',
  )
  generated_sources = []
  foreach blp : blueprint_files
    out = configure_file(
      input: blp,
      output: '@BASENAME@.ui',
      command: [blueprint_compiler, 'compile', '--output', '@OUTPUT@', '@INPUT@'],
    )
    generated_sources += out
  endforeach
endif

install_data(
  'data/org.gnome.PhotoViewer.desktop',
  install_dir : get_option('datadir') / 'applications',
)
```

- [ ] **Step 7: 编译并运行验证**

Run: `cargo build`
Expected: 编译成功

Run: `cargo run`
Expected: 窗口出现，含侧边栏（Photos/Albums/Trash）+ 空白内容区；窗口可关闭

- [ ] **Step 8: Commit**

```bash
git add src/ui/ data/ui/window.blp src/app.rs src/lib.rs meson.build
git commit -m "feat(ui): main window with OverlaySplitView

AdwApplicationWindow + AdwOverlaySplitView + AdwNavigationView
骨架；侧边栏 ListBox 含 Photos/Albums/Trash 三个 row
（M1 仅 Photos 实际工作，其他为占位）。"
```

---

## Task 10: GListStore 启动加载

**Files:**
- Modify: `src/app.rs`
- Create: `src/ui/photos_page.rs`
- Create: `data/ui/photos-page.blp`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/window.rs`
- Modify: `meson.build`

**Interfaces:**
- Produces: `photo_viewer::ui::photos_page::PhotosPage`
- Produces: `PhotosPage::new(media_list: gio::ListStore) -> Self`
- Produces: `PhotosPage::media_list() -> gio::ListStore`

- [ ] **Step 1: 创建占位 `data/ui/photos-page.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="PhotosPage" parent="AdwNavigationPage">
    <property name="title">Photos</property>
    <child>
      <object class="GtkBox" id="root_box">
        <property name="orientation">vertical</property>
        <child>
          <object class="AdwHeaderBar" id="header_bar">
            <property name="show-end-title-buttons">true</property>
          </object>
        </child>
        <child>
          <object class="GtkLabel" id="placeholder_label">
            <property name="label">M1 占位：MediaGrid 在 Task 12 加入</property>
            <property name="vexpand">true</property>
            <property name="halign">center</property>
            <property name="valign">center</property>
          </object>
        </child>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 2: 创建 `src/ui/photos_page.rs`**

```rust
//! PhotosPage：年/月/日视图（M1 占位，M1-Task 12 加入真实网格）
use gtk::prelude::*;
use gtk::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Ref;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gio::ListStore>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PhotosPage {
        const NAME: &'static str = "PhotosPage";
        type Type = super::PhotosPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotosPage {}
    impl WidgetImpl for PhotosPage {}
    impl NavigationPageImpl for PhotosPage {}
}

glib::wrapper! {
    pub struct PhotosPage(ObjectSubclass<imp::PhotosPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl PhotosPage {
    pub fn new(media_list: gio::ListStore) -> Self {
        let obj: Self = glib::Object::builder().build();
        *obj.imp().media_list.borrow_mut() = Some(media_list);
        obj
    }

    pub fn media_list(&self) -> Ref<'_, Option<gio::ListStore>> {
        self.imp().media_list.borrow()
    }
}
```

- [ ] **Step 3: 修改 `src/ui/mod.rs`**

```rust
pub mod photos_page;
pub mod window;

pub use photos_page::PhotosPage;
pub use window::MainWindow;
```

- [ ] **Step 4: 修改 `src/ui/window.rs` — 添加 `nav_view` 访问**

在 `imp` 模块中添加：
```rust
#[derive(Default)]
pub struct MainWindow {
    pub nav_view: OnceCell<adw::NavigationView>,
}
```

（替换原 `MainWindow {}` 空 struct）

```rust
use std::cell::OnceCell;
```

并在 `instance_init` 后初始化 nav_view：

修改 `instance_init`:
```rust
fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
    obj.init_template();
}
```
**保持不变**，但需要外部 `setup_nav_view` 方法。

在 MainWindow impl 中添加：
```rust
pub fn setup_nav_view(&self) -> adw::NavigationView {
    let nav: adw::NavigationView = self.imp().nav_view.get().unwrap().clone();
    nav
}
```

并在 bind_template 后通过 template id 获取：
```rust
fn class_init(klass: &mut Self::Class) {
    klass.bind_template();
}
```
**保持不变**。模板会自动绑定名为 `nav_view` 的子对象。

- [ ] **Step 5: 修改 `src/app.rs` — 加载媒体列表并 push PhotosPage**

```rust
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use photo_viewer::core::{init_pool, LocalBackend};
use photo_viewer::ui::{MainWindow, PhotosPage};
use std::sync::Arc;

pub fn build_app() -> adw::Application {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer")
        .build();

    app.connect_activate(|app| {
        let window = MainWindow::new(app);
        window.populate_sidebar();

        // 异步初始化 DB + 扫描
        let app_handle = app.clone();
        glib::MainContext::default().spawn_local(async move {
            match initialize().await {
                Ok(media_list) => {
                    let window: MainWindow = app_handle
                        .active_window()
                        .and_downcast::<MainWindow>()
                        .expect("MainWindow not found");
                    let nav = window.imp().nav_view.borrow().clone()
                        .expect("nav_view not initialized");
                    let photos = PhotosPage::new(media_list);
                    nav.push(&photos);
                }
                Err(e) => {
                    tracing::error!("初始化失败: {}", e);
                }
            }
        });

        window.present();
    });

    app
}

async fn initialize() -> anyhow::Result<gtk::gio::ListStore> {
    use photo_viewer::core::db;

    let data_dir = photo_viewer::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let pool = init_pool(&data_dir.join("photos.db"))?;

    // 启动扫描（这里只触发，更新在 Task 11）
    let backend = LocalBackend::new(pool.clone());
    let _ = backend;  // 暂时未使用，M1-T11 接入

    // 加载已有数据
    let items = db::list_all_media(&pool)?;
    let list = gtk::gio::ListStore::new::<photo_viewer::core::MediaItem>();
    for item in items {
        list.append(&item);
    }
    Ok(list)
}
```

需要 `OnceCell` import：

在 `src/ui/window.rs` 添加：
```rust
use std::cell::OnceCell;
```

并将 `imp::MainWindow` 结构改为：
```rust
#[derive(Default)]
pub struct MainWindow {
    pub nav_view: OnceCell<adw::NavigationView>,
}
```

且初始化 nav_view。在 `class_init` 后添加（GTK 模板绑定会自动填入 nav_view 字段）。

实际 GTK4 模板机制：bind_template 后，模板子对象通过 `obj.imp().nav_view` 可直接访问。但需要确保是 OnceCell 或 OnceCell<adw::NavigationView> 类型。

简化版本 — 直接用 `template` 中的 id：

修改 `data/ui/window.blp` 中的 nav_view：
```xml
<object class="AdwNavigationView" id="nav_view">
  <property name="vexpand">true</property>
</object>
```

并在 `imp::MainWindow` 添加：
```rust
#[derive(Default)]
pub struct MainWindow {
    pub nav_view: gtk::TemplateChild<adw::NavigationView>,
}
```

使用 `TemplateChild` 替代手写 ID 访问。修改 window.rs 完整版：

```rust
use gtk::prelude::*;
use gtk::{glib, ListBoxRow};
use libadwaita as adw;
use libadwaita::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MainWindow {
        #[template_child]
        pub sidebar_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub nav_view: TemplateChild<adw::NavigationView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "PhotoViewerWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MainWindow {}
    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}
    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}
}

glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends adw::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    pub fn nav_view(&self) -> adw::NavigationView {
        self.imp().nav_view.get().clone()
    }

    pub fn populate_sidebar(&self) {
        let list = &*self.imp().sidebar_list;
        for label in ["Photos", "Albums", "Trash"] {
            let row = ListBoxRow::new();
            let lbl = gtk::Label::builder()
                .label(label)
                .halign(gtk::Align::Start)
                .margin_start(12)
                .margin_end(12)
                .margin_top(8)
                .margin_bottom(8)
                .build();
            row.set_child(Some(&lbl));
            list.append(&row);
        }
    }
}
```

- [ ] **Step 6: 更新 meson.build — 添加新 .blp 文件**

```meson
if blueprint_compiler.found()
  blueprint_files = files(
    'data/ui/window.blp',
    'data/ui/photos-page.blp',
  )
  # ... 同 Task 9
endif
```

- [ ] **Step 7: 编译并验证**

Run: `cargo build`
Expected: 编译成功

Run: `cargo run`
Expected: 窗口出现；初次启动 placeholder 显示 "M1 占位"；关闭无错误

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/ui/ data/ui/ meson.build
git commit -m "feat(ui): load media list and push PhotosPage

启动时初始化 SQLite，加载所有 media_items 到 gio::ListStore，
push PhotosPage 到 NavigationView；M1 占位 label 待 Task 12
替换为真实 MediaGrid。"
```

---

## Task 11: 启动扫描集成

**Files:**
- Modify: `src/app.rs`
- Create: `src/core/backend/scan_worker.rs`

**Interfaces:**
- Produces: `photo_viewer::core::backend::scan_worker::spawn_scan(pool, paths) -> JoinHandle<()>`

- [ ] **Step 1: 创建 `src/core/backend/scan_worker.rs`**

```rust
//! 后台扫描 worker：扫描 root_paths 后 upsert 到 DB
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use std::path::PathBuf;
use tokio::task::JoinHandle;

pub fn spawn_scan(pool: DbPool, paths: Vec<PathBuf>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let backend = LocalBackend::new(pool);
        for path in &paths {
            tracing::info!("开始扫描: {}", path.display());
            match backend.scan_dir(path) {
                Ok(items) => {
                    let total = items.len();
                    let mut upserted = 0;
                    for item in &items {
                        match backend.upsert(item) {
                            Ok(_) => upserted += 1,
                            Err(e) => tracing::warn!("upsert 失败 {}: {}", item.uri, e),
                        }
                    }
                    tracing::info!("扫描完成: {} 张图片（{} 新增/更新）", total, upserted);
                }
                Err(e) => tracing::error!("扫描失败 {}: {}", path.display(), e),
            }
        }
    })
}
```

- [ ] **Step 2: 修改 `src/core/backend/mod.rs`**

```rust
pub mod local;
pub mod scan_worker;
```

- [ ] **Step 3: 修改 `src/app.rs` — 接入扫描**

```rust
use photo_viewer::core::backend::scan_worker::spawn_scan;
```

在 `initialize()` 中添加：

```rust
async fn initialize() -> anyhow::Result<gtk::gio::ListStore> {
    use photo_viewer::core::db;

    let data_dir = photo_viewer::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let pool = init_pool(&data_dir.join("photos.db"))?;

    // 启动后台扫描（M1 占位：扫描 ~/Pictures）
    let paths = vec![photo_viewer::config::data_dir()
        .parent().unwrap()
        .join("Pictures")];
    let scan_handle = spawn_scan(pool.clone(), paths);

    // 同步等待扫描完成（M1 简单版；M5 可改为后台通知）
    let _ = scan_handle.await;

    // 加载所有数据
    let items = db::list_all_media(&pool)?;
    let list = gtk::gio::ListStore::new::<photo_viewer::core::MediaItem>();
    for item in items {
        list.append(&item);
    }
    Ok(list)
}
```

- [ ] **Step 4: 手动测试**

Run: `mkdir -p ~/Pictures && echo "test" > ~/Pictures/test.txt && cargo run`
Expected: 启动后日志显示 "开始扫描" + "扫描完成"（0 张图片）

Run: 在 `~/Pictures` 放一张 JPEG 图片，重新启动：
```bash
cargo run
```
Expected: 日志显示扫描到 1 张图片；PhotosPage placeholder 仍显示（grid 在 Task 12）

- [ ] **Step 5: Commit**

```bash
git add src/core/backend/scan_worker.rs src/core/backend/mod.rs src/app.rs
git commit -m "feat(scan): background scan worker integration

启动时扫描 ~/Pictures（M1 硬编码），upsert 到 SQLite；
扫描完成后再加载 ListStore（M1 同步等待，M5 可改为
增量通知）。"
```

---

## Task 12: MediaGrid 复用组件 + PhotoTile + Section Header

**Files:**
- Create: `src/ui/media_grid.rs`
- Create: `src/ui/photo_tile.rs`
- Create: `src/ui/section_header.rs`
- Create: `data/ui/media-grid.blp`
- Create: `data/ui/photo-tile.blp`
- Create: `data/ui/section-header.blp`
- Modify: `src/ui/photos_page.rs`
- Modify: `src/ui/mod.rs`
- Modify: `meson.build`

**Interfaces:**
- Produces: `photo_viewer::ui::media_grid::MediaGrid`
- Produces: `MediaGrid::new(media_list: gio::ListStore, mode: GroupBy) -> Self`
- Produces: `MediaGrid::set_mode(&self, mode: GroupBy)`
- Produces: `photo_viewer::ui::photo_tile::PhotoTile`
- Produces: `photo_viewer::ui::section_header::SectionHeader`

- [ ] **Step 1: 创建 `data/ui/photo-tile.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="PhotoTile" parent="GtkFlowBoxChild">
    <property name="width-request">96</property>
    <property name="height-request">96</property>
    <child>
      <object class="GtkOverlay" id="overlay">
        <child>
          <object class="GtkPicture" id="picture">
            <property name="content-fit">cover</property>
            <property name="vexpand">true</property>
            <property name="hexpand">true</property>
          </object>
        </child>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 2: 实现 `src/ui/photo_tile.rs`**

```rust
//! 单个图片缩略图瓦片（M1 占位灰色，M2 接缩略图加载）
use gtk::prelude::*;
use gtk::{gdk, glib};
use libadwaita as adw;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PhotoTile {
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PhotoTile {
        const NAME: &'static str = "PhotoTile";
        type Type = super::PhotoTile;
        type ParentType = gtk::FlowBoxChild;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotoTile {}
    impl WidgetImpl for PhotoTile {}
    impl FlowBoxChildImpl for PhotoTile {}
}

glib::wrapper! {
    pub struct PhotoTile(ObjectSubclass<imp::PhotoTile>)
        @extends gtk::FlowBoxChild, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PhotoTile {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_placeholder(&self) {
        // M1 占位：浅灰色背景
        let css = gtk::CssProvider::new();
        css.load_from_data("picture { background-color: #d0d0d0; }");
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        self.imp().picture.get().set_paintable(None::<&gdk::Paintable>);
    }
}

impl Default for PhotoTile {
    fn default() -> Self { Self::new() }
}
```

- [ ] **Step 3: 创建 `data/ui/section-header.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="SectionHeader" parent="GtkFlowBoxChild">
    <property name="width-request">96</property>
    <property name="height-request">96</property>
    <child>
      <object class="GtkLabel" id="label">
        <property name="halign">start</property>
        <property name="margin-start">12</property>
        <property name="margin-top">12</property>
        <property name="margin-bottom">6</property>
        <property name="css-classes">["heading"]</property>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 4: 实现 `src/ui/section_header.rs`**

```rust
//! Section header（M1 仅 label，M2 可加折叠按钮）
use gtk::prelude::*;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SectionHeader {
        #[template_child]
        pub label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SectionHeader {
        const NAME: &'static str = "SectionHeader";
        type Type = super::SectionHeader;
        type ParentType = gtk::FlowBoxChild;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SectionHeader {}
    impl WidgetImpl for SectionHeader {}
    impl FlowBoxChildImpl for SectionHeader {}
}

glib::wrapper! {
    pub struct SectionHeader(ObjectSubclass<imp::SectionHeader>)
        @extends gtk::FlowBoxChild, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl SectionHeader {
    pub fn new(text: &str) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().label.get().set_label(text);
        obj
    }
}
```

- [ ] **Step 5: 创建 `data/ui/media-grid.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="MediaGrid" parent="GtkScrolledWindow">
    <property name="vexpand">true</property>
    <property name="hexpand">true</property>
    <child>
      <object class="GtkFlowBox" id="flow_box">
        <property name="orientation">horizontal</property>
        <property name="homogeneous">true</property>
        <property name="selection-mode">single</property>
        <property name="column-spacing">4</property>
        <property name="row-spacing">4</property>
        <property name="max-children-per-line">12</property>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 6: 实现 `src/ui/media_grid.rs`**

```rust
//! MediaGrid 复用组件：年/月/日 三模式共用
use crate::core::media::MediaItem;
use crate::core::section_model::{group_items, GroupBy};
use crate::ui::photo_tile::PhotoTile;
use crate::ui::section_header::SectionHeader;
use gtk::prelude::*;
use gtk::{gio, glib};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MediaGrid {
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
        pub mode: Cell<GroupBy>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MediaGrid {
        const NAME: &'static str = "MediaGrid";
        type Type = super::MediaGrid;
        type ParentType = gtk::ScrolledWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MediaGrid {}
    impl WidgetImpl for MediaGrid {}
    impl ScrolledWindowImpl for MediaGrid {}
}

glib::wrapper! {
    pub struct MediaGrid(ObjectSubclass<imp::MediaGrid>)
        @extends gtk::ScrolledWindow, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

use std::cell::Cell;

impl MediaGrid {
    pub fn new(media_list: gio::ListStore, mode: GroupBy) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().mode.set(mode);
        obj.rebuild(media_list, mode);
        obj
    }

    pub fn set_mode(&self, media_list: gio::ListStore, mode: GroupBy) {
        self.imp().mode.set(mode);
        self.rebuild(media_list, mode);
    }

    pub fn mode(&self) -> GroupBy { self.imp().mode.get() }

    fn rebuild(&self, media_list: gio::ListStore, mode: GroupBy) {
        // 1. 提取所有 MediaItem
        let mut items = Vec::with_capacity(media_list.n_items() as usize);
        for i in 0..media_list.n_items() {
            if let Some(obj) = media_list.item(i) {
                if let Ok(item) = obj.downcast::<MediaItem>() {
                    items.push(item);
                }
            }
        }

        // 2. 分组
        let sections = group_items(&items, mode);

        // 3. 清空 flow_box
        let flow = self.imp().flow_box.get();
        flow.remove_all();

        // 4. 填充
        for section in sections {
            let header = SectionHeader::new(&section.label);
            flow.append(&header);
            for item in &section.items {
                let tile = PhotoTile::new();
                tile.set_placeholder();
                flow.append(&tile);
            }
        }
    }
}
```

- [ ] **Step 7: 修改 `src/ui/mod.rs`**

```rust
pub mod media_grid;
pub mod photo_tile;
pub mod photos_page;
pub mod section_header;
pub mod window;

pub use media_grid::MediaGrid;
pub use photo_tile::PhotoTile;
pub use photos_page::PhotosPage;
pub use section_header::SectionHeader;
pub use window::MainWindow;
```

- [ ] **Step 8: 修改 `src/ui/photos_page.rs` — 集成 MediaGrid**

完整重写：

```rust
//! PhotosPage：年/月/日视图（共享 MediaGrid，ViewSwitcher 切换）
use crate::core::section_model::GroupBy;
use crate::ui::media_grid::MediaGrid;
use gtk::prelude::*;
use gtk::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Ref;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gio::ListStore>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub view_switcher: TemplateChild<adw::ViewSwitcher>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PhotosPage {
        const NAME: &'static str = "PhotosPage";
        type Type = super::PhotosPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotosPage {}
    impl WidgetImpl for PhotosPage {}
    impl NavigationPageImpl for PhotosPage {}
}

glib::wrapper! {
    pub struct PhotosPage(ObjectSubclass<imp::PhotosPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl PhotosPage {
    pub fn new(media_list: gio::ListStore) -> Self {
        let obj: Self = glib::Object::builder().build();
        let media_list_clone = media_list.clone();
        *obj.imp().media_list.borrow_mut() = Some(media_list);

        // 创建三个 MediaGrid 实例
        let year_grid = MediaGrid::new(media_list_clone.clone(), GroupBy::Year);
        let month_grid = MediaGrid::new(media_list_clone.clone(), GroupBy::Month);
        let day_grid = MediaGrid::new(media_list_clone, GroupBy::Day);

        obj.imp().view_stack.add_titled(
            &year_grid,
            Some("year"),
            "年",
        );
        obj.imp().view_stack.add_titled(
            &month_grid,
            Some("month"),
            "月",
        );
        obj.imp().view_stack.add_titled(
            &day_grid,
            Some("day"),
            "日",
        );

        obj
    }

    pub fn media_list(&self) -> Ref<'_, Option<gio::ListStore>> {
        self.imp().media_list.borrow()
    }
}
```

- [ ] **Step 9: 重写 `data/ui/photos-page.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="PhotosPage" parent="AdwNavigationPage">
    <property name="title">Photos</property>
    <child>
      <object class="GtkBox" id="root_box">
        <property name="orientation">vertical</property>
        <child>
          <object class="AdwHeaderBar" id="header_bar">
            <property name="show-end-title-buttons">true</property>
          </object>
        </child>
        <child>
          <object class="AdwViewSwitcherBar" id="switcher_bar">
            <property name="stack">view_stack</property>
            <property name="reveal">true</property>
          </object>
        </child>
        <child>
          <object class="AdwViewStack" id="view_stack">
            <property name="vexpand">true</property>
          </object>
        </child>
      </object>
    </child>
  </template>
</interface>
```

> 注：使用 `AdwViewSwitcherBar` 而非 `AdwViewSwitcher`，因为它是底部栏样式（macOS Photos 风格）

- [ ] **Step 10: 更新 meson.build**

```meson
if blueprint_compiler.found()
  blueprint_files = files(
    'data/ui/window.blp',
    'data/ui/photos-page.blp',
    'data/ui/media-grid.blp',
    'data/ui/photo-tile.blp',
    'data/ui/section-header.blp',
  )
  # ...
endif
```

- [ ] **Step 11: 编译并验证**

Run: `cargo build 2>&1 | head -50`
Expected: 编译可能因 GTK subclass 细节有 warning，但能成功

Run: `cargo run`
Expected: 窗口出现；底部有三个 tab（年/月/日）；切换 tab 显示不同分组粒度的占位瓦片；瓦片是灰色块

- [ ] **Step 12: Commit**

```bash
git add src/ui/ data/ui/ meson.build
git commit -m "feat(ui): MediaGrid + PhotoTile + SectionHeader + ViewSwitcher

复用组件：MediaGrid 接受 (media_list, GroupBy) 构造；
ViewSwitcherBar 在 PhotosPage 底部切换年/月/日；
每个模式独立 MediaGrid 实例，切换不重建（性能更好）。"
```

---

## Task 13: 端到端集成测试

**Files:**
- Create: `tests/e2e_browsing.rs`

**Interfaces:**
- 无（仅测试）

- [ ] **Step 1: 写 E2E 测试 — 启动应用 + 加载测试目录**

`tests/e2e_browsing.rs`:
```rust
//! 端到端：扫描测试目录 + 加载到 GListStore + 分组验证
mod common;
use common::*;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;
use photo_viewer::core::section_model::{group_items, GroupBy};
use std::path::Path;

#[test]
fn full_flow_scan_and_group() {
    let dir = tmp_dir();
    let root = dir.path();

    // 准备测试数据：3 个日期的图片
    write_plain_jpeg(root, "2025-03-01_a.jpg");
    write_plain_jpeg(root, "2025-03-01_b.jpg");
    write_plain_jpeg(root, "2025-03-15.jpg");
    write_plain_jpeg(root, "2024-12-25.jpg");

    // 1. 扫描
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());
    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 4);

    // 2. upsert
    for item in &items {
        backend.upsert(item).unwrap();
    }

    // 3. 加载
    let loaded = db::list_all_media(&pool).unwrap();
    assert_eq!(loaded.len(), 4);

    // 4. 按日分组
    let sections = group_items(&loaded, GroupBy::Day);
    assert_eq!(sections.len(), 3, "应有 3 个不同日期");

    // 5. 按月分组
    let sections = group_items(&loaded, GroupBy::Month);
    assert_eq!(sections.len(), 2, "应有 2 个不同月份");

    // 6. 按年分组
    let sections = group_items(&loaded, GroupBy::Year);
    assert_eq!(sections.len(), 2, "应有 2 个不同年份");
}
```

- [ ] **Step 2: 运行测试，验证通过**

Run: `cargo test --test e2e_browsing`
Expected: PASS

- [ ] **Step 3: 运行所有测试套件**

Run: `cargo test`
Expected: 全部通过（无失败）

- [ ] **Step 4: Commit**

```bash
git add tests/e2e_browsing.rs
git commit -m "test: end-to-end browsing flow

扫描 → upsert → 加载 → 三种粒度分组的全链路验证。"
```

---

## Task 14: 文档与 README

**Files:**
- Create: `README.md`
- Create: `CONTRIBUTING.md`
- Create: `LICENSE`（MIT）

- [ ] **Step 1: 创建 `README.md`**

```markdown
# Photo Viewer

基于 GNOME (GTK4 + Libadwaita) 的高性能相册工具。

## 状态

M1: 基础浏览（年/月/日视图 + 本地扫描 + SQLite 索引）

## 构建

```bash
# 系统依赖 (Fedora)
sudo dnf install gtk4-devel libadwaita-devel gdk-pixbuf2-devel \
                 libheif-devel sqlite-devel

# 系统依赖 (Ubuntu)
sudo apt install libgtk-4-dev libadwaita-1-dev libgdk-pixbuf-2.0-dev \
                 libheif-dev libsqlite3-dev

cargo build
cargo run
```

## 测试

```bash
cargo test
```

## 架构

参见 [spec](docs/superpowers/specs/2026-06-20-gnome-photo-viewer-design.md)
和 [M1 plan](docs/superpowers/plans/2026-06-20-m1-foundation-and-browsing.md)。
```

- [ ] **Step 2: 创建 `LICENSE`（MIT）**

```
MIT License

Copyright (c) 2026

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, ...
（标准 MIT 文本）
```

- [ ] **Step 3: 创建 `CONTRIBUTING.md`**

```markdown
# Contributing

## 开发流程

1. Fork & clone
2. 创建特性分支
3. TDD：先写失败测试
4. 实现到通过
5. cargo fmt + cargo clippy
6. PR 提交

## 模块说明

- `core/`：数据层（DB、扫描、元数据），与 UI 解耦
- `ui/`：GTK widgets
- `platform/`：XDG 集成
```

- [ ] **Step 4: Commit**

```bash
git add README.md LICENSE CONTRIBUTING.md
git commit -m "docs: add README, LICENSE, CONTRIBUTING"
```

---

## Self-Review Checklist (执行时由 subagent 完成)

实现者应自审：
- [ ] `cargo test` 全部通过
- [ ] `cargo clippy --all-targets -- -D warnings` 无警告
- [ ] `cargo fmt --check` 无格式问题
- [ ] `cargo run` 启动应用 + 切换年/月/日视图均工作
- [ ] 所有 commit message 符合约定式提交

## M1 完成交付

执行完 14 个 task 后：
- ✅ Cargo 项目可编译
- ✅ 应用启动 → 扫描 ~/Pictures → 加载到 PhotosPage
- ✅ 年/月/日三种视图可切换，显示分组 section + 占位灰色瓦片
- ✅ 缩略图、Viewer、Albums、Trash、Editor 在 M2-M5 实现
- ✅ 所有单元测试 + 集成测试 + E2E 测试通过
- ✅ 项目文档齐全

下一步：开始 M2 计划（缩略图流水线 + ViewerPage + 手势 + 预加载）。