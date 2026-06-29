//! PhotosPage: year/month/day view (shared MediaGrid, ModeSelector overlay).
//!
//! Hosts three MediaGrid instances. When the user clicks a tile, a `ViewerPage`
//! is pushed onto the host `AdwNavigationView` (injected via `set_nav_target`).
//! Shift/Ctrl-click on a tile multi-selects; the "Add to Album" toolbar button
//! appears whenever ≥1 tile is selected and opens the `AlbumPickerDialog`.
use std::cell::Cell;
use std::cell::Ref;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt, NavigationPageExt};

use crate::core::identity::MediaId;
use crate::core::i18n::tr;
use crate::core::media::MediaItem;
use crate::core::repository::MediaQuery;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::core::{
    albums,
    db::DbPool,
};
use crate::ui::album_picker;
use crate::ui::empty_states;
use crate::ui::media_grid::{FavoriteMenuState, MediaGrid, MediaGridCallbacks};
use crate::ui::mode_selector::ModeSelector;
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP};
use crate::ui::window::refresh_albums_sidebar;

mod imp {
    use super::*;
    use adw::subclass::prelude::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photos-page.ui")]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub pool: RefCell<Option<DbPool>>,
        /// Tracks the three MediaGrids so we can clear their selections and
        /// react to their `selection-changed` callbacks uniformly.
        pub grids: RefCell<Vec<MediaGrid>>,
        /// Global indices currently selected, in insertion order. Maintained
        /// by listening to each grid's `selection-changed` callback; not
        /// authoritative on its own — the per-grid `selected` set is.
        pub selected_ids: RefCell<HashSet<MediaId>>,
        /// Coalesces ModeSelector contrast refreshes while scrolling. A final
        /// color decision can require one or more GTK frames after the scroll
        /// adjustment value changes because tile bounds and newly-visible
        /// thumbnail brightness state settle asynchronously.
        pub contrast_update_pending: Cell<bool>,
        /// Debounces photo activation while NavigationView is pushing the
        /// viewer. Without this, rapid repeated clicks can stack viewer pages
        /// or race with viewer-level back handling during the transition.
        pub viewer_open_pending: Cell<bool>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub mode_selector: TemplateChild<ModeSelector>,
        #[template_child]
        pub select_all_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub add_to_album_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub favorite_btn: TemplateChild<gtk::Button>,
        /// 收藏/取消收藏弹层菜单项（在 new 里建好并 set_parent 到 favorite_btn；
        /// refresh_selection_ui 按选中集状态切换 sensitive）。
        pub favorite_item_btn: RefCell<Option<gtk::Button>>,
        pub unfavorite_item_btn: RefCell<Option<gtk::Button>>,
        #[template_child]
        pub delete_to_trash_btn: TemplateChild<gtk::Button>,
    }

    impl Default for PhotosPage {
        fn default() -> Self {
            Self {
                media_list: RefCell::new(None),
                loader: RefCell::new(None),
                nav_view: RefCell::new(None),
                pool: RefCell::new(None),
                grids: RefCell::new(Vec::new()),
                selected_ids: RefCell::new(HashSet::new()),
                contrast_update_pending: Cell::new(false),
                viewer_open_pending: Cell::new(false),
                header_bar: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                mode_selector: TemplateChild::default(),
                select_all_btn: TemplateChild::default(),
                add_to_album_btn: TemplateChild::default(),
                favorite_btn: TemplateChild::default(),
                favorite_item_btn: RefCell::new(None),
                unfavorite_item_btn: RefCell::new(None),
                delete_to_trash_btn: TemplateChild::default(),
            }
        }
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for PhotosPage {
        const NAME: &'static str = "PhotosPage";
        type Type = super::PhotosPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotosPage {}
    impl WidgetImpl for PhotosPage {}
    impl NavigationPageImpl for PhotosPage {}
}

gtk::glib::wrapper! {
    pub struct PhotosPage(ObjectSubclass<imp::PhotosPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl PhotosPage {
    /// Build a PhotosPage backed by `media_list`, sharing `loader` across the three
    /// mode-specific MediaGrids (Year/Month/Day).
    pub fn new(media_list: gtk::gio::ListStore, loader: Arc<ThumbnailLoader>) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        obj.set_title(&tr("page.photos.title"));
        // select_all_btn keeps a text label (toggles 全选/取消全选);
        // add_to_album_btn (+) and delete_to_trash_btn (trash) are icon-only.
        obj.imp()
            .select_all_btn
            .get()
            .set_label(&tr("photos.batch.select_all"));
        obj.imp()
            .select_all_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.batch.select_all")));
        obj.imp()
            .add_to_album_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.add_to_album")));
        // favorite_btn is the merged heart trigger (icon-only); clicking it
        // opens a popover with 收藏/取消收藏, so it carries a tooltip only.
        obj.imp()
            .favorite_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.batch.favorite")));
        obj.imp()
            .delete_to_trash_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.move_to_trash")));
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        // Snapshot the initial size before `media_list` is moved into MediaGrid.
        let is_empty = media_list.n_items() == 0;
        let media_list_for_empty_state = media_list.clone();

        let on_activate: Rc<dyn Fn(MediaId)> = {
            let weak = obj.downgrade();
            Rc::new(move |media_id| {
                if let Some(this) = weak.upgrade() {
                    this.open_viewer(media_id);
                }
            })
        };
        let on_background_changed: Rc<dyn Fn()> = {
            let weak = obj.downgrade();
            Rc::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.update_mode_selector_contrast();
                }
            })
        };
        let on_add_to_album: Rc<dyn Fn(Vec<MediaId>)> = {
            let weak = obj.downgrade();
            Rc::new(move |ids| {
                if let Some(this) = weak.upgrade() {
                    this.open_album_picker_for_ids(ids);
                }
            })
        };
        let on_move_to_trash: Rc<dyn Fn(Vec<MediaId>)> = {
            let weak = obj.downgrade();
            Rc::new(move |ids| {
                if let Some(this) = weak.upgrade() {
                    this.delete_to_trash_for_ids(ids);
                }
            })
        };
        let on_favorite: Rc<dyn Fn(Vec<MediaId>, bool)> = {
            let weak = obj.downgrade();
            Rc::new(move |ids, is_favorite| {
                if let Some(this) = weak.upgrade() {
                    this.set_favorite_for_ids(ids, is_favorite);
                }
            })
        };
        let on_query_favorite_state = {
            let weak = obj.downgrade();
            Rc::new(move |ids: Vec<MediaId>| {
                weak.upgrade()
                    .map(|this| this.favorite_state_for_ids(&ids))
                    .unwrap_or_default()
            })
        };
        let callbacks = MediaGridCallbacks {
            on_activate,
            on_background_changed,
            on_add_to_album,
            on_move_to_trash,
            on_set_favorite: on_favorite,
            on_query_favorite_state,
        };

        // Three independent MediaGrid instances — one per grouping mode.
        let year_grid = MediaGrid::new_with_initial_active(
            media_list.clone(),
            GroupBy::Year,
            loader.clone(),
            callbacks.clone(),
            true,
            false,
        );
        let month_grid = MediaGrid::new_with_initial_active(
            media_list.clone(),
            GroupBy::Month,
            loader.clone(),
            callbacks.clone(),
            true,
            false,
        );
        let day_grid = MediaGrid::new_with_initial_active(
            media_list,
            GroupBy::Day,
            loader,
            callbacks,
            true,
            true,
        );

        // Wire selection-changed: each grid fires when its own `selected` set
        // changes. We collect the union into PhotosPage's `selected_ids`
        // and toggle the "Add to Album" button visibility. We use the union
        // (rather than per-grid bookkeeping) so the toolbar reflects the total
        // selected across year/month/day — important because only one mode
        // grid is visible at a time but the user may have multi-selected
        // before switching.
        {
            let weak = obj.downgrade();
            year_grid.connect_selection_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_selection_ui();
                }
            });
        }
        {
            let weak = obj.downgrade();
            month_grid.connect_selection_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_selection_ui();
                }
            });
        }
        {
            let weak = obj.downgrade();
            day_grid.connect_selection_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_selection_ui();
                }
            });
        }
        for grid in [&year_grid, &month_grid, &day_grid] {
            let weak = obj.downgrade();
            grid.connect_view_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.schedule_mode_selector_contrast_update();
                }
            });
        }
        *obj.imp().grids.borrow_mut() =
            vec![year_grid.clone(), month_grid.clone(), day_grid.clone()];

        let stack = obj.imp().view_stack.get();
        stack.add_titled(&year_grid, Some("year"), &tr("photo.mode.year"));
        stack.add_titled(&month_grid, Some("month"), &tr("photo.mode.month"));
        stack.add_titled(&day_grid, Some("day"), &tr("photo.mode.day"));

        // Empty-state placeholder: shown when the media list is empty.
        // Added as a hidden stack child so we can swap to it without rebuilding.
        let empty_page = empty_states::no_photos();
        empty_page.set_hexpand(true);
        empty_page.set_vexpand(true);
        stack.add(&empty_page); // untitled → won't appear in the switcher bar

        // Decide initial visible child based on data size.
        if is_empty {
            stack.set_visible_child(&empty_page);
        } else {
            stack.set_visible_child_name("day");
        }
        {
            let stack = stack.clone();
            let empty_page = empty_page.clone();
            media_list_for_empty_state.connect_items_changed(move |list, _, _, _| {
                if list.n_items() == 0 {
                    stack.set_visible_child(&empty_page);
                    return;
                }
                let showing_empty = stack
                    .visible_child()
                    .as_ref()
                    .is_some_and(|child| child == empty_page.upcast_ref::<gtk::Widget>());
                if showing_empty {
                    stack.set_visible_child_name("day");
                }
            });
        }

        // Wire the ModeSelector to our view_stack (it drives the visible
        // child and reflects any external change back via notify).
        obj.imp().mode_selector.get().set_stack(&stack);
        {
            let weak = obj.downgrade();
            stack.connect_notify_local(Some("visible-child"), move |_, _| {
                if let Some(this) = weak.upgrade() {
                    this.sync_active_grid_rebuilds();
                    this.schedule_mode_selector_contrast_update();
                }
            });
        }
        obj.sync_active_grid_rebuilds();
        obj.schedule_mode_selector_contrast_update();

        // Wire the batch toolbar buttons. For selected items:
        // - Select All: select current mode's rendered tiles.
        // - Move to Album: open AlbumPickerDialog.
        // - Favorite / Unfavorite: batch update favorite flag.
        // - Move to Trash: bulk remove from media list and albums.
        // We forward the current selection and refresh state on success.
        let weak = obj.downgrade();
        obj.imp().select_all_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.select_all_in_current_mode();
        });

        let weak = obj.downgrade();
        obj.imp().add_to_album_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.open_album_picker_for_current_selection();
        });

        // 收藏/取消收藏合并为一个心形按钮 + glass-menu 弹层。弹层在 new 里
        // 建好并 set_parent 到 favorite_btn：点击 favorite_btn 弹出，菜单项按
        // 选中集状态启用（见 refresh_selection_ui）。
        {
            let menu = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .css_classes(["glass-menu-list"])
                .build();

            let favorite_item = gtk::Button::builder()
                .label(tr("photos.batch.favorite"))
                .css_classes(["glass-menu-item"])
                .build();
            let unfavorite_item = gtk::Button::builder()
                .label(tr("photos.batch.unfavorite"))
                .css_classes(["glass-menu-item"])
                .build();

            let popover = gtk::Popover::builder().autohide(true).build();
            popover.add_css_class("glass-menu");
            popover.set_child(Some(&menu));

            let weak = obj.downgrade();
            let popover_for_fav = popover.clone();
            favorite_item.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                let ids = this.selected_ids_vec();
                if !ids.is_empty() {
                    this.set_favorite_for_ids(ids, true);
                }
                }
                popover_for_fav.popdown();
            });

            let weak = obj.downgrade();
            let popover_for_unfav = popover.clone();
            unfavorite_item.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                let ids = this.selected_ids_vec();
                if !ids.is_empty() {
                    this.set_favorite_for_ids(ids, false);
                }
                }
                popover_for_unfav.popdown();
            });

            menu.append(&favorite_item);
            menu.append(&unfavorite_item);

            // Anchor the popover to the heart button.
            popover.set_parent(&obj.imp().favorite_btn.get());
            *obj.imp().favorite_item_btn.borrow_mut() = Some(favorite_item);
            *obj.imp().unfavorite_item_btn.borrow_mut() = Some(unfavorite_item);

            // Smart toggle: a uniformly favorited or uniformly unfavorited
            // selection acts directly (favorite all / unfavorite all); only a
            // mixed selection opens the popover with both options.
            let weak = obj.downgrade();
            let popover_for_trigger = popover.clone();
            obj.imp().favorite_btn.get().connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let ids = this.selected_ids_vec();
                if ids.is_empty() {
                    return;
                }
                let state = this.favorite_state_for_ids(&ids);
                if state.can_favorite && state.can_unfavorite {
                    popover_for_trigger.popup();
                } else if state.can_favorite {
                    this.set_favorite_for_ids(ids, true);
                } else if state.can_unfavorite {
                    this.set_favorite_for_ids(ids, false);
                }
            });
        }

        let weak = obj.downgrade();
        obj.imp()
            .delete_to_trash_btn
            .get()
            .connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let ids = this.selected_ids_vec();
                if ids.is_empty() {
                    return;
                }
                let count = ids.len();
                let body = if count == 1 {
                    tr("trash.confirm_body_one")
                } else {
                    tr("trash.confirm_body_many").replace("{count}", &count.to_string())
                };
                let dialog = adw::AlertDialog::builder()
                    .heading(tr("trash.confirm_title"))
                    .body(body)
                    .build();
                dialog.add_css_class("glass-alert-dialog");
                dialog.add_response("cancel", &tr("dialog.cancel"));
                dialog.add_response("trash", &tr("dialog.trash"));
                dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
                dialog.set_default_response(Some("cancel"));
                dialog.set_close_response("cancel");

                let weak2 = this.downgrade();
                let ids2 = ids;
                dialog.connect_response(None, move |_, response| {
                    if response == "trash" {
                        if let Some(this) = weak2.upgrade() {
                            this.delete_to_trash_for_ids(ids2.clone());
                        }
                    }
                });
                dialog.present(&this);
            });

        obj
    }

    /// Inject the `AdwNavigationView` we live inside — needed to push/pop
    /// the viewer page. Called by the host (`app::build_app`) after pushing
    /// the PhotosPage.
    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
    }

    /// Inject the `DbPool` so viewer pages can launch the editor panel with
    /// access to the database. Mirrors `set_nav_target`.
    pub fn set_db_pool(&self, pool: DbPool) {
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    pub fn media_list(&self) -> Ref<'_, Option<gtk::gio::ListStore>> {
        self.imp().media_list.borrow()
    }

    /// Rebuild the union of selected media ids from each visible grid, then
    /// show / hide the "Add to Album" button. Cheap (HashSet union) so it's
    /// fine to call on every `selection-changed` tick.
    fn refresh_selection_ui(&self) {
        let mut union: HashSet<MediaId> = HashSet::new();
        for grid in self.imp().grids.borrow().iter() {
            union.extend(grid.selected_ids());
        }
        let has_any = !union.is_empty();
        let all_displayed_selected = self
            .current_grid()
            .is_some_and(|grid| grid.is_all_displayed_selected());
        *self.imp().selected_ids.borrow_mut() = union;
        self.imp().select_all_btn.get().set_visible(has_any);
        self.imp().add_to_album_btn.get().set_visible(has_any);
        self.imp().delete_to_trash_btn.get().set_visible(has_any);
        // select_all_btn keeps a text label that toggles 全选/取消全选.
        if all_displayed_selected {
            self.imp()
                .select_all_btn
                .get()
                .set_label(&tr("photos.batch.unselect_all"));
            self.imp()
                .select_all_btn
                .get()
                .set_tooltip_text(Some(&tr("photos.batch.unselect_all")));
        } else {
            self.imp()
                .select_all_btn
                .get()
                .set_label(&tr("photos.batch.select_all"));
            self.imp()
                .select_all_btn
                .get()
                .set_tooltip_text(Some(&tr("photos.batch.select_all")));
        }

        let state = if has_any {
            let ids: Vec<MediaId> = self
                .imp()
                .selected_ids
                .borrow()
                .iter()
                .copied()
                .collect();
            self.favorite_state_for_ids(&ids)
        } else {
            FavoriteMenuState::default()
        };
        // Smart favorite toggle. The heart button shows whenever there is a
        // selection. It turns red (favorite-active — the same class/effect as
        // the viewer's favorited heart) when every selected photo is already
        // favorited; clicking then unfavorites all. If none are favorited,
        // clicking favorites all. A mixed selection leaves the heart plain and
        // opens the popover (handled in the click handler).
        let all_favorited = has_any && !state.can_favorite && state.can_unfavorite;
        let fav_btn = self.imp().favorite_btn.get();
        fav_btn.set_visible(has_any);
        if all_favorited {
            fav_btn.add_css_class("favorite-active");
            fav_btn.set_tooltip_text(Some(&tr("photos.batch.unfavorite")));
        } else {
            fav_btn.remove_css_class("favorite-active");
            fav_btn.set_tooltip_text(Some(&tr("photos.batch.favorite")));
        }
        // Popover items stay wired for the mixed case.
        if let Some(btn) = self.imp().favorite_item_btn.borrow().as_ref() {
            btn.set_sensitive(state.can_favorite);
        }
        if let Some(btn) = self.imp().unfavorite_item_btn.borrow().as_ref() {
            btn.set_sensitive(state.can_unfavorite);
        }
    }

    fn favorite_state_for_ids(&self, ids: &[MediaId]) -> FavoriteMenuState {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return FavoriteMenuState::default();
        };
        if ids.is_empty() {
            return FavoriteMenuState::default();
        }

        let repo = crate::core::repository::MediaRepository::new(pool);
        let summary = repo.favorite_state(ids).unwrap_or_default();

        FavoriteMenuState {
            can_favorite: summary.has_unfavorite,
            can_unfavorite: summary.has_favorite,
        }
    }

    fn selected_ids_vec(&self) -> Vec<MediaId> {
        let mut selected: Vec<MediaId> = self
            .imp()
            .selected_ids
            .borrow()
            .iter()
            .copied()
            .collect();
        selected.sort_unstable();
        selected
    }

    fn current_grid(&self) -> Option<MediaGrid> {
        let stack = self.imp().view_stack.get();
        let visible = stack.visible_child()?;
        visible.downcast::<MediaGrid>().ok()
    }

    fn sync_active_grid_rebuilds(&self) {
        let current = self.current_grid();
        for grid in self.imp().grids.borrow().iter() {
            let active = current.as_ref().is_some_and(|visible| visible == grid);
            grid.set_active(active);
        }
    }

    fn select_all_in_current_mode(&self) {
        if let Some(grid) = self.current_grid() {
            if grid.is_all_displayed_selected() {
                grid.clear_selection();
            } else {
                grid.select_all();
            }
        }
    }

    fn open_album_picker_for_current_selection(&self) {
        let ids = self.selected_ids_vec();
        if ids.is_empty() {
            return;
        }
        self.open_album_picker_for_ids(ids);
    }

    fn update_mode_selector_contrast(&self) {
        let selector = self.imp().mode_selector.get();
        let stack = self.imp().view_stack.get();
        let Some(visible) = stack.visible_child() else {
            selector.set_light_background(false);
            return;
        };
        let Some(grid) = visible.downcast_ref::<MediaGrid>() else {
            selector.set_light_background(false);
            return;
        };
        selector.set_light_background(grid.background_is_light_under(&selector).unwrap_or(false));
    }

    fn schedule_mode_selector_contrast_update(&self) {
        self.update_mode_selector_contrast();

        if self.imp().contrast_update_pending.replace(true) {
            return;
        }

        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.update_mode_selector_contrast();
            }
        });

        let weak = self.downgrade();
        let ticks_remaining = Rc::new(Cell::new(8u8));
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            if let Some(this) = weak.upgrade() {
                this.update_mode_selector_contrast();
                let remaining = ticks_remaining.get().saturating_sub(1);
                ticks_remaining.set(remaining);
                if remaining > 0 {
                    return glib::ControlFlow::Continue;
                }
                this.imp().contrast_update_pending.set(false);
            }
            glib::ControlFlow::Break
        });
    }

    fn open_album_picker_for_ids(&self, ids: Vec<MediaId>) {
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        let raw_ids: Vec<i64> = ids.into_iter().map(MediaId::get).collect();
        album_picker::AlbumPickerDialog::present(&nav, pool, raw_ids);
    }

    fn remove_media_by_ids(&self, ids: &[i64]) {
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        if ids.is_empty() {
            return;
        }

        let id_set: HashSet<i64> = ids.iter().copied().collect();
        let mut to_remove = Vec::new();
        for idx in 0..list.n_items() {
            let Some(obj) = list.item(idx) else {
                continue;
            };
            let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            if id_set.contains(&boxed.borrow::<crate::core::media::MediaItem>().id) {
                to_remove.push(idx);
            }
        }
        for idx in to_remove.into_iter().rev() {
            list.remove(idx);
        }
    }

    fn delete_to_trash_for_ids(&self, ids: Vec<MediaId>) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        if ids.is_empty() {
            return;
        }

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let removed = gtk::gio::spawn_blocking(move || {
                let repo = crate::core::repository::MediaRepository::new(pool.clone());
                let mutation = repo.move_to_trash(&ids_for_worker).unwrap_or_default();
                let _ = albums::refresh(&pool);
                mutation
                    .changed_ids
                    .into_iter()
                    .map(MediaId::get)
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();

            if let Some(this) = weak.upgrade() {
                this.remove_media_by_ids(&removed);
                this.clear_selection();
                if let Some(nav) = this.imp().nav_view.borrow().as_ref().cloned() {
                    refresh_albums_sidebar(&nav);
                }
            }
        });
    }

    fn set_favorite_for_ids(&self, ids: Vec<MediaId>, is_favorite: bool) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        if ids.is_empty() {
            return;
        }

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let _ = gtk::gio::spawn_blocking(move || {
                let repo = crate::core::repository::MediaRepository::new(pool.clone());
                let _ = repo.set_favorite(&ids_for_worker, is_favorite);
                let _ = albums::refresh(&pool);
            })
            .await;

            if let Some(this) = weak.upgrade() {
                let raw_ids: Vec<i64> = ids.iter().map(|id| id.get()).collect();
                this.update_media_favorite_flags(&raw_ids, is_favorite);
                this.clear_selection();
                if let Some(nav) = this.imp().nav_view.borrow().as_ref().cloned() {
                    refresh_albums_sidebar(&nav);
                }
            }
        });
    }

    fn update_media_favorite_flags(&self, ids: &[i64], is_favorite: bool) {
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let ids: HashSet<i64> = ids.iter().copied().collect();
        for i in 0..list.n_items() {
            let Some(obj) = list.item(i).and_downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let mut item = obj.borrow::<MediaItem>().clone();
            if ids.contains(&item.id) {
                item.is_favorite = is_favorite;
                list.splice(i, 1, &[glib::BoxedAnyObject::new(item)]);
            }
        }
    }

    /// Clear selection across all three sub-grids and hide the toolbar.
    /// Called after a successful batch operation so the user can continue
    /// browsing without the previous selection leaking in.
    pub fn clear_selection(&self) {
        for grid in self.imp().grids.borrow().iter() {
            grid.clear_selection();
        }
        *self.imp().selected_ids.borrow_mut() = HashSet::new();
        self.imp().select_all_btn.get().set_visible(false);
        self.imp().add_to_album_btn.get().set_visible(false);
        self.imp().favorite_btn.get().set_visible(false);
        self.imp().delete_to_trash_btn.get().set_visible(false);
    }

    fn open_viewer(&self, media_id: MediaId) {
        if self.imp().viewer_open_pending.get() {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "PhotosPage: ignoring duplicate viewer activation while push is pending"
            );
            return;
        }

        let nav = match self.imp().nav_view.borrow().as_ref() {
            Some(n) => n.clone(),
            None => return,
        };
        let self_page: adw::NavigationPage = self.clone().upcast();
        if nav
            .visible_page()
            .is_some_and(|visible| visible != self_page)
        {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "PhotosPage: ignoring viewer activation because PhotosPage is not visible"
            );
            return;
        }

        // Opening a viewer implicitly cancels any active multi-select — the
        // user is moving to single-photo mode. Otherwise stale selection
        // would persist after popping back, and a shift-click in the
        // viewer-mode area could re-add a stale index.
        self.clear_selection();

        let media_list = match self.imp().media_list.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let Some(global_index) = index_for_media_id(&media_list, media_id) else {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "PhotosPage: ignoring viewer activation for media_id={} because it is not in the current window",
                media_id.get()
            );
            return;
        };
        let displayed_indices = self
            .current_grid()
            .map(|grid| grid.displayed_indices())
            .unwrap_or_default();
        let displayed_pos = displayed_indices
            .iter()
            .position(|index| *index == global_index);
        let around = [
            global_index.checked_sub(2),
            global_index.checked_sub(1),
            Some(global_index),
            global_index.checked_add(1),
            global_index.checked_add(2),
        ]
        .into_iter()
        .flatten()
        .filter_map(|index| {
            media_item_for_index(&media_list, index)
                .map(|item| format!("{index}:{}:{}", item.id, item.display_name()))
        })
        .collect::<Vec<_>>();
        if let Some(item) = media_item_for_index(&media_list, global_index) {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_TRACE photos_open_viewer global_index={} list_len={} displayed_pos={:?} displayed_len={} displayed_first={:?} displayed_last={:?} around={:?} item_id={} item_name={} item_uri={} sort_time={}",
                global_index,
                media_list.n_items(),
                displayed_pos,
                displayed_indices.len(),
                displayed_indices.first(),
                displayed_indices.last(),
                around,
                item.id,
                item.display_name(),
                item.uri,
                item.sort_datetime()
            );
        } else {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_TRACE photos_open_viewer missing_item global_index={} list_len={} displayed_pos={:?} displayed_len={}",
                global_index,
                media_list.n_items(),
                displayed_pos,
                displayed_indices.len()
            );
        }
        let viewer_debug_label = format!("viewer-open-id-{}", media_id.get());
        nav.connect_visible_page_notify({
            let label = viewer_debug_label.clone();
            move |nav| {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIEWER_DEBUG nav visible_page_notify label={} visible={:?}",
                    label,
                    nav.visible_page().map(|page| page.title())
                );
            }
        });
        nav.connect_pushed({
            let label = viewer_debug_label.clone();
            move |nav| {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIEWER_DEBUG nav pushed label={} visible={:?}",
                    label,
                    nav.visible_page().map(|page| page.title())
                );
            }
        });
        nav.connect_popped({
            let label = viewer_debug_label.clone();
            move |nav, page| {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIEWER_DEBUG nav popped label={} popped_page={} visible_after={:?}",
                    label,
                    page.title(),
                    nav.visible_page().map(|page| page.title())
                );
            }
        });

        let viewer = ViewerPage::new_for_query(MediaQuery::LiveAll, media_id, media_list);

        // Wire the viewer's Edit button: it reveals the editor panel inside `nav`.
        if let Some(pool) = self.imp().pool.borrow().as_ref() {
            viewer.set_edit_target(&nav, pool.clone());
        }

        // Inject the shared thumbnail loader for the filmstrip.
        if let Some(loader) = self.imp().loader.borrow().as_ref() {
            viewer.set_thumbnail_loader(loader.clone());
        }

        viewer.show_at(global_index);

        let weak = self.downgrade();
        let nav_for_refresh = nav.downgrade();
        viewer.connect_favorite_state_changed(move |item_id, target_state| {
            if let Some(nav) = nav_for_refresh.upgrade() {
                refresh_albums_sidebar(&nav);
            }
            if let Some(this) = weak.upgrade() {
                this.update_media_favorite_flags(&[item_id], target_state);
            }
        });

        // Wire the viewer's keyboard callback: pops via the host NavigationView
        // for ESC, or advances/retreats the current index for ←/→.
        let viewer_weak = viewer.downgrade();
        let nav_weak = nav.downgrade();
        viewer.connect_navigation(move |delta: NavDelta| {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_DEBUG photos_page navigation_callback delta={}",
                delta
            );
            if delta == NAV_POP {
                if let Some(n) = nav_weak.upgrade() {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "VIEWER_DEBUG photos_page executing nav.pop visible_before={:?}",
                        n.visible_page().map(|page| page.title())
                    );
                    n.pop();
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "VIEWER_DEBUG photos_page after nav.pop visible_after={:?}",
                        n.visible_page().map(|page| page.title())
                    );
                }
                return;
            }
            if let Some(v) = viewer_weak.upgrade() {
                let cur = v.current_index();
                let next = (cur as i32 + delta).max(0) as u32;
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIEWER_DEBUG photos_page arrow_nav cur={} delta={} next={}",
                    cur,
                    delta,
                    next
                );
                if let Some(list) = v.imp().media_list.borrow().as_ref() {
                    if next < list.n_items() {
                        v.show_at(next);
                    }
                }
            }
        });

        self.imp().viewer_open_pending.set(true);
        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(350), move || {
            if let Some(this) = weak.upgrade() {
                this.imp().viewer_open_pending.set(false);
            }
        });

        // Push the new viewer. While the transition settles, duplicate
        // activations are ignored so rapid double-clicks cannot stack viewer
        // pages or immediately trip viewer-level navigation.
        nav.push(&viewer);
    }
}

fn media_item_for_index(
    media_list: &gtk::gio::ListStore,
    index: u32,
) -> Option<crate::core::media::MediaItem> {
    let obj = media_list.item(index)?;
    let boxed = obj.downcast::<glib::BoxedAnyObject>().ok()?;
    let item = (*boxed.borrow::<crate::core::media::MediaItem>()).clone();
    Some(item)
}

fn index_for_media_id(media_list: &gtk::gio::ListStore, media_id: MediaId) -> Option<u32> {
    for index in 0..media_list.n_items() {
        let Some(obj) = media_list.item(index) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<crate::core::media::MediaItem>().id == media_id.get() {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn sample_item(id: i64, name: &str) -> crate::core::media::MediaItem {
        let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
        crate::core::media::MediaItem {
            id,
            uri: format!("file:///tmp/{name}"),
            path: PathBuf::from(format!("/tmp/{name}")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/png".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(100),
            height: Some(100),
            video_duration_secs: None,
            taken_at: Some(dt),
            file_mtime: dt,
            file_size: 100,
            blake3_hash: format!("hash-{id}"),
            is_favorite: false,
            trashed_at: None,
        }
    }

    #[gtk::test]
    fn repeated_photo_activation_pushes_only_one_viewer_while_pending() {
        let _ = gtk::init();
        let tmp = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
        let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));

        let nav = adw::NavigationView::new();
        let page = PhotosPage::new(media_list, loader);
        page.set_nav_target(&nav);
        nav.push(&page);

        page.open_viewer(MediaId::from(1));
        page.open_viewer(MediaId::from(1));

        assert_eq!(
            nav.navigation_stack().n_items(),
            2,
            "back-to-back photo activations must not stack duplicate viewer pages"
        );
        assert!(
            nav.visible_page().and_downcast::<ViewerPage>().is_some(),
            "the single pushed page should be a ViewerPage"
        );
    }
}
