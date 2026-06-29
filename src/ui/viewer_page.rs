//! ViewerPage — fullscreen image viewer with preloading and gestures.
//!
//! `ViewerPage` is pushed onto the `AdwNavigationView` when the user clicks a
//! `PhotoTile`. It decodes the **original** image (no thumbnail pipeline) for
//! the current item, plus preloads the ±1 neighbours so panning feels
//! reasonably snappy. It also wires up a `GestureZoom` and a keyboard
//! controller for basic interaction.
//!
//! Note: items in the `gio::ListStore` are `BoxedAnyObject<MediaItem>` (see
//! M1-T10 / `app::initialize`). We unwrap via `BoxedAnyObject::borrow` rather
//! than `downcast::<MediaItem>()`.
use crate::core::db::DbPool;
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::metadata::{self, ExifSummary, VideoSummary};
use crate::core::motion_photo::{self, MediaAttributes};
use crate::core::orientation;
use crate::core::prefs;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::core::{albums, trash};
use crate::ui::editor_panel::{CropOverlayUpdate, EditorPanel, SaveResultKind, ToastKind};
use crate::ui::toasts;
use chrono::{Local, Utc};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{
    ActionRowExt, AdwDialogExt, AlertDialogExt, NavigationPageExt, PreferencesGroupExt,
    PreferencesRowExt,
};
use libadwaita::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
type FavoriteStateCallback = Rc<dyn Fn(i64, bool)>;

/// On-screen thumbnail height in the viewer filmstrip. Deliberately smaller
/// than the Year view (90 px) so the strip stays unobtrusive.
const THUMB_HEIGHT: i32 = 56;
const THUMB_MIN_WIDTH: i32 = 36;
const THUMB_MIN_ASPECT: f64 = 9.0 / 21.0;
const THUMB_MAX_ASPECT: f64 = 21.0 / 9.0;

/// 初次打开 viewer 时,缩略图栏向左右各加载的条数。缩略图栏保持有限窗口,
/// 不追随超宽视口无限补齐; 当前项通过视觉位移保持居中。
/// Initial filmstrip window per side. The strip keeps a bounded window and
/// centres the current item visually instead of trying to fill ultrawide bars.
const THUMB_INITIAL_HALF: u32 = 5;
const THUMB_DEFAULT_WINDOW_LEN: u32 = 2 * THUMB_INITIAL_HALF + 1;

/// Estimated thumbnail advance (button width + spacing) used before async
/// thumbnails have reported their natural widths.
const THUMB_ESTIMATED_ADVANCE: f64 = 78.0;
const THUMB_STRIP_SPACING: f64 = 6.0;

/// 用户滚动接近边缘时,每次懒加载追加的条数 —— "半栏"。滚动条触发后向一侧
/// 补这些,避免一次性预渲染全部缩略图导致 viewer 被撑大。
/// Lazy-load chunk per scroll-edge event: "half row" extension per side.
const THUMB_LAZY_HALF: u32 = 4;

/// 缩略图栏总条目硬上限,防止大图库场景下无限扩展。
/// Hard cap on total items kept in memory.
const THUMB_WINDOW_MAX: u32 = 40;
const THUMB_CENTER_RETRY_FRAMES: u8 = 8;
const CROP_HANDLE_RADIUS: f64 = 14.0;
const CROP_MIN_SOURCE_SIZE: u32 = 24;
const MIN_VIEWER_ZOOM: f64 = 1.0;
const MAX_VIEWER_ZOOM: f64 = 8.0;
const VIEWER_ZOOM_STEP: f64 = 1.25;

/// Direction hint the host receives from keyboard input. `i32::MIN` is the
/// "pop navigation" sentinel; other values are a delta on the current index.
pub type NavDelta = i32;
pub const NAV_POP: NavDelta = i32::MIN;

/// Callback the host registers for keyboard navigation. Shared via `Rc` so
/// closures capturing owned state can be cloned into GTK signal handlers.
pub type NavCallback = Rc<dyn Fn(NavDelta)>;
type ItemCallback = Rc<dyn Fn(i64)>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropDragMode {
    Move,
    ResizeNw,
    ResizeNe,
    ResizeSw,
    ResizeSe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropDragState {
    mode: CropDragMode,
    rect: (u32, u32, u32, u32),
}

/// Convert a `file://` URI stored on `MediaItem::uri` to a `PathBuf` for
/// the gdk-pixbuf loader. Anything without the `file://` prefix is treated
/// as a raw path (defensive — the scanner only emits `file://` URIs).
fn strip_file_uri(uri: &str) -> PathBuf {
    let stripped = uri.strip_prefix("file://").unwrap_or(uri);
    PathBuf::from(stripped)
}

fn should_retry_thumb_centering(applied: bool, attempts_remaining: u8) -> bool {
    !applied && attempts_remaining > 0
}

fn find_media_index_by_id(list: &gio::ListStore, item_id: i64) -> Option<u32> {
    for idx in 0..list.n_items() {
        let Some(obj) = list.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<MediaItem>().id == item_id {
            return Some(idx);
        }
    }
    None
}

fn next_index_after_deleted_item(deleted_index: u32, remaining_len: u32) -> Option<u32> {
    if remaining_len == 0 {
        None
    } else {
        Some(deleted_index.min(remaining_len - 1))
    }
}

fn apply_video_audio_preferences_to_stream(
    stream: &impl IsA<gtk::MediaStream>,
    muted: bool,
    volume: f64,
) {
    stream.set_muted(muted);
    stream.set_volume(volume.clamp(0.0, 1.0));
}

fn persist_video_volume_from_stream(stream: &impl IsA<gtk::MediaStream>) {
    if stream.is_muted() {
        return;
    }
    if let Err(err) = prefs::set_video_volume(stream.volume()) {
        tracing::warn!("ViewerPage: failed to persist video volume: {err}");
    }
}

fn motion_video_cache_path(item: &MediaItem) -> PathBuf {
    // Key on the db id only: it's already unique per motion photo, and
    // blake3_hash is no longer computed at scan time.
    std::env::temp_dir().join(format!("photo-viewer-motion-{}.mp4", item.id))
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/viewer-page.ui")]
    pub struct ViewerPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub current_index: Cell<u32>,
        pub current_media_id: Cell<i64>,
        pub media_query: RefCell<Option<MediaQuery>>,
        /// Per-`show_at` token: any older response is dropped on arrival.
        pub current_token: Cell<u64>,
        /// Cumulative zoom scale (1.0 = identity). GestureZoom multiplies into it.
        pub zoom_scale: Cell<f64>,
        /// Viewer image pan offset in allocated widget pixels.
        pub zoom_pan_x: Cell<f64>,
        pub zoom_pan_y: Cell<f64>,
        /// Pan offset captured at the start of the current drag gesture.
        pub zoom_drag_origin_x: Cell<f64>,
        pub zoom_drag_origin_y: Cell<f64>,
        /// Zoom captured at pinch begin; GestureZoom scale values are relative
        /// to that begin point, not incremental deltas.
        pub zoom_gesture_origin_scale: Cell<f64>,
        /// Callback registered by the host (PhotosPage) for keyboard navigation.
        pub nav_cb: RefCell<Option<NavCallback>>,
        /// Callback fired after this viewer successfully moves an item to trash.
        pub trashed_cb: RefCell<Option<ItemCallback>>,
        /// Cached CssProvider reused across gesture ticks. Without this
        /// we would allocate a new provider on every pinch-tick and
        /// never release the previous one.
        pub zoom_provider: RefCell<Option<gtk::CssProvider>>,
        /// Optional callback invoked whenever current media favorite state changes.
        pub favorite_state_cb: RefCell<Option<FavoriteStateCallback>>,
        /// DB pool injected by host (needed to construct the editor panel).
        pub pool: RefCell<Option<DbPool>>,
        /// Navigation view (kept for album picker push; editor no longer pushes).
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        /// Original texture saved before editing starts; restored on cancel.
        pub original_texture: RefCell<Option<gdk::Texture>>,
        /// True while the editor side-panel is open (prevents nav gestures).
        pub is_editing: Cell<bool>,
        /// Dynamic camera-parameter rows appended to `file_group`.
        pub camera_rows: RefCell<Vec<adw::ActionRow>>,
        /// Dynamic video-info rows appended to `file_group` (duration/codec/…).
        pub video_rows: RefCell<Vec<adw::ActionRow>>,
        /// 当前图片收藏状态（用于按钮即时渲染）。
        pub is_favorite: Cell<bool>,
        /// Thumbnail loader shared with grids — used for the filmstrip.
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        /// Inclusive start index of the current filmstrip window.
        /// 当前已加载的缩略图窗口左端(含)。
        pub thumb_window_start: Cell<u32>,
        /// Exclusive end index of the current filmstrip window.
        /// 当前已加载的缩略图窗口右端(不含)。
        pub thumb_window_end: Cell<u32>,
        /// Buttons currently in the filmstrip (in index order). Stored so
        /// highlight can be toggled without rebuilding the strip.
        pub thumb_items: RefCell<Vec<gtk::Button>>,
        /// 已排队但尚未执行的懒加载方向。滚动条触发后置位,扩展完成后清空,
        /// 防止 value-changed 在一次扩展未完成时反复触发导致重复构建。
        /// Pending lazy-extend direction (-1 left, +1 right, None idle).
        pub thumb_pending_extend: Cell<Option<i8>>,
        /// True while `ViewerPage` is setting the filmstrip adjustment itself.
        /// The adjustment emits `value-changed` synchronously, so the lazy-load
        /// edge listener must ignore these programmatic moves.
        pub thumb_programmatic_scroll: Cell<bool>,
        /// Cached CssProvider for filmstrip visual positioning. It keeps the
        /// current item centred without feeding child width into GTK layout.
        pub thumb_transform_provider: RefCell<Option<gtk::CssProvider>>,
        pub crop_overlay_active: Cell<bool>,
        pub crop_overlay_selected: Cell<bool>,
        pub crop_overlay_rect: Cell<Option<(u32, u32, u32, u32)>>,
        pub crop_overlay_dimensions: Cell<(u32, u32)>,
        pub crop_drag: RefCell<Option<CropDragState>>,
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub details_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub details_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub delete_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub details_close_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub details_split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub editor_split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub editor_panel: TemplateChild<EditorPanel>,
        #[template_child]
        pub edit_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub favorite_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub video: TemplateChild<gtk::Video>,
        #[template_child]
        pub motion_play_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub crop_overlay: TemplateChild<gtk::DrawingArea>,
        #[template_child]
        pub image_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub name_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub mime_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub dimensions_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub size_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub taken_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub thumb_scrolled: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub thumb_strip: TemplateChild<gtk::Box>,
        #[template_child]
        pub prev_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub next_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub zoom_out_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub zoom_reset_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub zoom_in_btn: TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ViewerPage {
        const NAME: &'static str = "ViewerPage";
        type Type = super::ViewerPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            EditorPanel::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ViewerPage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_details_sidebar_child_visible(false);
            obj.set_editor_sidebar_child_visible(false);
        }
    }
    impl WidgetImpl for ViewerPage {}
    impl NavigationPageImpl for ViewerPage {}
}

glib::wrapper! {
    pub struct ViewerPage(ObjectSubclass<imp::ViewerPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl ViewerPage {
    /// Build a new ViewerPage. Call `show_at(index)` after construction
    /// to actually paint something.
    pub fn new(media_list: gtk::gio::ListStore, index: u32) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&tr("page.viewer.title"));
        *obj.imp().media_list.borrow_mut() = Some(media_list);
        obj.imp().current_index.set(index);
        if let Some(item) = crate::ui::media_list::media_item_at(
            obj.imp()
                .media_list
                .borrow()
                .as_ref()
                .expect("media_list just set"),
            index,
        ) {
            obj.imp().current_media_id.set(item.id);
        }
        obj.imp().zoom_scale.set(MIN_VIEWER_ZOOM);
        obj.imp().zoom_gesture_origin_scale.set(MIN_VIEWER_ZOOM);
        obj.apply_i18n();
        obj.setup_zoom_controls();
        obj.setup_gesture();
        obj.setup_keyboard();
        obj.setup_edit_button();
        obj.setup_editor_callbacks();
        obj.setup_crop_overlay();
        obj.setup_delete_button();
        obj.setup_details_panel();
        obj.setup_favorite_button();
        obj.setup_nav_buttons();
        obj.setup_motion_play_button();
        obj.setup_thumb_strip_listener();
        obj.setup_navigation_pop_action();
        obj.setup_lifecycle_logging();
        obj
    }

    pub fn new_for_query(
        query: MediaQuery,
        current_id: MediaId,
        initial_items: gtk::gio::ListStore,
    ) -> Self {
        let index = index_for_media_id(&initial_items, current_id).unwrap_or(0);
        let obj = Self::new(initial_items, index);
        obj.imp().current_media_id.set(current_id.get());
        *obj.imp().media_query.borrow_mut() = Some(query);
        obj
    }

    fn apply_i18n(&self) {
        let imp = self.imp();
        imp.details_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.image_details")));
        imp.delete_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.move_to_trash")));
        imp.edit_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.edit")));
        imp.details_close_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.details.close")));
        imp.details_title
            .get()
            .set_label(&tr("viewer.details.title"));
        imp.zoom_in_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.zoom_in")));
        imp.zoom_out_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.zoom_out")));
        imp.zoom_reset_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.zoom_reset")));
        imp.motion_play_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.play_motion_photo")));
    }

    /// Inject the `AdwNavigationView` and DB pool used to push an
    /// the editor panel when the Edit button is pressed. Call this after
    /// construction (mirrors `PhotosPage::set_nav_target`).
    pub fn set_edit_target(&self, nav: &adw::NavigationView, pool: DbPool) {
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG set_edit_target index={} nav_visible={:?}",
            self.imp().current_index.get(),
            nav.visible_page().map(|page| page.title())
        );
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    /// Register a callback fired when the user presses ArrowLeft / ArrowRight /
    /// Escape. The callback receives the requested action: -1 / +1 / pop.
    pub fn connect_navigation<F: Fn(NavDelta) + 'static>(&self, f: F) {
        *self.imp().nav_cb.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_item_trashed<F: Fn(i64) + 'static>(&self, f: F) {
        *self.imp().trashed_cb.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_favorite_state_changed<F: Fn(i64, bool) + 'static>(&self, f: F) {
        *self.imp().favorite_state_cb.borrow_mut() = Some(Rc::new(f));
    }

    /// Inject the shared thumbnail loader. Must be called before `show_at`
    /// so the filmstrip can request thumbnails.
    pub fn set_thumbnail_loader(&self, loader: Arc<ThumbnailLoader>) {
        *self.imp().loader.borrow_mut() = Some(loader);
    }

    fn fire_nav(&self, delta: NavDelta) {
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG fire_nav delta={} index={} details_revealed={}",
            delta,
            self.imp().current_index.get(),
            self.imp().details_split_view.get().shows_sidebar()
        );
        let cb = self.imp().nav_cb.borrow().clone();
        if let Some(cb) = cb {
            cb(delta);
        }
    }

    fn navigate_by_delta(&self, delta: NavDelta) {
        if delta == NAV_POP {
            self.fire_nav(delta);
            return;
        }

        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            self.fire_nav(delta);
            return;
        };
        let Some(query) = self.imp().media_query.borrow().clone() else {
            self.fire_nav(delta);
            return;
        };
        let current_id = self.imp().current_media_id.get();
        if current_id == 0 {
            self.fire_nav(delta);
            return;
        }

        let weak = self.downgrade();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let repo = MediaRepository::new(pool);
            let result = repo.neighbor(query, MediaId::from(current_id), delta);
            let _ = tx.send(result);
        });
        glib::spawn_future_local(async move {
            let Ok(result) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(Some(neighbor)) => {
                    let index = this.ensure_media_item_in_window(neighbor.item);
                    this.show_at(index);
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!("ViewerPage: repository navigation failed: {err}");
                    this.fire_nav(delta);
                }
            }
        });
    }

    fn ensure_media_item_in_window(&self, item: MediaItem) -> u32 {
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return 0;
        };
        if let Some(index) = find_media_index_by_id(&list, item.id) {
            return index;
        }
        let index = list.n_items();
        list.append(&glib::BoxedAnyObject::new(item));
        index
    }

    /// Wire the Edit button: configure the embedded `EditorPanel` for the
    /// current item and reveal it as a right-side overlay (same pattern as
    /// the details panel), instead of pushing a separate `NavigationPage`.
    fn setup_edit_button(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.edit_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let pool = match this.imp().pool.borrow().as_ref() {
                Some(p) => p.clone(),
                None => {
                    tracing::warn!("ViewerPage: Edit pressed but pool not set");
                    return;
                }
            };
            let item = match this.current_media_item() {
                Some(i) => i,
                None => return,
            };
            if item.is_video() {
                return;
            }

            // Close details panel if open — only one side panel at a time.
            if this.imp().details_split_view.get().shows_sidebar() {
                this.set_details_revealed(false, "edit_start");
            }

            // Save the original texture so we can restore on cancel.
            *this.imp().original_texture.borrow_mut() = this
                .imp()
                .picture
                .get()
                .paintable()
                .and_then(|p| p.downcast::<gdk::Texture>().ok());

            // Configure and reveal the editor panel.
            this.imp().editor_panel.get().configure(item, pool);
            this.start_editing();
        });
    }

    /// Reveal the editor side-panel and lock navigation gestures.
    fn start_editing(&self) {
        self.reset_viewer_zoom();
        self.imp().is_editing.set(true);
        self.set_overlay_navigation_visible(false);
        self.set_zoom_controls_visible(false);
        self.imp().motion_play_btn.get().set_visible(false);
        self.set_editor_sidebar_child_visible(true);
        self.imp().editor_split_view.get().set_show_sidebar(true);
        self.set_can_pop(false);
    }

    /// Hide the editor side-panel, restore the original image, and
    /// re-enable navigation gestures.
    fn stop_editing(&self) {
        let imp = self.imp();
        imp.is_editing.set(false);
        self.set_overlay_navigation_visible(true);
        self.set_zoom_controls_visible(imp.picture.get().is_visible());
        if let Some(item) = self.current_media_item() {
            self.set_motion_play_button_for_item(&item);
        }
        imp.editor_split_view.get().set_show_sidebar(false);
        self.set_crop_overlay(CropOverlayUpdate {
            active: false,
            rect: None,
            image_dimensions: (0, 0),
        });

        // Restore the original texture (cancel case).
        if let Some(tex) = imp.original_texture.borrow().clone() {
            imp.picture.get().set_paintable(Some(&tex));
        }
        *imp.original_texture.borrow_mut() = None;

        // Re-enable pop after the slide-out animation.
        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            if let Some(this) = weak.upgrade() {
                if !this.imp().is_editing.get()
                    && !this.imp().editor_split_view.get().shows_sidebar()
                {
                    this.set_editor_sidebar_child_visible(false);
                    this.set_can_pop(true);
                }
            }
        });
    }

    /// Connect EditorPanel callbacks to ViewerPage state (picture, spinner,
    /// toast overlay). Called once during construction.
    fn setup_editor_callbacks(&self) {
        let panel = self.imp().editor_panel.get();

        // Preview texture → update the viewer's picture.
        let weak = self.downgrade();
        panel.connect_texture_ready(move |texture| {
            if let Some(this) = weak.upgrade() {
                this.imp().picture.get().set_paintable(Some(&texture));
                this.imp().crop_overlay.get().queue_draw();
            }
        });

        // Spinner visibility.
        let weak = self.downgrade();
        panel.connect_spinner(move |visible| {
            if let Some(this) = weak.upgrade() {
                this.imp().spinner.get().set_visible(visible);
            }
        });

        // Close (cancel or save-complete) → hide panel.
        let weak = self.downgrade();
        panel.connect_close(move || {
            if let Some(this) = weak.upgrade() {
                this.stop_editing();
            }
        });

        let weak = self.downgrade();
        panel.connect_save_result(move |kind, heading, body| {
            if let Some(this) = weak.upgrade() {
                if save_result_closes_editor(kind) {
                    this.stop_editing();
                }
                this.present_save_result_dialog(&heading, &body);
            }
        });

        // Toast messages.
        let weak = self.downgrade();
        panel.connect_toast(move |msg, kind| {
            if let Some(this) = weak.upgrade() {
                match kind {
                    ToastKind::Success => toasts::success(&this.imp().toast_overlay.get(), msg),
                    ToastKind::Error => toasts::error(&this.imp().toast_overlay.get(), msg),
                }
            }
        });

        let weak = self.downgrade();
        panel.connect_crop_overlay(move |update| {
            if let Some(this) = weak.upgrade() {
                this.set_crop_overlay(update);
            }
        });
    }

    fn present_save_result_dialog(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("ok", &tr("button.ok"));
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("ok");
        dialog.present(self);
    }

    fn set_crop_overlay(&self, update: CropOverlayUpdate) {
        let imp = self.imp();
        imp.crop_overlay_active.set(update.active);
        if !update.active {
            imp.crop_overlay_selected.set(false);
        }
        imp.crop_overlay_rect.set(update.rect);
        imp.crop_overlay_dimensions.set(update.image_dimensions);
        if !update.active {
            imp.crop_drag.borrow_mut().take();
        }
        imp.crop_overlay.get().set_visible(update.active);
        imp.crop_overlay.get().queue_draw();
    }

    fn setup_crop_overlay(&self) {
        let overlay = self.imp().crop_overlay.get();
        overlay.set_draw_func(
            glib::clone!(@weak self as this => move |area, cr, width, height| {
                this.draw_crop_overlay(area, cr, width, height);
            }),
        );

        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(glib::clone!(@weak self as this => move |_, x, y| {
            this.begin_crop_drag(x, y);
        }));
        drag.connect_drag_update(glib::clone!(@weak self as this => move |_, dx, dy| {
            this.update_crop_drag(dx, dy);
        }));
        drag.connect_drag_end(glib::clone!(@weak self as this => move |_, _, _| {
            this.imp().crop_drag.borrow_mut().take();
            this.imp().crop_overlay_selected.set(false);
            this.imp().crop_overlay.get().queue_draw();
        }));
        overlay.add_controller(drag);
    }

    fn draw_crop_overlay(
        &self,
        _area: &gtk::DrawingArea,
        cr: &gtk::cairo::Context,
        width: i32,
        height: i32,
    ) {
        let imp = self.imp();
        if !imp.crop_overlay_active.get() {
            return;
        }
        let Some(rect) = imp.crop_overlay_rect.get() else {
            return;
        };
        let image_dimensions = imp.crop_overlay_dimensions.get();
        let Some(image_rect) =
            compute_contained_image_rect(width as f64, height as f64, image_dimensions)
        else {
            return;
        };
        let Some(widget_rect) = crop_rect_to_widget(rect, image_dimensions, image_rect) else {
            return;
        };

        cr.set_source_rgba(0.0, 0.0, 0.0, 0.42);
        cr.rectangle(0.0, 0.0, width as f64, height as f64);
        cr.rectangle(
            widget_rect.x,
            widget_rect.y,
            widget_rect.width,
            widget_rect.height,
        );
        cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
        let _ = cr.fill();
        cr.set_fill_rule(gtk::cairo::FillRule::Winding);

        let selected = imp.crop_overlay_selected.get();
        if selected {
            cr.set_source_rgba(0.38, 0.72, 1.0, 0.98);
            cr.set_line_width(3.0);
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.92);
            cr.set_line_width(2.0);
        }
        cr.rectangle(
            widget_rect.x,
            widget_rect.y,
            widget_rect.width,
            widget_rect.height,
        );
        let _ = cr.stroke();

        for (x, y) in crop_handle_points(widget_rect) {
            let radius = if selected { 7.0 } else { 5.0 };
            cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }

    fn begin_crop_drag(&self, x: f64, y: f64) {
        let imp = self.imp();
        if !imp.crop_overlay_active.get() {
            return;
        }
        let Some(rect) = imp.crop_overlay_rect.get() else {
            return;
        };
        let image_dimensions = imp.crop_overlay_dimensions.get();
        let overlay = imp.crop_overlay.get();
        let Some(image_rect) = compute_contained_image_rect(
            overlay.allocated_width() as f64,
            overlay.allocated_height() as f64,
            image_dimensions,
        ) else {
            return;
        };
        let Some(widget_rect) = crop_rect_to_widget(rect, image_dimensions, image_rect) else {
            return;
        };
        let Some(mode) = hit_crop_drag_mode(x, y, widget_rect) else {
            imp.crop_overlay_selected.set(false);
            imp.crop_overlay.get().queue_draw();
            return;
        };
        imp.crop_overlay_selected.set(true);
        imp.crop_overlay.get().queue_draw();
        *imp.crop_drag.borrow_mut() = Some(CropDragState { mode, rect });
    }

    fn update_crop_drag(&self, dx: f64, dy: f64) {
        let Some(drag) = *self.imp().crop_drag.borrow() else {
            return;
        };
        let image_dimensions = self.imp().crop_overlay_dimensions.get();
        let overlay = self.imp().crop_overlay.get();
        let Some(image_rect) = compute_contained_image_rect(
            overlay.allocated_width() as f64,
            overlay.allocated_height() as f64,
            image_dimensions,
        ) else {
            return;
        };
        let sx = dx / image_rect.width * image_dimensions.0 as f64;
        let sy = dy / image_rect.height * image_dimensions.1 as f64;
        let rect = drag_rect(drag, sx, sy, image_dimensions);
        self.imp().crop_overlay_rect.set(Some(rect));
        self.imp().crop_overlay.get().queue_draw();
        self.imp()
            .editor_panel
            .get()
            .set_crop_rect_from_overlay(rect);
    }

    fn setup_delete_button(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.delete_btn.get().connect_clicked(move |_button| {
            let Some(this) = weak.upgrade() else { return };

            let dialog = adw::AlertDialog::builder()
                .heading(tr("trash.confirm_title"))
                .body(tr("trash.confirm_body_one"))
                .build();
            dialog.add_css_class("glass-alert-dialog");
            dialog.add_response("cancel", &tr("dialog.cancel"));
            dialog.add_response("trash", &tr("dialog.trash"));
            dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let weak2 = this.downgrade();
            dialog.connect_response(None, move |_, response| {
                if response != "trash" {
                    return;
                }
                let Some(this) = weak2.upgrade() else { return };
                let pool = match this.imp().pool.borrow().as_ref() {
                    Some(p) => p.clone(),
                    None => {
                        tracing::warn!("ViewerPage: Delete pressed but pool not set");
                        return;
                    }
                };
                let item = match this.current_media_item() {
                    Some(i) => i,
                    None => return,
                };

                let item_id = item.id;
                let item_uri = item.uri.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                gio::spawn_blocking(move || {
                    // 先标记后移动（见 trash::move_to_trash_marked）：否则文件监听
                    // 器会在 mark_trashed 提交前按 Remove 事件把行删掉。
                    let result = trash::move_to_trash_marked(&pool, item_id, &item_uri)
                        .and_then(|_| albums::refresh(&pool));
                    let _ = tx.send(result);
                });

                let weak_after = this.downgrade();
                glib::spawn_future_local(async move {
                    let result = rx.await;
                    match result {
                        Ok(Ok(())) => {
                            if let Some(this) = weak_after.upgrade() {
                                toasts::success(
                                    &this.imp().toast_overlay.get(),
                                    &tr("viewer.toast.moved_to_trash"),
                                );
                                this.remove_deleted_item(item_id);
                                if let Some(cb) = this.imp().trashed_cb.borrow().clone() {
                                    cb(item_id);
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("ViewerPage: Move to Trash failed: {e}");
                            if let Some(this) = weak_after.upgrade() {
                                toasts::error(
                                    &this.imp().toast_overlay.get(),
                                    &format!("{}: {e}", &tr("viewer.toast.move_to_trash_failed")),
                                );
                            }
                        }
                        Err(_) => {
                            tracing::warn!("ViewerPage: Move to Trash worker dropped");
                            if let Some(this) = weak_after.upgrade() {
                                toasts::error(
                                    &this.imp().toast_overlay.get(),
                                    &tr("viewer.toast.move_to_trash_failed"),
                                );
                            }
                        }
                    }
                });
            });
            dialog.present(&this);
        });
    }

    fn setup_favorite_button(&self) {
        // The favorite-active visual lives in the global CSS provider; if
        // install() was missed the button will silently look wrong. Assert at
        // construction time so the regression surfaces as a panic in tests.
        crate::ui::grid_css::assert_installed();

        let imp = self.imp();
        imp.favorite_btn.get().add_css_class("viewer-favorite-btn");
        imp.favorite_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.favorite")));
        self.refresh_favorite_button(false);

        let weak = self.downgrade();
        imp.favorite_btn.get().connect_clicked(move |button| {
            let Some(this) = weak.upgrade() else { return };
            let pool = match this.imp().pool.borrow().as_ref() {
                Some(p) => p.clone(),
                None => {
                    tracing::warn!("ViewerPage: Favorite pressed but pool not set");
                    return;
                }
            };
            let item_id = match this.current_media_item() {
                Some(i) => i.id,
                None => return,
            };

            let next_state = !this.imp().is_favorite.get();
            button.set_sensitive(false);
            let button_weak = button.downgrade();
            let (tx, rx) = tokio::sync::oneshot::channel();
            let token = this.imp().current_token.get();
            gio::spawn_blocking(move || {
                let result = MediaRepository::new(pool)
                    .set_favorite(&[MediaId::from(item_id)], next_state)
                    .map(|_| ());
                let _ = tx.send((result, next_state, token));
            });

            let weak_after = this.downgrade();
            glib::spawn_future_local(async move {
                let result = rx.await;
                if let Some(button) = button_weak.upgrade() {
                    button.set_sensitive(true);
                }
                let Ok((db_result, target_state, token_expected)) = result else {
                    return;
                };
                if let Some(this) = weak_after.upgrade() {
                    if this.imp().current_token.get() != token_expected {
                        return;
                    }
                    match db_result {
                        Ok(()) => {
                            this.refresh_favorite_button(target_state);
                            if let Some(cb) = this.imp().favorite_state_cb.borrow().clone() {
                                cb(item_id, target_state);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("ViewerPage: Toggle favorite failed: {e}");
                            toasts::error(
                                &this.imp().toast_overlay.get(),
                                &format!("{}: {e}", &tr("viewer.toast.favorite_update_failed")),
                            );
                        }
                    }
                }
            });
        });
    }

    fn remove_deleted_item(&self, item_id: i64) {
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            self.fire_nav(NAV_POP);
            return;
        };
        let deleted_index = find_media_index_by_id(&list, item_id).unwrap_or_else(|| {
            self.imp()
                .current_index
                .get()
                .min(list.n_items().saturating_sub(1))
        });
        if deleted_index < list.n_items() {
            list.remove(deleted_index);
        }

        match next_index_after_deleted_item(deleted_index, list.n_items()) {
            Some(next) => self.show_at(next),
            None => self.fire_nav(NAV_POP),
        }
    }

    fn refresh_favorite_button(&self, is_favorite: bool) {
        self.imp().is_favorite.set(is_favorite);
        let button = self.imp().favorite_btn.get();
        // The button always shows the heart icon (emblem-favorite-symbolic,
        // set in the template — same glyph as the Favorites album). Favoriting
        // only flips the .favorite-active class so the global CSS recolors the
        // heart translucent red; there is no label/icon swap and no button
        // capsule.
        if is_favorite {
            button.add_css_class("favorite-active");
            button.set_tooltip_text(Some(&tr("viewer.button.favorite_active")));
        } else {
            button.remove_css_class("favorite-active");
            button.set_tooltip_text(Some(&tr("viewer.button.favorite")));
        }
    }

    /// Wire the `<` / `>` viewer navigation buttons.
    fn setup_nav_buttons(&self) {
        let imp = self.imp();
        imp.prev_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.previous")));
        imp.next_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.next")));

        let weak = self.downgrade();
        imp.prev_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.navigate_by_delta(-1);
            }
        });
        let weak = self.downgrade();
        imp.next_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.navigate_by_delta(1);
            }
        });
    }

    fn setup_motion_play_button(&self) {
        let weak = self.downgrade();
        self.imp().motion_play_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.play_current_motion_photo();
            }
        });
    }

    fn set_motion_play_button_for_item(&self, item: &MediaItem) {
        self.imp().motion_play_btn.get().set_visible(
            item.is_motion_photo() && !item.is_video() && !self.imp().is_editing.get(),
        );
    }

    fn restore_image_after_motion_video(&self, token: u64) {
        if self.imp().current_token.get() != token {
            return;
        }
        self.stop_video_playback();
        self.imp().video.get().set_visible(false);
        self.imp().picture.get().set_visible(true);
        self.imp().edit_btn.get().set_sensitive(true);
        self.set_zoom_controls_visible(!self.imp().is_editing.get());
        if let Some(item) = self.current_media_item() {
            self.set_motion_play_button_for_item(&item);
        }
    }

    fn play_current_motion_photo(&self) {
        let Some(item) = self.current_media_item() else {
            return;
        };
        let attrs = MediaAttributes::from_json(&item.media_attributes);
        let Some(info) = attrs.motion_photo else {
            self.imp().motion_play_btn.get().set_visible(false);
            return;
        };

        let token = self.imp().current_token.get();
        let source = item.path.clone();
        let dest = motion_video_cache_path(&item);
        self.imp().spinner.get().set_visible(true);
        self.imp().motion_play_btn.get().set_visible(false);

        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let result = motion_photo::extract_video_to(&source, &info, &dest)
                .map(|()| dest)
                .map_err(|err| err.to_string());
            let _ = tx.send(result);
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let video_path = match rx.await {
                Ok(Ok(path)) => path,
                Ok(Err(err)) => {
                    tracing::warn!("ViewerPage: failed to extract motion photo video: {err}");
                    if let Some(this) = weak.upgrade() {
                        this.imp().spinner.get().set_visible(false);
                        if let Some(item) = this.current_media_item() {
                            this.set_motion_play_button_for_item(&item);
                        }
                    }
                    return;
                }
                Err(_) => return,
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            this.show_motion_video_stage(video_path, token);
        });
    }

    fn setup_zoom_controls(&self) {
        let imp = self.imp();

        let weak = self.downgrade();
        imp.zoom_in_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.step_viewer_zoom(1);
            }
        });

        let weak = self.downgrade();
        imp.zoom_out_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.step_viewer_zoom(-1);
            }
        });

        let weak = self.downgrade();
        imp.zoom_reset_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.reset_viewer_zoom();
            }
        });

        self.update_zoom_buttons();
    }

    fn step_viewer_zoom(&self, direction: i32) {
        let next = step_zoom(self.imp().zoom_scale.get(), direction);
        self.set_viewer_zoom(
            next,
            self.imp().zoom_pan_x.get(),
            self.imp().zoom_pan_y.get(),
        );
    }

    fn reset_viewer_zoom(&self) {
        self.set_viewer_zoom(MIN_VIEWER_ZOOM, 0.0, 0.0);
    }

    fn set_viewer_zoom(&self, scale: f64, pan_x: f64, pan_y: f64) {
        let picture = self.imp().picture.get();
        let scale = scale.clamp(MIN_VIEWER_ZOOM, MAX_VIEWER_ZOOM);
        let (pan_x, pan_y) = clamp_zoom_pan(
            scale,
            pan_x,
            pan_y,
            picture.allocated_width() as f64,
            picture.allocated_height() as f64,
        );
        self.imp().zoom_scale.set(scale);
        self.imp().zoom_pan_x.set(pan_x);
        self.imp().zoom_pan_y.set(pan_y);
        self.update_zoom_transform();
        self.update_zoom_buttons();
    }

    #[cfg(test)]
    fn set_viewer_zoom_for_tests(&self, scale: f64, pan_x: f64, pan_y: f64) {
        self.set_viewer_zoom(scale, pan_x, pan_y);
    }

    fn update_zoom_transform(&self) {
        let imp = self.imp();
        let scale = imp.zoom_scale.get();
        let pan_x = imp.zoom_pan_x.get();
        let pan_y = imp.zoom_pan_y.get();
        if let Some(provider) = imp.zoom_provider.borrow().as_ref() {
            provider.load_from_data(&format!(
                "picture.viewer-image-frame {{ transform: translate({pan_x}px, {pan_y}px) scale({scale}); }}"
            ));
        }
        imp.picture.get().queue_draw();
    }

    fn set_zoom_controls_visible(&self, visible: bool) {
        if let Some(parent) = self.imp().zoom_in_btn.get().parent() {
            parent.set_visible(visible);
        }
        self.update_zoom_buttons();
    }

    fn update_zoom_buttons(&self) {
        let imp = self.imp();
        let zoomed = imp.zoom_scale.get() > MIN_VIEWER_ZOOM;
        imp.zoom_in_btn.get().set_visible(true);
        imp.zoom_out_btn.get().set_visible(zoomed);
        imp.zoom_reset_btn.get().set_visible(zoomed);
    }

    fn stop_video_playback(&self) {
        // Pause the in-flight stream so audio/playback does not continue behind
        // an image, then detach it. The GtkVideo keeps its own built-in
        // play/pause + progress controls, so there is no separate slider to reset.
        if let Some(stream) = self.imp().video.get().media_stream() {
            stream.pause();
        }
        self.imp()
            .video
            .get()
            .set_media_stream(gtk::MediaStream::NONE);
    }

    fn show_image_stage(&self) {
        self.stop_video_playback();
        self.imp().video.get().set_visible(false);
        self.imp().picture.get().set_visible(true);
        self.imp().edit_btn.get().set_sensitive(true);
        self.set_zoom_controls_visible(!self.imp().is_editing.get());
    }

    fn show_video_stage(&self, item: &MediaItem) {
        self.stop_video_playback();
        self.reset_viewer_zoom();
        self.imp().motion_play_btn.get().set_visible(false);
        self.imp()
            .picture
            .get()
            .set_paintable(None::<&gdk::Paintable>);
        self.imp().picture.get().set_visible(false);
        self.imp().video.get().set_visible(true);
        self.set_zoom_controls_visible(false);
        self.imp().spinner.get().set_visible(false);
        self.imp().edit_btn.get().set_sensitive(false);
        self.set_crop_overlay(CropOverlayUpdate {
            active: false,
            rect: None,
            image_dimensions: (0, 0),
        });

        let stream = gtk::MediaFile::for_filename(&item.path);
        stream.set_loop(false);
        let default_muted = prefs::video_default_muted();
        let persisted_volume = prefs::video_volume();
        self.imp().video.get().set_media_stream(Some(&stream));
        apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
        stream.connect_volume_notify(persist_video_volume_from_stream);
        stream.set_playing(true);
        let stream_weak = stream.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(stream) = stream_weak.upgrade() {
                apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
            }
        });
    }

    fn show_motion_video_stage(&self, video_path: PathBuf, token: u64) {
        self.stop_video_playback();
        self.reset_viewer_zoom();
        self.imp().picture.get().set_visible(false);
        self.imp().video.get().set_visible(true);
        self.imp().motion_play_btn.get().set_visible(false);
        self.set_zoom_controls_visible(false);
        self.imp().spinner.get().set_visible(false);
        self.imp().edit_btn.get().set_sensitive(false);

        let stream = gtk::MediaFile::for_filename(&video_path);
        stream.set_loop(false);
        let default_muted = prefs::video_default_muted();
        let persisted_volume = prefs::video_volume();
        self.imp().video.get().set_media_stream(Some(&stream));
        apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
        stream.connect_volume_notify(persist_video_volume_from_stream);

        let weak = self.downgrade();
        stream.connect_ended_notify(move |stream| {
            if !stream.is_ended() {
                return;
            }
            if let Some(this) = weak.upgrade() {
                this.restore_image_after_motion_video(token);
            }
        });

        stream.set_playing(true);
        let stream_weak = stream.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(stream) = stream_weak.upgrade() {
                apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
            }
        });
    }

    /// Rebuild or update the filmstrip for the current index. Called from
    /// `show_at`. When the current index is still inside the existing window,
    /// only the highlight is toggled and the strip scrolls to reveal the
    /// current item; otherwise the strip is rebuilt with an initial window
    /// (±THUMB_INITIAL_HALF) centred on the current index.
    fn refresh_thumb_strip(&self) {
        let current = self.imp().current_index.get();
        let start = self.imp().thumb_window_start.get();
        let end = self.imp().thumb_window_end.get();
        let list_len = self.list_n_items().unwrap_or(0);

        let in_window = end > start && current >= start && current < end;
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_refresh current={} list_len={} existing_window=[{}, {}) in_window={} current_item={}",
            current,
            list_len,
            start,
            end,
            in_window,
            self.media_item_summary_at(current)
        );

        if in_window {
            self.update_thumb_highlight(current);
        } else {
            self.load_initial_thumb_window(current);
        }
        self.try_extend_thumb_window_for_current();
        self.schedule_scroll_thumb_to_current();
    }

    /// First-time load: centre a small bounded window around `current`.
    /// The visible strip is positioned later by CSS transform so ultrawide
    /// viewports do not force the viewer to load the whole album.
    fn load_initial_thumb_window(&self, current: u32) {
        let Some(n_items) = self.list_n_items() else {
            return;
        };
        if n_items == 0 {
            return;
        }
        let (start, end) =
            compute_initial_thumb_window_for_len(current, n_items, THUMB_DEFAULT_WINDOW_LEN);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_initial_window current={} n_items={} target_len={} computed_window=[{}, {}) current_item={}",
            current,
            n_items,
            THUMB_DEFAULT_WINDOW_LEN,
            start,
            end,
            self.media_item_summary_at(current)
        );
        self.rebuild_thumb_strip(start, end, current);
    }

    /// Lazy extend the loaded window by `THUMB_LAZY_HALF` items in the given
    /// direction (`-1` = prepend on the left, `+1` = append on the right).
    /// Bounded by `[0, n_items)` and the `THUMB_WINDOW_MAX` cap.
    fn try_extend_thumb_window(&self, direction: i8) {
        let imp = self.imp();
        if imp.thumb_pending_extend.get() == Some(direction) {
            // Debounce: rebuild itself can fire value-changed; suppress
            // cascading extends until the next idle clears this flag.
            return;
        }
        let Some(n_items) = self.list_n_items() else {
            return;
        };
        let start = imp.thumb_window_start.get();
        let end = imp.thumb_window_end.get();
        let items_len = imp.thumb_items.borrow().len();

        let Some((new_start, new_end)) =
            compute_extended_thumb_window(direction, start, end, n_items, items_len)
        else {
            return;
        };

        let current = imp.current_index.get();
        imp.thumb_pending_extend.set(Some(direction));
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_extend direction={} old_window=[{}, {}) new_window=[{}, {}) current={} items_len={} list_len={}",
            direction,
            start,
            end,
            new_start,
            new_end,
            current,
            items_len,
            n_items
        );
        self.rebuild_thumb_strip(new_start, new_end, current);
        self.schedule_scroll_thumb_to_current();

        // Clear the debounce flag on next idle so a subsequent scroll
        // past the new edge can extend again.
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.imp().thumb_pending_extend.set(None);
            }
        });
    }

    fn try_extend_thumb_window_for_current(&self) {
        let imp = self.imp();
        let Some(n_items) = self.list_n_items() else {
            return;
        };
        let current = imp.current_index.get();
        let start = imp.thumb_window_start.get();
        let end = imp.thumb_window_end.get();
        let items_len = imp.thumb_items.borrow().len();

        let Some(direction) =
            compute_current_thumb_extend_direction(current, start, end, n_items, items_len)
        else {
            return;
        };
        self.try_extend_thumb_window(direction);
    }

    /// Tear down the existing strip and rebuild with `[start, end)`.
    /// Each item is a frame-less `GtkButton` wrapping a `GtkPicture` with
    /// `content-fit: cover`. After the thumbnail texture arrives,
    /// `width-request` is set from the image aspect ratio, clamped to
    /// 21:9 / 9:21 so extreme panoramas do not dominate the filmstrip.
    fn rebuild_thumb_strip(&self, start: u32, end: u32, current: u32) {
        let imp = self.imp();
        let strip = imp.thumb_strip.get();
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_rebuild start={} end={} current={} current_offset={:?} list_len={} current_item={}",
            start,
            end,
            current,
            current.checked_sub(start),
            self.list_n_items().unwrap_or(0),
            self.media_item_summary_at(current)
        );

        // Clear old buttons.
        while let Some(child) = strip.first_child() {
            strip.remove(&child);
        }
        imp.thumb_items.borrow_mut().clear();

        let mut new_items = Vec::with_capacity((end - start) as usize);
        for idx in start..end {
            let Some(btn) = self.make_thumb_button(idx, current) else {
                continue;
            };
            strip.append(&btn);
            new_items.push(btn);
        }

        imp.thumb_window_start.set(start);
        imp.thumb_window_end.set(end);
        *imp.thumb_items.borrow_mut() = new_items;
    }

    /// Construct one filmstrip button + async thumbnail request. Shared by
    /// initial load and lazy extend so both code paths render identically.
    /// Returns `None` only when the media list / loader hasn't been injected
    /// yet (early construction), which the caller treats as a no-op.
    fn make_thumb_button(&self, idx: u32, current: u32) -> Option<gtk::Button> {
        let loader = self.imp().loader.borrow().as_ref()?.clone();
        let item = {
            let media_guard = self.imp().media_list.borrow();
            let list = media_guard.as_ref()?;
            crate::ui::media_list::media_item_at(list, idx)?
        };

        let button = gtk::Button::new();
        button.set_has_frame(false);
        button.add_css_class("viewer-thumb-item");
        if idx == current {
            button.add_css_class("viewer-thumb-current");
        }
        let fs_mtime = std::fs::metadata(&item.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let item_mtime = fs_mtime.unwrap_or_else(|| std::time::SystemTime::from(item.file_mtime));
        let cache_key =
            ThumbnailLoader::cache_key_for(&item.uri, ThumbnailSize::Small, Some(item_mtime));
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_button idx={} is_current={} item_id={} item_name={} item_uri={} media_item_mtime={} request_mtime={:?} fs_mtime={:?} cache_key={:?}",
            idx,
            idx == current,
            item.id,
            item.display_name(),
            item.uri,
            item.file_mtime,
            item_mtime,
            fs_mtime,
            cache_key
        );

        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .width_request(THUMB_MIN_WIDTH)
            .height_request(THUMB_HEIGHT)
            .can_shrink(true)
            .build();
        button.set_child(Some(&picture));

        // Request thumbnail. The ThumbnailLoader caches by `path + mtime`, so
        // extending the strip after the items were already requested once is a
        // cache hit (no extra decode).
        let item_uri = item.uri.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(
            item_uri,
            ThumbnailSize::Small,
            Some(item_mtime),
            tx,
            crate::core::thumbnails::TIER_NORMAL,
        );

        let pic_weak = picture.downgrade();
        let viewer_weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(loaded) = rx.await else {
                return;
            };
            let Some(pic) = pic_weak.upgrade() else {
                return;
            };
            let tex = loaded.texture;
            let tex_w = tex.width();
            let tex_h = tex.height();
            pic.set_paintable(Some(&tex));
            pic.set_width_request(clamped_thumb_width_for_texture(tex_w, tex_h));
            if let Some(this) = viewer_weak.upgrade() {
                this.schedule_scroll_thumb_to_current();
            }
        });

        // Click → navigate to this index.
        let weak = self.downgrade();
        button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let delta = idx as i32 - this.current_index() as i32;
                if delta != 0 {
                    this.navigate_by_delta(delta);
                }
            }
        });

        Some(button)
    }

    /// Toggle the `.viewer-thumb-current` class so only the current item is
    /// highlighted, without rebuilding the strip.
    fn update_thumb_highlight(&self, current: u32) {
        let start = self.imp().thumb_window_start.get();
        let items = self.imp().thumb_items.borrow();
        for (i, btn) in items.iter().enumerate() {
            let idx = start + i as u32;
            if idx == current {
                btn.add_css_class("viewer-thumb-current");
            } else {
                btn.remove_css_class("viewer-thumb-current");
            }
        }
    }

    fn update_thumb_scroll_position(&self) -> bool {
        let hadj = self.imp().thumb_scrolled.get().hadjustment();
        let page_size = hadj.page_size();
        let upper = hadj.upper();
        if page_size <= 0.0 {
            return false;
        }

        let start = self.imp().thumb_window_start.get();
        let current = self.imp().current_index.get();
        let Some(offset) = current.checked_sub(start).map(|v| v as usize) else {
            return false;
        };
        let items = self.imp().thumb_items.borrow();
        let Some(btn) = items.get(offset) else {
            return false;
        };

        let alloc = btn.allocation();
        if alloc.width() <= 0 {
            return false;
        }

        let item_widths = thumb_item_widths(&items);
        let (button_x, button_w, content_width) =
            thumb_item_content_geometry(&item_widths, offset, THUMB_STRIP_SPACING).unwrap_or((
                alloc.x() as f64,
                alloc.width() as f64,
                (items.len() as f64) * THUMB_ESTIMATED_ADVANCE,
            ));
        let (target, residual, visual_transform) =
            compute_thumb_positioning(button_x, button_w, page_size, upper, content_width);
        let imp = self.imp();
        imp.thumb_programmatic_scroll.set(true);
        hadj.set_value(target);
        imp.thumb_programmatic_scroll.set(false);
        self.apply_thumb_strip_transform(visual_transform);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE thumb_scroll current={} start={} button_x={} content_x={} button_w={} page_size={} upper={} content_width={} target={} residual={} transform={}",
            current,
            start,
            alloc.x(),
            button_x,
            button_w,
            page_size,
            upper,
            content_width,
            target,
            residual,
            visual_transform
        );
        true
    }

    fn apply_thumb_strip_transform(&self, offset: f64) {
        let imp = self.imp();
        if imp.thumb_transform_provider.borrow().is_none() {
            let provider = gtk::CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            *imp.thumb_transform_provider.borrow_mut() = Some(provider);
        }

        let css = if offset.abs() < 0.5 {
            ".viewer-thumb-strip { transform: none; }".to_string()
        } else {
            format!(".viewer-thumb-strip {{ transform: translate({offset}px, 0); }}")
        };
        if let Some(provider) = imp.thumb_transform_provider.borrow().as_ref() {
            provider.load_from_data(&css);
        }
        imp.thumb_strip.get().queue_draw();
    }

    /// Position the filmstrip around the current item. GTK's adjustment is
    /// used when it has a real scroll range; otherwise a CSS transform provides
    /// virtual scrolling without increasing the window's natural width.
    fn scroll_thumb_to_current(&self) -> bool {
        self.update_thumb_scroll_position()
    }

    fn schedule_scroll_thumb_to_current(&self) {
        let weak = self.downgrade();
        let attempts_remaining = Rc::new(Cell::new(THUMB_CENTER_RETRY_FRAMES));
        self.imp()
            .thumb_scrolled
            .get()
            .add_tick_callback(move |_, _| {
                let applied = weak
                    .upgrade()
                    .map(|this| this.scroll_thumb_to_current())
                    .unwrap_or(true);

                let remaining = attempts_remaining.get().saturating_sub(1);
                attempts_remaining.set(remaining);
                if should_retry_thumb_centering(applied, remaining) {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
    }

    /// Wire the horizontal adjustment's `value-changed` signal so that
    /// scrolling near either edge of the strip lazy-loads another half-row
    /// of thumbnails (see `try_extend_thumb_window`).
    fn setup_thumb_strip_listener(&self) {
        let scrolled = self.imp().thumb_scrolled.get();
        let hadj = scrolled.hadjustment();
        let weak = self.downgrade();
        hadj.connect_value_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.on_thumb_adj_changed();
            }
        });
    }

    fn on_thumb_adj_changed(&self) {
        let imp = self.imp();

        if imp.thumb_programmatic_scroll.get() {
            return;
        }

        let scrolled = imp.thumb_scrolled.get();
        let hadj = scrolled.hadjustment();
        let value = hadj.value();
        let page_size = hadj.page_size();
        let upper = hadj.upper();
        if page_size <= 0.0 {
            return;
        }

        // Distance (in pixels) from each scroll edge.
        let left_dist = value;
        let right_dist = upper - value - page_size;
        // Trigger when within ~30% of page size from the edge — far enough
        // that the user has clearly committed to scrolling further, close
        // enough that the rebuild happens before they hit the hard stop.
        let threshold = page_size * 0.3;

        let Some(n_items) = self.list_n_items() else {
            return;
        };
        let start = imp.thumb_window_start.get();
        let end = imp.thumb_window_end.get();
        let items_len = imp.thumb_items.borrow().len();
        let at_cap = items_len >= THUMB_WINDOW_MAX as usize;

        let mut direction: Option<i8> = None;
        if left_dist < threshold && start > 0 && !at_cap {
            direction = Some(-1);
        }
        if right_dist < threshold && end < n_items && !at_cap {
            // If both edges qualify, pick the one the user is closer to.
            direction = Some(match direction {
                Some(-1) if right_dist < left_dist => 1,
                other => other.unwrap_or(1),
            });
        }

        if let Some(dir) = direction {
            self.try_extend_thumb_window(dir);
        }
    }

    /// Convenience accessor for `gio::ListStore::n_items` that swallows the
    /// `media_list not injected yet` case and returns `None`.
    fn list_n_items(&self) -> Option<u32> {
        self.imp().media_list.borrow().as_ref().map(|l| l.n_items())
    }

    fn media_item_summary_at(&self, index: u32) -> String {
        let media_guard = self.imp().media_list.borrow();
        let Some(list) = media_guard.as_ref() else {
            return "media_list=None".to_string();
        };
        match crate::ui::media_list::media_item_at(list, index) {
            Some(item) => format!(
                "{}:{}:{}:{}",
                item.id,
                item.display_name(),
                item.uri,
                item.sort_datetime()
            ),
            None => format!("missing@{index}"),
        }
    }

    /// 从数据库异步同步当前图片收藏状态。与 `show_at()` 的 token 绑定，避免异步回写过期。
    fn sync_favorite_state(&self, item_id: i64) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            self.refresh_favorite_button(false);
            return;
        };

        let token = self.imp().current_token.get();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let result = MediaRepository::new(pool)
                .favorite_state(&[MediaId::from(item_id)])
                .map(|summary| summary.has_favorite);
            let _ = tx.send((result, token));
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok((result, token_expected)) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token_expected {
                return;
            }
            match result {
                Ok(is_favorite) => this.refresh_favorite_button(is_favorite),
                Err(e) => {
                    tracing::warn!("ViewerPage: failed to read favorite state: {e}");
                    this.refresh_favorite_button(false);
                }
            }
        });
    }

    fn setup_details_panel(&self) {
        let imp = self.imp();

        let weak = self.downgrade();
        imp.details_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let split_view = this.imp().details_split_view.get();
            let before = split_view.shows_sidebar();
            let next = !split_view.shows_sidebar();
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG details_btn clicked index={} before_revealed={} next_revealed={}",
                this.imp().current_index.get(),
                before,
                next
            );
            this.set_details_revealed(next, "details_btn");
            if next {
                if let Some(item) = this.current_media_item() {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG details_btn loading_details index={} name={}",
                        this.imp().current_index.get(),
                        item.display_name()
                    );
                    this.update_details(&item);
                }
            }
        });

        let weak = self.downgrade();
        imp.details_close_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let split_view = this.imp().details_split_view.get();
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG details_close_btn clicked index={} before_revealed={}",
                this.imp().current_index.get(),
                split_view.shows_sidebar()
            );
            this.set_details_revealed(false, "details_close_btn");
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG details_close_btn after set_reveal_child(false) revealed={}",
                split_view.shows_sidebar()
            );
            this.log_nav_state("details_close_btn immediate");
            let weak_after = this.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(this) = weak_after.upgrade() {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG details_close_btn idle_after revealed={} mapped={} visible={} root_is_some={}",
                        this.imp().details_split_view.get().shows_sidebar(),
                        this.is_mapped(),
                        this.is_visible(),
                        this.root().is_some()
                    );
                    this.log_nav_state("details_close_btn idle_after");
                } else {
                    tracing::debug!(target: crate::core::log_targets::VIEWER, "VIEWER_DEBUG details_close_btn idle_after viewer_dropped");
                }
            });
        });
    }

    fn setup_navigation_pop_action(&self) {
        let action_group = gio::SimpleActionGroup::new();
        let pop_action = gio::SimpleAction::new("pop", None);
        let weak = self.downgrade();
        pop_action.connect_activate(move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let details_split_view = this.imp().details_split_view.get();
            let editor_split_view = this.imp().editor_split_view.get();
            if editor_split_view.shows_sidebar() {
                this.stop_editing();
            } else if details_split_view.shows_sidebar() {
                this.set_details_revealed(false, "navigation.pop");
            } else {
                this.fire_nav(NAV_POP);
            }
        });
        action_group.add_action(&pop_action);
        self.insert_action_group("navigation", Some(&action_group));
    }

    fn set_details_revealed(&self, revealed: bool, reason: &str) {
        let split_view = self.imp().details_split_view.get();
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG set_details_revealed reason={} index={} from={} to={} can_pop_before={}",
            reason,
            self.imp().current_index.get(),
            split_view.shows_sidebar(),
            revealed,
            self.can_pop()
        );

        if revealed {
            self.set_details_sidebar_child_visible(true);
        }
        split_view.set_show_sidebar(revealed);

        if revealed {
            // While the side panel is open, the viewer page must not be popped
            // by NavigationView's built-in back gesture/action.
            self.set_can_pop(false);
        } else {
            // Keep pop disabled until the slide transition finishes. The log
            // evidence showed NavigationView can emit a delayed built-in pop
            // shortly after the details revealer starts closing.
            self.set_can_pop(false);
            let weak = self.downgrade();
            glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
                let Some(this) = weak.upgrade() else {
                    tracing::debug!(target: crate::core::log_targets::VIEWER, "VIEWER_DEBUG restore_can_pop viewer_dropped");
                    return;
                };
                if !this.imp().details_split_view.get().shows_sidebar() {
                    this.set_details_sidebar_child_visible(false);
                    this.set_can_pop(true);
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG restore_can_pop restored index={} can_pop={} visible={:?}",
                        this.imp().current_index.get(),
                        this.can_pop(),
                        this.imp()
                            .nav_view
                            .borrow()
                            .as_ref()
                            .and_then(|nav| nav.visible_page())
                            .map(|page| page.title())
                    );
                } else {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG restore_can_pop skipped_details_open index={} can_pop={}",
                        this.imp().current_index.get(),
                        this.can_pop()
                    );
                }
            });
        }

        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG set_details_revealed done reason={} index={} revealed={} can_pop_after={}",
            reason,
            self.imp().current_index.get(),
            split_view.shows_sidebar(),
            self.can_pop()
        );
    }

    fn set_details_sidebar_child_visible(&self, visible: bool) {
        if let Some(sidebar) = self.imp().details_split_view.get().sidebar() {
            sidebar.set_visible(visible);
        }
    }

    fn set_editor_sidebar_child_visible(&self, visible: bool) {
        self.imp().editor_panel.get().set_visible(visible);
    }

    fn set_overlay_navigation_visible(&self, visible: bool) {
        if let Some(container) = self.imp().prev_btn.get().parent() {
            container.set_visible(visible);
        }
    }

    fn setup_lifecycle_logging(&self) {
        let weak = self.downgrade();
        self.connect_unmap(move |_| {
            if let Some(this) = weak.upgrade() {
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_DEBUG viewer unmap index={} title={} details_revealed={}",
                    this.imp().current_index.get(),
                    this.title(),
                    this.imp().details_split_view.get().shows_sidebar()
                );
                this.log_nav_state("viewer unmap");
            }
        });

        let weak = self.downgrade();
        self.connect_unrealize(move |_| {
            if let Some(this) = weak.upgrade() {
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_DEBUG viewer unrealize index={} title={} details_revealed={}",
                    this.imp().current_index.get(),
                    this.title(),
                    this.imp().details_split_view.get().shows_sidebar()
                );
                this.log_nav_state("viewer unrealize");
            }
        });
    }

    fn log_nav_state(&self, label: &str) {
        if let Some(nav) = self.imp().nav_view.borrow().as_ref() {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG nav_state label=\"{}\" visible={:?} viewer_title={} viewer_mapped={} viewer_visible={} root_is_some={}",
                label,
                nav.visible_page().map(|page| page.title()),
                self.title(),
                self.is_mapped(),
                self.is_visible(),
                self.root().is_some()
            );
        } else {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG nav_state label=\"{}\" nav_view=None viewer_title={} viewer_mapped={} viewer_visible={} root_is_some={}",
                label,
                self.title(),
                self.is_mapped(),
                self.is_visible(),
                self.root().is_some()
            );
        }
    }

    /// Resolve the `MediaItem` at the current index out of the
    /// `BoxedAnyObject<MediaItem>` store. Returns `None` if the index is
    /// out of range or the item can't be downcast.
    fn current_media_item(&self) -> Option<MediaItem> {
        let list = self.imp().media_list.borrow();
        let list = list.as_ref()?;
        let idx = self.imp().current_index.get();
        crate::ui::media_list::media_item_at(list, idx)
    }

    /// Display the item at `index`, decode the **original** image off the
    /// main thread, and preload its immediate neighbours. Safe to call
    /// multiple times.
    pub fn show_at(&self, index: u32) {
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG show_at requested_index={} current_before={} details_revealed={}",
            index,
            self.imp().current_index.get(),
            self.imp().details_split_view.get().shows_sidebar()
        );
        self.imp().current_index.set(index);
        self.imp().spinner.get().set_visible(true);
        self.reset_viewer_zoom();

        // Bump token so a stale response from a previous show_at() doesn't
        // overwrite the current picture.
        let token = {
            let t = self.imp().current_token.get() + 1;
            self.imp().current_token.set(t);
            t
        };

        let Some(item) = self.current_media_item() else {
            return;
        };
        self.imp().current_media_id.set(item.id);
        self.set_title(item.display_name());
        self.sync_favorite_state(item.id);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_TRACE viewer_show_at index={} list_len={} item_id={} item_name={} item_uri={} sort_time={}",
            index,
            self.list_n_items().unwrap_or(0),
            item.id,
            item.display_name(),
            item.uri,
            item.sort_datetime()
        );
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG show_at resolved index={} item_id={} title={} uri={} media_path={} details_revealed={}",
            index,
            item.id,
            item.display_name(),
            item.uri,
            item.path.display(),
            self.imp().details_split_view.get().shows_sidebar()
        );
        if self.imp().details_split_view.get().shows_sidebar() {
            self.update_details(&item);
        }
        if item.is_video() {
            self.refresh_thumb_strip();
            self.imp().motion_play_btn.get().set_visible(false);
            self.show_video_stage(&item);
            return;
        }
        self.show_image_stage();
        self.set_motion_play_button_for_item(&item);
        if prefs::auto_play_motion_photo() && item.is_motion_photo() {
            self.play_current_motion_photo();
        }
        let path = strip_file_uri(&item.uri);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG viewer decode_start index={} item_id={} item_name={} source_uri={} decode_path={}",
            index,
            item.id,
            item.display_name(),
            item.uri,
            path.display()
        );

        // Preload neighbours first (fire-and-forget — we just want the OS
        // page cache warm). Preload is reduced from ±1±2 to ±1 only because
        // each original decode can be tens of MB; holding 4 in memory at
        // once is too much for typical browsing.
        self.preload_neighbor(-1);
        self.preload_neighbor(1);

        // Update the bottom filmstrip (highlight or rebuild + scroll).
        self.refresh_thumb_strip();

        // Decode the current image off the main thread. `Pixbuf::from_file`
        // dispatches via gdk-pixbuf loaders (JPEG/PNG/HEIC/AVIF/...) and is
        // CPU-bound for big images — `spawn_blocking` keeps the UI responsive.
        // We use `gio::spawn_blocking` (matches `editor_panel.rs`) rather than
        // `tokio::task::spawn_blocking`. Pixbuf itself is `!Send`, so the
        // worker converts it to a `gdk::Texture` (which IS Send) before
        // returning — that way we can hand the texture across the oneshot.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let decode_item_name = item.display_name().to_string();
        let decode_source_uri = item.uri.clone();
        let decode_path = path.clone();
        gio::spawn_blocking(move || {
            let result = orientation::load_oriented_pixbuf(&path)
                .map(|pb| gdk::Texture::for_pixbuf(&pb))
                .map_err(|e| format!("load_oriented_pixbuf({path:?}) failed: {e}"));
            let _ = tx.send(result);
        });

        let picture_weak = self.imp().picture.downgrade();
        let spinner_weak = self.imp().spinner.downgrade();
        let token_holder = self.imp().current_token.clone();
        glib::spawn_future_local(async move {
            let texture = match rx.await {
                Ok(Ok(t)) => t,
                Ok(Err(e)) => {
                    tracing::warn!("ViewerPage: {e}");
                    if let Some(spinner) = spinner_weak.upgrade() {
                        spinner.set_visible(false);
                    }
                    return;
                }
                Err(_) => return, // sender dropped — cancelled
            };
            // Stale response: another show_at() ran in the meantime.
            if token_holder.get() != token {
                return;
            }
            if let (Some(picture), Some(spinner)) = (picture_weak.upgrade(), spinner_weak.upgrade())
            {
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_DEBUG viewer decode_loaded token={} item_name={} source_uri={} decode_path={} texture={}x{}",
                    token,
                    decode_item_name,
                    decode_source_uri,
                    decode_path.display(),
                    texture.width(),
                    texture.height()
                );
                picture.set_paintable(Some(&texture));
                spinner.set_visible(false);
            }
        });
    }

    /// Decode the neighbour at `current + offset` and drop the result. Used
    /// purely to warm the OS page cache so navigation feels snappier. The
    /// returned `Pixbuf` is dropped immediately; the OS still retains the
    /// file pages for the next decode.
    fn preload_neighbor(&self, offset: i32) {
        let cur = self.imp().current_index.get() as i32;
        let target = cur + offset;
        let path = {
            let list = self.imp().media_list.borrow();
            let list = match list.as_ref() {
                Some(l) => l,
                None => return,
            };
            if target < 0 {
                return;
            }
            let target_u = target as u32;
            if target_u >= list.n_items() {
                return;
            }
            let Some(obj) = list.item(target_u) else {
                return;
            };
            let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let item = boxed.borrow::<MediaItem>();
            if item.is_video() {
                return;
            }
            let uri = item.uri.clone();
            strip_file_uri(&uri)
        };
        gio::spawn_blocking(move || {
            // Result intentionally dropped — we only care that the file
            // got read into the page cache.
            let _ = orientation::load_oriented_pixbuf(&path);
        });
    }

    fn setup_gesture(&self) {
        // Lazily allocate a single CssProvider and install it once on the
        // display. Subsequent gesture ticks only `load_from_data` to
        // update the transform, avoiding a fresh provider (and a leak)
        // on every pinch event.
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        *self.imp().zoom_provider.borrow_mut() = Some(provider);

        let gesture = gtk::GestureZoom::new();
        let weak = self.downgrade();
        gesture.connect_begin(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.imp()
                    .zoom_gesture_origin_scale
                    .set(this.imp().zoom_scale.get());
            }
        });
        let weak = self.downgrade();
        gesture.connect_scale_changed(move |_, scale| {
            if let Some(this) = weak.upgrade() {
                let next =
                    pinch_zoom_from_origin(this.imp().zoom_gesture_origin_scale.get(), scale);
                this.set_viewer_zoom(
                    next,
                    this.imp().zoom_pan_x.get(),
                    this.imp().zoom_pan_y.get(),
                );
            }
        });
        self.imp().image_overlay.get().add_controller(gesture);

        let drag = gtk::GestureDrag::new();
        let weak = self.downgrade();
        drag.connect_drag_begin(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.imp()
                    .zoom_drag_origin_x
                    .set(this.imp().zoom_pan_x.get());
                this.imp()
                    .zoom_drag_origin_y
                    .set(this.imp().zoom_pan_y.get());
            }
        });
        let weak = self.downgrade();
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(this) = weak.upgrade() {
                if this.imp().is_editing.get()
                    || this.imp().crop_overlay_active.get()
                    || this.imp().zoom_scale.get() <= MIN_VIEWER_ZOOM
                {
                    return;
                }
                this.set_viewer_zoom(
                    this.imp().zoom_scale.get(),
                    this.imp().zoom_drag_origin_x.get() + dx,
                    this.imp().zoom_drag_origin_y.get() + dy,
                );
            }
        });
        self.imp().image_overlay.get().add_controller(drag);
    }

    fn setup_keyboard(&self) {
        let key_ctrl = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        key_ctrl.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Right => {
                if let Some(this) = weak.upgrade() {
                    if this.imp().is_editing.get() {
                        return glib::Propagation::Stop;
                    }
                    this.navigate_by_delta(1);
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Left => {
                if let Some(this) = weak.upgrade() {
                    if this.imp().is_editing.get() {
                        return glib::Propagation::Stop;
                    }
                    this.navigate_by_delta(-1);
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Escape => {
                if let Some(this) = weak.upgrade() {
                    if this.imp().editor_split_view.get().shows_sidebar() {
                        this.stop_editing();
                        return glib::Propagation::Stop;
                    }
                    let details_split_view = this.imp().details_split_view.get();
                    if details_split_view.shows_sidebar() {
                        this.set_details_revealed(false, "key Escape");
                        return glib::Propagation::Stop;
                    }
                    this.fire_nav(NAV_POP);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        self.imp().picture.get().add_controller(key_ctrl);
    }

    /// Current item index in the backing `ListStore`.
    pub fn current_index(&self) -> u32 {
        self.imp().current_index.get()
    }

    fn update_details(&self, item: &MediaItem) {
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG update_details index={} name={} path={}",
            self.imp().current_index.get(),
            item.display_name(),
            item.path.display()
        );
        let imp = self.imp();
        imp.name_row.get().set_title(&tr("viewer.details.name"));
        imp.folder_row.get().set_title(&tr("viewer.details.folder"));
        imp.mime_row.get().set_title(&tr("viewer.details.type"));
        imp.dimensions_row
            .get()
            .set_title(&tr("viewer.details.dimensions"));
        imp.size_row.get().set_title(&tr("viewer.details.size"));
        imp.taken_row
            .get()
            .set_title(&tr("viewer.details.captured"));

        imp.name_row.get().set_subtitle(item.display_name());
        imp.folder_row
            .get()
            .set_subtitle(&item.folder_path.to_string_lossy());
        imp.mime_row.get().set_subtitle(&item.mime_type);

        // Hide rows whose value is absent instead of showing "Not available".
        let dim = format_dimensions(item.width, item.height);
        imp.dimensions_row
            .get()
            .set_visible(item.width.is_some() && item.height.is_some());
        imp.dimensions_row.get().set_subtitle(&dim);

        if item.file_size > 0 {
            imp.size_row.get().set_visible(true);
            imp.size_row
                .get()
                .set_subtitle(&format_file_size(item.file_size));
        } else {
            imp.size_row.get().set_visible(false);
        }

        if let Some(dt) = item.taken_at {
            imp.taken_row.get().set_visible(true);
            imp.taken_row.get().set_subtitle(&format_datetime(Some(dt)));
        } else {
            // No value in DB — hide for now. If the fresh EXIF parse below
            // finds one, the callback will make it visible again.
            imp.taken_row.get().set_visible(false);
        }

        self.clear_camera_rows();
        self.clear_video_rows();
        if item.is_video() {
            self.load_video_details(item.path.clone(), self.imp().current_token.get());
        } else {
            self.load_camera_details(item.path.clone(), self.imp().current_token.get());
        }
    }

    /// Walk up from an ActionRow to its owning PreferencesGroup.
    fn file_group(&self) -> Option<adw::PreferencesGroup> {
        self.imp()
            .name_row
            .get()
            .ancestor(adw::PreferencesGroup::static_type())
            .and_downcast::<adw::PreferencesGroup>()
    }

    /// Remove all dynamically-created camera-parameter rows from the file group.
    fn clear_camera_rows(&self) {
        if let Some(g) = &self.file_group() {
            for row in self.imp().camera_rows.borrow_mut().drain(..) {
                g.remove(&row);
            }
        }
    }

    fn load_camera_details(&self, path: PathBuf, token: u64) {
        let path_dbg = path.display().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let meta = metadata::extract(&path).ok();
            let summary = meta.as_ref().and_then(|m| m.camera.clone());
            let taken_at = meta.as_ref().and_then(|m| m.taken_at);
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "load_camera_details spawn_blocking path={} summary_some={} taken_at_some={}",
                path_dbg,
                summary.is_some(),
                taken_at.is_some(),
            );
            let _ = tx.send((summary, taken_at));
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok((summary, taken_at)) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            // If the stored MediaItem has no taken_at (e.g. HEIC scanned
            // before the ISOBMFF parser was fixed), fill it from the fresh
            // EXIF parse.
            if let Some(dt) = taken_at {
                let imp = this.imp();
                let row = imp.taken_row.get();
                if !row.is_visible() {
                    row.set_visible(true);
                }
                row.set_subtitle(&format_datetime(Some(dt)));
            }
            this.populate_camera_rows(summary);
        });
    }

    /// Build `ActionRow`s from `ExifSummary` and append them to the file
    /// group (same group that holds name / folder / dimensions etc.).
    ///
    /// Related parameters are merged into fewer rows with compact notation
    /// so the details panel stays scannable.
    fn populate_camera_rows(&self, summary: Option<ExifSummary>) {
        let Some(group) = self.file_group() else {
            tracing::warn!("populate_camera_rows: no PreferencesGroup for name_row");
            return;
        };
        let imp = self.imp();

        let Some(s) = summary else {
            return;
        };

        let mut rows = imp.camera_rows.borrow_mut();

        // Device: Make + Model (phone lens name duplicates body/focal/aperture,
        // so we skip the lens row and show only the merged body here).
        let body = match (&s.make, &s.model) {
            (Some(mk), Some(md)) if md.contains(mk.as_str()) => md.clone(),
            (Some(mk), Some(md)) => format!("{} {}", mk, md),
            (_, Some(md)) => md.clone(),
            (Some(mk), _) => mk.clone(),
            _ => String::new(),
        };
        if !body.is_empty() {
            let row = action_row(&tr("camera.body"), &body);
            group.add(&row);
            rows.push(row);
        }

        // Exposure triangle: aperture, shutter, ISO
        let mut exp: Vec<String> = Vec::new();
        if let Some(v) = s.aperture {
            exp.push(format!("f/{:.1}", v));
        }
        if let Some((num, den)) = s.exposure_time {
            exp.push(if den == 0 {
                format!("{}/{}s", num, den)
            } else {
                format_exposure(num, den)
            });
        }
        if let Some(v) = s.iso {
            exp.push(format!("ISO {}", v));
        }
        if !exp.is_empty() {
            let row = action_row(&tr("camera.exposure"), &exp.join("  "));
            group.add(&row);
            rows.push(row);
        }

        // Focal length + 35mm eq
        match (s.focal_length_mm, s.focal_length_35mm_mm) {
            (Some(fl), Some(fl35)) => {
                let row = action_row(
                    &tr("camera.focal_length"),
                    &format!("{:.1} mm  (35mm: {} mm)", fl, fl35),
                );
                group.add(&row);
                rows.push(row);
            }
            (Some(fl), None) => {
                let row = action_row(&tr("camera.focal_length"), &format!("{:.1} mm", fl));
                group.add(&row);
                rows.push(row);
            }
            (None, Some(fl35)) => {
                let row = action_row(&tr("camera.focal_length"), &format!("35mm: {} mm", fl35));
                group.add(&row);
                rows.push(row);
            }
            _ => {}
        }

        // Exposure mode + bias
        let mode_str = s.exposure_mode.map(|m| {
            use crate::core::metadata::ExposureMode;
            tr(match m {
                ExposureMode::Auto => "camera.exposure_mode.auto",
                ExposureMode::Manual => "camera.exposure_mode.manual",
                ExposureMode::AutoBracket => "camera.exposure_mode.auto_bracket",
                ExposureMode::AperturePriority => "camera.exposure_mode.aperture_priority",
                ExposureMode::ShutterPriority => "camera.exposure_mode.shutter_priority",
                ExposureMode::Program => "camera.exposure_mode.program",
            })
        });
        let bias_str = s.exposure_bias_ev.map(|v| {
            let sign = if v >= 0.0 { "+" } else { "" };
            format!("{}{:.1} EV", sign, v)
        });
        match (mode_str, bias_str) {
            (Some(m), Some(b)) => {
                let row = action_row(&tr("camera.exposure_mode"), &format!("{}, {}", m, b));
                group.add(&row);
                rows.push(row);
            }
            (Some(m), None) => {
                let row = action_row(&tr("camera.exposure_mode"), &m);
                group.add(&row);
                rows.push(row);
            }
            (None, Some(b)) => {
                let row = action_row(&tr("camera.exposure_bias"), &b);
                group.add(&row);
                rows.push(row);
            }
            _ => {}
        }

        // Location: GPS + altitude
        let gps_str = s.gps.as_ref().map(|gps| {
            format!(
                "{}°{}′{:.1}″{}  {}°{}′{:.1}″{}",
                gps.lat.deg,
                gps.lat.min,
                gps.lat.sec,
                if gps.lat.north_or_east { "N" } else { "S" },
                gps.lon.deg,
                gps.lon.min,
                gps.lon.sec,
                if gps.lon.north_or_east { "E" } else { "W" },
            )
        });
        let alt_str = s.altitude_m.map(|a| format!("{:.1} m", a));
        match (gps_str, alt_str) {
            (Some(g), Some(a)) => {
                let row = action_row(&tr("camera.location"), &format!("{}  .  {}", g, a));
                group.add(&row);
                rows.push(row);
            }
            (Some(g), None) => {
                let row = action_row(&tr("camera.location"), &g);
                group.add(&row);
                rows.push(row);
            }
            (None, Some(a)) => {
                let row = action_row(&tr("camera.location"), &a);
                group.add(&row);
                rows.push(row);
            }
            _ => {}
        }

        // Secondary: metering, flash, WB
        let metering_str = s.metering_mode.map(|m| {
            use crate::core::metadata::MeteringMode;
            tr(match m {
                MeteringMode::Average => "camera.metering.average",
                MeteringMode::CenterWeighted => "camera.metering.center_weighted",
                MeteringMode::Spot => "camera.metering.spot",
                MeteringMode::Other => "camera.metering.other",
            })
        });
        let flash_str = s.flash.and_then(|f| {
            use crate::core::metadata::FlashState;
            match f {
                FlashState::Fired => Some(tr("camera.flash.fired")),
                FlashState::NotFired => None,
            }
        });
        let wb_str = s.white_balance.and_then(|w| {
            use crate::core::metadata::WhiteBalance;
            match w {
                WhiteBalance::Auto => None,
                WhiteBalance::Manual => Some(tr("camera.white_balance.manual")),
            }
        });
        let secondary: Vec<String> = [metering_str, flash_str, wb_str]
            .into_iter()
            .flatten()
            .collect();
        if !secondary.is_empty() {
            let row = action_row(&tr("camera.secondary"), &secondary.join("  .  "));
            group.add(&row);
            rows.push(row);
        }
    }

    /// Remove all dynamically-created video-info rows from the file group.
    fn clear_video_rows(&self) {
        if let Some(g) = &self.file_group() {
            for row in self.imp().video_rows.borrow_mut().drain(..) {
                g.remove(&row);
            }
        }
    }

    /// 异步加载视频元数据（ffprobe），完成后填充视频属性行；带 token 过期保护。
    /// 镜像 [`load_camera_details`]。
    fn load_video_details(&self, path: PathBuf, token: u64) {
        let path_dbg = path.display().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let meta = metadata::extract(&path).ok();
            let summary = meta.as_ref().and_then(|m| m.video.clone());
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "load_video_details spawn_blocking path={} summary_some={}",
                path_dbg,
                summary.is_some(),
            );
            let _ = tx.send(summary);
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(summary) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            this.populate_video_rows(summary.as_ref());
        });
    }

    /// Build video `ActionRow`s (duration / codec / fps / bitrate / container /
    /// device) and append them to the file group. Mirrors `populate_camera_rows`.
    fn populate_video_rows(&self, summary: Option<&VideoSummary>) {
        let Some(group) = self.file_group() else {
            tracing::warn!("populate_video_rows: no PreferencesGroup for name_row");
            return;
        };
        let Some(s) = summary else {
            return;
        };

        let mut rows = self.imp().video_rows.borrow_mut();

        if let Some(d) = s.duration_secs {
            if let Some(formatted) = format_duration(d) {
                let row = action_row(&tr("video.duration"), &formatted);
                group.add(&row);
                rows.push(row);
            }
        }
        if let Some(codec) = &s.codec {
            let row = action_row(&tr("video.codec"), codec);
            group.add(&row);
            rows.push(row);
        }
        if let Some(fps) = s.fps {
            let row = action_row(&tr("video.fps"), &format!("{:.0} fps", fps.round()));
            group.add(&row);
            rows.push(row);
        }
        if let Some(br) = s.bitrate {
            if let Some(formatted) = format_bitrate(br) {
                let row = action_row(&tr("video.bitrate"), &formatted);
                group.add(&row);
                rows.push(row);
            }
        }
        if let Some(container) = &s.container {
            let row = action_row(&tr("video.container"), container);
            group.add(&row);
            rows.push(row);
        }
        // Device: make + model 合并（与相机行一致）。
        let body = match (&s.make, &s.model) {
            (Some(mk), Some(md)) if md.contains(mk.as_str()) => md.clone(),
            (Some(mk), Some(md)) => format!("{} {}", mk, md),
            (_, Some(md)) => md.clone(),
            (Some(mk), _) => mk.clone(),
            _ => String::new(),
        };
        if !body.is_empty() {
            let row = action_row(&tr("video.device"), &body);
            group.add(&row);
            rows.push(row);
        }
    }
}

/// Build a non-activatable `ActionRow` with translated title and plain subtitle.
fn action_row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(false)
        .build()
}

/// Pretty-print an exposure-time rational.
///
/// - `(1, 125)` → `"1/125s"`
/// - `(1865378, 1000000000)` ≈ 1/536 → `"1/536s"`
/// - `(5, 10)` = 0.5s → `"0.5s"`
fn format_exposure(num: u32, den: u32) -> String {
    if den == 0 {
        return format!("{}/{}s", num, den);
    }
    let v = num as f64 / den as f64;
    if v < 1.0 {
        // Fractional: display as 1/N so photographers can read it naturally.
        let n = (1.0 / v).round() as u32;
        if n >= 10000 {
            // Fallback: too large a reciprocal, just show the decimal.
            format!("{:.4}s", v)
        } else {
            format!("1/{}s", n)
        }
    } else {
        format!("{:.1}s", v)
    }
}

/// 格式化视频时长（秒）为 `M:SS` / `MM:SS`，超过 1 小时则 `H:MM:SS`。
/// 非正数或非有限值返回 `None`（UI 隐藏该行）。
fn format_duration(secs: f64) -> Option<String> {
    if !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    let total = secs.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    Some(if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    })
}

/// 格式化比特率：≥ 1 Mbps 用 Mbps，否则 kbps。0 返回 `None`。
fn format_bitrate(bps: u64) -> Option<String> {
    if bps == 0 {
        return None;
    }
    let mbps = bps as f64 / 1_000_000.0;
    Some(if mbps >= 1.0 {
        format!("{:.1} Mbps", mbps)
    } else {
        format!("{} kbps", bps / 1000)
    })
}

fn step_zoom(current: f64, direction: i32) -> f64 {
    let factor = if direction >= 0 {
        VIEWER_ZOOM_STEP
    } else {
        1.0 / VIEWER_ZOOM_STEP
    };
    (current * factor).clamp(MIN_VIEWER_ZOOM, MAX_VIEWER_ZOOM)
}

fn pinch_zoom_from_origin(origin_scale: f64, gesture_scale: f64) -> f64 {
    (origin_scale * gesture_scale).clamp(MIN_VIEWER_ZOOM, MAX_VIEWER_ZOOM)
}

fn clamp_zoom_pan(
    scale: f64,
    pan_x: f64,
    pan_y: f64,
    viewport_width: f64,
    viewport_height: f64,
) -> (f64, f64) {
    if scale <= MIN_VIEWER_ZOOM || viewport_width <= 0.0 || viewport_height <= 0.0 {
        return (0.0, 0.0);
    }

    let max_x = viewport_width * (scale - 1.0) / 2.0;
    let max_y = viewport_height * (scale - 1.0) / 2.0;
    (pan_x.clamp(-max_x, max_x), pan_y.clamp(-max_y, max_y))
}

/// Pure calculation: scroll adjustment value plus a visual-only residual that
/// centres a thumbnail at the clamped scroller edges. The returned `value` is
/// always inside the adjustment's legal range; `residual` is applied as a CSS
/// transform and therefore does not affect GTK's size request. The residual is
/// bounded so the transformed content never leaves empty space on the opposite
/// edge when the strip content is not wider than the viewport.
fn compute_thumb_scroll_and_residual(
    btn_x: f64,
    btn_w: f64,
    page_size: f64,
    upper: f64,
) -> (f64, f64) {
    let raw = btn_x + btn_w / 2.0 - page_size / 2.0;
    let max_value = (upper - page_size).max(0.0);
    let value = raw.clamp(0.0, max_value);
    let residual = clamp_thumb_residual(value - raw, upper, page_size);
    (value, residual)
}

fn compute_thumb_positioning(
    btn_x: f64,
    btn_w: f64,
    page_size: f64,
    adjustment_upper: f64,
    content_width: f64,
) -> (f64, f64, f64) {
    if content_width <= page_size {
        let transform = page_size / 2.0 - (btn_x + btn_w / 2.0);
        return (0.0, transform, transform);
    }

    let effective_upper = adjustment_upper.max(content_width);
    let (target, residual) =
        compute_thumb_scroll_and_residual(btn_x, btn_w, page_size, effective_upper);
    let transform = compute_thumb_visual_transform(target, residual, adjustment_upper, page_size);
    (target, residual, transform)
}

fn thumb_item_widths(items: &[gtk::Button]) -> Vec<f64> {
    items
        .iter()
        .map(|item| item.allocation().width() as f64)
        .collect()
}

fn clamped_thumb_width_for_texture(tex_w: i32, tex_h: i32) -> i32 {
    if tex_w <= 0 || tex_h <= 0 {
        return THUMB_MIN_WIDTH;
    }

    let aspect = (tex_w as f64 / tex_h as f64).clamp(THUMB_MIN_ASPECT, THUMB_MAX_ASPECT);
    (((THUMB_HEIGHT as f64) * aspect).round() as i32).max(THUMB_MIN_WIDTH)
}

fn thumb_item_content_geometry(
    widths: &[f64],
    offset: usize,
    spacing: f64,
) -> Option<(f64, f64, f64)> {
    let width = *widths.get(offset)?;
    if width <= 0.0 || widths.iter().any(|width| *width <= 0.0) {
        return None;
    }

    let x = widths
        .iter()
        .take(offset)
        .fold(0.0, |acc, width| acc + width + spacing);
    let content_width =
        widths.iter().sum::<f64>() + widths.len().saturating_sub(1) as f64 * spacing;
    Some((x, width, content_width))
}

fn clamp_thumb_residual(residual: f64, upper: f64, page_size: f64) -> f64 {
    let scrollable = (upper - page_size).max(0.0);
    if scrollable <= 0.0 {
        0.0
    } else {
        residual.clamp(-scrollable, scrollable)
    }
}

fn compute_thumb_visual_transform(
    target: f64,
    residual: f64,
    adjustment_upper: f64,
    page_size: f64,
) -> f64 {
    if adjustment_upper - page_size > 0.5 {
        residual
    } else {
        residual - target
    }
}

fn compute_contained_image_rect(
    widget_width: f64,
    widget_height: f64,
    image_dimensions: (u32, u32),
) -> Option<ImageRect> {
    let (image_width, image_height) = image_dimensions;
    if widget_width <= 0.0 || widget_height <= 0.0 || image_width == 0 || image_height == 0 {
        return None;
    }
    let widget_ratio = widget_width / widget_height;
    let image_ratio = image_width as f64 / image_height as f64;
    let (width, height) = if widget_ratio > image_ratio {
        let height = widget_height;
        (height * image_ratio, height)
    } else {
        let width = widget_width;
        (width, width / image_ratio)
    };
    Some(ImageRect {
        x: (widget_width - width) / 2.0,
        y: (widget_height - height) / 2.0,
        width,
        height,
    })
}

fn crop_rect_to_widget(
    rect: (u32, u32, u32, u32),
    image_dimensions: (u32, u32),
    image_rect: ImageRect,
) -> Option<ImageRect> {
    let (image_width, image_height) = image_dimensions;
    if image_width == 0 || image_height == 0 {
        return None;
    }
    Some(ImageRect {
        x: image_rect.x + rect.0 as f64 / image_width as f64 * image_rect.width,
        y: image_rect.y + rect.1 as f64 / image_height as f64 * image_rect.height,
        width: rect.2 as f64 / image_width as f64 * image_rect.width,
        height: rect.3 as f64 / image_height as f64 * image_rect.height,
    })
}

fn crop_handle_points(rect: ImageRect) -> [(f64, f64); 4] {
    [
        (rect.x, rect.y),
        (rect.x + rect.width, rect.y),
        (rect.x, rect.y + rect.height),
        (rect.x + rect.width, rect.y + rect.height),
    ]
}

fn hit_crop_drag_mode(x: f64, y: f64, rect: ImageRect) -> Option<CropDragMode> {
    for (idx, (hx, hy)) in crop_handle_points(rect).into_iter().enumerate() {
        if (x - hx).hypot(y - hy) <= CROP_HANDLE_RADIUS {
            return Some(match idx {
                0 => CropDragMode::ResizeNw,
                1 => CropDragMode::ResizeNe,
                2 => CropDragMode::ResizeSw,
                _ => CropDragMode::ResizeSe,
            });
        }
    }
    let inside =
        x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height;
    inside.then_some(CropDragMode::Move)
}

fn drag_rect(
    drag: CropDragState,
    dx: f64,
    dy: f64,
    image_dimensions: (u32, u32),
) -> (u32, u32, u32, u32) {
    let (image_width, image_height) = image_dimensions;
    let (x, y, width, height) = drag.rect;
    let min = CROP_MIN_SOURCE_SIZE
        .min(image_width)
        .min(image_height)
        .max(1);
    match drag.mode {
        CropDragMode::Move => {
            let nx = (x as f64 + dx)
                .round()
                .clamp(0.0, image_width.saturating_sub(width) as f64) as u32;
            let ny = (y as f64 + dy)
                .round()
                .clamp(0.0, image_height.saturating_sub(height) as f64) as u32;
            (nx, ny, width, height)
        }
        CropDragMode::ResizeNw => resize_from_edges(
            (x as f64 + dx).round() as i32,
            (y as f64 + dy).round() as i32,
            (x + width) as i32,
            (y + height) as i32,
            image_dimensions,
            min,
        ),
        CropDragMode::ResizeNe => resize_from_edges(
            x as i32,
            (y as f64 + dy).round() as i32,
            (x as f64 + width as f64 + dx).round() as i32,
            (y + height) as i32,
            image_dimensions,
            min,
        ),
        CropDragMode::ResizeSw => resize_from_edges(
            (x as f64 + dx).round() as i32,
            y as i32,
            (x + width) as i32,
            (y as f64 + height as f64 + dy).round() as i32,
            image_dimensions,
            min,
        ),
        CropDragMode::ResizeSe => resize_from_edges(
            x as i32,
            y as i32,
            (x as f64 + width as f64 + dx).round() as i32,
            (y as f64 + height as f64 + dy).round() as i32,
            image_dimensions,
            min,
        ),
    }
}

fn resize_from_edges(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    image_dimensions: (u32, u32),
    min_size: u32,
) -> (u32, u32, u32, u32) {
    let (image_width, image_height) = image_dimensions;
    let left = left.clamp(0, image_width.saturating_sub(min_size) as i32);
    let top = top.clamp(0, image_height.saturating_sub(min_size) as i32);
    let right = right.clamp(left + min_size as i32, image_width as i32);
    let bottom = bottom.clamp(top + min_size as i32, image_height as i32);
    (
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    )
}

fn index_for_media_id(media_list: &gtk::gio::ListStore, media_id: MediaId) -> Option<u32> {
    for index in 0..media_list.n_items() {
        let Some(obj) = media_list.item(index) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<MediaItem>().id == media_id.get() {
            return Some(index);
        }
    }
    None
}

/// Pure calculation: compute the initial `[start, end)` window centred on
/// `current`. The window is bounded by `[0, n_items)` and clips at the album
/// ends (no negative or out-of-bounds indices).
#[cfg(test)]
fn compute_initial_thumb_window(current: u32, n_items: u32) -> (u32, u32) {
    compute_initial_thumb_window_for_len(current, n_items, THUMB_DEFAULT_WINDOW_LEN)
}

fn compute_initial_thumb_window_for_len(current: u32, n_items: u32, target_len: u32) -> (u32, u32) {
    if n_items == 0 {
        return (0, 0);
    }
    let target_len = target_len.clamp(1, THUMB_WINDOW_MAX).min(n_items);
    let left_half = target_len / 2;
    let mut start = current.saturating_sub(left_half);
    let mut end = start.saturating_add(target_len).min(n_items);
    start = end.saturating_sub(target_len);
    end = start.saturating_add(target_len).min(n_items);
    (start, end)
}

fn save_result_closes_editor(kind: SaveResultKind) -> bool {
    kind == SaveResultKind::Success
}

/// Pure calculation: extend `[current_start, current_end)` by `THUMB_LAZY_HALF`
/// in `direction` (`-1` = prepend on the left, `+1` = append on the right).
/// Returns `None` when there's nothing to extend (already at album edge, or
/// the `THUMB_WINDOW_MAX` cap is reached).
fn compute_extended_thumb_window(
    direction: i8,
    current_start: u32,
    current_end: u32,
    n_items: u32,
    current_items_len: usize,
) -> Option<(u32, u32)> {
    debug_assert!(
        direction == -1 || direction == 1,
        "compute_extended_thumb_window: direction must be -1 or 1, got {direction}"
    );
    if current_items_len >= THUMB_WINDOW_MAX as usize {
        return None;
    }
    if direction < 0 {
        let new_start = current_start.saturating_sub(THUMB_LAZY_HALF);
        if new_start == current_start {
            return None;
        }
        Some((new_start, current_end))
    } else {
        let new_end = current_end.saturating_add(THUMB_LAZY_HALF).min(n_items);
        if new_end == current_end {
            return None;
        }
        Some((current_start, new_end))
    }
}

fn compute_current_thumb_extend_direction(
    current: u32,
    start: u32,
    end: u32,
    n_items: u32,
    current_items_len: usize,
) -> Option<i8> {
    if current < start
        || current >= end
        || current_items_len >= THUMB_WINDOW_MAX as usize
        || end <= start
    {
        return None;
    }

    let left_remaining = current.saturating_sub(start);
    let right_remaining = end.saturating_sub(current).saturating_sub(1);

    let wants_left = left_remaining <= THUMB_LAZY_HALF && start > 0;
    let wants_right = right_remaining <= THUMB_LAZY_HALF && end < n_items;

    match (wants_left, wants_right) {
        (true, true) if left_remaining <= right_remaining => Some(-1),
        (true, true) => Some(1),
        (true, false) => Some(-1),
        (false, true) => Some(1),
        (false, false) => None,
    }
}

fn format_dimensions(width: Option<u32>, height: Option<u32>) -> String {
    match (width, height) {
        (Some(width), Some(height)) => format!("{width} x {height}"),
        _ => tr("viewer.not_available"),
    }
}

fn format_file_size(size: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let size = size as f64;

    if size >= GB {
        format!("{:.1} GB", size / GB)
    } else if size >= MB {
        format!("{:.1} MB", size / MB)
    } else if size >= KB {
        format!("{:.1} KB", size / KB)
    } else {
        format!("{size:.0} B")
    }
}

fn format_datetime(value: Option<chrono::DateTime<Utc>>) -> String {
    value
        .map(|dt| {
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| tr("viewer.not_available"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use glib::value::ToValue;
    use std::cell::Cell;

    // ── filmstrip window calculations ──────────────────────────────────

    #[test]
    fn save_result_closes_editor_only_on_success() {
        assert!(save_result_closes_editor(SaveResultKind::Success));
        assert!(!save_result_closes_editor(SaveResultKind::Error));
    }

    #[test]
    fn initial_window_centred_on_current_in_middle_of_album() {
        // 100 photos, current = 50 → ±5 items centred, no clipping.
        let (start, end) = compute_initial_thumb_window(50, 100);
        assert_eq!(start, 45);
        assert_eq!(end, 56);
        assert_eq!(end - start, THUMB_DEFAULT_WINDOW_LEN);
    }

    #[test]
    fn initial_window_clips_at_album_start() {
        // current near 0 → start clamped to 0, missing left-side items are
        // backfilled on the right so the strip still has a full window.
        let (start, end) = compute_initial_thumb_window(2, 100);
        assert_eq!(start, 0);
        assert_eq!(end, THUMB_DEFAULT_WINDOW_LEN);
        assert!(end > 2);
    }

    #[test]
    fn initial_window_clips_at_album_end() {
        // current near the end → end clamped to n_items, with a full window
        // backfilled on the left when enough items exist.
        let n = 100u32;
        let current = n - 2;
        let (start, end) = compute_initial_thumb_window(current, n);
        assert_eq!(end, n);
        assert_eq!(end - start, THUMB_DEFAULT_WINDOW_LEN);
        assert!(start <= current);
    }

    #[test]
    fn initial_window_is_empty_for_empty_album() {
        assert_eq!(compute_initial_thumb_window(0, 0), (0, 0));
        assert_eq!(compute_initial_thumb_window(5, 0), (0, 0));
    }

    #[test]
    fn extend_left_grows_window_without_changing_end() {
        // 100 photos, window [30, 40], extend left by LAZY_HALF.
        let (new_start, new_end) = compute_extended_thumb_window(-1, 30, 40, 100, 10).unwrap();
        assert_eq!(new_start, 30 - THUMB_LAZY_HALF);
        assert_eq!(new_end, 40);
    }

    #[test]
    fn extend_right_grows_window_without_changing_start() {
        let (new_start, new_end) = compute_extended_thumb_window(1, 30, 40, 100, 10).unwrap();
        assert_eq!(new_start, 30);
        assert_eq!(new_end, 40 + THUMB_LAZY_HALF);
    }

    #[test]
    fn extend_left_returns_none_at_album_start() {
        // Already at 0, can't go further left.
        assert!(compute_extended_thumb_window(-1, 0, 10, 100, 10).is_none());
    }

    #[test]
    fn extend_right_returns_none_at_album_end() {
        // Window already touches the end of the album.
        assert!(compute_extended_thumb_window(1, 90, 100, 100, 10).is_none());
    }

    #[test]
    fn extend_returns_none_at_window_cap() {
        // Already at the cap, regardless of direction.
        assert!(
            compute_extended_thumb_window(-1, 50, 90, 100, THUMB_WINDOW_MAX as usize).is_none()
        );
        assert!(compute_extended_thumb_window(1, 50, 90, 100, THUMB_WINDOW_MAX as usize).is_none());
    }

    #[test]
    fn extend_left_clamps_to_zero_not_negative() {
        // start is small but non-zero → new_start must not underflow.
        let (new_start, _) = compute_extended_thumb_window(-1, 2, 12, 100, 10).unwrap();
        assert_eq!(new_start, 0);
    }

    #[test]
    fn extend_right_clamps_to_n_items() {
        let (_, new_end) = compute_extended_thumb_window(1, 92, 99, 100, 10).unwrap();
        assert_eq!(new_end, 100);
    }

    #[test]
    fn current_near_right_edge_triggers_right_thumb_extend() {
        assert_eq!(
            compute_current_thumb_extend_direction(7, 0, 11, 100, 11),
            Some(1),
            "current at offset 7 leaves only 3 thumbnails to the right, so preload more"
        );
    }

    #[test]
    fn current_near_left_edge_triggers_left_thumb_extend() {
        assert_eq!(
            compute_current_thumb_extend_direction(23, 20, 31, 100, 11),
            Some(-1),
            "current at offset 3 leaves only 3 thumbnails to the left, so preload more"
        );
    }

    #[test]
    fn current_in_middle_does_not_extend_thumb_window() {
        assert_eq!(
            compute_current_thumb_extend_direction(50, 45, 56, 100, 11),
            None
        );
    }

    #[test]
    fn current_edge_extend_respects_album_edges_and_window_cap() {
        assert_eq!(
            compute_current_thumb_extend_direction(2, 0, 11, 100, 11),
            None,
            "near the left edge cannot extend when already at album start"
        );
        assert_eq!(
            compute_current_thumb_extend_direction(97, 89, 100, 100, 11),
            None,
            "near the right edge cannot extend when already at album end"
        );
        assert_eq!(
            compute_current_thumb_extend_direction(57, 50, 90, 100, THUMB_WINDOW_MAX as usize),
            None,
            "window cap still prevents eager extension"
        );
    }

    #[test]
    fn initial_window_total_item_count_matches_docstring() {
        // Regression: the fallback count remains 11 when no viewport
        // allocation is available yet.
        for n in [11u32, 100, 1000] {
            let current = n / 2;
            let (start, end) = compute_initial_thumb_window(current, n);
            let actual = end - start;
            assert!(actual <= THUMB_DEFAULT_WINDOW_LEN, "n={n} actual={actual}");
        }
    }

    // ── scroll-to-current adjustment calculation ────────────────────────

    /// Reasonable layout: page_size=300, 11 items, button width=60,
    /// spacing=6. Total upper = 720.
    const SCROLL_PAGE_SIZE: f64 = 300.0;
    const SCROLL_BTN_W: f64 = 60.0;
    const SCROLL_SPACING: f64 = 6.0;
    const SCROLL_UPPER: f64 = 720.0;

    #[test]
    fn residual_centres_first_thumbnail_without_layout_padding() {
        let (value, residual) =
            compute_thumb_scroll_and_residual(0.0, SCROLL_BTN_W, SCROLL_PAGE_SIZE, SCROLL_UPPER);
        assert_eq!(value, 0.0);
        assert!(residual > 0.0);
        assert!(
            (0.0 + SCROLL_BTN_W / 2.0 - value + residual - SCROLL_PAGE_SIZE / 2.0).abs() < 0.5,
            "first thumbnail center should align with viewport center without layout padding"
        );
    }

    #[test]
    fn residual_is_suppressed_when_content_does_not_exceed_viewport() {
        let (value, residual) = compute_thumb_scroll_and_residual(
            0.0,
            SCROLL_BTN_W,
            SCROLL_PAGE_SIZE,
            SCROLL_PAGE_SIZE,
        );
        assert_eq!(value, 0.0);
        assert_eq!(residual, 0.0);
    }

    #[test]
    fn visual_transform_uses_css_offset_when_adjustment_has_no_scroll_range() {
        assert_eq!(
            compute_thumb_visual_transform(240.0, 0.0, 300.0, 300.0),
            -240.0
        );
    }

    #[test]
    fn visual_transform_uses_only_residual_when_adjustment_can_scroll() {
        assert_eq!(
            compute_thumb_visual_transform(240.0, 12.0, 720.0, 300.0),
            12.0
        );
    }

    #[test]
    fn crop_overlay_contain_rect_centers_letterboxed_image() {
        let rect = compute_contained_image_rect(1000.0, 500.0, (400, 300)).unwrap();

        assert!((rect.x - 166.666).abs() < 0.01);
        assert_eq!(rect.y, 0.0);
        assert!((rect.width - 666.666).abs() < 0.01);
        assert_eq!(rect.height, 500.0);
    }

    #[test]
    fn crop_overlay_drag_move_clamps_to_image_bounds() {
        let drag = CropDragState {
            mode: CropDragMode::Move,
            rect: (300, 220, 100, 80),
        };

        assert_eq!(drag_rect(drag, 60.0, 60.0, (400, 300)), (300, 220, 100, 80));
        assert_eq!(
            drag_rect(drag, -40.0, -20.0, (400, 300)),
            (260, 200, 100, 80)
        );
    }

    #[test]
    fn crop_overlay_drag_corner_resizes_rect() {
        let drag = CropDragState {
            mode: CropDragMode::ResizeSe,
            rect: (50, 60, 120, 90),
        };

        assert_eq!(drag_rect(drag, 30.0, 20.0, (400, 300)), (50, 60, 150, 110));
    }

    #[test]
    fn positioning_centres_current_when_content_is_narrower_than_viewport() {
        let (target, residual, transform) =
            compute_thumb_positioning(-10.0, 56.0, 7081.0, 7081.0, 2826.0);
        assert_eq!(target, 0.0);
        assert_eq!(residual, transform);
        assert!(
            (-10.0 + 56.0 / 2.0 + transform - 7081.0 / 2.0).abs() < 0.5,
            "current thumbnail should be visually centred even when the loaded strip is narrower than the viewport"
        );
    }

    #[test]
    fn filmstrip_thumbnail_width_is_clamped_to_reasonable_aspect_ratio() {
        assert_eq!(clamped_thumb_width_for_texture(2100, 900), 131);
        assert_eq!(clamped_thumb_width_for_texture(4200, 900), 131);
        assert_eq!(clamped_thumb_width_for_texture(900, 2100), 36);
        assert_eq!(clamped_thumb_width_for_texture(900, 900), 56);
        assert_eq!(clamped_thumb_width_for_texture(0, 900), 36);
    }

    #[test]
    fn item_geometry_uses_sequence_when_current_allocation_x_is_stale() {
        let widths = [118.0, 118.0, 118.0, 118.0, 112.0, 118.0, 118.0];
        let (content_x, width, content_width) =
            thumb_item_content_geometry(&widths, 4, THUMB_STRIP_SPACING).unwrap();

        assert_eq!(content_x, 4.0 * (118.0 + THUMB_STRIP_SPACING));
        assert_eq!(width, 112.0);
        assert_eq!(
            content_width,
            widths.iter().sum::<f64>() + (widths.len() - 1) as f64 * THUMB_STRIP_SPACING
        );

        let (_, _, transform) = compute_thumb_positioning(content_x, width, 1140.0, 1140.0, 1140.0);
        assert!(
            transform.abs() < 100.0,
            "current index 4 must not be positioned from a stale allocation x near the left edge"
        );
    }

    #[test]
    fn scroll_value_centres_middle_thumbnail_without_residual() {
        let middle_btn_x = 5.0 * (SCROLL_BTN_W + SCROLL_SPACING);
        let (value, residual) = compute_thumb_scroll_and_residual(
            middle_btn_x,
            SCROLL_BTN_W,
            SCROLL_PAGE_SIZE,
            SCROLL_UPPER,
        );
        assert_eq!(residual, 0.0);
        assert!(
            (middle_btn_x + SCROLL_BTN_W / 2.0 - value + residual - SCROLL_PAGE_SIZE / 2.0).abs()
                < 0.5,
            "middle thumbnail center should align with viewport center after scrolling"
        );
    }

    #[test]
    fn residual_centres_last_thumbnail_without_layout_padding() {
        let last_btn_x = 10.0 * (SCROLL_BTN_W + SCROLL_SPACING);
        let (value, residual) = compute_thumb_scroll_and_residual(
            last_btn_x,
            SCROLL_BTN_W,
            SCROLL_PAGE_SIZE,
            SCROLL_UPPER,
        );
        assert_eq!(value, SCROLL_UPPER - SCROLL_PAGE_SIZE);
        assert!(residual < 0.0);
        assert!(
            (last_btn_x + SCROLL_BTN_W / 2.0 - value + residual - SCROLL_PAGE_SIZE / 2.0).abs()
                < 0.5,
            "last thumbnail center should align with viewport center without layout padding"
        );
    }

    #[test]
    fn thumb_centering_retries_until_allocation_is_ready() {
        assert!(
            should_retry_thumb_centering(false, 3),
            "initial viewer entry can run before thumbnail allocation; it must retry"
        );
        assert!(
            !should_retry_thumb_centering(true, 3),
            "successful centering should stop the tick callback"
        );
        assert!(
            !should_retry_thumb_centering(false, 0),
            "retry loop must have a hard stop"
        );
    }

    #[gtk::test]
    fn thumb_strip_template_starts_without_layout_spacers() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

        assert!(
            !viewer.imp().thumb_scrolled.get().propagates_natural_width(),
            "thumb scroller must not propagate the filmstrip child width into the viewer window"
        );
        let (hpolicy, vpolicy) = viewer.imp().thumb_scrolled.get().policy();
        assert_eq!(
            hpolicy,
            gtk::PolicyType::External,
            "thumb scroller needs a horizontal adjustment but no visible scrollbar; Never lets child width resize the viewer"
        );
        assert_eq!(
            vpolicy,
            gtk::PolicyType::Never,
            "thumb scroller should not expose vertical scrolling"
        );
        let bottom_bar = viewer
            .imp()
            .thumb_scrolled
            .get()
            .parent()
            .expect("thumb scroller should be inside viewer bottom bar");
        let bottom_bar_classes = bottom_bar.css_classes();
        assert!(
            bottom_bar_classes
                .iter()
                .any(|class| class == "viewer-thumb-bar"),
            "thumb scroller parent should keep the viewer-thumb-bar layout class"
        );
        assert!(
            !bottom_bar_classes
                .iter()
                .any(|class| class == "glass-raised"),
            "viewer thumbnail strip should not render a raised glass background bar"
        );
        assert!(
            viewer
                .imp()
                .thumb_strip
                .get()
                .css_classes()
                .iter()
                .any(|class| class == "viewer-thumb-strip"),
            "thumb_strip must carry viewer-thumb-strip so CSS can suppress natural-width growth"
        );

        let strip = viewer.imp().thumb_strip.get();
        let mut count = 0;
        let mut child = strip.first_child();
        while let Some(widget) = child {
            count += 1;
            child = widget.next_sibling();
        }

        assert_eq!(
            count, 0,
            "thumb_strip must not contain template spacer children because viewport-sized children feed back into ScrolledWindow allocation"
        );
    }

    #[gtk::test]
    fn editing_hides_overlay_navigation_buttons() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        let nav_container = viewer
            .imp()
            .prev_btn
            .get()
            .parent()
            .expect("prev button should live inside the overlay nav container");

        assert!(
            nav_container.is_visible(),
            "overlay navigation should be visible before editing"
        );

        viewer.start_editing();
        assert!(
            !nav_container.is_visible(),
            "opening the editor should hide previous/next overlay navigation"
        );

        viewer.stop_editing();
        assert!(
            nav_container.is_visible(),
            "closing the editor should restore previous/next overlay navigation"
        );
    }

    #[test]
    fn zoom_step_clamps_to_viewer_limits() {
        assert_eq!(step_zoom(1.0, 1), 1.25);
        assert_eq!(step_zoom(1.25, -1), 1.0);
        assert_eq!(step_zoom(7.9, 1), MAX_VIEWER_ZOOM);
        assert_eq!(step_zoom(MIN_VIEWER_ZOOM, -1), MIN_VIEWER_ZOOM);
    }

    #[test]
    fn pinch_zoom_uses_gesture_origin_instead_of_compounding_updates() {
        assert_eq!(pinch_zoom_from_origin(2.0, 1.1), 2.2);
        assert_eq!(
            pinch_zoom_from_origin(2.0, 1.2),
            2.4,
            "a second pinch update should still use the gesture origin, not compound 2.2 * 1.2"
        );
    }

    #[test]
    fn zoom_pan_is_clamped_and_resets_at_identity() {
        assert_eq!(
            clamp_zoom_pan(1.0, 120.0, -80.0, 1000.0, 700.0),
            (0.0, 0.0),
            "identity zoom should never keep a drag offset"
        );
        assert_eq!(
            clamp_zoom_pan(2.0, 800.0, -500.0, 1000.0, 700.0),
            (500.0, -350.0),
            "zoomed images should pan only across the extra visible area"
        );
    }

    #[gtk::test]
    fn zoom_controls_live_in_top_right_with_reset_out_decrease_increase_order() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        let imp = viewer.imp();

        let zoom_parent = imp
            .zoom_in_btn
            .get()
            .parent()
            .expect("zoom buttons should live inside a control container");
        assert!(
            zoom_parent
                .css_classes()
                .iter()
                .any(|class| class == "viewer-zoom-controls"),
            "zoom buttons need a distinct overlay container so they do not disturb prev/next layout"
        );
        assert_eq!(
            zoom_parent.halign(),
            gtk::Align::End,
            "zoom controls should sit at the image area's top-right edge"
        );
        assert_eq!(
            zoom_parent.valign(),
            gtk::Align::Start,
            "zoom controls should sit at the image area's top-right edge"
        );

        assert_eq!(
            zoom_parent.first_child(),
            Some(imp.zoom_reset_btn.get().upcast::<gtk::Widget>()),
            "zoom controls should start with reset"
        );
        assert_eq!(
            imp.zoom_reset_btn.get().next_sibling(),
            Some(imp.zoom_out_btn.get().upcast::<gtk::Widget>()),
            "zoom-out should follow reset"
        );
        assert_eq!(
            imp.zoom_out_btn.get().next_sibling(),
            Some(imp.zoom_in_btn.get().upcast::<gtk::Widget>()),
            "zoom-in should follow zoom-out"
        );

        for (name, button) in [
            ("zoom_in_btn", imp.zoom_in_btn.get()),
            ("zoom_out_btn", imp.zoom_out_btn.get()),
            ("zoom_reset_btn", imp.zoom_reset_btn.get()),
        ] {
            assert!(
                button
                    .css_classes()
                    .iter()
                    .any(|class| class == "glass-toolbar-button"),
                "{name} should reuse the existing viewer glass button treatment"
            );
        }

        assert_eq!(
            imp.zoom_reset_btn.get().icon_name().as_deref(),
            Some("zoom-fit-best-symbolic"),
            "reset should use the fit-to-view icon"
        );
        assert!(imp.zoom_in_btn.get().is_visible());
        assert!(!imp.zoom_out_btn.get().is_visible());
        assert!(!imp.zoom_reset_btn.get().is_visible());

        viewer.set_viewer_zoom_for_tests(1.25, 0.0, 0.0);
        assert!(imp.zoom_in_btn.get().is_visible());
        assert!(imp.zoom_out_btn.get().is_visible());
        assert!(imp.zoom_reset_btn.get().is_visible());
    }

    #[gtk::test]
    fn reset_zoom_restores_identity_state() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

        viewer.set_viewer_zoom_for_tests(2.0, 100.0, -50.0);
        viewer.imp().zoom_reset_btn.get().emit_clicked();

        assert_eq!(viewer.imp().zoom_scale.get(), 1.0);
        assert_eq!(viewer.imp().zoom_pan_x.get(), 0.0);
        assert_eq!(viewer.imp().zoom_pan_y.get(), 0.0);
    }

    fn sample_media_item() -> MediaItem {
        MediaItem {
            id: 1,
            uri: "file:///tmp/sample.jpg".into(),
            path: PathBuf::from("/tmp/sample.jpg"),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(64),
            height: Some(48),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1024,
            blake3_hash: "hash".into(),
            is_favorite: false,
            trashed_at: None,
        }
    }

    fn init_viewer_test() {
        let _ = gtk::init();
        crate::ui::grid_css::install();
    }

    #[gtk::test]
    fn escape_closes_details_panel_without_navigation_pop() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        viewer.imp().details_split_view.get().set_show_sidebar(true);

        let nav_pop_fired = Rc::new(Cell::new(false));
        let nav_pop_fired_for_cb = nav_pop_fired.clone();
        viewer.connect_navigation(move |delta| {
            if delta == NAV_POP {
                nav_pop_fired_for_cb.set(true);
            }
        });

        let key_ctrl = viewer
            .imp()
            .picture
            .get()
            .observe_controllers()
            .snapshot()
            .into_iter()
            .find_map(|controller| controller.downcast::<gtk::EventControllerKey>().ok())
            .expect("viewer picture should have a key controller");
        let args: &[&dyn ToValue] = &[&gdk::Key::Escape, &0u32, &gdk::ModifierType::empty()];
        let handled: bool = key_ctrl.emit_by_name("key-pressed", args);

        assert!(
            handled,
            "Escape should be consumed when details are visible"
        );
        assert!(
            !viewer.imp().details_split_view.get().shows_sidebar(),
            "Escape should close only the details panel"
        );
        assert!(
            !nav_pop_fired.get(),
            "Escape while details are visible must not pop the viewer page"
        );
    }

    #[gtk::test]
    fn close_details_button_keeps_viewer_page_visible() {
        init_viewer_test();
        let nav = adw::NavigationView::new();
        let root = adw::NavigationPage::builder()
            .title("Root")
            .child(&gtk::Label::new(Some("root")))
            .build();
        nav.push(&root);

        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        nav.push(&viewer);
        viewer.imp().details_split_view.get().set_show_sidebar(true);

        viewer.imp().details_close_btn.get().emit_clicked();

        assert!(
            !viewer.imp().details_split_view.get().shows_sidebar(),
            "details close button should hide only the details panel"
        );
        assert_eq!(
            nav.visible_page().map(|page| page.title()).as_deref(),
            Some(viewer.title().as_str()),
            "details close button must not pop the viewer page"
        );
    }

    #[gtk::test]
    fn navigation_pop_closes_details_before_leaving_viewer() {
        init_viewer_test();
        let nav = adw::NavigationView::new();
        let root = adw::NavigationPage::builder()
            .title("Root")
            .child(&gtk::Label::new(Some("root")))
            .build();
        nav.push(&root);

        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        nav.push(&viewer);
        viewer.imp().details_split_view.get().set_show_sidebar(true);

        let _ = viewer.activate_action("navigation.pop", None);

        assert!(
            !viewer.imp().details_split_view.get().shows_sidebar(),
            "navigation pop should first close the details panel"
        );
        assert_eq!(
            nav.visible_page().map(|page| page.title()).as_deref(),
            Some(viewer.title().as_str()),
            "navigation pop while details are visible must not leave viewer"
        );
    }

    #[gtk::test]
    fn details_panel_temporarily_disables_navigation_pop() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

        assert!(
            viewer.can_pop(),
            "viewer should normally allow navigation pop"
        );

        viewer.set_details_revealed(true, "test-open");
        assert!(
            !viewer.can_pop(),
            "opening details should disable NavigationView built-in pop"
        );

        viewer.set_details_revealed(false, "test-close");
        assert!(
            !viewer.can_pop(),
            "closing details should keep pop disabled during the close animation"
        );

        let ctx = glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(900);
        while std::time::Instant::now() < deadline && !viewer.can_pop() {
            ctx.iteration(true);
        }

        assert!(
            viewer.can_pop(),
            "viewer should allow navigation pop again after the guard delay"
        );
    }

    #[test]
    fn next_index_after_deleted_item_stays_in_bounds() {
        assert_eq!(next_index_after_deleted_item(0, 2), Some(0));
        assert_eq!(next_index_after_deleted_item(1, 2), Some(1));
        assert_eq!(next_index_after_deleted_item(2, 2), Some(1));
        assert_eq!(next_index_after_deleted_item(0, 0), None);
    }

    #[gtk::test]
    fn find_media_index_by_id_uses_item_identity() {
        let _ = gtk::init();
        let list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let mut first = sample_media_item();
        first.id = 10;
        let mut second = sample_media_item();
        second.id = 20;
        list.append(&glib::BoxedAnyObject::new(first));
        list.append(&glib::BoxedAnyObject::new(second));

        assert_eq!(find_media_index_by_id(&list, 20), Some(1));
        assert_eq!(find_media_index_by_id(&list, 30), None);
    }

    #[gtk::test]
    fn video_audio_preferences_are_applied_to_media_stream() {
        let _ = gtk::init();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.mp4");
        std::fs::write(&path, b"fake mp4").unwrap();
        let stream = gtk::MediaFile::for_filename(&path);

        apply_video_audio_preferences_to_stream(&stream, true, 0.42);

        assert!(stream.is_muted(), "video should respect default muted pref");
        assert_eq!(stream.volume(), 0.42);
    }
}
