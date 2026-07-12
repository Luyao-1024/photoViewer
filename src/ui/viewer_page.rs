//! ViewerPage — media viewer with preloading.
//!
//! `ViewerPage` is pushed onto the `AdwNavigationView` when the user clicks a
//! `PhotoTile`. It decodes the **original** image (no thumbnail pipeline) for
//! the current item, plus preloads the ±1 neighbours so panning feels
//! reasonably snappy. Keyboard interaction is routed through the main-window
//! keyboard subsystem.
//!
//! Note: items in the `gio::ListStore` are `BoxedAnyObject<MediaItem>` (see
//! M1-T10 / `app::initialize`). We unwrap via `BoxedAnyObject::borrow` rather
//! than `downcast::<MediaItem>()`.
#[path = "viewer/actions.rs"]
mod actions;
#[path = "viewer/crop.rs"]
mod crop;
#[path = "viewer/details.rs"]
mod details;
#[path = "viewer/editor.rs"]
mod editor;
#[path = "viewer/filmstrip.rs"]
mod filmstrip;
#[path = "viewer/fullscreen.rs"]
mod fullscreen;
#[path = "viewer/fullscreen_window.rs"]
mod fullscreen_window;
#[path = "viewer/navigation.rs"]
mod navigation;
#[path = "viewer/stage.rs"]
mod stage;
#[path = "viewer/transform.rs"]
mod transform;

use crate::core::db::DbPool;
use crate::core::db_actor::DbActorHandle;
use crate::core::i18n::{tr, trf};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::prefs;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::editor_panel::{CropOverlayUpdate, EditorPanel};
use crate::ui::keyboard::{KeyboardAction, KeyboardResult};
use crate::ui::toasts;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{ActionRowExt, NavigationPageExt};
use libadwaita::subclass::prelude::*;
use std::cell::{Cell, RefCell};
#[cfg(test)]
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crop::CropDragState;
use navigation::{find_media_index_by_id, index_for_media_id};
use stage::{should_play_animated_image, strip_file_uri};
type FavoriteStateCallback = Rc<dyn Fn(i64, bool)>;

const MIN_VIEWER_ZOOM: f64 = 1.0;
const MAX_VIEWER_ZOOM: f64 = 8.0;
const VIEWER_ZOOM_STEP: f64 = 1.25;
const VIEWER_FULLSCREEN_ICON: &str = "view-fullscreen-symbolic";

/// Direction hint the host receives from keyboard input. `i32::MIN` is the
/// "pop navigation" sentinel; other values are a delta on the current index.
pub type NavDelta = i32;
pub const NAV_POP: NavDelta = i32::MIN;

pub(crate) const VIEWER_OPEN_POP_GUARD_MS: u64 = 350;

/// Callback the host registers for keyboard navigation. Shared via `Rc` so
/// closures capturing owned state can be cloned into GTK signal handlers.
pub type NavCallback = Rc<dyn Fn(NavDelta)>;
type ItemCallback = Rc<dyn Fn(i64)>;

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;

    pub(super) fn sample_media_item() -> MediaItem {
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

    pub(super) fn init_viewer_test() {
        let _ = gtk::init();
        crate::ui::grid_css::install();
    }

    pub(super) fn test_texture() -> gdk::Texture {
        let bytes = glib::Bytes::from_owned(vec![255_u8, 0, 0, 255]);
        gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::R8g8b8a8, &bytes, 4).upcast()
    }

    pub(super) fn widget_tree_has_class<W: IsA<gtk::Widget>>(widget: &W, class_name: &str) -> bool {
        let widget = widget.as_ref();
        if widget.css_classes().iter().any(|class| class == class_name) {
            return true;
        }

        let mut child = widget.first_child();
        while let Some(current) = child {
            if widget_tree_has_class(&current, class_name) {
                return true;
            }
            child = current.next_sibling();
        }
        false
    }

    pub(super) fn widget_tree_has_button_icon<W: IsA<gtk::Widget>>(
        widget: &W,
        icon_name: &str,
    ) -> bool {
        let widget = widget.as_ref();
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.icon_name().as_deref() == Some(icon_name) {
                return true;
            }
        }

        let mut child = widget.first_child();
        while let Some(current) = child {
            if widget_tree_has_button_icon(&current, icon_name) {
                return true;
            }
            child = current.next_sibling();
        }
        false
    }
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
        /// The `current_token` whose **original** full-resolution texture has
        /// already been painted. Lets the preview-thumbnail callback avoid
        /// overwriting the original with a late-arriving Medium thumbnail.
        ///
        /// For non-JPEG images (e.g. PNG screenshots) the thumbnail path
        /// decodes the full-resolution source before downscaling, so it can
        /// land *after* the (lighter) original decode and clobber the already-
        /// painted original — leaving the viewer permanently stuck on the
        /// thumbnail. JPEG thumbnails take the turbojpeg IDCT fast path and
        /// finish first, so only non-JPEG items hit this race. The thumbnail
        /// is a progressive placeholder; once the original is up for this
        /// token, a stale thumbnail must not repaint. `0` = no original yet.
        pub original_painted_token: Cell<u64>,
        /// Navigation coalescing token. Bumped on every left/right press so a
        /// stale in-flight prefetch (DB neighbour lookup or thumbnail wait)
        /// can be discarded when the user pressed again. 0 = no pending nav.
        pub nav_token: Cell<u64>,
        /// The `nav_token` whose deferred switch has already been settled
        /// (i.e. `show_at` ran). Prevents the thumb-ready and timeout
        /// fallback from both firing `show_at` for the same nav.
        pub nav_settled_token: Cell<u64>,
        /// `current_media_id` the neighbour cache was populated for. 0 = empty.
        /// Lets `navigate_by_delta` skip the DB neighbour query when the user
        /// presses again on the same item we prefetched during `show_at`.
        pub cached_neighbor_for_id: Cell<i64>,
        /// Prefetched +1 neighbour item (next), warmed by `prefetch_neighbors`.
        pub cached_next_item: RefCell<Option<MediaItem>>,
        /// Prefetched -1 neighbour item (previous), warmed by `prefetch_neighbors`.
        pub cached_prev_item: RefCell<Option<MediaItem>>,
        /// Cumulative zoom scale (1.0 = identity).
        pub zoom_scale: Cell<f64>,
        /// Viewer-local image rotation in clockwise degrees. This affects only
        /// the current on-screen transform and is never persisted.
        pub viewer_rotation_degrees: Cell<i32>,
        /// Viewer image pan offset in allocated widget pixels.
        pub zoom_pan_x: Cell<f64>,
        pub zoom_pan_y: Cell<f64>,
        /// Callback registered by the host (PhotosPage) for keyboard navigation.
        pub nav_cb: RefCell<Option<NavCallback>>,
        /// Callback fired after this viewer successfully moves an item to trash.
        pub trashed_cb: RefCell<Option<ItemCallback>>,
        /// Cached CssProvider reused by viewer-local zoom/rotation transforms.
        pub zoom_provider: RefCell<Option<gtk::CssProvider>>,
        /// Optional callback invoked whenever current media favorite state changes.
        pub favorite_state_cb: RefCell<Option<FavoriteStateCallback>>,
        /// DB pool injected by host (needed to construct the editor panel).
        pub pool: RefCell<Option<DbPool>>,
        /// Single-thread DB mutation actor used for precise refresh events.
        pub db_actor: RefCell<Option<DbActorHandle>>,
        /// Navigation view (kept for album picker push; editor no longer pushes).
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        /// Original texture saved before editing starts; restored on cancel.
        pub original_texture: RefCell<Option<gdk::Texture>>,
        /// Pending frame timer for animated image playback.
        pub animated_image_source: RefCell<Option<glib::SourceId>>,
        /// Previously playing video stream retained for one idle cycle after
        /// it is detached from the `GtkVideo`. Dropping the only reference
        /// synchronously (via `set_media_stream(NONE)`) finalized the
        /// `GtkMediaFile` while its GstPlay thread was still emitting
        /// state-changed signals, which crashed that thread with a
        /// use-after-free inside libgobject. See `stop_video_playback`.
        pub attached_video_media_id: Cell<i64>,
        pub retired_video_stream: RefCell<Option<gtk::MediaStream>>,
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
        /// Monotonic id for scheduled filmstrip centering callbacks. Used only
        /// to correlate jitter logs across rebuilds, thumbnail loads, and ticks.
        pub thumb_scroll_schedule_seq: Cell<u64>,
        /// Last visual transform applied to the filmstrip. Used only for logs.
        pub thumb_last_transform: Cell<f64>,
        /// True while a filmstrip centering tick callback is already queued.
        /// Coalesces bursts of thumbnail-loaded events into one layout read.
        pub thumb_scroll_scheduled: Cell<bool>,
        /// Monotonic id for animated adjustment moves. A newer target cancels
        /// any previous filmstrip scroll animation on its next frame.
        pub thumb_scroll_animation_seq: Cell<u64>,
        pub crop_overlay_active: Cell<bool>,
        pub crop_overlay_selected: Cell<bool>,
        pub crop_overlay_rect: Cell<Option<(u32, u32, u32, u32)>>,
        pub crop_overlay_dimensions: Cell<(u32, u32)>,
        pub(super) crop_drag: RefCell<Option<CropDragState>>,
        pub fullscreen_preview_window: RefCell<Option<gtk::Window>>,
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub date_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub details_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub details_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub fullscreen_btn: TemplateChild<gtk::Button>,
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
        pub video_error_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub video_error_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub video_error_subtitle: TemplateChild<gtk::Label>,
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
        pub name_entry: TemplateChild<gtk::Entry>,
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
        pub viewer_bottom_stack: TemplateChild<gtk::Box>,
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
        pub rotate_left_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub rotate_right_btn: TemplateChild<gtk::Button>,
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
        obj.apply_i18n();
        obj.setup_zoom_controls();
        obj.setup_zoom_transform_provider();
        obj.setup_video_playback_interactions();
        obj.setup_edit_button();
        obj.setup_editor_callbacks();
        obj.setup_crop_overlay();
        obj.setup_delete_button();
        obj.setup_details_panel();
        obj.setup_favorite_button();
        obj.setup_fullscreen_button();
        obj.setup_nav_buttons();
        obj.setup_motion_play_button();
        obj.setup_thumb_strip_listener();
        obj.setup_navigation_pop_action();
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
        imp.fullscreen_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.fullscreen")));
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
        imp.rotate_left_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.rotate_left")));
        imp.rotate_right_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.rotate_right")));
        imp.motion_play_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.play_motion_photo")));
        imp.video_error_title
            .get()
            .set_label(&tr("viewer.video_error.title"));
        imp.video_error_subtitle
            .get()
            .set_label(&tr("viewer.video_error.subtitle"));
    }

    /// Inject the `AdwNavigationView` and DB pool used to push an
    /// the editor panel when the Edit button is pressed. Call this after
    /// construction (mirrors `PhotosPage::set_nav_target`).
    pub fn set_edit_target(&self, nav: &adw::NavigationView, pool: DbPool) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    pub fn set_db_actor(&self, db_actor: DbActorHandle) {
        *self.imp().db_actor.borrow_mut() = Some(db_actor.clone());
        self.imp().editor_panel.get().set_db_actor(db_actor);
    }

    /// Register a callback fired when the user presses ArrowLeft / ArrowRight /
    /// Escape. The callback receives the requested action: -1 / +1 / pop.
    pub fn connect_navigation<F: Fn(NavDelta) + 'static>(&self, f: F) {
        *self.imp().nav_cb.borrow_mut() = Some(Rc::new(f));
    }

    pub(crate) fn is_editing_keyboard_scope(&self) -> bool {
        self.imp().is_editing.get() || self.imp().editor_split_view.get().shows_sidebar()
    }

    pub(crate) fn handle_keyboard_action(&self, action: KeyboardAction) -> KeyboardResult {
        match action {
            KeyboardAction::ViewerNext => {
                if !self.is_editing_keyboard_scope() {
                    self.navigate_by_delta(1);
                }
                KeyboardResult::Handled
            }
            KeyboardAction::ViewerPrevious => {
                if !self.is_editing_keyboard_scope() {
                    self.navigate_by_delta(-1);
                }
                KeyboardResult::Handled
            }
            KeyboardAction::CancelOrClose => {
                if self.imp().editor_split_view.get().shows_sidebar() {
                    self.stop_editing();
                } else if self.imp().details_split_view.get().shows_sidebar() {
                    self.set_details_revealed(false, "keyboard action");
                } else if !self.can_pop() {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "ViewerPage: ignoring keyboard close while navigation pop is guarded"
                    );
                } else {
                    self.fire_nav(NAV_POP);
                }
                KeyboardResult::Handled
            }
            KeyboardAction::ViewerTogglePlayback => {
                if self.toggle_video_playback() {
                    KeyboardResult::Handled
                } else {
                    KeyboardResult::Ignored
                }
            }
            KeyboardAction::ViewerZoomIn => self.handle_image_keyboard_action(|this| {
                this.step_viewer_zoom(1);
            }),
            KeyboardAction::ViewerZoomOut => self.handle_image_keyboard_action(|this| {
                this.step_viewer_zoom(-1);
            }),
            KeyboardAction::ViewerZoomReset => self.handle_image_keyboard_action(|this| {
                this.reset_viewer_zoom();
            }),
            KeyboardAction::ViewerRotateLeft => self.handle_image_keyboard_action(|this| {
                this.rotate_viewer_image(-90);
            }),
            KeyboardAction::ViewerRotateRight => self.handle_image_keyboard_action(|this| {
                this.rotate_viewer_image(90);
            }),
            KeyboardAction::ViewerFullscreenPreview => self.handle_image_keyboard_action(|this| {
                this.open_fullscreen_preview_window();
            }),
            KeyboardAction::ViewerToggleDetails => {
                let next = !self.imp().details_split_view.get().shows_sidebar();
                self.set_details_revealed(next, "keyboard action");
                if next {
                    if let Some(item) = self.current_media_item() {
                        self.update_details(&item);
                    }
                }
                KeyboardResult::Handled
            }
            KeyboardAction::ViewerToggleEdit => {
                if self.imp().edit_btn.get().is_sensitive() {
                    self.imp().edit_btn.get().emit_clicked();
                    KeyboardResult::Handled
                } else {
                    KeyboardResult::Ignored
                }
            }
            KeyboardAction::ViewerToggleFavorite => {
                self.imp().favorite_btn.get().emit_clicked();
                KeyboardResult::Handled
            }
            KeyboardAction::Delete => {
                self.imp().delete_btn.get().emit_clicked();
                KeyboardResult::Handled
            }
            _ => KeyboardResult::Ignored,
        }
    }

    fn handle_image_keyboard_action<F>(&self, f: F) -> KeyboardResult
    where
        F: FnOnce(&Self),
    {
        if self.is_editing_keyboard_scope() || !self.imp().picture.get().is_visible() {
            return KeyboardResult::Ignored;
        }
        f(self);
        KeyboardResult::Handled
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

    /// Convenience accessor for `gio::ListStore::n_items` that swallows the
    /// `media_list not injected yet` case and returns `None`.
    pub(super) fn list_n_items(&self) -> Option<u32> {
        self.imp().media_list.borrow().as_ref().map(|l| l.n_items())
    }

    pub(super) fn media_item_summary_at(&self, index: u32) -> String {
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

    fn set_details_sidebar_child_visible(&self, visible: bool) {
        if let Some(sidebar) = self.imp().details_split_view.get().sidebar() {
            sidebar.set_visible(visible);
        }
    }

    fn set_editor_sidebar_child_visible(&self, visible: bool) {
        self.imp().editor_panel.get().set_visible(visible);
    }

    pub(super) fn set_overlay_navigation_visible(&self, visible: bool) {
        if let Some(container) = self.imp().prev_btn.get().parent() {
            container.set_visible(visible);
        }
    }

    fn start_inline_rename(&self) {
        let Some(item) = self.current_media_item() else {
            return;
        };
        let stem = item
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_else(|| item.display_name());
        let imp = self.imp();
        imp.name_entry.get().set_text(stem);
        imp.name_row.get().set_subtitle("");
        imp.name_entry.get().set_visible(true);
        imp.name_entry.get().grab_focus();
        imp.name_entry.get().select_region(0, -1);
    }

    fn finish_inline_rename(&self, commit: bool) {
        let imp = self.imp();
        if !gtk::prelude::WidgetExt::is_visible(&imp.name_entry.get()) {
            return;
        }
        let requested = imp.name_entry.get().text().to_string();
        imp.name_entry.get().set_visible(false);
        if let Some(item) = self.current_media_item() {
            imp.name_row.get().set_subtitle(item.display_name());
        }
        if commit {
            self.rename_current_media(requested);
        }
    }

    fn rename_current_media(&self, requested_name: String) {
        let Some(item) = self.current_media_item() else {
            return;
        };
        let current_stem = item
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_else(|| item.display_name());
        if requested_name.trim() == current_stem {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            tracing::warn!("ViewerPage: inline rename requested but pool not set");
            return;
        };
        let db_actor = self.imp().db_actor.borrow().clone();
        let media_id = MediaId::from(item.id);
        let weak = self.downgrade();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let repo = MediaRepository::new(pool);
            let result = match db_actor.as_ref() {
                Some(actor) => repo.rename_media_file_with_actor(media_id, &requested_name, actor),
                None => Err(crate::core::error::AppError::Backend(
                    "DB actor unavailable for media rename".into(),
                )),
            };
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
                Ok(mutation) => {
                    if let Some(item) = mutation.changed_items.into_iter().next() {
                        this.replace_current_media_item(item.clone());
                        this.set_title(item.display_name());
                        this.update_date_label(&item);
                        this.update_details(&item);
                        this.refresh_thumb_strip();
                    }
                }
                Err(err) => {
                    let message = trf("viewer.toast.rename_failed", &[("error", &err.to_string())]);
                    toasts::error(&this.imp().toast_overlay.get(), &message);
                }
            }
        });
    }

    fn replace_current_media_item(&self, item: MediaItem) {
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let Some(index) = find_media_index_by_id(&list, item.id) else {
            return;
        };
        list.splice(index, 1, &[glib::BoxedAnyObject::new(item.clone())]);
        self.imp().current_index.set(index);
        self.imp().current_media_id.set(item.id);
    }

    /// Display the item at `index`, decode the **original** image off the
    /// main thread, and preload its immediate neighbours. Safe to call
    /// multiple times.
    #[tracing::instrument(name = "viewer:show_at", skip(self), fields(index, item_id, token))]
    pub fn show_at(&self, index: u32) {
        tracing::Span::current().record("index", index);
        self.imp().current_index.set(index);
        self.stop_animated_image_playback();
        // Keep the previous frame on screen until a new texture arrives — no
        // proactive spinner on navigation. The spinner only appears when there
        // is genuinely nothing to show (first viewer open, or the previous
        // frame was already cleared). This is the base layer that removes the
        // "loading animation" during the decode gap; the deferred-switch path
        // in `navigate_by_delta` guarantees the new preview is already warm
        // before we even get here.
        let had_paintable = self.imp().picture.get().paintable().is_some();
        self.set_spinner_visible(!had_paintable);
        self.reset_viewer_transform();

        // Bump token so a stale response from a previous show_at() doesn't
        // overwrite the current picture.
        let token = {
            let t = self.imp().current_token.get() + 1;
            self.imp().current_token.set(t);
            t
        };
        tracing::Span::current().record("token", token);

        let item = {
            let list = self.imp().media_list.borrow();
            let Some(list) = list.as_ref() else {
                return;
            };
            let Some(item) = crate::ui::media_list::media_item_at(list, index) else {
                return;
            };
            item
        };
        tracing::Span::current().record("item_id", item.id);
        if self.imp().current_index.get() != index {
            self.imp().current_index.set(index);
        }
        if self.imp().current_media_id.get() != item.id {
            self.imp().current_media_id.set(item.id);
        }
        self.set_title(item.display_name());
        self.update_date_label(&item);
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
        if self.imp().details_split_view.get().shows_sidebar() {
            self.update_details(&item);
        }
        if item.is_video() {
            self.refresh_thumb_strip();
            self.imp().motion_play_btn.get().set_visible(false);
            // Intentionally not clearing the picture: the previous frame stays
            // visible until the video preview thumbnail (or stream) replaces
            // it, so there is no blank/spinner gap.
            self.request_current_preview_thumbnail(&item, token);
            // If the resolved item is the same video we are already playing,
            // reuse the live stream instead of tearing it down and rebuilding.
            // The startup scan re-anchors the render index onto the same item
            // when media is inserted before it; rebuilding here would destroy a
            // live GstPlay mid-flight for no reason (and race the teardown
            // crash fixed in `stop_video_playback`).
            let attached_video_media_id = self.imp().attached_video_media_id.get();
            if attached_video_media_id == item.id && self.imp().video.get().media_stream().is_some()
            {
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_TRACE video_stage_reused index={} item_id={} attached_video_media_id={} (startup-scan re-anchor, no rebuild)",
                    index,
                    item.id,
                    attached_video_media_id
                );
                return;
            }
            self.show_video_stage(&item, token);
            return;
        }
        self.show_image_stage();
        // Intentionally not clearing the picture: the previous frame stays
        // visible until the new preview/original texture arrives. This is what
        // removes the loading animation during switching.
        self.imp().edit_btn.get().set_sensitive(false);
        self.set_motion_play_button_for_item(&item);
        if prefs::auto_play_motion_photo() && item.is_motion_photo() {
            self.play_current_motion_photo();
        }
        let path = strip_file_uri(&item.uri);

        self.request_current_preview_thumbnail(&item, token);

        // Warm the OS page cache for the ±1 neighbours so the next original
        // decode does not stall on disk I/O. This only `read`s the bytes
        // (cheap); thumbnail warming — the bigger win for perceived latency —
        // is done by `prefetch_neighbors`.
        self.preload_neighbor_pages(-1);
        self.preload_neighbor_pages(1);
        // Prefetch ±1 neighbour items (cuts the next switch's DB neighbour
        // query) and warm their Medium preview thumbnails (makes the next
        // switch's preview a mem-cache hit).
        self.prefetch_neighbors();

        // Update the bottom filmstrip (highlight or rebuild + scroll).
        self.refresh_thumb_strip();

        if should_play_animated_image(&item) && self.start_animated_image_playback(&path, token) {
            return;
        }

        self.request_current_original_image(path, token, item.display_name().to_string());
    }

    fn setup_zoom_transform_provider(&self) {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        *self.imp().zoom_provider.borrow_mut() = Some(provider);
    }

    /// Current item index in the backing `ListStore`.
    pub fn current_index(&self) -> u32 {
        self.imp().current_index.get()
    }
}

#[cfg(test)]
mod tests;
