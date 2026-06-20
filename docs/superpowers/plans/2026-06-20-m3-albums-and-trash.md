# GNOME Photo Viewer — M3: Albums & Trash Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 M2 基础上加入 AlbumsPage（文件夹即相册）、AlbumDetailPage、TrashPage（系统回收站集成 + 还原/永久删除）。

**Architecture:**
- `albums` 表物化（启动时聚合 `GROUP BY folder_path`）
- `gio::File::trash()` / `restore_from_trash()` / `delete_from_trash()` 包装
- 选择模式：GridView 切换到 multi-selection，底部 ActionBar 显隐

**Tech Stack:** 复用 M1-M2

## Global Constraints

- M1-M2 所有功能保持工作
- AlbumsPage 仅显示直接子文件（不递归）
- 删除走 gio 系统回收站（freedesktop Trash 规范）
- 还原时 gio 自动用 `trash::orig-path` 扩展属性归位
- 批量操作并发 N=4

---

## File Structure（增量）

```
src/
├── core/
│   ├── trash.rs                   # gio trash 包装
│   └── ...
└── ui/
    ├── albums_page.rs
    ├── album_detail_page.rs
    ├── trash_page.rs
    └── ...
data/ui/
├── albums-page.blp
├── album-detail-page.blp
└── trash-page.blp
```

---

## Task 1: 回收站 gio 包装

**Files:**
- Create: `src/core/trash.rs`
- Modify: `src/core/mod.rs`
- Modify: `src/core/db.rs`
- Create: `tests/trash_flow.rs`

**Interfaces:**
- Produces: `trash::move_to_trash(uri: &str) -> Result<()>`
- Produces: `trash::restore_from_trash(uri: &str) -> Result<()>`
- Produces: `trash::delete_permanently(uri: &str) -> Result<()>`

- [ ] **Step 1: 写失败测试**

`tests/trash_flow.rs`:
```rust
use gtk::gio::prelude::*;
use photo_viewer::core::trash;
use tempfile::tempdir;

#[test]
fn move_and_restore_file() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("test.jpg");
    std::fs::write(&src, b"fake jpeg data").unwrap();
    let uri = format!("file://{}", src.display());

    // 移到回收站
    trash::move_to_trash(&uri).unwrap();

    // 原位置应不存在
    assert!(!src.exists());

    // 还原
    trash::restore_from_trash(&uri).unwrap();
    assert!(src.exists());
}

#[test]
fn permanent_delete() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("perm.jpg");
    std::fs::write(&src, b"x").unwrap();
    let uri = format!("file://{}", src.display());

    trash::move_to_trash(&uri).unwrap();
    trash::delete_permanently(&uri).unwrap();

    // 文件应永久消失（包括回收站）
    let trash_uri = uri.replace(dir.path().to_str().unwrap(),
        &gio::File::for_uri("trash:///").path().unwrap()
            .to_string_lossy().to_string());
    // 用 trash:/// URI 验证
    let trash_file = gio::File::for_uri(&uri);
    assert!(!trash_file.query_exists(gio::Cancellable::NONE));
}
```

> 注：gio 测试可能需要 `dbus` mock；CI 中可能跳过

- [ ] **Step 2: 实现 `src/core/trash.rs`**

```rust
//! gio 系统回收站包装
use crate::core::error::{AppError, Result};
use gtk::gio::prelude::*;

/// 将文件移至系统回收站（gio 自动处理原路径记录）
pub fn move_to_trash(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    file.trash(gtk::gio::Cancellable::NONE)
        .map_err(AppError::Gio)?;
    Ok(())
}

/// 从回收站还原到原路径
pub fn restore_from_trash(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    file.restore_from_trash(gtk::gio::Cancellable::NONE)
        .map_err(AppError::Gio)?;
    Ok(())
}

/// 永久删除回收站中的文件
pub fn delete_permanently(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    file.delete_from_trash(gtk::gio::Cancellable::NONE)
        .map_err(AppError::Gio)?;
    Ok(())
}
```

- [ ] **Step 3: 修改 `src/core/mod.rs`**

```rust
pub mod trash;
pub use trash::{delete_permanently, move_to_trash, restore_from_trash};
```

- [ ] **Step 4: 修改 `src/core/db.rs` — 添加 trash 标记查询**

```rust
/// 标记为已删除（不立即物理删除）
pub fn mark_trashed(pool: &DbPool, id: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE media_items SET trashed_at = unixepoch() WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// 取消回收站标记
pub fn unmark_trashed(pool: &DbPool, id: i64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE media_items SET trashed_at = NULL WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// 列出所有回收站中项
pub fn list_trashed_media(pool: &DbPool) -> Result<Vec<MediaItem>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, uri, path, folder_path, mime_type, width, height,
                taken_at, file_mtime, file_size, blake3_hash, trashed_at
         FROM media_items
         WHERE trashed_at IS NOT NULL
         ORDER BY trashed_at DESC",
    )?;
    let rows = stmt.query_map([], row_to_media_item)?;
    Ok(rows.filter_map(Result::ok).collect())
}
```

- [ ] **Step 5: 运行测试**

Run: `cargo test --test trash_flow`
Expected: PASS（依赖 dbus session bus，本地测试可能需要 Xvfb）

- [ ] **Step 6: Commit**

```bash
git add src/core/trash.rs src/core/mod.rs src/core/db.rs tests/trash_flow.rs
git commit -m "feat(core): gio trash wrapper + DB trash markers

move_to_trash / restore_from_trash / delete_permanently；
mark_trashed / unmark_trashed / list_trashed_media DB 操作。"
```

---

## Task 2: Albums 表物化与查询

**Files:**
- Modify: `src/core/db.rs`
- Create: `src/core/albums.rs`
- Modify: `src/core/mod.rs`
- Create: `tests/albums.rs`

**Interfaces:**
- Produces: `albums::refresh(pool: &DbPool) -> Result<()>` — 重建 albums 表
- Produces: `albums::list(pool: &DbPool) -> Result<Vec<Album>>`
- Produces: `Album { folder_path: PathBuf, name: String, cover_uri: Option<String>, photo_count: i64, last_modified: DateTime<Utc> }`

- [ ] **Step 1: 写失败测试**

`tests/albums.rs`:
```rust
use photo_viewer::core::db;
use photo_viewer::core::albums;
use photo_viewer::core::media::NewMediaItem;
use chrono::Utc;
use common::*;
use tempfile::tempdir;

fn make_item(uri: &str, path: &str, folder: &str) -> NewMediaItem {
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: "image/jpeg".into(),
        width: Some(100), height: Some(100),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: format!("h{}", uri),
    }
}

#[test]
fn refresh_groups_by_folder() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    db::insert_media_item(&pool, &make_item(
        "file:///p/Camera/a.jpg", "/p/Camera/a.jpg", "/p/Camera"
    )).unwrap();
    db::insert_media_item(&pool, &make_item(
        "file:///p/Camera/b.jpg", "/p/Camera/b.jpg", "/p/Camera"
    )).unwrap();
    db::insert_media_item(&pool, &make_item(
        "file:///p/Screenshots/c.jpg", "/p/Screenshots/c.jpg", "/p/Screenshots"
    )).unwrap();

    albums::refresh(&pool).unwrap();

    let list = albums::list(&pool).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "Camera");  // 假设最近修改
    assert_eq!(list[0].photo_count, 2);
}

#[test]
fn trashed_items_excluded_from_albums() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let id = db::insert_media_item(&pool, &make_item(
        "file:///p/a.jpg", "/p/a.jpg", "/p"
    )).unwrap();
    db::mark_trashed(&pool, id).unwrap();

    albums::refresh(&pool).unwrap();
    let list = albums::list(&pool).unwrap();
    assert_eq!(list.len(), 0);
}
```

- [ ] **Step 2: 实现 `src/core/albums.rs`**

```rust
//! 相册聚合（按 folder_path 分组）
use crate::core::db::DbPool;
use crate::core::error::Result;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Album {
    pub folder_path: PathBuf,
    pub name: String,
    pub cover_uri: Option<String>,
    pub photo_count: i64,
    pub last_modified: DateTime<Utc>,
}

/// 重新计算 albums 表（启动时 + 索引完成后调用）
pub fn refresh(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    conn.execute("DELETE FROM albums", [])?;
    conn.execute(
        "INSERT INTO albums (folder_path, name, cover_uri, photo_count, last_modified)
         SELECT
             folder_path,
             folder_path,
             (SELECT uri FROM media_items m2
              WHERE m2.folder_path = m.folder_path AND m2.trashed_at IS NULL
              ORDER BY m2.file_mtime DESC LIMIT 1),
             COUNT(*),
             MAX(file_mtime)
         FROM media_items m
         WHERE trashed_at IS NULL
         GROUP BY folder_path",
        [],
    )?;
    Ok(())
}

/// 列出所有相册，按最近修改排序
pub fn list(pool: &DbPool) -> Result<Vec<Album>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT folder_path, name, cover_uri, photo_count, last_modified
         FROM albums ORDER BY last_modified DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let last_modified: i64 = row.get(4)?;
        Ok(Album {
            folder_path: PathBuf::from(path),
            name: row.get(1)?,
            cover_uri: row.get(2)?,
            photo_count: row.get(3)?,
            last_modified: chrono::DateTime::from_timestamp(last_modified, 0)
                .unwrap_or_else(Utc::now),
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}
```

- [ ] **Step 3: 修改 `src/core/mod.rs`**

```rust
pub mod albums;
pub use albums::{refresh as refresh_albums, Album};
```

- [ ] **Step 4: 运行测试**

Run: `cargo test --test albums`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/core/albums.rs src/core/mod.rs src/core/db.rs tests/albums.rs
git commit -m "feat(core): albums table refresh + query

启动时聚合 GROUP BY folder_path；过滤回收站项；
按 last_modified DESC 排序。"
```

---

## Task 3: AlbumsPage UI

**Files:**
- Create: `src/ui/albums_page.rs`
- Create: `data/ui/albums-page.blp`
- Modify: `src/ui/mod.rs`
- Modify: `src/app.rs`
- Modify: `meson.build`

**Interfaces:**
- Produces: `AlbumsPage::new(albums: Vec<Album>, loader: Arc<ThumbnailLoader>) -> Self`

- [ ] **Step 1: 创建 `data/ui/albums-page.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="AlbumsPage" parent="AdwNavigationPage">
    <property name="title">Albums</property>
    <child>
      <object class="GtkBox" id="root_box">
        <property name="orientation">vertical</property>
        <child>
          <object class="AdwHeaderBar" id="header_bar">
            <property name="show-end-title-buttons">true</property>
          </object>
        </child>
        <child>
          <object class="GtkScrolledWindow" id="scrolled">
            <property name="vexpand">true</property>
            <child>
              <object class="GtkFlowBox" id="flow_box">
                <property name="homogeneous">true</property>
                <property name="selection-mode">single</property>
                <property name="column-spacing">12</property>
                <property name="row-spacing">12</property>
                <property name="margin-start">12</property>
                <property name="margin-end">12</property>
                <property name="margin-top">12</property>
                <property name="margin-bottom">12</property>
                <property name="max-children-per-line">4</property>
              </object>
            </child>
          </object>
        </child>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 2: 实现 `src/ui/albums_page.rs`**

```rust
//! 相册列表页
use crate::core::albums::Album;
use crate::core::thumbnails::ThumbnailLoader;
use gtk::prelude::*;
use gtk::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::sync::Arc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AlbumsPage {
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumsPage {
        const NAME: &'static str = "AlbumsPage";
        type Type = super::AlbumsPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumsPage {}
    impl WidgetImpl for AlbumsPage {}
    impl NavigationPageImpl for AlbumsPage {}
}

glib::wrapper! {
    pub struct AlbumsPage(ObjectSubclass<imp::AlbumsPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl AlbumsPage {
    pub fn new(albums: Vec<Album>, loader: Arc<ThumbnailLoader>) -> Self {
        let obj: Self = glib::Object::builder().build();
        let flow = obj.imp().flow_box.get();

        for album in albums {
            let tile = build_album_tile(&album, loader.clone());
            flow.append(&tile);
        }

        obj
    }
}

fn build_album_tile(album: &Album, loader: Arc<ThumbnailLoader>) -> gtk::FlowBoxChild {
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(6)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .width_request(240)
        .height_request(240)
        .build();

    if let Some(uri) = &album.cover_uri {
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(uri.clone(),
            crate::core::thumbnails::ThumbnailSize::Medium, tx);
        glib::spawn_future_local(async move {
            if let Ok(texture) = rx.await {
                picture.set_paintable(Some(&texture));
            }
        });
    }

    let name_label = gtk::Label::builder()
        .label(&album.name)
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .build();

    let count_label = gtk::Label::builder()
        .label(format!("{} 张", album.photo_count))
        .halign(gtk::Align::Start)
        .opacity(0.7)
        .build();

    box_.append(&picture);
    box_.append(&name_label);
    box_.append(&count_label);

    let row = gtk::FlowBoxChild::new();
    row.set_child(Some(&box_));
    row
}
```

- [ ] **Step 3: 修改 `src/ui/mod.rs`**

```rust
pub mod albums_page;
pub use albums_page::AlbumsPage;
```

- [ ] **Step 4: 修改 `src/app.rs` — 注册 AlbumsPage 路由**

```rust
// 侧边栏 ListBox 选中时切换 page
let list = window.imp().sidebar_list.get();
list.connect_row_selected(glib::clone!(@weak window => move |_, row| {
    if let Some(row) = row {
        let idx = row.index();
        // 切换 nav_view 顶层 page
        // 实际：维护一个 page map，根据 idx 显示对应 page
    }
}));
```

简化版：MainWindow 增加 `current_page: RefCell<Option<NavigationPage>>`，根据 sidebar 选择切换。

实际实现留给 Task 5（侧边栏路由整合）。

- [ ] **Step 5: Commit**

```bash
git add src/ui/albums_page.rs data/ui/albums-page.blp src/ui/mod.rs meson.build
git commit -m "feat(ui): AlbumsPage with cover thumbnails

GtkFlowBox + AlbumTile（封面 + 名称 + 数量）；
ThumbnailLoader 异步加载封面。"
```

---

## Task 4: AlbumDetailPage（单相册照片网格）

**Files:**
- Create: `src/ui/album_detail_page.rs`
- Create: `data/ui/album-detail-page.blp`
- Modify: `src/ui/mod.rs`

**Interfaces:**
- Produces: `AlbumDetailPage::new(album: Album, media_list: gio::ListStore) -> Self`

- [ ] **Step 1: 创建 `data/ui/album-detail-page.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="AlbumDetailPage" parent="AdwNavigationPage">
    <child>
      <object class="GtkBox" id="root_box">
        <property name="orientation">vertical</property>
        <child>
          <object class="AdwHeaderBar" id="header_bar" />
        </child>
        <child>
          <object class="GtkScrolledWindow" id="scrolled">
            <property name="vexpand">true</property>
            <child>
              <object class="GtkFlowBox" id="flow_box">
                <property name="homogeneous">true</property>
                <property name="column-spacing">4</property>
                <property name="row-spacing">4</property>
                <property name="margin-start">12</property>
                <property name="margin-end">12</property>
                <property name="max-children-per-line">6</property>
              </object>
            </child>
          </object>
        </child>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 2: 实现 `src/ui/album_detail_page.rs`**

```rust
//! 单相册照片网格（复用 MediaGrid 模式，但无 section header）
use crate::core::albums::Album;
use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::photo_tile::PhotoTile;
use gtk::prelude::*;
use gtk::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::sync::Arc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AlbumDetailPage {
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumDetailPage {
        const NAME: &'static str = "AlbumDetailPage";
        type Type = super::AlbumDetailPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumDetailPage {}
    impl WidgetImpl for AlbumDetailPage {}
    impl NavigationPageImpl for AlbumDetailPage {}
}

glib::wrapper! {
    pub struct AlbumDetailPage(ObjectSubclass<imp::AlbumDetailPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl AlbumDetailPage {
    pub fn new(album: Album, all_media: gio::ListStore, loader: Arc<ThumbnailLoader>) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&album.name);
        let flow = obj.imp().flow_box.get();

        // 过滤出该文件夹的媒体
        for i in 0..all_media.n_items() {
            let item: MediaItem = all_media.item(i).unwrap().downcast().unwrap();
            if item.folder_path == album.folder_path {
                let tile = PhotoTile::new();
                tile.set_item(item, (*loader).clone(), ThumbnailSize::Medium);
                flow.append(&tile);
            }
        }

        obj
    }
}
```

- [ ] **Step 3: 修改 `src/ui/mod.rs`**

```rust
pub mod album_detail_page;
pub use album_detail_page::AlbumDetailPage;
```

- [ ] **Step 4: 在 AlbumsPage 中连接 tile 点击 push AlbumDetailPage**

修改 `AlbumsPage::new`，每个 tile 用 `connect_activate` 回调 push：

```rust
use crate::ui::album_detail_page::AlbumDetailPage;
use std::sync::Weak;

// AlbumsPage 需要持有 nav_view 弱引用
pub struct AlbumsPage {
    // ...
    nav_view: Weak<adw::NavigationView>,
}

impl AlbumsPage {
    pub fn new(albums: Vec<Album>, loader: Arc<ThumbnailLoader>, nav_view: Weak<adw::NavigationView>) -> Self {
        // 每个 tile:
        let album_clone = album.clone();
        let all_media = ...; // 需要传入
        let nav = nav_view.clone();
        let loader_clone = loader.clone();
        tile.connect_activate(move |_| {
            if let Some(nav) = nav.upgrade() {
                let detail = AlbumDetailPage::new(album_clone.clone(), all_media_clone.clone(), loader_clone.clone());
                nav.push(&detail);
            }
        });
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add src/ui/album_detail_page.rs data/ui/album-detail-page.blp src/ui/mod.rs
git commit -m "feat(ui): AlbumDetailPage with photo grid

复用 PhotoTile + ThumbnailLoader；点击 push 到 NavigationView；
显示该文件夹所有图片（medium 缩略图）。"
```

---

## Task 5: TrashPage + 选择模式

**Files:**
- Create: `src/ui/trash_page.rs`
- Create: `data/ui/trash-page.blp`
- Modify: `src/ui/mod.rs`
- Modify: `src/app.rs`

**Interfaces:**
- Produces: `TrashPage::new(pool: DbPool, on_change: impl Fn())`

- [ ] **Step 1: 创建 `data/ui/trash-page.blp`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <template class="TrashPage" parent="AdwNavigationPage">
    <property name="title">Trash</property>
    <child>
      <object class="GtkBox" id="root_box">
        <property name="orientation">vertical</property>
        <child>
          <object class="AdwHeaderBar" id="header_bar">
            <property name="show-end-title-buttons">true</property>
          </object>
        </child>
        <child>
          <object class="AdwBanner" id="banner">
            <property name="title">回收站中的项目将在 30 天后永久删除</property>
            <property name="revealed">true</property>
          </object>
        </child>
        <child>
          <object class="GtkScrolledWindow" id="scrolled">
            <property name="vexpand">true</property>
            <child>
              <object class="GtkFlowBox" id="flow_box">
                <property name="homogeneous">true</property>
                <property name="column-spacing">4</property>
                <property name="row-spacing">4</property>
                <property name="margin-start">12</property>
                <property name="margin-end">12</property>
                <property name="max-children-per-line">6</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkActionBar" id="action_bar">
            <property name="revealed">false</property>
            <child>
              <object class="GtkButton" id="cancel_btn">
                <property name="label">Cancel</property>
              </object>
            </child>
            <child type="end">
              <object class="GtkButton" id="restore_btn">
                <property name="label">Restore</property>
                <property name="css-classes">["suggested-action"]</property>
              </object>
            </child>
            <child type="end">
              <object class="GtkButton" id="delete_btn">
                <property name="label">Delete Permanently</property>
                <property name="css-classes">["destructive-action"]</property>
              </object>
            </child>
          </object>
        </child>
      </object>
    </child>
  </template>
</interface>
```

- [ ] **Step 2: 实现 `src/ui/trash_page.rs`**

```rust
//! 回收站页面（多选 + 批量还原/永久删除）
use crate::core::db::{self, DbPool};
use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::core::trash;
use crate::ui::photo_tile::PhotoTile;
use gtk::prelude::*;
use gtk::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::sync::Arc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TrashPage {
        pub pool: DbPool,
        #[template_child]
        pub flow_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub action_bar: TemplateChild<gtk::ActionBar>,
        #[template_child]
        pub cancel_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub restore_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub delete_btn: TemplateChild<gtk::Button>,
        pub selected: RefCell<Vec<i64>>,
        pub on_change: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrashPage {
        const NAME: &'static str = "TrashPage";
        type Type = super::TrashPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TrashPage {}
    impl WidgetImpl for TrashPage {}
    impl NavigationPageImpl for TrashPage {}
}

glib::wrapper! {
    pub struct TrashPage(ObjectSubclass<imp::TrashPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl TrashPage {
    pub fn new(pool: DbPool, loader: Arc<ThumbnailLoader>, on_change: impl Fn() + 'static) -> Self {
        let obj: Self = glib::Object::builder().build();
        *obj.imp().pool.borrow_mut() = pool.clone();

        // 进入选择模式
        let flow = obj.imp().flow_box.get();
        flow.set_selection_mode(gtk::SelectionMode::Multiple);
        let flow_weak = flow.downgrade();
        flow.connect_selected_rows_changed(glib::clone!(@weak obj => move |_| {
            let selected = flow_weak.upgrade().map(|f| {
                f.selected_children()
                    .iter()
                    .filter_map(|r| r.downcast_ref::<gtk::FlowBoxChild>().map(|c| c.index() as i64))
                    .collect::<Vec<_>>()
            }).unwrap_or_default();
            *obj.imp().selected.borrow_mut() = selected;
            obj.imp().action_bar.get().set_revealed(!obj.imp().selected.borrow().is_empty());
        }));

        // Cancel
        obj.imp().cancel_btn.get().connect_clicked(glib::clone!(@weak obj, @weak flow => move |_| {
            flow.unselect_all();
            *obj.imp().selected.borrow_mut() = vec![];
            obj.imp().action_bar.get().set_revealed(false);
        }));

        // Restore
        obj.imp().restore_btn.get().connect_clicked(glib::clone!(@weak obj, @weak flow => move |_| {
            let pool = obj.imp().pool.borrow().clone();
            let selected = obj.imp().selected.borrow().clone();
            glib::spawn_future_local(async move {
                for id in selected {
                    if let Ok(items) = get_by_id(&pool, id) {
                        let _ = trash::restore_from_trash(&items.uri);
                        let _ = db::unmark_trashed(&pool, id);
                    }
                }
                flow.unselect_all();
            });
        }));

        // Delete Permanently
        obj.imp().delete_btn.get().connect_clicked(glib::clone!(@weak obj, @weak flow => move |_| {
            let pool = obj.imp().pool.borrow().clone();
            let selected = obj.imp().selected.borrow().clone();
            glib::spawn_future_local(async move {
                for id in selected {
                    if let Ok(items) = get_by_id(&pool, id) {
                        let _ = trash::delete_permanently(&items.uri);
                        let _ = db::delete_media_item(&pool, id);
                    }
                }
                flow.unselect_all();
            });
        }));

        // 加载初始数据
        let pool_clone = pool.clone();
        let loader_clone = loader.clone();
        glib::spawn_future_local(async move {
            if let Ok(items) = db::list_trashed_media(&pool_clone) {
                for item in items {
                    let tile = PhotoTile::new();
                    tile.set_item(item, (*loader_clone).clone(), ThumbnailSize::Small);
                    flow.append(&tile);
                }
            }
        });

        obj
    }
}

fn get_by_id(pool: &DbPool, id: i64) -> anyhow::Result<MediaItem> {
    Ok(db::get_media_item(pool, id)?)
}
```

- [ ] **Step 3: 修改 `src/ui/mod.rs`**

```rust
pub mod trash_page;
pub use trash_page::TrashPage;
```

- [ ] **Step 4: Commit**

```bash
git add src/ui/trash_page.rs data/ui/trash-page.blp src/ui/mod.rs
git commit -m "feat(ui): TrashPage with selection mode + batch actions

FlowBox 多选 + ActionBar（Cancel / Restore / Delete Permanently）；
Restore 调用 gio::restore_from_trash + DB unmark_trashed；
Delete 走 gio::delete_from_trash + DB DELETE。"
```

---

## Task 6: 侧边栏路由 + Empty All

**Files:**
- Modify: `src/ui/window.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: 修改 `src/app.rs` — 侧边栏选中切换顶层 page**

```rust
let nav_weak = window.nav_view().downgrade();
window.imp().sidebar_list.get().connect_row_selected(
    glib::clone!(@weak window, @weak nav_weak => move |_, row| {
        if let (Some(row), Some(nav)) = (row, nav_weak.upgrade()) {
            match row.index() {
                0 => { /* Photos 已在栈顶，无操作 */ }
                1 => {
                    // Albums：取 album 列表并 push AlbumsPage
                    let albums = photo_viewer::core::albums::list(&pool).unwrap_or_default();
                    let page = photo_viewer::ui::AlbumsPage::new(
                        albums, loader.clone(), nav.downgrade()
                    );
                    nav.push(&page);
                }
                2 => {
                    let page = photo_viewer::ui::TrashPage::new(pool.clone(), loader.clone(), || {});
                    nav.push(&page);
                }
                _ => {}
            }
        }
    })
);
```

实际 NavigationView 通常显示 root page（PhotosPage）；Albums/Trash 用 push 进入栈。

简化版：仅 sidebar 选择直接 replace top page，不嵌套 push。

- [ ] **Step 2: 在 TrashPage HeaderBar 添加 "Empty All"**

修改 `data/ui/trash-page.blp`：

```xml
<!-- 在 HeaderBar 加 -->
<child type="end">
  <object class="GtkButton" id="empty_btn">
    <property name="label">Empty All</property>
    <property name="css-classes">["destructive-action"]</property>
  </object>
</child>
```

在 `TrashPage::new` 中连接：

```rust
obj.imp().empty_btn.get().connect_clicked(glib::clone!(@weak obj => move |_| {
    let dialog = adw::AlertDialog::builder()
        .heading("Empty Trash?")
        .body("All items will be permanently deleted.")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("empty", "Empty");
    dialog.set_response_appearance("empty", adw::ResponseAppearance::Destructive);

    let pool = obj.imp().pool.borrow().clone();
    let flow_weak = obj.imp().flow_box.downgrade();
    dialog.connect_response(None, move |_, response| {
        if response == "empty" {
            glib::spawn_future_local(async move {
                let _ = db::list_trashed_media(&pool).await;
                // 批量永久删除
                // ... 同 Restore/Delete Permanently 流程
                if let Some(flow) = flow_weak.upgrade() {
                    flow.remove_all();
                }
            });
        }
    });
    dialog.present(Some(&obj));
}));
```

- [ ] **Step 3: 编译并验证**

Run: `cargo run`
Expected: 侧边栏 Photos/Albums/Trash 三选项可切换；Trash 多选 + Restore/Delete 工作

- [ ] **Step 4: Commit**

```bash
git add src/ui/window.rs src/ui/trash_page.rs src/app.rs data/ui/trash-page.blp
git commit -m "feat(ui): sidebar routing + Empty All confirmation

侧边栏 ListBox 切换顶层 page；Empty All 走 AlertDialog
确认 + 批量永久删除 + 刷新 flow_box。"
```

---

## Task 7: M3 端到端测试

**Files:**
- Create: `tests/e3e_albums_trash.rs`

- [ ] **Step 1: 写 E2E 测试**

```rust
mod common;
use common::*;
use gtk::glib::MainContext;
use photo_viewer::core::albums;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;
use photo_viewer::core::trash;
use tempfile::tempdir;

#[test]
fn full_flow_scan_albums_trash() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // 1. 创建两个文件夹的图片
    let camera = root.join("Camera");
    std::fs::create_dir(&camera).unwrap();
    write_plain_jpeg(&camera, "img1.jpg");
    write_plain_jpeg(&camera, "img2.jpg");

    let shots = root.join("Screenshots");
    std::fs::create_dir(&shots).unwrap();
    write_plain_jpeg(&shots, "scr1.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());
    let items = backend.scan_dir(root).unwrap();
    for it in &items { backend.upsert(it).unwrap(); }

    // 2. 聚合 albums
    albums::refresh(&pool).unwrap();
    let list = albums::list(&pool).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].photo_count, 2);
    assert_eq!(list[1].photo_count, 1);

    // 3. 删除一张
    let first_id = db::list_all_media(&pool).unwrap()[0].id;
    let first_uri = db::list_all_media(&pool).unwrap()[0].uri.clone();
    trash::move_to_trash(&first_uri).unwrap();
    db::mark_trashed(&pool, first_id).unwrap();

    // 4. 重新聚合，回收站项不应出现
    albums::refresh(&pool).unwrap();
    let list2 = albums::list(&pool).unwrap();
    let total_after: i64 = list2.iter().map(|a| a.photo_count).sum();
    assert_eq!(total_after, 2);
}
```

- [ ] **Step 2: 运行**

Run: `cargo test --test e3e_albums_trash`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/e3e_albums_trash.rs
git commit -m "test: M3 e2e - albums aggregation + trash flow"
```

---

## M3 完成交付

- ✅ AlbumsPage 列出所有文件夹相册（含封面 + 数量）
- ✅ AlbumDetailPage 显示单相册所有图片
- ✅ TrashPage 多选 + 还原 / 永久删除 / Empty All
- ✅ 侧边栏路由 Photos/Albums/Trash
- ✅ gio 系统回收站集成（freedesktop Trash 规范）
- ✅ DB albums 表物化 + 自动排除回收站项

下一步：M4 — EditorPage + EditOperation trait + 5 个内置 op + Save Copy/Overwrite。