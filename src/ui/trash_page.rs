//! TrashPage — 回收站页面（多选 + 批量还原/永久删除）
//!
//! 布局：
//! - `AdwHeaderBar`：标题栏
//! - `AdwBanner`：还原 / 手动永久删除提示
//! - `GtkScrolledWindow` + `GtkFlowBox`（multi-select）：显示已删除的媒体项
//! - `GtkActionBar`：底部操作栏（仅在有选中项时 reveal）
//!   - Cancel：清空选择
//!   - Restore：批量还原
//!   - Delete Permanently：批量永久删除
//!
//! 多选用 `GtkFlowBox::selected_children()` 收集被选中的子项索引，
//! 这些索引对应 `db::list_trashed_media` 返回的顺序 — 因此可用作
//! `MediaItem.id` 的查找键。
use std::cell::RefCell;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};
use libadwaita::subclass::prelude::*;

use crate::core::albums;
use crate::core::db::{self, DbPool};
use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::core::trash;
use crate::ui::empty_states;
use crate::ui::media_grid::square_tile::SquareTile;

const TRASH_TILE_PX: i32 = 270;
const TRASH_THUMB_SIZE: ThumbnailSize = ThumbnailSize::Large;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/trash-page.ui")]
    pub struct TrashPage {
        pub pool: RefCell<Option<DbPool>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub visible_items: RefCell<Vec<MediaItem>>,
        pub trashed_ids: RefCell<Vec<i64>>,
        #[template_child]
        pub scrolled: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub grid_viewport: TemplateChild<gtk::Viewport>,
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
        #[template_child]
        pub empty_btn: TemplateChild<gtk::Button>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for TrashPage {
        const NAME: &'static str = "TrashPage";
        type Type = super::TrashPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TrashPage {}
    impl WidgetImpl for TrashPage {}
    impl NavigationPageImpl for TrashPage {}
}

gtk::glib::wrapper! {
    pub struct TrashPage(ObjectSubclass<imp::TrashPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl TrashPage {
    /// 构造一个回收站页面。
    ///
    /// - `pool`：SQLite 连接池；用于查询 `trashed_at IS NOT NULL` 的项以及更新/删除
    /// - `loader`：缩略图加载器，用于填充每张已删除图片的缩略图
    pub fn new(pool: DbPool, loader: Arc<ThumbnailLoader>) -> Self {
        crate::ui::grid_css::install();

        let obj: Self = glib::Object::builder().build();
        *obj.imp().pool.borrow_mut() = Some(pool.clone());
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        let flow = obj.imp().flow_box.get();

        // 选择模式：FlowBox 多选
        flow.set_selection_mode(gtk::SelectionMode::Multiple);

        // 选中变化 → 维护 selected 列表 + 切换 ActionBar revealed
        flow.connect_selected_children_changed(glib::clone!(@weak obj => move |flow| {
            let visible_items = obj.imp().visible_items.borrow();
            let selected_indices = flow
                .selected_children()
                .iter()
                .filter_map(|c| c.downcast_ref::<gtk::FlowBoxChild>().map(|c| c.index()))
                .collect::<Vec<_>>();
            let selected = selected_ids_for_indices(&visible_items, selected_indices);
            *obj.imp().trashed_ids.borrow_mut() = selected;
            let revealed = !obj.imp().trashed_ids.borrow().is_empty();
            obj.imp().action_bar.get().set_revealed(revealed);
        }));

        // Cancel：清空选择 + 隐藏 ActionBar
        obj.imp().cancel_btn.get().connect_clicked(
            glib::clone!(@weak obj, @weak flow => move |_| {
                flow.unselect_all();
                *obj.imp().trashed_ids.borrow_mut() = vec![];
                obj.imp().action_bar.get().set_revealed(false);
            }),
        );

        // Restore：批量还原
        obj.imp().restore_btn.get().connect_clicked(
            glib::clone!(@weak obj, @weak flow => move |_| {
                let pool = match obj.imp().pool.borrow().as_ref() {
                    Some(p) => p.clone(),
                    None => return,
                };
                let ids = obj.imp().trashed_ids.borrow().clone();
                let page_weak = obj.downgrade();

                // 异步批处理：避免阻塞 UI
                glib::spawn_future_local(async move {
                    for id in ids {
                        if let Ok(item) = db::get_media_item(&pool, id) {
                            let _ = trash::restore_from_trash(&item.uri);
                            let _ = db::unmark_trashed(&pool, id);
                        }
                    }
                    // albums::refresh — 还原让文件回到原相册，侧栏下次重建 AlbumsPage
                    // 时拿到新计数。
                    let _ = albums::refresh(&pool);
                    flow.unselect_all();
                    // refresh — 让 FlowBox 反映 DB 最新状态（trashed_at=NULL 的项消失）
                    if let Some(page) = page_weak.upgrade() {
                        page.refresh();
                    }
                });
            }),
        );

        // Delete Permanently：批量永久删除
        obj.imp().delete_btn.get().connect_clicked(
            glib::clone!(@weak obj, @weak flow => move |_| {
                let pool = match obj.imp().pool.borrow().as_ref() {
                    Some(p) => p.clone(),
                    None => return,
                };
                let ids = obj.imp().trashed_ids.borrow().clone();
                let page_weak = obj.downgrade();

                glib::spawn_future_local(async move {
                    for id in ids {
                        if let Ok(item) = db::get_media_item(&pool, id) {
                            let _ = trash::delete_permanently(&item.uri);
                            let _ = db::delete_media_item(&pool, id);
                        }
                    }
                    // albums::refresh — 永久删除让侧栏相册计数相应减少。
                    let _ = albums::refresh(&pool);
                    flow.unselect_all();
                    // refresh — 完整刷新 FlowBox（全空时自动切到空状态页面），
                    // 避免部分删除后残留旧 tile。
                    if let Some(page) = page_weak.upgrade() {
                        page.refresh();
                    }
                });
            }),
        );

        // Empty All：弹 AdwAlertDialog 确认后批量永久删除所有回收站项
        obj.imp()
            .empty_btn
            .get()
            .connect_clicked(glib::clone!(@weak obj => move |_| {
                let pool = match obj.imp().pool.borrow().as_ref() {
                    Some(p) => p.clone(),
                    None => return,
                };
                let page_weak = obj.downgrade();

                let dialog = adw::AlertDialog::builder()
                    .heading("Empty Trash?")
                    .body("All items in trash will be permanently deleted.")
                    .build();
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("empty", "Empty");
                dialog.set_response_appearance("empty", adw::ResponseAppearance::Destructive);

                dialog.connect_response(
                    None,
                    move |_, response| {
                        if response == "empty" {
                            let pool = pool.clone();
                            let page_weak = page_weak.clone();
                            glib::spawn_future_local(async move {
                                if let Ok(items) = db::list_trashed_media(&pool) {
                                    for item in items {
                                        let _ = trash::delete_permanently(&item.uri);
                                        let _ = db::delete_media_item(&pool, item.id);
                                    }
                                }
                                // albums::refresh — 清空回收站后侧栏相册计数同步。
                                let _ = albums::refresh(&pool);
                                // refresh — 全删后 DB 已空，refresh 内部会切到空状态页面。
                                if let Some(page) = page_weak.upgrade() {
                                    page.refresh();
                                }
                            });
                        }
                    },
                );

                dialog.present(&obj);
            }));

        // 加载初始数据
        let pool_clone = pool.clone();
        let loader_clone = loader.clone();
        let flow_weak = obj.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(items) = db::list_trashed_media(&pool_clone) {
                if items.is_empty() {
                    if let Some(obj) = flow_weak.upgrade() {
                        *obj.imp().visible_items.borrow_mut() = vec![];
                        show_empty_trash(&obj);
                    }
                } else {
                    if let Some(obj) = flow_weak.upgrade() {
                        *obj.imp().visible_items.borrow_mut() = items.clone();
                    }
                    for item in items {
                        let tile = build_trash_tile(item, loader_clone.clone());
                        if let Some(obj) = flow_weak.upgrade() {
                            obj.imp().flow_box.get().append(&tile);
                        } else {
                            // Page 已销毁，丢弃 tile
                            break;
                        }
                    }
                }
            }
        });

        obj
    }

    /// 刷新回收站项（清空当前 FlowBox 并重新加载）
    pub fn refresh(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().clone() else {
            return;
        };
        // 清空当前条目与已选
        let flow = self.imp().flow_box.get();
        while let Some(child) = flow.first_child() {
            flow.remove(&child);
        }
        *self.imp().trashed_ids.borrow_mut() = vec![];
        *self.imp().visible_items.borrow_mut() = vec![];
        self.imp().action_bar.get().set_revealed(false);

        // 重新加载
        let page_weak = self.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(items) = db::list_trashed_media(&pool) {
                if let Some(page) = page_weak.upgrade() {
                    if items.is_empty() {
                        *page.imp().visible_items.borrow_mut() = vec![];
                        show_empty_trash(&page);
                    } else {
                        *page.imp().visible_items.borrow_mut() = items.clone();
                        // Restore the same Viewport/Box/FlowBox structure used by
                        // PhotosPage, so the FlowBox is measured by content height
                        // instead of being stretched by the ScrolledWindow.
                        page.imp()
                            .scrolled
                            .get()
                            .set_child(Some(&page.imp().grid_viewport.get()));
                        for item in items {
                            let tile = build_trash_tile(item, loader.clone());
                            flow.append(&tile);
                        }
                    }
                }
            }
        });
    }
}

fn trash_thumbnail_item(mut item: MediaItem) -> MediaItem {
    match trash::trashed_file_uri(&item.uri) {
        Ok(uri) => item.uri = uri,
        Err(e) => tracing::warn!("TrashPage: failed to resolve trash thumbnail URI: {e}"),
    }
    item
}

fn build_trash_tile(item: MediaItem, loader: Arc<ThumbnailLoader>) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(TRASH_TILE_PX);

    let item = trash_thumbnail_item(item);
    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request(item.uri, TRASH_THUMB_SIZE, tx);
    let tile_weak = tile.downgrade();
    gtk::glib::spawn_future_local(async move {
        if let Ok(texture) = rx.await {
            if let Some(tile) = tile_weak.upgrade() {
                tile.set_paintable(Some(&texture));
            }
        }
    });

    tile
}

fn selected_ids_for_indices(
    items: &[MediaItem],
    indices: impl IntoIterator<Item = i32>,
) -> Vec<i64> {
    indices
        .into_iter()
        .filter_map(|index| items.get(index as usize).map(|item| item.id))
        .collect()
}

/// Replace the scrolled window's child with an empty-state `AdwStatusPage`.
/// Keeps the action bar (Empty All button) revealed in the header so the
/// user can still see the page is the Trash.
fn show_empty_trash(page: &TrashPage) {
    let empty = empty_states::empty_trash();
    empty.set_hexpand(true);
    empty.set_vexpand(true);
    page.imp().scrolled.get().set_child(Some(&empty));
}

impl Default for TrashPage {
    fn default() -> Self {
        crate::ui::grid_css::install();
        glib::Object::builder().build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;

    fn empty_loader() -> Arc<ThumbnailLoader> {
        let dir = tempfile::tempdir().unwrap().keep();
        let pool = db::init_pool(&dir.join("test.db")).unwrap();
        Arc::new(ThumbnailLoader::new(pool, dir.join("cache")))
    }

    fn media_item(id: i64) -> MediaItem {
        MediaItem {
            id,
            uri: format!("file:///tmp/{id}.jpg"),
            path: PathBuf::from(format!("/tmp/{id}.jpg")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".to_string(),
            width: Some(1),
            height: Some(1),
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "hash".to_string(),
            trashed_at: Some(Utc::now()),
        }
    }

    #[test]
    fn selected_indices_map_to_media_item_ids_not_indices() {
        let items = vec![media_item(42), media_item(99), media_item(123)];

        assert_eq!(selected_ids_for_indices(&items, [0, 2]), vec![42, 123]);
    }

    #[gtk::test]
    fn trash_flow_box_matches_photo_grid_day_view_style() {
        let _ = gtk::init();
        let page = TrashPage::default();
        let flow = page.imp().flow_box.get();

        assert!(page
            .imp()
            .grid_viewport
            .get()
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
            .is_some());
        assert!(flow
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Box>().ok())
            .is_some());
        assert!(flow.has_css_class("thumb-grid"));
        assert!(!flow.has_css_class("trash-grid"));
        assert!(flow.is_homogeneous());
        assert_eq!(flow.column_spacing(), 2);
        assert_eq!(flow.row_spacing(), 2);
        assert_eq!(flow.max_children_per_line(), 100);
        assert_eq!(flow.selection_mode(), gtk::SelectionMode::Multiple);
    }

    #[gtk::test]
    fn trash_tile_uses_day_view_square_thumbnail_spec() {
        let _ = gtk::init();
        let tile = build_trash_tile(media_item(7), empty_loader());

        assert!(tile.is::<crate::ui::media_grid::square_tile::SquareTile>());
        assert_eq!(tile.target(), TRASH_TILE_PX);
        assert_eq!(TRASH_TILE_PX, 270);
        assert_eq!(TRASH_THUMB_SIZE, ThumbnailSize::Large);
    }

    /// 选取 gio 可支持的真实文件系统路径（拒绝 tmpfs）。
    fn real_scratch() -> std::path::PathBuf {
        std::env::var_os("TMPDIR_REAL")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
    }

    /// Insert a single trashed item and pump the main loop until the
    /// `TrashPage`'s FlowBox has loaded exactly one tile for it.
    fn page_with_one_trashed_item() -> (TrashPage, i64, std::path::PathBuf) {
        let _ = gtk::init();
        let ctx = glib::MainContext::default();

        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

        let real_dir = real_scratch();
        let real_path = real_dir.join(format!(
            "photo-viewer-trash-restore-test-{}.jpg",
            std::process::id()
        ));
        std::fs::write(&real_path, b"data").unwrap();

        let item = crate::core::media::NewMediaItem {
            uri: format!("file://{}", real_path.display()),
            path: real_path.clone(),
            folder_path: real_dir.clone(),
            mime_type: "image/jpeg".to_string(),
            width: Some(1),
            height: Some(1),
            taken_at: None,
            file_mtime: chrono::Utc::now(),
            file_size: 4,
            blake3_hash: "h".to_string(),
        };
        let id = db::insert_media_item(&pool, &item).unwrap();
        db::mark_trashed(&pool, id).unwrap();

        let page = TrashPage::new(pool.clone(), empty_loader());
        let flow = page.imp().flow_box.get();
        let mut pumped = 0;
        while pumped < 100 && flow.observe_children().n_items() == 0 {
            ctx.iteration(false);
            pumped += 1;
        }
        assert_eq!(
            flow.observe_children().n_items(),
            1,
            "TrashPage::new should load the one trashed item into FlowBox"
        );
        (page, id, real_path)
    }

    /// 点 Restore 后，FlowBox 必须立即清掉已还原的项 —— 之前因为没调
    /// `page.refresh()`，tile 还残留在界面上让用户以为还原失败。
    #[gtk::test]
    fn restore_btn_refreshes_flow_box_after_restoring_items() {
        let (page, id, real_path) = page_with_one_trashed_item();
        let ctx = glib::MainContext::default();
        let flow = page.imp().flow_box.get();

        *page.imp().trashed_ids.borrow_mut() = vec![id];
        page.imp().restore_btn.get().emit_clicked();

        let mut pumped = 0;
        while pumped < 200 && flow.observe_children().n_items() != 0 {
            ctx.iteration(false);
            pumped += 1;
        }
        assert_eq!(
            flow.observe_children().n_items(),
            0,
            "Flow box should be empty after restoring the only item (refresh wasn't called?)"
        );

        let _ = std::fs::remove_file(&real_path);
    }

    /// 部分永久删除后，FlowBox 必须移除被删的项；只保留剩余的 trashed 项。
    /// 之前因为只在全空时才 `show_empty_trash`，部分删除后残留旧 tile。
    #[gtk::test]
    fn delete_btn_refreshes_flow_box_after_partial_delete() {
        let _ = gtk::init();
        let ctx = glib::MainContext::default();

        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        let real_dir = real_scratch();

        let path_a = real_dir.join(format!(
            "photo-viewer-trash-del-a-{}.jpg",
            std::process::id()
        ));
        let path_b = real_dir.join(format!(
            "photo-viewer-trash-del-b-{}.jpg",
            std::process::id()
        ));
        std::fs::write(&path_a, b"a").unwrap();
        std::fs::write(&path_b, b"b").unwrap();

        let mk = |p: &std::path::Path, h: &str| -> i64 {
            let item = crate::core::media::NewMediaItem {
                uri: format!("file://{}", p.display()),
                path: p.to_path_buf(),
                folder_path: real_dir.clone(),
                mime_type: "image/jpeg".to_string(),
                width: Some(1),
                height: Some(1),
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: h.to_string(),
            };
            let id = db::insert_media_item(&pool, &item).unwrap();
            db::mark_trashed(&pool, id).unwrap();
            id
        };
        let id_a = mk(&path_a, "a");
        let _id_b = mk(&path_b, "b");

        let page = TrashPage::new(pool.clone(), empty_loader());
        let flow = page.imp().flow_box.get();
        let mut pumped = 0;
        while pumped < 100 && flow.observe_children().n_items() < 2 {
            ctx.iteration(false);
            pumped += 1;
        }
        assert_eq!(
            flow.observe_children().n_items(),
            2,
            "Both trashed items should be loaded into FlowBox"
        );

        // 只删 A
        *page.imp().trashed_ids.borrow_mut() = vec![id_a];
        page.imp().delete_btn.get().emit_clicked();

        pumped = 0;
        while pumped < 200 && flow.observe_children().n_items() != 1 {
            ctx.iteration(false);
            pumped += 1;
        }
        assert_eq!(
            flow.observe_children().n_items(),
            1,
            "Flow box should retain only the remaining trashed item after partial delete"
        );

        let _ = std::fs::remove_file(&path_a);
        let _ = std::fs::remove_file(&path_b);
    }
}
