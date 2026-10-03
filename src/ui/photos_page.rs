//! PhotosPage: virtual GtkGridView year/month/day view with a ModeSelector
//! overlay.
//!
//! Hosts three virtual-grid mode instances. When the user clicks a tile, a
//! `ViewerPage` is pushed onto the host `AdwNavigationView` (injected via
//! `set_nav_target`).
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
use std::time::Duration;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt, NavigationPageExt};

use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::i18n::{tr, trf};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::MediaQuery;
use crate::core::section_model::GroupBy;
use crate::core::sync::{
    live_progress, SyncLivePhase, SyncLiveProgress, SyncOverview, SyncOverviewStatus, SyncStore,
};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::album_picker;
use crate::ui::empty_states;
use crate::ui::keyboard::{KeyboardAction, KeyboardResult};
use crate::ui::media_grid::{FavoriteMenuState, MediaGridCallbacks};
use crate::ui::mode_selector::ModeSelector;
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP, VIEWER_OPEN_POP_GUARD_MS};
use crate::ui::virtual_media_grid::VirtualMediaGrid;
use crate::ui::window::refresh_albums_sidebar;

const PHOTOS_SELECT_ALL_LIMIT: u32 = 2_000;
const OVERVIEW_SYNC_ROTATION_PERIOD: Duration = Duration::from_secs(2);
const OVERVIEW_SYNC_PULL_COOLDOWN: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
struct PhotosOverviewSnapshot {
    photos: u32,
    videos: u32,
    /// Every live media, i.e. what 全选 can actually reach. The select-all limit
    /// used to run its own COUNT on each `selection-changed` tick; this worker
    /// already runs off the main thread every two seconds, so the number comes
    /// from here instead (P2-5).
    live_total: u32,
    sync: SyncOverview,
    sync_progress: Option<SyncLiveProgress>,
}

fn sync_overview_text(overview: SyncOverview, progress: Option<SyncLiveProgress>) -> String {
    match overview.status {
        SyncOverviewStatus::Disabled => String::new(),
        SyncOverviewStatus::NotConfigured => tr("photos.overview.sync.not_configured"),
        SyncOverviewStatus::Paused => tr("photos.overview.sync.paused"),
        SyncOverviewStatus::Running => sync_running_text(progress),
        SyncOverviewStatus::Failed => tr("photos.overview.sync.failed"),
        SyncOverviewStatus::Ready => tr("photos.overview.sync.ready"),
        SyncOverviewStatus::Completed if overview.conflict_images > 0 => trf(
            "photos.overview.sync.completed_conflicts",
            &[
                ("count", &overview.synced_items.to_string()),
                ("conflicts", &overview.conflict_images.to_string()),
            ],
        ),
        SyncOverviewStatus::Completed => trf(
            "photos.overview.sync.completed",
            &[("count", &overview.synced_items.to_string())],
        ),
    }
}

/// While a run is active, describe the current transfer activity with live
/// file counts: downloads show "synced X of Y", uploads show "uploading X of
/// Y photos/videos". Byte-level movement is logged per transfer instead of
/// shown here. Before planning has produced totals (and between phases) fall
/// back to the generic running label.
fn sync_running_text(progress: Option<SyncLiveProgress>) -> String {
    let Some(progress) = progress else {
        return tr("photos.overview.sync.running");
    };
    match progress.phase {
        SyncLivePhase::Uploading if progress.upload_total > 0 => trf(
            "photos.overview.sync.running_upload",
            &[
                ("done", &progress.uploaded.to_string()),
                ("total", &progress.upload_total.to_string()),
            ],
        ),
        SyncLivePhase::Downloading if progress.download_total > 0 => trf(
            "photos.overview.sync.running_download",
            &[
                ("done", &progress.downloaded.to_string()),
                ("total", &progress.download_total.to_string()),
            ],
        ),
        _ => tr("photos.overview.sync.running"),
    }
}

fn sync_overview_icon(status: SyncOverviewStatus) -> &'static str {
    match status {
        SyncOverviewStatus::Disabled => "",
        SyncOverviewStatus::Paused => "media-playback-pause-symbolic",
        SyncOverviewStatus::Running => "emblem-synchronizing-symbolic",
        SyncOverviewStatus::Failed => "dialog-warning-symbolic",
        SyncOverviewStatus::Completed => "emblem-ok-symbolic",
        SyncOverviewStatus::NotConfigured | SyncOverviewStatus::Ready => "folder-remote-symbolic",
    }
}

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

/// The three placeholders the Photos stack swaps in when a grid has nothing to
/// show. They are kept together so `update_placeholder_child` stays the single
/// place that decides *why* the grid is empty — indexing, a failed scan, or a
/// genuinely empty library. Presenting only "暂无照片" during a 16 s scan tells
/// the user their library is empty when it is not.
struct Placeholders {
    empty: adw::StatusPage,
    scanning: adw::StatusPage,
    scan_error: adw::StatusPage,
}

const PLACEHOLDER_EMPTY: &str = "empty";
const PLACEHOLDER_SCANNING: &str = "scanning";
const PLACEHOLDER_SCAN_ERROR: &str = "scan-error";

fn is_placeholder_name(name: &str) -> bool {
    matches!(
        name,
        PLACEHOLDER_EMPTY | PLACEHOLDER_SCANNING | PLACEHOLDER_SCAN_ERROR
    )
}

mod imp {
    use super::*;
    use adw::subclass::prelude::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/luyao_1024/photoviewer/ui/photos-page.ui")]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub pool: RefCell<Option<DbPool>>,
        pub db_actor: RefCell<Option<DbActorHandle>>,
        /// Tracks the three Photos virtual grids so selection and mode
        /// callbacks remain shared across Year, Month, and Day.
        pub(super) grids: RefCell<Vec<VirtualMediaGrid>>,
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
        /// Placeholder children built once in `constructed`, so switching
        /// between "indexing / scan failed / really empty" never reallocates
        /// the stack.
        pub(super) placeholders: RefCell<Option<Placeholders>>,
        /// True while the startup index pass is running. Drives the scanning
        /// placeholder and the live "已找到 N 个项目" count.
        pub scan_active: Cell<bool>,
        /// Failure text from the last scan pass, if it did not complete.
        pub scan_error: RefCell<Option<String>>,
        /// Guards against stacking retry scans when the button is clicked twice.
        pub scan_retry_in_flight: Cell<bool>,
        /// Debounces photo activation while NavigationView is pushing the
        /// viewer. Without this, rapid repeated clicks can stack viewer pages
        /// or race with viewer-level back handling during the transition.
        pub viewer_open_pending: Cell<bool>,
        pub overview_refresh_in_flight: Cell<bool>,
        /// Live-media total, refreshed by the overview worker. It only decides
        /// 全选 vs 取消全选, and reading it on a `selection-changed` tick used to
        /// mean a synchronous COUNT on the main thread (P2-5).
        pub live_total: Cell<Option<u32>>,
        /// Bumped whenever the selection set actually changes, so an answer
        /// computed for an older selection is dropped instead of painting the
        /// batch heart from stale data.
        pub selection_generation: Cell<u64>,
        pub favorite_state_in_flight: Cell<bool>,
        /// The favorite visuals currently applied to the batch heart. The click
        /// handler decides from this rather than querying again.
        pub selection_favorite_state: Cell<FavoriteMenuState>,
        pub select_all_in_flight: Cell<bool>,
        pub overview_poll_source: RefCell<Option<glib::SourceId>>,
        pub overview_sync_running: Cell<bool>,
        pub overview_sync_tick_active: Cell<bool>,
        pub overview_sync_started_at: Cell<Option<std::time::Instant>>,
        pub overview_sync_last_pull_at: Cell<Option<std::time::Instant>>,
        pub overview_sync_pull_in_flight: Cell<bool>,
        #[template_child]
        pub overview_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub overview_panel: TemplateChild<gtk::Box>,
        #[template_child]
        pub overview_count_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub overview_sync_row: TemplateChild<gtk::Box>,
        #[template_child]
        pub overview_sync_icon_host: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub overview_sync_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub overview_sync_spinner: TemplateChild<gtk::DrawingArea>,
        #[template_child]
        pub overview_sync_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub overview_sync_retry_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub scroll_date_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub scroll_date_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub search_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub overview_toggle_btn: TemplateChild<gtk::Button>,
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
        /// The corner caret that says "this heart opens a menu" — only for a
        /// mixed selection (P2-9).
        #[template_child]
        pub favorite_menu_hint: TemplateChild<gtk::Image>,
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
        #[template_child]
        pub selection_count_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub selection_count_label: TemplateChild<gtk::Label>,
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
                placeholders: RefCell::new(None),
                scan_active: Cell::new(false),
                scan_error: RefCell::new(None),
                scan_retry_in_flight: Cell::new(false),
                viewer_open_pending: Cell::new(false),
                overview_refresh_in_flight: Cell::new(false),
                live_total: Cell::new(None),
                selection_generation: Cell::new(0),
                favorite_state_in_flight: Cell::new(false),
                selection_favorite_state: Cell::new(FavoriteMenuState::default()),
                select_all_in_flight: Cell::new(false),
                overview_poll_source: RefCell::new(None),
                overview_sync_running: Cell::new(false),
                overview_sync_tick_active: Cell::new(false),
                overview_sync_started_at: Cell::new(None),
                overview_sync_last_pull_at: Cell::new(None),
                overview_sync_pull_in_flight: Cell::new(false),
                overview_revealer: TemplateChild::default(),
                overview_panel: TemplateChild::default(),
                overview_count_label: TemplateChild::default(),
                overview_sync_row: TemplateChild::default(),
                overview_sync_icon_host: TemplateChild::default(),
                overview_sync_icon: TemplateChild::default(),
                overview_sync_spinner: TemplateChild::default(),
                overview_sync_label: TemplateChild::default(),
                overview_sync_retry_btn: TemplateChild::default(),
                scroll_date_revealer: TemplateChild::default(),
                scroll_date_label: TemplateChild::default(),
                header_bar: TemplateChild::default(),
                search_btn: TemplateChild::default(),
                overview_toggle_btn: TemplateChild::default(),
                grid_overlay: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                mode_selector: TemplateChild::default(),
                select_all_btn: TemplateChild::default(),
                exit_multi_select_btn: TemplateChild::default(),
                add_to_album_btn: TemplateChild::default(),
                favorite_btn: TemplateChild::default(),
                favorite_menu_hint: TemplateChild::default(),
                favorite_popover: RefCell::new(None),
                favorite_item_btn: RefCell::new(None),
                unfavorite_item_btn: RefCell::new(None),
                delete_to_trash_btn: TemplateChild::default(),
                select_all_revealer: TemplateChild::default(),
                exit_multi_select_revealer: TemplateChild::default(),
                delete_to_trash_revealer: TemplateChild::default(),
                favorite_revealer: TemplateChild::default(),
                add_to_album_revealer: TemplateChild::default(),
                selection_count_revealer: TemplateChild::default(),
                selection_count_label: TemplateChild::default(),
            }
        }
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for PhotosPage {
        const NAME: &'static str = "PhotosPage";
        type Type = super::PhotosPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            crate::ensure_resources_registered();
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotosPage {
        fn dispose(&self) {
            if let Some(source) = self.overview_poll_source.borrow_mut().take() {
                if glib::MainContext::default()
                    .find_source_by_id(&source)
                    .is_some()
                {
                    source.remove();
                }
            }
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
    /// Build a PhotosPage backed by `media_list`, sharing `loader` across the
    /// three virtual mode-specific grids (Year/Month/Day).
    pub fn new(media_list: gtk::gio::ListStore, loader: Arc<ThumbnailLoader>) -> Self {
        tracing::info!(
            target: crate::core::log_targets::BROWSING,
            "photos grid renderer initialized: GtkGridView"
        );
        let obj: Self = gtk::glib::Object::builder().build();
        crate::ui::motion::apply_to(&obj);
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
        // opens a popover with 收藏/取消收藏. Its tooltip, accessible name and
        // mixed-state caret are painted once below by
        // apply_selection_favorite_state, which is the single writer.
        // The mark itself comes from the shared name, so this button, the
        // viewer's, and the Day-view tile badge cannot end up on different
        // glyphs.
        obj.imp()
            .favorite_btn
            .get()
            .set_icon_name(crate::ui::favorite_icon::NAME);
        obj.imp()
            .delete_to_trash_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.move_to_trash")));
        obj.imp()
            .search_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.search.tooltip")));
        obj.imp()
            .overview_sync_retry_btn
            .get()
            .set_label(&tr("common.retry"));
        obj.apply_overview_disclosure_state();
        obj.imp()
            .overview_count_label
            .get()
            .set_label(&tr("photos.overview.loading"));
        let sync_enabled = crate::core::prefs::webdav_sync_enabled();
        obj.imp().overview_sync_row.get().set_visible(sync_enabled);
        if sync_enabled {
            obj.imp()
                .overview_sync_label
                .get()
                .set_label(&tr("photos.overview.loading"));
        }
        obj.setup_overview_sync_spinner();
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        // Keep a handle on the list before it is moved into the grids; the
        // placeholder switch listens to it.
        let media_list_for_placeholder = media_list.clone();

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

        // Three independent virtual Photos grids — one per grouping mode. Only
        // the currently visible grid is active, so inactive instances do
        // not start metadata/range/thumbnail work while they sit in the stack.
        let year_grid = VirtualMediaGrid::new(
            media_list.clone(),
            GroupBy::Year,
            loader.clone(),
            callbacks.clone(),
            false,
        );
        let month_grid = VirtualMediaGrid::new(
            media_list.clone(),
            GroupBy::Month,
            loader.clone(),
            callbacks.clone(),
            false,
        );
        let day_grid = VirtualMediaGrid::new(media_list, GroupBy::Day, loader, callbacks, true);
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
            let mode = grid.mode();
            let weak = obj.downgrade();
            grid.connect_view_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.schedule_mode_selector_contrast_update();
                    this.schedule_scroll_date_update();
                    this.hide_overview_after_leaving_top();
                }
            });
            let weak = obj.downgrade();
            grid.connect_scroll_intent(move |delta_y| {
                if let Some(this) = weak.upgrade() {
                    this.handle_overview_scroll_intent(mode, delta_y);
                }
            });
        }
        *obj.imp().grids.borrow_mut() =
            vec![year_grid.clone(), month_grid.clone(), day_grid.clone()];

        let stack = obj.imp().view_stack.get();
        stack.add_titled(&year_grid, Some("year"), &tr("photo.mode.year"));
        stack.add_titled(&month_grid, Some("month"), &tr("photo.mode.month"));
        stack.add_titled(&day_grid, Some("day"), &tr("photo.mode.day"));

        // Placeholder states: kept as untitled stack children so we can swap to
        // them without rebuilding the grids. They split the single old
        // "empty means no photos" conclusion into three different facts —
        // indexing, indexing failed, and genuinely nothing to show.
        let empty_page = empty_states::no_photos();
        empty_states::add_action(&empty_page, &tr("empty.no_photos.action"), {
            let weak = obj.downgrade();
            Rc::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.open_library_settings();
                }
            })
        });
        let scanning_page = empty_states::scanning();
        let scan_error_page = empty_states::scan_error("");
        empty_states::add_action(&scan_error_page, &tr("empty.scan_failed.retry"), {
            let weak = obj.downgrade();
            Rc::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.restart_scan();
                }
            })
        });
        for page in [&empty_page, &scanning_page, &scan_error_page] {
            page.set_hexpand(true);
            page.set_vexpand(true);
        }
        stack.add_named(&empty_page, Some(PLACEHOLDER_EMPTY));
        stack.add_named(&scanning_page, Some(PLACEHOLDER_SCANNING));
        stack.add_named(&scan_error_page, Some(PLACEHOLDER_SCAN_ERROR));
        *obj.imp().placeholders.borrow_mut() = Some(Placeholders {
            empty: empty_page,
            scanning: scanning_page,
            scan_error: scan_error_page,
        });
        // GtkStack shows its first added child until something says otherwise,
        // and that first child is the Year grid. `update_placeholder_child` only
        // swaps away from a placeholder, so the default has to be stated here.
        stack.set_visible_child_name("day");
        obj.update_placeholder_child();

        {
            let weak = obj.downgrade();
            media_list_for_placeholder.connect_items_changed(move |_, _, _, _| {
                if let Some(this) = weak.upgrade() {
                    this.update_placeholder_child();
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
                    this.imp().overview_revealer.set_reveal_child(false);
                    this.sync_active_grid_rebuilds();
                    this.schedule_scroll_date_update();
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
        obj.schedule_scroll_date_update();
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

        // P2-8: the overview used to be reachable only by overscrolling past the
        // first row, which nothing announces. The header chevron gives it a
        // visible affordance, and its glyph is a *mirror* of the revealer (not a
        // second state machine), so a pull, a click and a programmatic hide all
        // agree.
        let weak = obj.downgrade();
        obj.imp()
            .overview_toggle_btn
            .get()
            .connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let revealer = this.imp().overview_revealer.get();
                revealer.set_reveal_child(!revealer.reveals_child());
            });

        let weak = obj.downgrade();
        obj.imp()
            .overview_revealer
            .get()
            .connect_reveal_child_notify(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.apply_overview_disclosure_state();
                }
            });

        let weak = obj.downgrade();
        obj.imp()
            .overview_sync_retry_btn
            .get()
            .connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                this.trigger_sync_from_home_pull();
                this.apply_overview_retry_affordance(true);
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
            obj.imp().favorite_btn.get().connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let ids = this.selected_ids_vec();
                if ids.is_empty() {
                    return;
                }
                this.decide_favorite_action(ids);
            });
        }
        // Paint the heart before the first selection so an AT user hears the same
        // verb the tooltip shows, and the mixed-state caret starts hidden.
        obj.apply_selection_favorite_state(FavoriteMenuState::default());

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

    /// Apply the user-selected fixed column count to the Day Photos mode.
    pub fn set_grid_columns(&self, columns: usize) {
        tracing::trace!(
            target: "ui::grid_settings",
            columns,
            grid_count = self.imp().grids.borrow().len(),
            "photos_page_apply_day_grid_columns"
        );
        for grid in self.imp().grids.borrow().iter() {
            grid.set_grid_columns(columns);
        }
    }

    /// Return the minimum Photos content width needed by the configured Day
    /// grid. The setting changes the number of columns, not thumbnail scale.
    pub fn preferred_day_grid_width(columns: usize) -> i32 {
        crate::ui::virtual_media_grid::preferred_day_grid_width(columns)
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
        self.start_overview_updates();
    }

    pub fn set_db_actor(&self, db_actor: DbActorHandle) {
        *self.imp().db_actor.borrow_mut() = Some(db_actor);
    }

    /// Report the startup index pass lifecycle. `active` is true while a scan
    /// runs; `error` carries the failure text when a pass did not complete.
    /// Without this the page can only see "no rows yet", which it used to read
    /// as "the library is empty" during a scan that takes tens of seconds.
    pub fn set_scan_phase(&self, active: bool, error: Option<String>) {
        let imp = self.imp();
        imp.scan_active.set(active);
        let message = error.filter(|text| !text.trim().is_empty());
        *imp.scan_error.borrow_mut() = message.clone();
        if !active {
            imp.scan_retry_in_flight.set(false);
        }
        if let Some(placeholders) = imp.placeholders.borrow().as_ref() {
            placeholders
                .scan_error
                .set_description(Some(&empty_states::scan_error_text(message.as_deref())));
        }
        self.update_placeholder_child();
    }

    /// The single place that decides which placeholder, if any, the view stack
    /// shows. A populated grid always wins — tiles arriving during a scan are
    /// better feedback than a spinner. Only an empty list needs an explanation,
    /// and the three reasons for emptiness must not share one sentence.
    fn update_placeholder_child(&self) {
        let imp = self.imp();
        let stack = imp.view_stack.get();
        let item_count = imp
            .media_list
            .borrow()
            .as_ref()
            .map(|list| list.n_items())
            .unwrap_or(0);
        if item_count > 0 {
            let showing_placeholder = stack
                .visible_child_name()
                .is_some_and(|name| is_placeholder_name(&name));
            if showing_placeholder {
                stack.set_visible_child_name("day");
            }
            return;
        }
        let binding = imp.placeholders.borrow();
        let Some(placeholders) = binding.as_ref() else {
            return;
        };
        if imp.scan_active.get() {
            stack.set_visible_child(&placeholders.scanning);
            return;
        }
        if imp.scan_error.borrow().is_some() {
            stack.set_visible_child(&placeholders.scan_error);
            return;
        }
        stack.set_visible_child(&placeholders.empty);
    }

    /// Retry the index pass from the scan-failed placeholder. Requires the DB
    /// handles injected after construction, so this is a no-op on a page that
    /// never finished wiring.
    fn restart_scan(&self) {
        let imp = self.imp();
        if imp.scan_retry_in_flight.replace(true) {
            return;
        }
        let (Some(pool), Some(db_actor)) = (
            imp.pool.borrow().as_ref().cloned(),
            imp.db_actor.borrow().as_ref().cloned(),
        ) else {
            imp.scan_retry_in_flight.set(false);
            return;
        };
        let roots = crate::config::media_roots();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result =
                crate::core::bootstrap::scan_and_aggregate_with_actor(&pool, &roots, db_actor)
                    .await;
            if let Some(this) = weak.upgrade() {
                this.imp().scan_retry_in_flight.set(false);
                this.update_placeholder_child();
            }
            if let Err(error) = result {
                tracing::warn!("manual rescan failed: {error}");
            }
        });
    }

    /// The empty state's way out: folders are added in Settings, so the page
    /// that says "add folders in Settings" has to open that dialog.
    fn open_library_settings(&self) {
        let Some(window) = self
            .ancestor(crate::ui::MainWindow::static_type())
            .and_downcast::<crate::ui::MainWindow>()
        else {
            tracing::warn!("photos empty state has no MainWindow ancestor to open settings");
            return;
        };
        window.show_settings_dialog();
    }

    pub fn media_list(&self) -> Ref<'_, Option<gtk::gio::ListStore>> {
        self.imp().media_list.borrow()
    }

    fn start_overview_updates(&self) {
        self.refresh_overview_async();
        if self.imp().overview_poll_source.borrow().is_some() {
            return;
        }
        let weak = self.downgrade();
        let source = glib::timeout_add_local(Duration::from_secs(2), move || {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            this.refresh_overview_async();
            glib::ControlFlow::Continue
        });
        *self.imp().overview_poll_source.borrow_mut() = Some(source);
    }

    fn refresh_overview_async(&self) {
        if self.imp().overview_refresh_in_flight.replace(true) {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            self.imp().overview_refresh_in_flight.set(false);
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || {
                let repository = crate::core::repository::MediaRepository::new(pool.clone());
                Ok::<_, crate::core::error::AppError>(PhotosOverviewSnapshot {
                    photos: repository.count(MediaQuery::Images)?,
                    videos: repository.count(MediaQuery::Videos)?,
                    live_total: repository.count(MediaQuery::LiveAll)?,
                    sync: SyncStore::new(pool).overview()?,
                    sync_progress: live_progress(),
                })
            })
            .await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.imp().overview_refresh_in_flight.set(false);
            match result {
                Ok(Ok(snapshot)) => this.apply_overview_snapshot(snapshot),
                Ok(Err(error)) => {
                    tracing::warn!("failed to load Photos overview: {error}");
                    this.apply_overview_error();
                }
                Err(error) => {
                    tracing::warn!("failed to join Photos overview worker: {error:?}");
                    this.apply_overview_error();
                }
            }
        });
    }

    fn apply_overview_snapshot(&self, snapshot: PhotosOverviewSnapshot) {
        let PhotosOverviewSnapshot {
            photos,
            videos,
            live_total,
            sync,
            sync_progress,
        } = snapshot;
        // The select-all label asks "is everything reachable already selected?",
        // which needs this number; it arrives asynchronously now, so re-run the
        // chrome once when a selection was waiting on it.
        let total_changed = self.imp().live_total.replace(Some(live_total)) != Some(live_total);
        if total_changed && !self.imp().selected_ids.borrow().is_empty() {
            self.refresh_selection_ui();
        }
        self.imp().overview_count_label.get().set_label(&trf(
            "photos.overview.counts",
            &[
                ("photos", &photos.to_string()),
                ("videos", &videos.to_string()),
            ],
        ));
        self.imp()
            .overview_sync_label
            .get()
            .set_label(&sync_overview_text(sync, sync_progress));
        let sync_visible = sync.status != SyncOverviewStatus::Disabled;
        self.imp().overview_sync_row.get().set_visible(sync_visible);
        self.apply_overview_retry_affordance(
            sync_visible && sync.status == SyncOverviewStatus::Failed,
        );
        if !sync_visible {
            self.set_overview_sync_running(false);
            return;
        }
        self.apply_overview_sync_icon(sync.status);
    }

    pub(crate) fn refresh_day_sync_badges(&self) {
        if let Some(grid) = self.imp().grids.borrow().get(2) {
            grid.refresh_sync_badges();
        }
    }

    fn apply_overview_sync_icon(&self, status: SyncOverviewStatus) {
        let imp = self.imp();
        let spinner = imp.overview_sync_spinner.get();
        let icon = imp.overview_sync_icon.get();
        let running = status == SyncOverviewStatus::Running;

        spinner.set_visible(running);
        icon.set_visible(!running);
        self.set_overview_sync_running(running);
        if !running {
            icon.set_icon_name(Some(sync_overview_icon(status)));
        }
    }

    fn setup_overview_sync_spinner(&self) {
        self.imp().overview_sync_spinner.get().set_draw_func(
            glib::clone!(@weak self as this => move |area, cr, width, height| {
                if !this.imp().overview_sync_running.get() {
                    return;
                }
                let started = this
                    .imp()
                    .overview_sync_started_at
                    .get()
                    .unwrap_or_else(std::time::Instant::now);
                let phase = (started.elapsed().as_secs_f64()
                    / OVERVIEW_SYNC_ROTATION_PERIOD.as_secs_f64())
                    .fract();
                let angle = phase * std::f64::consts::TAU;
                let radius = ((width.min(height) as f64 - 4.0) / 2.0).max(1.0);
                let color = area.style_context().color();

                cr.set_source_rgba(
                    color.red() as f64,
                    color.green() as f64,
                    color.blue() as f64,
                    color.alpha() as f64,
                );
                cr.set_line_width(2.0);
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                cr.arc(
                    width as f64 / 2.0,
                    height as f64 / 2.0,
                    radius,
                    angle,
                    angle + std::f64::consts::TAU * 0.72,
                );
                let _ = cr.stroke();
            }),
        );
    }

    fn set_overview_sync_running(&self, running: bool) {
        let imp = self.imp();
        let was_running = imp.overview_sync_running.replace(running);
        if !running {
            imp.overview_sync_started_at.set(None);
            imp.overview_sync_spinner.get().queue_draw();
            return;
        }
        if !was_running {
            imp.overview_sync_started_at
                .set(Some(std::time::Instant::now()));
        }
        if imp.overview_sync_tick_active.replace(true) {
            return;
        }

        let weak = self.downgrade();
        let _ = imp
            .overview_sync_spinner
            .get()
            .add_tick_callback(move |area, _| {
                let Some(this) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if !this.imp().overview_sync_running.get() {
                    this.imp().overview_sync_tick_active.set(false);
                    return glib::ControlFlow::Break;
                }
                area.queue_draw();
                glib::ControlFlow::Continue
            });
    }

    /// The header chevron mirrors `overview_revealer` rather than owning a second
    /// disclosure flag, so a pull, a click and the mode-switch hide all keep the
    /// glyph and the announced name truthful (P2-8).
    fn apply_overview_disclosure_state(&self) {
        let revealed = self.imp().overview_revealer.reveals_child();
        let name = tr(if revealed {
            "photos.overview.hide"
        } else {
            "photos.overview.show"
        });
        let button = self.imp().overview_toggle_btn.get();
        button.set_icon_name(if revealed {
            "pan-up-symbolic"
        } else {
            "pan-down-symbolic"
        });
        button.set_tooltip_text(Some(&name));
        // An icon-only button has no child label to read, and its state is part
        // of its meaning, so the name itself carries show/hide.
        button.update_property(&[gtk::accessible::Property::Label(&name)]);
    }

    /// A failed sync is the one overview state with an action attached (P2-8);
    /// every other state leaves the row as plain information. The pull is already
    /// coalesced by `overview_sync_pull_in_flight`, and the button reports that
    /// rather than swallowing a second click.
    fn apply_overview_retry_affordance(&self, failed: bool) {
        let in_flight = self.imp().overview_sync_pull_in_flight.get();
        self.imp().overview_sync_retry_btn.set_visible(failed);
        self.imp().overview_sync_retry_btn.set_sensitive(!in_flight);
    }

    fn apply_overview_error(&self) {
        let unavailable = tr("photos.overview.unavailable");
        self.imp()
            .overview_count_label
            .get()
            .set_label(&unavailable);
        self.imp().overview_sync_label.get().set_label(&unavailable);
        self.imp().overview_sync_row.get().set_visible(true);
        let imp = self.imp();
        self.set_overview_sync_running(false);
        imp.overview_sync_spinner.get().set_visible(false);
        imp.overview_sync_icon
            .get()
            .set_icon_name(Some("dialog-warning-symbolic"));
        imp.overview_sync_icon.get().set_visible(true);
        // The retry re-enters the same pull the header gesture uses, and that
        // pull is a no-op while sync is off — so only offer it when it can act.
        self.apply_overview_retry_affordance(crate::core::prefs::webdav_sync_enabled());
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
        // Only a real change invalidates the in-flight favorite query: a grid can
        // re-emit the same selection, and bumping then would make the answer for
        // the current set look stale forever.
        let selection_changed = *self.imp().selected_ids.borrow() != union;
        if selection_changed {
            self.imp()
                .selection_generation
                .set(self.imp().selection_generation.get() + 1);
        }
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

        // Name the object the batch buttons act on. The header buttons say
        // *what* will happen; nothing on screen said *to how many* until now.
        let count = self.imp().selected_ids.borrow().len();
        let mut count_text = trf("photos.selection.count", &[("n", &count.to_string())]);
        if select_all_limit_reached {
            count_text.push_str(&tr("photos.selection.limit"));
        }
        self.imp()
            .selection_count_label
            .get()
            .set_label(&count_text);
        self.imp()
            .selection_count_revealer
            .get()
            .set_reveal_child(has_any);

        // Smart favorite toggle. The heart button shows whenever there is a
        // selection. It turns red (favorite-active — the same class/effect as
        // the viewer's favorited heart) when every selected photo is already
        // favorited; clicking then unfavorites all. If none are favorited,
        // clicking favorites all. A mixed selection leaves the heart plain and
        // opens the popover (handled in the click handler).
        self.imp().favorite_revealer.get().set_reveal_child(has_any);
        if has_any {
            self.refresh_selection_favorite_state();
        } else {
            self.apply_selection_favorite_state(FavoriteMenuState::default());
        }
    }

    /// Ask the database what the batch heart should look like, off the main
    /// thread (P2-5). This ran synchronously on every `selection-changed` tick -
    /// once per pointer motion during a drag-select, over up to 2000 ids.
    /// At most one query is in flight; when it lands, either the selection has
    /// not moved and the answer is applied, or it has and a fresh query starts,
    /// so the settled selection always gets its own answer.
    fn refresh_selection_favorite_state(&self) {
        let ids: Vec<MediaId> = self.imp().selected_ids.borrow().iter().copied().collect();
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        if self.imp().favorite_state_in_flight.replace(true) {
            return;
        }
        let generation = self.imp().selection_generation.get();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || {
                let repository = crate::core::repository::MediaRepository::new(pool);
                repository
                    .favorite_state(&ids)
                    .map_err(|error| error.to_string())
            })
            .await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.imp().favorite_state_in_flight.set(false);
            match result {
                Ok(Ok(summary)) => {
                    if this.imp().selection_generation.get() != generation {
                        this.refresh_selection_favorite_state();
                        return;
                    }
                    this.apply_selection_favorite_state(FavoriteMenuState {
                        can_favorite: summary.has_unfavorite,
                        can_unfavorite: summary.has_favorite,
                    });
                }
                Ok(Err(error)) => {
                    tracing::warn!("failed to read the selection's favorite state: {error}")
                }
                Err(error) => {
                    tracing::warn!("favorite-state worker failed: {error:?}");
                }
            }
        });
    }

    /// Paint the favorite-dependent chrome from an answer that is current for
    /// `selected_ids`, and remember it for the tests that ask what the header
    /// believes.
    fn apply_selection_favorite_state(&self, state: FavoriteMenuState) {
        self.imp().selection_favorite_state.set(state);
        let has_any = !self.imp().selected_ids.borrow().is_empty();
        let all_favorited = has_any && !state.can_favorite && state.can_unfavorite;
        // Both verbs still apply, so the click cannot pick one: that is the case
        // where the heart opens the menu instead of acting (P2-9).
        let mixed = has_any && state.can_favorite && state.can_unfavorite;
        let key = if all_favorited {
            "photos.batch.unfavorite"
        } else if mixed {
            "photos.batch.favorite.mixed"
        } else {
            "photos.batch.favorite"
        };
        let name = tr(key);
        let fav_btn = self.imp().favorite_btn.get();
        if all_favorited {
            fav_btn.add_css_class("favorite-active");
        } else {
            fav_btn.remove_css_class("favorite-active");
        }
        fav_btn.set_tooltip_text(Some(&name));
        // The name has to carry it too: a screen-reader user never sees the caret.
        fav_btn.update_property(&[gtk::accessible::Property::Label(&name)]);
        self.imp().favorite_menu_hint.set_visible(mixed);
        // Popover items stay wired for the mixed case.
        if let Some(btn) = self.imp().favorite_item_btn.borrow().as_ref() {
            btn.set_sensitive(state.can_favorite);
        }
        if let Some(btn) = self.imp().unfavorite_item_btn.borrow().as_ref() {
            btn.set_sensitive(state.can_unfavorite);
        }
    }

    /// Carry out the heart's smart toggle. Which branch applies is a database
    /// question, and the painted state may still belong to the selection before
    /// this one, so the click asks rather than trusting the paint (P2-5). The
    /// action then runs where it always ran - asynchronously - so the extra hop
    /// is not a new wait for the user.
    fn decide_favorite_action(&self, ids: Vec<MediaId>) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let generation = self.imp().selection_generation.get();
        let query_ids = ids.clone();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || {
                let repository = crate::core::repository::MediaRepository::new(pool);
                repository
                    .favorite_state(&query_ids)
                    .map_err(|error| error.to_string())
            })
            .await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            let summary = match result {
                Ok(Ok(summary)) => summary,
                Ok(Err(error)) => {
                    tracing::warn!("failed to read the selection's favorite state: {error}");
                    return;
                }
                Err(error) => {
                    tracing::warn!("favorite-state worker failed: {error:?}");
                    return;
                }
            };
            let state = FavoriteMenuState {
                can_favorite: summary.has_unfavorite,
                can_unfavorite: summary.has_favorite,
            };
            if this.imp().selection_generation.get() == generation {
                this.apply_selection_favorite_state(state);
            }
            if state.can_favorite && state.can_unfavorite {
                if let Some(popover) = this.imp().favorite_popover.borrow().as_ref() {
                    popover.popup();
                }
            } else if state.can_favorite {
                this.set_favorite_for_ids(ids, true);
            } else if state.can_unfavorite {
                this.set_favorite_for_ids(ids, false);
            }
        });
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
        if matches!(
            action,
            KeyboardAction::ActivateFocused | KeyboardAction::ToggleSelection
        ) {
            if let Some(grid) = self.current_grid() {
                let result = grid.handle_keyboard_action(action);
                if result.is_handled() {
                    return result;
                }
            }
        }
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

    fn current_grid(&self) -> Option<VirtualMediaGrid> {
        let stack = self.imp().view_stack.get();
        let visible_name = stack.visible_child_name()?;
        self.imp()
            .grids
            .borrow()
            .iter()
            .find(|grid| group_mode_name(grid.mode()) == visible_name)
            .cloned()
    }

    fn handle_overview_scroll_intent(&self, mode: GroupBy, delta_y: f64) {
        if !delta_y.is_finite() || delta_y == 0.0 {
            return;
        }
        let Some(grid) = self.current_grid() else {
            self.imp().overview_revealer.set_reveal_child(false);
            return;
        };
        if grid.mode() != mode {
            return;
        }

        if delta_y < 0.0 && grid.is_scrolled_to_top() {
            self.imp().overview_revealer.set_reveal_child(true);
            let now = std::time::Instant::now();
            let can_trigger = self
                .imp()
                .overview_sync_last_pull_at
                .get()
                .is_none_or(|last| now.duration_since(last) >= OVERVIEW_SYNC_PULL_COOLDOWN);
            if can_trigger {
                self.imp().overview_sync_last_pull_at.set(Some(now));
                self.trigger_sync_from_home_pull();
            }
        } else if delta_y > 0.0 {
            self.imp().overview_revealer.set_reveal_child(false);
        }
    }

    fn trigger_sync_from_home_pull(&self) {
        if !crate::core::prefs::webdav_sync_enabled()
            || self.imp().overview_sync_pull_in_flight.replace(true)
        {
            return;
        }
        let (Some(pool), Some(actor)) = (
            self.imp().pool.borrow().clone(),
            self.imp().db_actor.borrow().clone(),
        ) else {
            self.imp().overview_sync_pull_in_flight.set(false);
            return;
        };
        let service = crate::core::sync::SyncService::with_actor(pool, actor);
        let weak = self.downgrade();
        let task = tokio::spawn(async move { service.trigger_saved_jobs_once().await });
        glib::spawn_future_local(async move {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!("home pull synchronization failed: {error}"),
                Err(error) => tracing::warn!("home pull synchronization task failed: {error}"),
            }
            if let Some(page) = weak.upgrade() {
                page.imp().overview_sync_pull_in_flight.set(false);
                page.refresh_overview_async();
            }
        });
    }

    fn hide_overview_after_leaving_top(&self) {
        let should_hide = self
            .current_grid()
            .is_none_or(|grid| !grid.is_scrolled_to_top());
        if should_hide {
            self.imp().overview_revealer.set_reveal_child(false);
        }
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
        let active_mode = current.as_ref().map(|grid| grid.mode());
        for grid in grids.iter() {
            let active = active_mode.is_some_and(|mode| grid.mode() == mode);
            if active {
                span.record("active_mode", group_mode_name(grid.mode()));
            }
            grid.set_active(active);
        }
        if active_mode.is_none() {
            span.record("active_mode", "none");
        }
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
        if self.imp().select_all_in_flight.replace(true) {
            return;
        }
        // P2-5: reading up to 2000 rows was a synchronous main-thread query, so
        // the header button froze for the duration of the fetch. The grids are
        // only told once the ids are back.
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || {
                let repository = crate::core::repository::MediaRepository::new(pool);
                let ids = repository
                    .items(MediaQuery::LiveAll, 0, PHOTOS_SELECT_ALL_LIMIT)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(|item| MediaId::from(item.id))
                    .collect::<Vec<_>>();
                Ok::<_, String>(ids)
            })
            .await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.imp().select_all_in_flight.set(false);
            match result {
                Ok(Ok(ids)) => {
                    for grid in this.imp().grids.borrow().iter() {
                        grid.select_ids(&ids);
                    }
                }
                Ok(Err(error)) => tracing::warn!("failed to load select-all ids: {error}"),
                Err(error) => tracing::warn!("select-all worker failed: {error:?}"),
            }
        });
    }

    fn selected_reaches_select_all_limit(&self) -> bool {
        let selected_count = self.imp().selected_ids.borrow().len();
        if selected_count == 0 {
            return false;
        }
        // P2-5: this used to COUNT the whole library on the main thread, once per
        // `selection-changed` tick. The total now rides along with the overview
        // worker that already runs off-thread; until its first snapshot lands,
        // fall back to what the loaded grid can actually answer.
        let Some(total) = self.imp().live_total.get() else {
            return self
                .current_grid()
                .is_some_and(|grid| grid.is_all_displayed_selected());
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
        let Some(grid) = self.current_grid() else {
            selector.set_light_background(false);
            selector.queue_draw();
            return;
        };
        // Unloaded cells/gutters have no reliable sample. Keep the last tint
        // instead of oscillating to dark while thumbnails stream in.
        if let Some(is_light) = grid.background_is_light_under(&selector) {
            selector.queue_light_background(is_light);
        }
        // The selector uses `backdrop-filter`, so its pixels depend on the
        // stack content behind it even when none of its own CSS classes
        // change. GTK does not always invalidate an overlay backdrop while a
        // sibling GtkStack crossfades, which can leave a stale vertical strip
        // until the grid scrolls. Explicitly redraw the bounded glass widget
        // while sampling its contrast so the backdrop follows every frame.
        selector.queue_draw();
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
        // The mode stack crossfades for 200 ms. Sixteen 16-ms refreshes cover
        // that whole transition plus a final settled frame.
        let ticks_remaining = Rc::new(Cell::new(16u8));
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

    /// Refresh the scroll-date label from the active viewport's date coverage,
    /// then reveal the fixed top-left overlay. Hides itself only while there is
    /// no resolved media range.
    fn update_scroll_date(&self) {
        let Some(grid) = self.current_grid() else {
            self.imp().scroll_date_revealer.set_reveal_child(false);
            return;
        };
        let Some((first_visible, last_visible)) = grid.visible_date_range() else {
            self.imp().scroll_date_revealer.set_reveal_child(false);
            return;
        };

        let imp = self.imp();
        imp.scroll_date_label
            .set_label(&crate::core::section_model::make_visible_range_label(
                &first_visible,
                &last_visible,
            ));
        imp.scroll_date_revealer.set_reveal_child(true);
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
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        let raw_ids: Vec<i64> = ids.into_iter().map(MediaId::get).collect();
        album_picker::AlbumPickerDialog::present(&nav, pool, db_actor, loader, raw_ids);
    }

    fn delete_to_trash_for_ids(&self, ids: Vec<MediaId>) {
        if ids.is_empty() {
            return;
        }
        let trace = crate::core::telemetry::OperationTrace::start(
            crate::core::telemetry::TraceChain::Mutation,
            "move_to_trash",
        );
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            crate::core::telemetry::log_warning(
                &trace,
                "precondition",
                "DB actor is not initialized",
            );
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            crate::core::telemetry::log_warning(
                &trace,
                "precondition",
                "database pool is not initialized",
            );
            return;
        };

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let prepared = db_actor
                .execute_in_trace(
                    trace.clone(),
                    DbCommand::MarkTrashed {
                        ids: ids_for_worker,
                    },
                )
                .await
                .inspect_err(|error| crate::core::telemetry::log_error(&trace, "db_mark", error))
                .ok();

            let Some(crate::core::DbCommandResult::MediaItems(items)) = prepared else {
                return;
            };
            if let Some(this) = weak.upgrade() {
                crate::ui::trash_fallback::move_marked_items_with_fallback(
                    &this,
                    pool,
                    db_actor,
                    items,
                    trace,
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

        for grid in self.imp().grids.borrow().iter() {
            grid.begin_favorite_item_update();
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

            match result {
                Ok(crate::core::DbCommandResult::MediaItems(items)) => {
                    if let Some(this) = weak.upgrade() {
                        let changed_ids = items
                            .iter()
                            .map(|item| MediaId::from(item.id))
                            .collect::<Vec<_>>();
                        this.update_virtual_favorite_flags(&changed_ids, is_favorite);
                        this.clear_selection();
                    }
                }
                Ok(other) => tracing::warn!(
                    target: crate::core::log_targets::BROWSING,
                    "favorite batch returned unexpected result: {other:?}"
                ),
                Err(error) => tracing::warn!(
                    target: crate::core::log_targets::BROWSING,
                    "favorite batch failed: {error}"
                ),
            }
        });
    }

    fn update_media_favorite_flags(&self, ids: &[i64], is_favorite: bool) {
        let ids: HashSet<i64> = ids.iter().copied().collect();
        if let Some(list) = self.imp().media_list.borrow().as_ref().cloned() {
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
        let ids = ids.iter().copied().map(MediaId::from).collect::<Vec<_>>();
        self.update_virtual_favorite_flags(&ids, is_favorite);
    }

    fn update_virtual_favorite_flags(&self, ids: &[MediaId], is_favorite: bool) {
        for grid in self.imp().grids.borrow().iter() {
            grid.update_favorite_flags(ids, is_favorite);
        }
    }

    /// Clear selection across all three sub-grids and hide the toolbar.
    /// Called after a successful batch operation so the user can continue
    /// browsing without the previous selection leaking in.
    pub fn clear_selection(&self) {
        let selection_chrome_visible = self.imp().select_all_revealer.get().reveals_child()
            || self.imp().add_to_album_revealer.get().reveals_child()
            || self.imp().favorite_revealer.get().reveals_child()
            || self.imp().delete_to_trash_revealer.get().reveals_child()
            || self.imp().exit_multi_select_revealer.get().reveals_child();
        // Batch action buttons live inside revealers. If the clicked button
        // remains focused while its revealer hides, GTK falls back to the
        // first GridView item. Keep focus on a visible tile before changing
        // the selection UI so the viewport never takes that detour. A
        // single-item context-menu favorite has no selection toolbar to hide,
        // so leave the right-clicked tile's focus untouched.
        if selection_chrome_visible {
            if let Some(grid) = self.current_grid() {
                grid.focus_visible_tile();
            }
        }
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

        let shared_media_list = match self.imp().media_list.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let (media_list, initial_index, source) = if let Some(index) =
            index_for_media_id(&shared_media_list, media_id)
        {
            (shared_media_list, index, "shared-window")
        } else if let Some(seed) = self
            .current_grid()
            .and_then(|grid| grid.viewer_seed_for(media_id))
        {
            // A virtual range can be far outside the bounded GTK-facing
            // startup window. The Viewer will resolve subsequent neighbours
            // through LiveAll, so one ready item is enough to open correctly.
            (seed, 0, "virtual-range")
        } else {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "PhotosPage: ignoring viewer activation for media_id={} because it is not ready in the active grid",
                media_id.get()
            );
            return;
        };
        let around = [
            initial_index.checked_sub(2),
            initial_index.checked_sub(1),
            Some(initial_index),
            initial_index.checked_add(1),
            initial_index.checked_add(2),
        ]
        .into_iter()
        .flatten()
        .filter_map(|index| {
            media_item_for_index(&media_list, index)
                .map(|item| format!("{index}:{}:{}", item.id, item.display_name()))
        })
        .collect::<Vec<_>>();
        if let Some(item) = media_item_for_index(&media_list, initial_index) {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_TRACE photos_open_viewer source={} initial_index={} list_len={} around={:?} item_id={} item_name={} item_uri={} sort_time={}",
                source,
                initial_index,
                media_list.n_items(),
                around,
                item.id,
                item.display_name(),
                item.uri,
                item.sort_datetime()
            );
        } else {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_TRACE photos_open_viewer missing_item source={} initial_index={} list_len={}",
                source,
                initial_index,
                media_list.n_items(),
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

        viewer.show_at(initial_index);

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
