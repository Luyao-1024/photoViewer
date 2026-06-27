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

use crate::core::i18n::tr;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::core::{
    albums,
    db::{self, DbPool},
    trash,
};
use crate::ui::album_picker;
use crate::ui::empty_states;
use crate::ui::media_grid::{FavoriteMenuState, MediaGrid};
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
        pub selected_indices: RefCell<HashSet<u32>>,
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
        #[template_child]
        pub unfavorite_btn: TemplateChild<gtk::Button>,
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
                selected_indices: RefCell::new(HashSet::new()),
                contrast_update_pending: Cell::new(false),
                viewer_open_pending: Cell::new(false),
                header_bar: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                mode_selector: TemplateChild::default(),
                select_all_btn: TemplateChild::default(),
                add_to_album_btn: TemplateChild::default(),
                favorite_btn: TemplateChild::default(),
                unfavorite_btn: TemplateChild::default(),
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
        obj.imp()
            .add_to_album_btn
            .get()
            .set_label(&tr("photos.batch.move_to_album"));
        obj.imp()
            .add_to_album_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.batch.move_to_album")));
        obj.imp()
            .favorite_btn
            .get()
            .set_label(&tr("photos.batch.favorite"));
        obj.imp()
            .favorite_btn
            .set_tooltip_text(Some(&tr("viewer.button.favorite")));
        obj.imp()
            .unfavorite_btn
            .get()
            .set_label(&tr("photos.batch.unfavorite"));
        obj.imp()
            .unfavorite_btn
            .set_tooltip_text(Some(&tr("viewer.button.favorite_active")));
        obj.imp()
            .delete_to_trash_btn
            .get()
            .set_label(&tr("viewer.tooltip.move_to_trash"));
        obj.imp()
            .delete_to_trash_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.move_to_trash")));
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        // Snapshot the initial size before `media_list` is moved into MediaGrid.
        let is_empty = media_list.n_items() == 0;

        let on_activate: Rc<dyn Fn(u32)> = {
            let weak = obj.downgrade();
            Rc::new(move |global_index| {
                if let Some(this) = weak.upgrade() {
                    this.open_viewer(global_index);
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
        let on_add_to_album: Rc<dyn Fn(Vec<u32>)> = {
            let weak = obj.downgrade();
            Rc::new(move |indices| {
                if let Some(this) = weak.upgrade() {
                    this.open_album_picker_for_indices(indices);
                }
            })
        };
        let on_move_to_trash: Rc<dyn Fn(Vec<u32>)> = {
            let weak = obj.downgrade();
            Rc::new(move |indices| {
                if let Some(this) = weak.upgrade() {
                    this.delete_to_trash_for_indices(indices);
                }
            })
        };
        let on_favorite: Rc<dyn Fn(Vec<u32>, bool)> = {
            let weak = obj.downgrade();
            Rc::new(move |indices, is_favorite| {
                if let Some(this) = weak.upgrade() {
                    this.set_favorite_for_indices(indices, is_favorite);
                }
            })
        };
        let on_query_favorite_state = {
            let weak = obj.downgrade();
            Rc::new(move |indices: Vec<u32>| {
                weak.upgrade()
                    .map(|this| this.favorite_state_for_indices(&indices))
                    .unwrap_or_default()
            })
        };

        // Three independent MediaGrid instances — one per grouping mode.
        let year_grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Year,
            loader.clone(),
            on_activate.clone(),
            on_background_changed.clone(),
            on_add_to_album.clone(),
            on_move_to_trash.clone(),
            on_favorite.clone(),
            on_query_favorite_state.clone(),
            true,
        );
        let month_grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Month,
            loader.clone(),
            on_activate.clone(),
            on_background_changed.clone(),
            on_add_to_album.clone(),
            on_move_to_trash.clone(),
            on_favorite.clone(),
            on_query_favorite_state.clone(),
            true,
        );
        let day_grid = MediaGrid::new(
            media_list,
            GroupBy::Day,
            loader,
            on_activate,
            on_background_changed,
            on_add_to_album,
            on_move_to_trash,
            on_favorite,
            on_query_favorite_state,
            true,
        );

        // Wire selection-changed: each grid fires when its own `selected` set
        // changes. We collect the union into PhotosPage's `selected_indices`
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

        // Wire the ModeSelector to our view_stack (it drives the visible
        // child and reflects any external change back via notify).
        obj.imp().mode_selector.get().set_stack(&stack);
        {
            let weak = obj.downgrade();
            stack.connect_notify_local(Some("visible-child"), move |_, _| {
                if let Some(this) = weak.upgrade() {
                    this.schedule_mode_selector_contrast_update();
                }
            });
        }
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

        let weak = obj.downgrade();
        obj.imp().favorite_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let indices = this.selected_indices_vec();
            if indices.is_empty() {
                return;
            }
            this.set_favorite_for_indices(indices, true);
        });

        let weak = obj.downgrade();
        obj.imp().unfavorite_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let indices = this.selected_indices_vec();
            if indices.is_empty() {
                return;
            }
            this.set_favorite_for_indices(indices, false);
        });

        let weak = obj.downgrade();
        obj.imp()
            .delete_to_trash_btn
            .get()
            .connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let indices = this.selected_indices_vec();
                if indices.is_empty() {
                    return;
                }
                let count = indices.len();
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
                let indices2 = indices;
                dialog.connect_response(None, move |_, response| {
                    if response == "trash" {
                        if let Some(this) = weak2.upgrade() {
                            this.delete_to_trash_for_indices(indices2.clone());
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

    /// Rebuild the union of selected indices from each visible grid, then
    /// show / hide the "Add to Album" button. Cheap (HashSet union) so it's
    /// fine to call on every `selection-changed` tick.
    fn refresh_selection_ui(&self) {
        let mut union: HashSet<u32> = HashSet::new();
        for grid in self.imp().grids.borrow().iter() {
            union.extend(grid.selected_indices());
        }
        let has_any = !union.is_empty();
        let all_displayed_selected = self
            .current_grid()
            .is_some_and(|grid| grid.is_all_displayed_selected());
        *self.imp().selected_indices.borrow_mut() = union;
        self.imp().select_all_btn.get().set_visible(has_any);
        self.imp().add_to_album_btn.get().set_visible(has_any);
        self.imp().delete_to_trash_btn.get().set_visible(has_any);
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
            let indices: Vec<u32> = self
                .imp()
                .selected_indices
                .borrow()
                .iter()
                .copied()
                .collect();
            self.favorite_state_for_indices(&indices)
        } else {
            FavoriteMenuState::default()
        };
        self.imp()
            .favorite_btn
            .get()
            .set_visible(state.can_favorite);
        self.imp()
            .unfavorite_btn
            .get()
            .set_visible(state.can_unfavorite);
    }

    fn favorite_state_for_indices(&self, indices: &[u32]) -> FavoriteMenuState {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return FavoriteMenuState::default();
        };
        let ids = self.media_ids_for_indices(indices);
        if ids.is_empty() {
            return FavoriteMenuState::default();
        }

        let mut has_favorite = false;
        let mut has_unfavorite = false;
        for id in ids {
            match db::is_media_favorite(&pool, id) {
                Ok(is_favorite) => {
                    if is_favorite {
                        has_favorite = true;
                    } else {
                        has_unfavorite = true;
                    }
                }
                Err(_) => {
                    has_unfavorite = true;
                }
            }
        }

        FavoriteMenuState {
            can_favorite: has_unfavorite,
            can_unfavorite: has_favorite,
        }
    }

    fn selected_indices_vec(&self) -> Vec<u32> {
        let mut selected: Vec<u32> = self
            .imp()
            .selected_indices
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
        let indices = self.selected_indices_vec();
        if indices.is_empty() {
            return;
        }
        self.open_album_picker_for_indices(indices);
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

    fn media_ids_for_indices(&self, indices: &[u32]) -> Vec<i64> {
        let media_list = self.imp().media_list.borrow();
        let list = match media_list.as_ref() {
            Some(list) => list,
            None => return Vec::new(),
        };
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        for &gi in indices {
            if gi >= list.n_items() {
                continue;
            }
            let Some(obj) = list.item(gi) else {
                continue;
            };
            let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let id = boxed.borrow::<crate::core::media::MediaItem>().id;
            if seen.insert(id) {
                ids.push(id);
            }
        }
        ids
    }

    fn open_album_picker_for_indices(&self, indices: Vec<u32>) {
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let ids = self.media_ids_for_indices(&indices);
        if ids.is_empty() {
            return;
        }
        album_picker::AlbumPickerDialog::present(&nav, pool, ids);
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

    fn delete_to_trash_for_indices(&self, indices: Vec<u32>) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let ids = self.media_ids_for_indices(&indices);
        if ids.is_empty() {
            return;
        }

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let removed = gtk::gio::spawn_blocking(move || {
                let mut removed = Vec::new();
                for id in ids_for_worker {
                    let Ok(item) = db::get_media_item(&pool, id) else {
                        continue;
                    };
                    // 先标记后移动（见 trash::move_to_trash_marked）：否则文件监听
                    // 器会在 mark_trashed 提交前按 Remove 事件把行删掉。
                    if trash::move_to_trash_marked(&pool, item.id, &item.uri).is_ok() {
                        removed.push(item.id);
                    }
                }
                let _ = albums::refresh(&pool);
                removed
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

    fn set_favorite_for_indices(&self, indices: Vec<u32>, is_favorite: bool) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let ids = self.media_ids_for_indices(&indices);
        if ids.is_empty() {
            return;
        }

        let weak = self.downgrade();
        let ids_for_worker = ids;
        glib::spawn_future_local(async move {
            let _ = gtk::gio::spawn_blocking(move || {
                for id in ids_for_worker {
                    let _ = db::set_media_favorite(&pool, id, is_favorite);
                }
                let _ = albums::refresh(&pool);
            })
            .await;

            if let Some(this) = weak.upgrade() {
                this.clear_selection();
                if let Some(nav) = this.imp().nav_view.borrow().as_ref().cloned() {
                    refresh_albums_sidebar(&nav);
                }
            }
        });
    }

    /// Clear selection across all three sub-grids and hide the toolbar.
    /// Called after a successful batch operation so the user can continue
    /// browsing without the previous selection leaking in.
    pub fn clear_selection(&self) {
        for grid in self.imp().grids.borrow().iter() {
            grid.clear_selection();
        }
        *self.imp().selected_indices.borrow_mut() = HashSet::new();
        self.imp().select_all_btn.get().set_visible(false);
        self.imp().add_to_album_btn.get().set_visible(false);
        self.imp().favorite_btn.get().set_visible(false);
        self.imp().unfavorite_btn.get().set_visible(false);
        self.imp().delete_to_trash_btn.get().set_visible(false);
    }

    fn open_viewer(&self, global_index: u32) {
        if self.imp().viewer_open_pending.get() {
            tracing::debug!(
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
            tracing::info!(
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
            tracing::info!(
                "VIEWER_TRACE photos_open_viewer missing_item global_index={} list_len={} displayed_pos={:?} displayed_len={}",
                global_index,
                media_list.n_items(),
                displayed_pos,
                displayed_indices.len()
            );
        }
        let viewer_debug_label = format!("viewer-open-index-{global_index}");
        nav.connect_visible_page_notify({
            let label = viewer_debug_label.clone();
            move |nav| {
                tracing::debug!(
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
                    "VIEWER_DEBUG nav popped label={} popped_page={} visible_after={:?}",
                    label,
                    page.title(),
                    nav.visible_page().map(|page| page.title())
                );
            }
        });

        let viewer = ViewerPage::new(media_list, global_index);

        // Wire the viewer's Edit button: it reveals the editor panel inside `nav`.
        if let Some(pool) = self.imp().pool.borrow().as_ref() {
            viewer.set_edit_target(&nav, pool.clone());
        }

        // Wire the viewer's "Add to Album" entry (single-photo).
        if let Some(pool) = self.imp().pool.borrow().as_ref() {
            viewer.set_album_target(&nav, pool.clone());
        }

        // Inject the shared thumbnail loader for the filmstrip.
        if let Some(loader) = self.imp().loader.borrow().as_ref() {
            viewer.set_thumbnail_loader(loader.clone());
        }

        viewer.show_at(global_index);

        let nav_for_refresh = nav.downgrade();
        viewer.connect_favorite_state_changed(move |_, _| {
            if let Some(nav) = nav_for_refresh.upgrade() {
                refresh_albums_sidebar(&nav);
            }
        });

        // Wire the viewer's keyboard callback: pops via the host NavigationView
        // for ESC, or advances/retreats the current index for ←/→.
        let viewer_weak = viewer.downgrade();
        let nav_weak = nav.downgrade();
        viewer.connect_navigation(move |delta: NavDelta| {
            tracing::debug!(
                "VIEWER_DEBUG photos_page navigation_callback delta={}",
                delta
            );
            if delta == NAV_POP {
                if let Some(n) = nav_weak.upgrade() {
                    tracing::debug!(
                        "VIEWER_DEBUG photos_page executing nav.pop visible_before={:?}",
                        n.visible_page().map(|page| page.title())
                    );
                    n.pop();
                    tracing::debug!(
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
            width: Some(100),
            height: Some(100),
            taken_at: Some(dt),
            file_mtime: dt,
            file_size: 100,
            blake3_hash: format!("hash-{id}"),
            trashed_at: None,
        }
    }

    #[gtk::test]
    fn repeated_photo_activation_pushes_only_one_viewer_while_pending() {
        let _ = gtk::init();
        let tmp = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
        let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));

        let nav = adw::NavigationView::new();
        let page = PhotosPage::new(media_list, loader);
        page.set_nav_target(&nav);
        nav.push(&page);

        page.open_viewer(0);
        page.open_viewer(0);

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
