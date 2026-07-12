//! PhotosPage: year/month/day view (shared MediaGrid, ModeSelector overlay).
//!
//! Hosts three MediaGrid instances. When the user clicks a tile, a `ViewerPage`
//! is pushed onto the host `AdwNavigationView` (injected via `set_nav_target`).
//! Multi-select is entered via the right-click "Multi-select" item; while it is
//! active the batch toolbar (select all / exit multi-select / add to album /
//! favorite / trash) appears. ESC or the "Exit Multi-select" button leaves
//! multi-select. The "Add to Album" toolbar button opens the `AlbumPickerDialog`.
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

use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::MediaQuery;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::album_picker;
use crate::ui::empty_states;
use crate::ui::keyboard::{KeyboardAction, KeyboardResult};
use crate::ui::media_grid::{FavoriteMenuState, MediaGrid, MediaGridCallbacks};
use crate::ui::mode_selector::ModeSelector;
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP, VIEWER_OPEN_POP_GUARD_MS};
use crate::ui::window::refresh_albums_sidebar;

const PHOTOS_SELECT_ALL_LIMIT: u32 = 2_000;

fn group_mode_name(mode: GroupBy) -> &'static str {
    match mode {
        GroupBy::Year => "year",
        GroupBy::Month => "month",
        GroupBy::Day => "day",
    }
}

fn stack_visible_child_name(stack: &gtk::Stack) -> String {
    stack
        .visible_child_name()
        .map(|name| name.to_string())
        .unwrap_or_else(|| "(none)".to_string())
}

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
        pub db_actor: RefCell<Option<DbActorHandle>>,
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
        /// Coalescing flag for `schedule_scroll_date_update` (mirrors
        /// `contrast_update_pending`).
        pub scroll_date_update_pending: Cell<bool>,
        /// One-shot hide timer: reset on every scroll event; fires ~700ms after
        /// the last scroll to fade the pill out.
        pub scroll_date_hide_timer: RefCell<Option<glib::SourceId>>,
        /// Debounces photo activation while NavigationView is pushing the
        /// viewer. Without this, rapid repeated clicks can stack viewer pages
        /// or race with viewer-level back handling during the transition.
        pub viewer_open_pending: Cell<bool>,
        #[template_child]
        pub scroll_date_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub scroll_date_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub search_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub grid_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub view_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub mode_selector: TemplateChild<ModeSelector>,
        #[template_child]
        pub select_all_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub exit_multi_select_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub add_to_album_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub favorite_btn: TemplateChild<gtk::Button>,
        /// 收藏/取消收藏弹层菜单项（在 new 里建好并 set_parent 到 favorite_btn；
        /// refresh_selection_ui 按选中集状态切换 sensitive）。
        pub favorite_popover: RefCell<Option<gtk::Popover>>,
        pub favorite_item_btn: RefCell<Option<gtk::Button>>,
        pub unfavorite_item_btn: RefCell<Option<gtk::Button>>,
        #[template_child]
        pub delete_to_trash_btn: TemplateChild<gtk::Button>,
        // Selection-gated header buttons are wrapped in Revealers (see
        // photos-page.blp) so they slide in/out of the header instead of
        // snapping when selection mode toggles. The button TemplateChildren
        // above stay valid — Blueprint ids are global within the template
        // regardless of nesting.
        #[template_child]
        pub select_all_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub exit_multi_select_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub delete_to_trash_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub favorite_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub add_to_album_revealer: TemplateChild<gtk::Revealer>,
    }

    impl Default for PhotosPage {
        fn default() -> Self {
            Self {
                media_list: RefCell::new(None),
                loader: RefCell::new(None),
                nav_view: RefCell::new(None),
                pool: RefCell::new(None),
                db_actor: RefCell::new(None),
                grids: RefCell::new(Vec::new()),
                selected_ids: RefCell::new(HashSet::new()),
                contrast_update_pending: Cell::new(false),
                scroll_date_update_pending: Cell::new(false),
                scroll_date_hide_timer: RefCell::new(None),
                viewer_open_pending: Cell::new(false),
                scroll_date_revealer: TemplateChild::default(),
                scroll_date_label: TemplateChild::default(),
                header_bar: TemplateChild::default(),
                search_btn: TemplateChild::default(),
                grid_overlay: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                mode_selector: TemplateChild::default(),
                select_all_btn: TemplateChild::default(),
                exit_multi_select_btn: TemplateChild::default(),
                add_to_album_btn: TemplateChild::default(),
                favorite_btn: TemplateChild::default(),
                favorite_popover: RefCell::new(None),
                favorite_item_btn: RefCell::new(None),
                unfavorite_item_btn: RefCell::new(None),
                delete_to_trash_btn: TemplateChild::default(),
                select_all_revealer: TemplateChild::default(),
                exit_multi_select_revealer: TemplateChild::default(),
                delete_to_trash_revealer: TemplateChild::default(),
                favorite_revealer: TemplateChild::default(),
                add_to_album_revealer: TemplateChild::default(),
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

    impl ObjectImpl for PhotosPage {
        fn dispose(&self) {
            if let Some(popover) = self.favorite_popover.borrow_mut().take() {
                popover.unparent();
            }
        }
    }
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
        // exit_multi_select_btn carries a fixed "退出多选" label + tooltip.
        obj.imp()
            .exit_multi_select_btn
            .get()
            .set_label(&tr("photos.batch.exit_multi_select"));
        obj.imp()
            .exit_multi_select_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.batch.exit_multi_select")));
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
        obj.imp()
            .search_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.search.tooltip")));
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
            on_set_album_cover: None,
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
        let context_menu_overlay = obj.imp().grid_overlay.get();
        year_grid.set_context_menu_overlay(Some(&context_menu_overlay));
        month_grid.set_context_menu_overlay(Some(&context_menu_overlay));
        day_grid.set_context_menu_overlay(Some(&context_menu_overlay));

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
                    this.schedule_scroll_date_update();
                    this.arm_scroll_date_hide();
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
        stack.add_child(&empty_page); // untitled → won't appear in the switcher bar

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
            let prewarm_loader = obj.imp().loader.borrow().clone();
            stack.connect_notify_local(Some("visible-child"), move |stack, _| {
                let this = weak.upgrade();
                let visible_child = stack_visible_child_name(stack);
                let list_len = this
                    .as_ref()
                    .and_then(|page| {
                        page.imp()
                            .media_list
                            .borrow()
                            .as_ref()
                            .map(|list| list.n_items())
                    })
                    .unwrap_or(0);
                let span = tracing::info_span!(
                    target: crate::core::log_targets::BROWSING,
                    "photos:mode_stack_switch",
                    visible_child = %visible_child,
                    list_len
                );
                let _trace = span.enter();
                if let Some(this) = this {
                    this.sync_active_grid_rebuilds();
                    let contrast_span = tracing::info_span!(
                        target: crate::core::log_targets::BROWSING,
                        "photos:mode_contrast_schedule",
                    );
                    let _contrast = contrast_span.enter();
                    this.schedule_mode_selector_contrast_update();
                }
                // 同步后台预热缩略图尺寸到当前视图模式
                if let Some(loader) = &prewarm_loader {
                    let size = match stack.visible_child_name().as_deref() {
                        Some("year") => ThumbnailSize::Small,
                        Some("month" | "day") => ThumbnailSize::Medium,
                        _ => return,
                    };
                    let prewarm_span = tracing::info_span!(
                        target: crate::core::log_targets::BROWSING,
                        "photos:mode_prewarm_size",
                        size = ?size,
                    );
                    let _prewarm = prewarm_span.enter();
                    loader.set_prewarm_thumbnail_size(size);
                }
            });
        }
        // ESC exits multi-select (same as the toolbar "退出多选" button).
        // Capture phase so we intercept the key before any FlowBox binding
        // (e.g. GTK's own selection-mode Escape) can consume it. Only acts
        // while a grid is in multi-select; otherwise the key propagates.
        let esc_ctrl = gtk::EventControllerKey::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let weak = obj.downgrade();
        esc_ctrl.connect_key_pressed(move |_, key, _, _| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape {
                let any_multi = this
                    .imp()
                    .grids
                    .borrow()
                    .iter()
                    .any(|g| g.is_multi_select_mode());
                if any_multi {
                    this.clear_selection();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        obj.add_controller(esc_ctrl);

        obj.sync_active_grid_rebuilds();
        obj.schedule_mode_selector_contrast_update();
        // 初始化时也同步一次
        if let Some(loader) = obj.imp().loader.borrow().as_ref() {
            loader.set_prewarm_thumbnail_size(ThumbnailSize::Small); // Year 是默认模式
        }

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
        obj.imp().search_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.open_search_page();
            }
        });

        // Exit multi-select: clears selection across every grid and hides the
        // batch toolbar. Same effect as the right-click "Exit Multi-select".
        let weak = obj.downgrade();
        obj.imp()
            .exit_multi_select_btn
            .get()
            .connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.clear_selection();
                }
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
            *obj.imp().favorite_popover.borrow_mut() = Some(popover.clone());
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

    pub fn set_db_actor(&self, db_actor: DbActorHandle) {
        *self.imp().db_actor.borrow_mut() = Some(db_actor);
    }

    pub fn media_list(&self) -> Ref<'_, Option<gtk::gio::ListStore>> {
        self.imp().media_list.borrow()
    }

    pub(crate) fn open_search_page(&self) {
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return;
        };
        let page = crate::ui::search_page::SearchPage::new(pool, loader);
        if let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() {
            page.set_db_actor(db_actor);
        }
        page.set_nav_target(&nav);
        nav.push(&page);
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
        let any_multi = self
            .imp()
            .grids
            .borrow()
            .iter()
            .any(|g| g.is_multi_select_mode());
        *self.imp().selected_ids.borrow_mut() = union;
        let select_all_limit_reached = self.selected_reaches_select_all_limit();
        self.imp()
            .select_all_revealer
            .get()
            .set_reveal_child(has_any);
        self.imp()
            .add_to_album_revealer
            .get()
            .set_reveal_child(has_any);
        self.imp()
            .delete_to_trash_revealer
            .get()
            .set_reveal_child(has_any);
        // Exit-multi-select is bound to multi-select *mode*, not to having a
        // selection, so the user can always leave multi-select even after
        // deselecting everything (otherwise they'd be stuck with no toolbar).
        self.imp()
            .exit_multi_select_revealer
            .get()
            .set_reveal_child(any_multi);
        // select_all_btn keeps a text label that toggles 全选/取消全选.
        if select_all_limit_reached {
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
            let ids: Vec<MediaId> = self.imp().selected_ids.borrow().iter().copied().collect();
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
        self.imp().favorite_revealer.get().set_reveal_child(has_any);
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
        let mut selected: Vec<MediaId> = self.imp().selected_ids.borrow().iter().copied().collect();
        selected.sort_unstable();
        selected
    }

    pub(crate) fn handle_keyboard_action(&self, action: KeyboardAction) -> KeyboardResult {
        match action {
            KeyboardAction::SelectAll => {
                self.select_all_in_current_mode();
                KeyboardResult::Handled
            }
            KeyboardAction::Delete => {
                let ids = self.selected_ids_vec();
                if ids.is_empty() {
                    KeyboardResult::Ignored
                } else {
                    self.delete_to_trash_for_ids(ids);
                    KeyboardResult::Handled
                }
            }
            KeyboardAction::CancelOrClose => {
                let has_selection = !self.selected_ids_vec().is_empty()
                    || self
                        .imp()
                        .grids
                        .borrow()
                        .iter()
                        .any(|grid| grid.is_multi_select_mode());
                if has_selection {
                    self.clear_selection();
                    KeyboardResult::Handled
                } else {
                    KeyboardResult::Ignored
                }
            }
            _ => KeyboardResult::Ignored,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_count_for_tests(&self) -> usize {
        self.imp().selected_ids.borrow().len()
    }

    fn current_grid(&self) -> Option<MediaGrid> {
        let stack = self.imp().view_stack.get();
        let visible = stack.visible_child()?;
        visible.downcast::<MediaGrid>().ok()
    }

    fn sync_active_grid_rebuilds(&self) {
        let stack = self.imp().view_stack.get();
        let visible_child = stack_visible_child_name(&stack);
        let current = self.current_grid();
        let grids = self.imp().grids.borrow();
        let span = tracing::info_span!(
            target: crate::core::log_targets::BROWSING,
            "photos:sync_active_grid_rebuilds",
            visible_child = %visible_child,
            grid_count = grids.len(),
            active_mode = tracing::field::Empty
        );
        let _trace = span.enter();
        let mut active_mode = "none";
        for grid in grids.iter() {
            let active = current.as_ref().is_some_and(|visible| visible == grid);
            if active {
                active_mode = group_mode_name(grid.mode());
            }
            grid.set_active(active);
        }
        span.record("active_mode", active_mode);
    }

    fn select_all_in_current_mode(&self) {
        if self.selected_reaches_select_all_limit() {
            self.clear_selection();
            return;
        }

        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            if let Some(grid) = self.current_grid() {
                grid.select_all();
            }
            return;
        };
        let repo = crate::core::repository::MediaRepository::new(pool);
        let Ok(items) = repo.items(MediaQuery::LiveAll, 0, PHOTOS_SELECT_ALL_LIMIT) else {
            return;
        };
        let ids = items
            .into_iter()
            .map(|item| MediaId::from(item.id))
            .collect::<Vec<_>>();
        for grid in self.imp().grids.borrow().iter() {
            grid.select_ids(&ids);
        }
    }

    fn selected_reaches_select_all_limit(&self) -> bool {
        let selected_count = self.imp().selected_ids.borrow().len();
        if selected_count == 0 {
            return false;
        }
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return self
                .current_grid()
                .is_some_and(|grid| grid.is_all_displayed_selected());
        };
        let repo = crate::core::repository::MediaRepository::new(pool);
        let Ok(total) = repo.count(MediaQuery::LiveAll) else {
            return false;
        };
        let target = total.min(PHOTOS_SELECT_ALL_LIMIT) as usize;
        target > 0 && selected_count >= target
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

    /// Refresh the scroll-date pill: resolve the current grid's date section,
    /// update the label, position the pill alongside the scrollbar thumb, and
    /// reveal it. Hides itself when there is nothing to show.
    fn update_scroll_date(&self) {
        let Some(grid) = self.current_grid() else {
            self.imp().scroll_date_revealer.set_reveal_child(false);
            return;
        };
        let Some(key) = grid.current_scroll_section_key() else {
            self.imp().scroll_date_revealer.set_reveal_child(false);
            return;
        };

        let imp = self.imp();
        imp.scroll_date_label
            .set_label(&crate::core::section_model::make_label_nocount(&key));

        // Track the thumb vertically. Only position once the overlay is
        // allocated; before that, heights are 0 and we just reveal at the top.
        let overlay = imp.grid_overlay.get();
        let revealer = imp.scroll_date_revealer.get();
        let overlay_h = overlay.height() as f32;
        let pill_h = revealer.height().max(1) as f32;
        if overlay_h > pill_h {
            let margin = 8.0_f32;
            let usable = (overlay_h - pill_h - 2.0 * margin).max(0.0);
            let top = margin + (grid.scroll_fraction() as f32) * usable;
            revealer.set_margin_top(top.round() as i32);
        }
        revealer.set_reveal_child(true);
    }

    /// Coalesce scroll-date updates (the resolution is cheap but we still avoid
    /// queuing more than one per idle tick during kinetic scrolling). Mirrors
    /// `schedule_mode_selector_contrast_update`.
    fn schedule_scroll_date_update(&self) {
        if self.imp().scroll_date_update_pending.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.update_scroll_date();
                this.imp().scroll_date_update_pending.set(false);
            }
        });
    }

    /// (Re)arm the one-shot hide timer so the pill fades out ~700ms after the
    /// last scroll event.
    fn arm_scroll_date_hide(&self) {
        let imp = self.imp();
        // Cancel any previously-armed hide timer. A one-shot source is
        // auto-destroyed by GLib once it fires, and `SourceId::remove` panics
        // ("source not found") when called on such a stale id. The fire
        // callback below clears the stored id; this guard is defense-in-depth —
        // a fired source is simply gone, so there is nothing to remove.
        if let Some(old) = imp.scroll_date_hide_timer.borrow_mut().take() {
            if glib::MainContext::default()
                .find_source_by_id(&old)
                .is_some()
            {
                old.remove();
            }
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            if let Some(this) = weak.upgrade() {
                this.imp().scroll_date_revealer.set_reveal_child(false);
                // Drop the now-fired source's id so the next arm does not try
                // to remove a source GLib has already destroyed.
                *this.imp().scroll_date_hide_timer.borrow_mut() = None;
            }
        });
        *imp.scroll_date_hide_timer.borrow_mut() = Some(id);
    }

    fn open_album_picker_for_ids(&self, ids: Vec<MediaId>) {
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        let raw_ids: Vec<i64> = ids.into_iter().map(MediaId::get).collect();
        album_picker::AlbumPickerDialog::present(&nav, pool, db_actor, raw_ids);
    }

    fn delete_to_trash_for_ids(&self, ids: Vec<MediaId>) {
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            tracing::warn!(
                target: crate::core::log_targets::BROWSING,
                "TRASH_TRACE photos_delete_requested_no_actor count={} ids={:?}",
                ids.len(),
                ids.iter().map(|id| id.get()).collect::<Vec<_>>()
            );
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            tracing::warn!(
                target: crate::core::log_targets::BROWSING,
                "TRASH_TRACE photos_delete_requested_no_pool count={} ids={:?}",
                ids.len(),
                ids.iter().map(|id| id.get()).collect::<Vec<_>>()
            );
            return;
        };
        if ids.is_empty() {
            return;
        }
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "TRASH_TRACE photos_delete_requested count={} ids={:?}",
            ids.len(),
            ids.iter().map(|id| id.get()).collect::<Vec<_>>()
        );

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let prepared = db_actor
                .execute(DbCommand::MarkTrashed {
                    ids: ids_for_worker,
                })
                .await
                .ok();

            let Some(crate::core::DbCommandResult::MediaItems(items)) = prepared else {
                tracing::warn!(
                    target: crate::core::log_targets::BROWSING,
                    "TRASH_TRACE photos_mark_failed"
                );
                return;
            };
            if let Some(this) = weak.upgrade() {
                crate::ui::trash_fallback::move_marked_items_with_fallback(
                    &this,
                    pool,
                    db_actor,
                    items,
                    move |_| {
                        if let Some(this) = weak.upgrade() {
                            this.clear_selection();
                        }
                    },
                );
            }
        });
    }

    fn set_favorite_for_ids(&self, ids: Vec<MediaId>, is_favorite: bool) {
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            return;
        };
        if ids.is_empty() {
            return;
        }

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let result = db_actor
                .execute(DbCommand::SetFavorite {
                    ids: ids_for_worker,
                    is_favorite,
                })
                .await;

            if result.is_ok() {
                if let Some(this) = weak.upgrade() {
                    this.clear_selection();
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
        self.imp().select_all_revealer.get().set_reveal_child(false);
        self.imp()
            .add_to_album_revealer
            .get()
            .set_reveal_child(false);
        self.imp().favorite_revealer.get().set_reveal_child(false);
        self.imp()
            .delete_to_trash_revealer
            .get()
            .set_reveal_child(false);
        self.imp()
            .exit_multi_select_revealer
            .get()
            .set_reveal_child(false);
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
        // The browsing refactor wraps PhotosPage inside `browsing_root_page`,
        // so `nav.visible_page()` is the wrapper rather than this page.
        // Match AlbumDetailPage's check: bail when this widget is not actually
        // the visible browsing child (e.g. a viewer/search/trash page is on
        // top of the nav stack).
        if !self.is_visible() {
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
        let viewer = ViewerPage::new_for_query(MediaQuery::LiveAll, media_id, media_list);

        // Wire the viewer's Edit button: it reveals the editor panel inside `nav`.
        if let Some(pool) = self.imp().pool.borrow().as_ref() {
            viewer.set_edit_target(&nav, pool.clone());
        }
        if let Some(db_actor) = self.imp().db_actor.borrow().as_ref() {
            viewer.set_db_actor(db_actor.clone());
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
            if delta == NAV_POP {
                if let Some(n) = nav_weak.upgrade() {
                    n.pop();
                }
                return;
            }
            if let Some(v) = viewer_weak.upgrade() {
                let cur = v.current_index();
                let next = (cur as i32 + delta).max(0) as u32;
                if let Some(list) = v.imp().media_list.borrow().as_ref() {
                    if next < list.n_items() {
                        v.show_at(next);
                    }
                }
            }
        });

        self.imp().viewer_open_pending.set(true);
        let source_page = nav.visible_page();
        if let Some(page) = source_page.as_ref() {
            page.set_sensitive(false);
        }
        let weak = self.downgrade();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(VIEWER_OPEN_POP_GUARD_MS),
            move || {
                if let Some(this) = weak.upgrade() {
                    this.imp().viewer_open_pending.set(false);
                }
                if let Some(page) = source_page {
                    page.set_sensitive(true);
                }
            },
        );

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
mod tests;
