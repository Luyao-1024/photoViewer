//! TrashPage — 回收站页面（多选 + 批量还原/永久删除）
//!
//! 布局：
//! - `AdwHeaderBar`：标题栏
//! - `AdwBanner`：30 天永久删除提示
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

use crate::core::db::{self, DbPool};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::core::trash;
use crate::ui::empty_states;
use crate::ui::photo_tile::PhotoTile;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/trash-page.ui")]
    pub struct TrashPage {
        pub pool: RefCell<Option<DbPool>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub trashed_ids: RefCell<Vec<i64>>,
        #[template_child]
        pub scrolled: TemplateChild<gtk::ScrolledWindow>,
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
        let obj: Self = glib::Object::builder().build();
        *obj.imp().pool.borrow_mut() = Some(pool.clone());
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        let flow = obj.imp().flow_box.get();

        // 选择模式：FlowBox 多选
        flow.set_selection_mode(gtk::SelectionMode::Multiple);

        // 选中变化 → 维护 selected 列表 + 切换 ActionBar revealed
        flow.connect_selected_children_changed(glib::clone!(@weak obj => move |flow| {
            let selected: Vec<i64> = flow
                .selected_children()
                .iter()
                .filter_map(|c| c.downcast_ref::<gtk::FlowBoxChild>().map(|c| c.index() as i64))
                .collect();
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

                // 异步批处理：避免阻塞 UI
                glib::spawn_future_local(async move {
                    for id in ids {
                        if let Ok(item) = db::get_media_item(&pool, id) {
                            let _ = trash::restore_from_trash(&item.uri);
                            let _ = db::unmark_trashed(&pool, id);
                        }
                    }
                    flow.unselect_all();
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
                    flow.unselect_all();
                    // If no items remain, swap the scrolled child for the
                    // empty-state status page.
                    if let Ok(remaining) = db::list_trashed_media(&pool) {
                        if remaining.is_empty() {
                            if let Some(page) = page_weak.upgrade() {
                                show_empty_trash(&page);
                            }
                        }
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
                let flow_weak = obj.imp().flow_box.downgrade();
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
                            let flow_weak = flow_weak.clone();
                            let page_weak = page_weak.clone();
                            glib::spawn_future_local(async move {
                                if let Ok(items) = db::list_trashed_media(&pool) {
                                    for item in items {
                                        let _ = trash::delete_permanently(&item.uri);
                                        let _ = db::delete_media_item(&pool, item.id);
                                    }
                                }
                                if let Some(flow) = flow_weak.upgrade() {
                                    while let Some(child) = flow.first_child() {
                                        flow.remove(&child);
                                    }
                                }
                                // After emptying, the trash is empty — show the
                                // empty-state status page.
                                if let Some(page) = page_weak.upgrade() {
                                    show_empty_trash(&page);
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
                        show_empty_trash(&obj);
                    }
                } else {
                    for item in items {
                        let tile = PhotoTile::new();
                        tile.set_item(item, loader_clone.clone(), ThumbnailSize::Small, 125);
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
        self.imp().action_bar.get().set_revealed(false);

        // 重新加载
        let page_weak = self.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(items) = db::list_trashed_media(&pool) {
                if let Some(page) = page_weak.upgrade() {
                    if items.is_empty() {
                        show_empty_trash(&page);
                    } else {
                        // Restore the flow box as the scrolled child before re-populating.
                        page.imp().scrolled.get().set_child(Some(&flow));
                        for item in items {
                            let tile = PhotoTile::new();
                            tile.set_item(item, loader.clone(), ThumbnailSize::Small, 125);
                            flow.append(&tile);
                        }
                    }
                }
            }
        });
    }
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
        glib::Object::builder().build()
    }
}
