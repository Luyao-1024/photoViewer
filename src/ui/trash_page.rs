//! TrashPage — 回收站页面（多选 + 批量还原/永久删除）
//!
//! 布局：
//! - `AdwHeaderBar`：标题栏
//! - `AdwBanner`：还原 / 手动永久删除提示
//! - `VirtualMediaGrid`（multi-select）：显示已删除的媒体项
//! - `GtkActionBar`：底部操作栏（仅在有选中项时 reveal）
//!   - Cancel：清空选择
//!   - Restore：批量还原
//!   - Delete Permanently：批量永久删除
//!
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt, NavigationPageExt};
use libadwaita::subclass::prelude::*;

#[cfg(test)]
use crate::core::db;
use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand, DbCommandResult};
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::thumbnails::ThumbnailLoader;
use crate::core::trash;
use crate::ui::empty_states;
use crate::ui::media_grid::{FavoriteMenuState, MediaGridCallbacks};
use crate::ui::virtual_media_grid::VirtualMediaGrid;

fn restore_items(pool: &DbPool, db_actor: Option<&DbActorHandle>, ids: Vec<i64>) -> Vec<MediaItem> {
    let ids = ids.into_iter().map(MediaId::from).collect::<Vec<_>>();
    if let Some(actor) = db_actor {
        let items = ids
            .iter()
            .filter_map(|id| crate::core::db::get_media_item(pool, id.get()).ok())
            .collect::<Vec<_>>();
        for item in &items {
            if trash::restore_from_trash(&item.uri).is_err() {
                return Vec::new();
            }
        }
        return match actor.execute_blocking(DbCommand::RestoreTrashed { ids }) {
            Ok(DbCommandResult::MediaItems(items)) => items,
            _ => Vec::new(),
        };
    }
    MediaRepository::new(pool.clone())
        .restore_from_trash(&ids)
        .map(|mutation| mutation.changed_items)
        .unwrap_or_default()
}

fn delete_items_permanently(pool: &DbPool, db_actor: Option<&DbActorHandle>, ids: Vec<i64>) {
    let ids = ids.into_iter().map(MediaId::from).collect::<Vec<_>>();
    if let Some(actor) = db_actor {
        let items = ids
            .iter()
            .filter_map(|id| crate::core::db::get_media_item(pool, id.get()).ok())
            .collect::<Vec<_>>();
        for item in &items {
            let _ = trash::delete_permanently(&item.uri);
        }
        let _ = actor.execute_blocking(DbCommand::DeleteTrashedRows { ids });
    } else {
        let _ = MediaRepository::new(pool.clone()).delete_permanently(&ids);
    }
}

fn empty_trash(pool: &DbPool, db_actor: Option<&DbActorHandle>) {
    let ids = crate::core::db::list_trashed_media(pool)
        .unwrap_or_default()
        .into_iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    delete_items_permanently(pool, db_actor, ids);
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/trash-page.ui")]
    pub struct TrashPage {
        pub pool: RefCell<Option<DbPool>>,
        pub db_actor: RefCell<Option<DbActorHandle>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub trashed_ids: RefCell<Vec<i64>>,
        pub grid: RefCell<Option<VirtualMediaGrid>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub trash_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub content_stack: TemplateChild<gtk::Stack>,
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

    impl ObjectImpl for TrashPage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let empty = empty_states::empty_trash();
            empty.set_hexpand(true);
            empty.set_vexpand(true);
            obj.imp()
                .content_stack
                .get()
                .add_named(&empty, Some("empty"));
            obj.imp()
                .content_stack
                .get()
                .set_visible_child_name("empty");
        }
    }
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
        Self::build(pool, loader, None)
    }

    pub fn with_media_list(
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
        media_list: gtk::gio::ListStore,
    ) -> Self {
        Self::build(pool, loader, Some(media_list))
    }

    pub fn with_media_list_and_actor(
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
        media_list: gtk::gio::ListStore,
        db_actor: DbActorHandle,
    ) -> Self {
        let page = Self::build(pool, loader, Some(media_list));
        *page.imp().db_actor.borrow_mut() = Some(db_actor);
        page
    }

    fn build(
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
        media_list: Option<gtk::gio::ListStore>,
    ) -> Self {
        crate::ui::grid_css::install();

        let obj: Self = glib::Object::builder().build();
        obj.set_title(&tr("page.trash.title"));
        *obj.imp().pool.borrow_mut() = Some(pool.clone());
        *obj.imp().loader.borrow_mut() = Some(loader.clone());
        *obj.imp().media_list.borrow_mut() = media_list;
        obj.imp().trash_banner.get().set_title(&tr("trash.banner"));
        obj.imp().empty_btn.get().set_label(&tr("trash.empty_all"));
        obj.imp().cancel_btn.get().set_label(&tr("trash.cancel"));
        obj.imp().restore_btn.get().set_label(&tr("trash.restore"));
        obj.imp()
            .delete_btn
            .get()
            .set_label(&tr("trash.delete_permanently"));

        let grid = VirtualMediaGrid::new_for_query(
            gtk::gio::ListStore::new::<glib::BoxedAnyObject>(),
            MediaQuery::Trash,
            crate::core::section_model::GroupBy::Day,
            loader,
            MediaGridCallbacks {
                on_activate: Rc::new(|_| {}),
                on_background_changed: Rc::new(|| {}),
                on_add_to_album: Rc::new(|_| {}),
                on_move_to_trash: Rc::new(|_| {}),
                on_set_favorite: Rc::new(|_, _| {}),
                on_query_favorite_state: Rc::new(|_| FavoriteMenuState::default()),
                on_set_album_cover: None,
            },
            true,
        );
        grid.set_thumbnail_uri_resolver(|item| match trash::trashed_file_uri(&item.uri) {
            Ok(uri) => Some(uri),
            Err(error) => {
                tracing::warn!("TrashPage: failed to resolve trash thumbnail URI: {error}");
                None
            }
        });
        grid.set_multi_select_mode(true);
        {
            let weak = obj.downgrade();
            let grid = grid.clone();
            grid.clone().connect_selection_changed(move || {
                if let Some(page) = weak.upgrade() {
                    let selected = grid.selected_ids().into_iter().map(MediaId::get).collect();
                    *page.imp().trashed_ids.borrow_mut() = selected;
                    page.imp()
                        .action_bar
                        .get()
                        .set_revealed(!page.imp().trashed_ids.borrow().is_empty());
                }
            });
        }
        obj.imp()
            .content_stack
            .get()
            .add_named(&grid, Some("content"));
        *obj.imp().grid.borrow_mut() = Some(grid.clone());

        // Cancel：清空选择 + 隐藏 ActionBar
        obj.imp().cancel_btn.get().connect_clicked(
            glib::clone!(@weak obj, @weak grid => move |_| {
                grid.clear_selection();
                grid.set_multi_select_mode(true);
                *obj.imp().trashed_ids.borrow_mut() = vec![];
                obj.imp().action_bar.get().set_revealed(false);
            }),
        );

        // Restore：批量还原
        obj.imp().restore_btn.get().connect_clicked(
            glib::clone!(@weak obj, @weak grid => move |_| {
                let pool = match obj.imp().pool.borrow().as_ref() {
                    Some(p) => p.clone(),
                    None => return,
                };
                let ids = obj.imp().trashed_ids.borrow().clone();
                let db_actor = obj.imp().db_actor.borrow().clone();
                let media_list = obj.imp().media_list.borrow().clone();
                let page_weak = obj.downgrade();

                glib::spawn_future_local(async move {
                    let restored_items = gtk::gio::spawn_blocking(move || restore_items(&pool, db_actor.as_ref(), ids))
                        .await
                        .unwrap_or_default();
                    if let Some(list) = media_list {
                        for item in restored_items {
                            insert_media_item_sorted(&list, item);
                        }
                    }
                    grid.clear_selection();
                    grid.set_multi_select_mode(true);
                    if let Some(page) = page_weak.upgrade() {
                        page.refresh();
                    }
                });
            }),
        );

        // Delete Permanently：批量永久删除
        obj.imp().delete_btn.get().connect_clicked(
            glib::clone!(@weak obj, @weak grid => move |_| {
                let pool = match obj.imp().pool.borrow().as_ref() {
                    Some(p) => p.clone(),
                    None => return,
                };
                let ids = obj.imp().trashed_ids.borrow().clone();
                let db_actor = obj.imp().db_actor.borrow().clone();
                let page_weak = obj.downgrade();

                glib::spawn_future_local(async move {
                    let _ = gtk::gio::spawn_blocking(move || delete_items_permanently(&pool, db_actor.as_ref(), ids)).await;
                    grid.clear_selection();
                    grid.set_multi_select_mode(true);
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
                let db_actor = obj.imp().db_actor.borrow().clone();
                let page_weak = obj.downgrade();

                let dialog = adw::AlertDialog::builder()
                    .heading(tr("trash.empty_title"))
                    .body(tr("trash.empty_body"))
                    .build();
                dialog.add_css_class("glass-alert-dialog");
                dialog.add_response("cancel", &tr("dialog.cancel"));
                dialog.add_response("empty", &tr("dialog.empty"));
                dialog.set_response_appearance("empty", adw::ResponseAppearance::Destructive);

                dialog.connect_response(
                    None,
                    move |_, response| {
                        if response == "empty" {
                            let pool = pool.clone();
                            let db_actor = db_actor.clone();
                            let page_weak = page_weak.clone();
                            glib::spawn_future_local(async move {
                                let _ = gtk::gio::spawn_blocking(move || empty_trash(&pool, db_actor.as_ref())).await;
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

        obj.refresh();

        obj
    }

    /// Refresh the virtual Trash query and its empty state.
    pub fn refresh(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let Some(grid) = self.imp().grid.borrow().as_ref().cloned() else {
            return;
        };
        *self.imp().trashed_ids.borrow_mut() = vec![];
        self.imp().action_bar.get().set_revealed(false);
        grid.clear_selection();
        grid.set_multi_select_mode(true);
        grid.refresh_from_shared_projection();

        let page_weak = self.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(Ok(total)) = gtk::gio::spawn_blocking(move || {
                MediaRepository::new(pool).count(MediaQuery::Trash)
            })
            .await
            {
                if let Some(page) = page_weak.upgrade() {
                    page.imp()
                        .content_stack
                        .get()
                        .set_visible_child_name(if total == 0 { "empty" } else { "content" });
                }
            }
        });
    }
}

fn insert_media_item_sorted(list: &gtk::gio::ListStore, item: MediaItem) {
    if media_list_contains_id(list, item.id) {
        return;
    }
    let insert_at = (0..list.n_items())
        .find(|&idx| {
            let Some(existing) = crate::ui::media_list::media_item_at(list, idx) else {
                return false;
            };
            item.sort_datetime() > existing.sort_datetime()
                || (item.sort_datetime() == existing.sort_datetime() && item.id > existing.id)
        })
        .unwrap_or_else(|| list.n_items());
    list.insert(insert_at, &glib::BoxedAnyObject::new(item));
}

fn media_list_contains_id(list: &gtk::gio::ListStore, item_id: i64) -> bool {
    (0..list.n_items()).any(|idx| {
        crate::ui::media_list::media_item_at(list, idx)
            .map(|item| item.id == item_id)
            .unwrap_or(false)
    })
}

#[cfg(test)]
fn selected_ids_for_indices(
    items: &[MediaItem],
    indices: impl IntoIterator<Item = i32>,
) -> Vec<i64> {
    indices
        .into_iter()
        .filter_map(|index| items.get(index as usize).map(|item| item.id))
        .collect()
}

impl Default for TrashPage {
    fn default() -> Self {
        crate::ui::grid_css::install();
        glib::Object::builder().build()
    }
}

#[cfg(test)]
mod tests;
