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
#[path = "viewer/navigation.rs"]
mod navigation;
#[path = "viewer/stage.rs"]
mod stage;

use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::i18n::{tr, trf};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::prefs;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::thumbnails::ThumbnailLoader;
#[cfg(test)]
use crate::core::thumbnails::ThumbnailSize;
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
use libadwaita::prelude::{ActionRowExt, AdwDialogExt, AlertDialogExt, NavigationPageExt};
use libadwaita::subclass::prelude::*;
use std::cell::{Cell, RefCell};
#[cfg(test)]
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crop::CropDragState;
#[cfg(test)]
use crop::{compute_contained_image_rect, drag_rect, CropDragMode};
#[cfg(test)]
use filmstrip::{
    clamped_thumb_width_for_texture, classify_thumb_window_update,
    compute_current_thumb_extend_direction, compute_extended_thumb_window,
    compute_initial_thumb_window, compute_thumb_animated_scroll_value, compute_thumb_positioning,
    compute_thumb_scroll_and_residual, compute_thumb_visual_transform,
    should_retry_thumb_centering, thumb_item_content_geometry, thumb_width_for_media_dimensions,
    ThumbWindowUpdateKind, THUMB_DEFAULT_WINDOW_LEN, THUMB_EDGE_INSET, THUMB_LAZY_HALF,
    THUMB_MIN_WIDTH, THUMB_STRIP_SPACING, THUMB_WINDOW_MAX,
};
use fullscreen::viewer_overlay_button;
use navigation::{find_media_index_by_id, index_for_media_id, next_index_after_deleted_item};
#[cfg(test)]
use stage::{
    animated_image_next_delay, apply_video_audio_preferences_to_stream,
    should_reveal_prepared_video_stage, should_toggle_video_from_stage_click,
    viewer_preview_thumbnail_size, AnimatedImageFrame, ANIMATED_IMAGE_LOOP_PAUSE_MS,
};
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
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG set_edit_target index={} nav_visible={:?}",
            self.imp().current_index.get(),
            nav.visible_page().map(|page| page.title())
        );
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    pub fn set_db_actor(&self, db_actor: DbActorHandle) {
        *self.imp().db_actor.borrow_mut() = Some(db_actor);
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

    pub(crate) fn guard_initial_navigation_pop(&self) {
        self.set_can_pop(false);
        let weak = self.downgrade();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(VIEWER_OPEN_POP_GUARD_MS),
            move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if !this.imp().details_split_view.get().shows_sidebar()
                    && !this.imp().editor_split_view.get().shows_sidebar()
                {
                    this.set_can_pop(true);
                }
            },
        );
    }

    /// Inject the shared thumbnail loader. Must be called before `show_at`
    /// so the filmstrip can request thumbnails.
    pub fn set_thumbnail_loader(&self, loader: Arc<ThumbnailLoader>) {
        *self.imp().loader.borrow_mut() = Some(loader);
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
                let db_actor = match this.imp().db_actor.borrow().as_ref() {
                    Some(actor) => actor.clone(),
                    None => {
                        tracing::warn!(
                            target: crate::core::log_targets::VIEWER,
                            "TRASH_TRACE viewer_delete_no_actor"
                        );
                        return;
                    }
                };
                let item = match this.current_media_item() {
                    Some(i) => i,
                    None => return,
                };

                let item_id = item.id;
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "TRASH_TRACE viewer_delete_requested id={} uri={}",
                    item.id,
                    item.uri
                );
                let weak_after = this.downgrade();
                glib::spawn_future_local(async move {
                    let prepared = db_actor
                        .execute(DbCommand::MarkTrashed {
                            ids: vec![MediaId::from(item_id)],
                        })
                        .await;
                    let Ok(crate::core::DbCommandResult::MediaItems(mut items)) = prepared else {
                        tracing::warn!(
                            target: crate::core::log_targets::VIEWER,
                            "TRASH_TRACE viewer_mark_failed id={item_id}"
                        );
                        if let Some(this) = weak_after.upgrade() {
                            toasts::error(
                                &this.imp().toast_overlay.get(),
                                &tr("viewer.toast.move_to_trash_failed"),
                            );
                        }
                        return;
                    };
                    let Some(item) = items.pop() else {
                        return;
                    };
                    let Some(this) = weak_after.upgrade() else {
                        let _ = db_actor
                            .execute(DbCommand::RollbackTrashed {
                                ids: vec![MediaId::from(item_id)],
                            })
                            .await;
                        return;
                    };
                    let Some(pool) = this.imp().pool.borrow().as_ref().cloned() else {
                        let _ = db_actor
                            .execute(DbCommand::RollbackTrashed {
                                ids: vec![MediaId::from(item_id)],
                            })
                            .await;
                        toasts::error(
                            &this.imp().toast_overlay.get(),
                            &tr("viewer.toast.move_to_trash_failed"),
                        );
                        return;
                    };

                    let weak_for_callback = this.downgrade();
                    crate::ui::trash_fallback::move_marked_items_with_fallback(
                        &this,
                        pool,
                        db_actor,
                        vec![item],
                        move |moved_ids| {
                            if !moved_ids.iter().any(|id| id.get() == item_id) {
                                return;
                            }
                            if let Some(this) = weak_for_callback.upgrade() {
                                toasts::success(
                                    &this.imp().toast_overlay.get(),
                                    &tr("viewer.toast.moved_to_trash"),
                                );
                                this.remove_deleted_item(item_id);
                                if let Some(cb) = this.imp().trashed_cb.borrow().clone() {
                                    cb(item_id);
                                }
                            }
                        },
                    );
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
            let db_actor = match this.imp().db_actor.borrow().as_ref() {
                Some(actor) => actor.clone(),
                None => {
                    tracing::warn!("ViewerPage: Favorite pressed but DB actor not set");
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
            let token = this.imp().current_token.get();
            let weak_after = this.downgrade();
            glib::spawn_future_local(async move {
                let db_result = db_actor
                    .execute(DbCommand::SetFavorite {
                        ids: vec![MediaId::from(item_id)],
                        is_favorite: next_state,
                    })
                    .await
                    .map(|_| ());
                if let Some(button) = button_weak.upgrade() {
                    button.set_sensitive(true);
                }
                if let Some(this) = weak_after.upgrade() {
                    if this.imp().current_token.get() != token {
                        return;
                    }
                    match db_result {
                        Ok(()) => {
                            this.refresh_favorite_button(next_state);
                            if let Some(cb) = this.imp().favorite_state_cb.borrow().clone() {
                                cb(item_id, next_state);
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

    fn setup_fullscreen_button(&self) {
        self.update_fullscreen_button();
        let weak = self.downgrade();
        self.imp().fullscreen_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG fullscreen_preview_button_clicked index={} already_open={} root_present={} can_pop={} header_visible={} bottom_visible={}",
                this.imp().current_index.get(),
                this.imp().fullscreen_preview_window.borrow().is_some(),
                this.root().is_some(),
                this.can_pop(),
                this.imp().header_bar.get().is_visible(),
                this.imp().viewer_bottom_stack.get().is_visible()
            );
            this.open_fullscreen_preview_window();
        });
    }

    fn open_fullscreen_preview_window(&self) {
        if let Some(window) = self.imp().fullscreen_preview_window.borrow().as_ref() {
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG fullscreen_preview_present_existing index={}",
                self.imp().current_index.get()
            );
            window.present();
            return;
        }

        let Some(paintable) = self.imp().picture.get().paintable() else {
            tracing::warn!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG fullscreen_preview_no_paintable index={}",
                self.imp().current_index.get()
            );
            return;
        };

        let title = self
            .current_media_item()
            .map(|item| item.display_name().to_string())
            .unwrap_or_else(|| tr("page.viewer.title"));
        let parent_window = self
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok());
        let (default_width, default_height) = parent_window
            .as_ref()
            .and_then(|parent| {
                let surface = parent.surface()?;
                let display = gtk::prelude::WidgetExt::display(parent);
                let monitor = display.monitor_at_surface(&surface)?;
                let geometry = monitor.geometry();
                Some((geometry.width(), geometry.height()))
            })
            .unwrap_or((1024, 768));
        let window = gtk::Window::builder()
            .title(title.as_str())
            .default_width(default_width)
            .default_height(default_height)
            .decorated(false)
            .fullscreened(true)
            .build();
        if let Some(application) = parent_window
            .as_ref()
            .and_then(|parent| parent.application())
        {
            window.set_application(Some(&application));
        }

        let overlay = gtk::Overlay::new();
        overlay.add_css_class("viewer-stage");

        let picture = gtk::Picture::builder()
            .paintable(&paintable)
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        picture.add_css_class("viewer-media-surface");
        picture.add_css_class("viewer-fullscreen-preview-picture");
        overlay.set_child(Some(&picture));

        // Capture a weak ref now (before `picture` is moved into the
        // transform-update closure below) so the entrance fade at present() can
        // tag it. See picture.viewer-fullscreen-preview-picture in grid_css.rs.
        let pic_for_fade = picture.downgrade();

        let preview_provider = Rc::new(gtk::CssProvider::new());
        gtk::style_context_add_provider_for_display(
            &gtk::prelude::WidgetExt::display(&picture),
            preview_provider.as_ref(),
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let preview_scale = Rc::new(Cell::new(MIN_VIEWER_ZOOM));
        let preview_rotation = Rc::new(Cell::new(0_i32));

        let nav_controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_bottom(34)
            .margin_end(10)
            .build();
        nav_controls.add_css_class("viewer-overlay-nav");
        let preview_prev_btn =
            viewer_overlay_button("go-previous-symbolic", &tr("viewer.tooltip.previous"));
        let preview_next_btn =
            viewer_overlay_button("go-next-symbolic", &tr("viewer.tooltip.next"));
        nav_controls.append(&preview_prev_btn);
        nav_controls.append(&preview_next_btn);
        overlay.add_overlay(&nav_controls);

        let zoom_controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(10)
            .margin_end(10)
            .build();
        zoom_controls.add_css_class("viewer-zoom-controls");
        let preview_zoom_reset_btn =
            viewer_overlay_button("zoom-fit-best-symbolic", &tr("viewer.tooltip.zoom_reset"));
        let preview_zoom_out_btn =
            viewer_overlay_button("zoom-out-symbolic", &tr("viewer.tooltip.zoom_out"));
        let preview_rotate_left_btn = viewer_overlay_button(
            "object-rotate-left-symbolic",
            &tr("viewer.tooltip.rotate_left"),
        );
        let preview_rotate_right_btn = viewer_overlay_button(
            "object-rotate-right-symbolic",
            &tr("viewer.tooltip.rotate_right"),
        );
        let preview_restore_btn = viewer_overlay_button(
            "view-restore-symbolic",
            &tr("viewer.tooltip.exit_fullscreen"),
        );
        let preview_zoom_in_btn =
            viewer_overlay_button("zoom-in-symbolic", &tr("viewer.tooltip.zoom_in"));
        zoom_controls.append(&preview_zoom_reset_btn);
        zoom_controls.append(&preview_zoom_out_btn);
        zoom_controls.append(&preview_rotate_left_btn);
        zoom_controls.append(&preview_rotate_right_btn);
        zoom_controls.append(&preview_restore_btn);
        zoom_controls.append(&preview_zoom_in_btn);
        overlay.add_overlay(&zoom_controls);
        window.set_child(Some(&overlay));

        let update_preview_controls: Rc<dyn Fn()> = Rc::new({
            let picture = picture.clone();
            let provider = preview_provider.clone();
            let scale = preview_scale.clone();
            let rotation = preview_rotation.clone();
            let zoom_reset_btn = preview_zoom_reset_btn.clone();
            let zoom_out_btn = preview_zoom_out_btn.clone();
            let rotate_left_btn = preview_rotate_left_btn.clone();
            let rotate_right_btn = preview_rotate_right_btn.clone();
            let zoom_in_btn = preview_zoom_in_btn.clone();
            move || {
                let current_scale = scale.get();
                let current_rotation = rotation.get();
                provider.load_from_data(&format!(
                    "picture.viewer-fullscreen-preview-picture {{ transform: rotate({current_rotation}deg) scale({current_scale}); }}"
                ));
                let zoomed = current_scale > MIN_VIEWER_ZOOM;
                zoom_in_btn.set_visible(true);
                zoom_out_btn.set_visible(zoomed);
                zoom_reset_btn.set_visible(zoomed);
                rotate_left_btn.set_visible(!zoomed);
                rotate_right_btn.set_visible(!zoomed);
                picture.queue_draw();
            }
        });
        update_preview_controls();

        let paintable_handler_id: Rc<RefCell<Option<glib::SignalHandlerId>>> =
            Rc::new(RefCell::new(None));
        let handler_id = self.imp().picture.get().connect_paintable_notify({
            let preview_picture = picture.downgrade();
            let preview_scale = preview_scale.clone();
            let preview_rotation = preview_rotation.clone();
            let update_preview_controls = update_preview_controls.clone();
            move |main_picture| {
                let Some(preview_picture) = preview_picture.upgrade() else {
                    return;
                };
                if let Some(paintable) = main_picture.paintable() {
                    preview_picture.set_paintable(Some(&paintable));
                    preview_scale.set(MIN_VIEWER_ZOOM);
                    preview_rotation.set(0);
                    update_preview_controls();
                }
            }
        });
        *paintable_handler_id.borrow_mut() = Some(handler_id);

        let weak = self.downgrade();
        let preview_scale_for_prev = preview_scale.clone();
        let preview_rotation_for_prev = preview_rotation.clone();
        let update_for_prev = update_preview_controls.clone();
        preview_prev_btn.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                preview_scale_for_prev.set(MIN_VIEWER_ZOOM);
                preview_rotation_for_prev.set(0);
                update_for_prev();
                this.navigate_by_delta(-1);
            }
        });
        let weak = self.downgrade();
        let preview_scale_for_next = preview_scale.clone();
        let preview_rotation_for_next = preview_rotation.clone();
        let update_for_next = update_preview_controls.clone();
        preview_next_btn.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                preview_scale_for_next.set(MIN_VIEWER_ZOOM);
                preview_rotation_for_next.set(0);
                update_for_next();
                this.navigate_by_delta(1);
            }
        });

        let scale_for_zoom_in = preview_scale.clone();
        let update_for_zoom_in = update_preview_controls.clone();
        preview_zoom_in_btn.connect_clicked(move |_| {
            scale_for_zoom_in.set(step_zoom(scale_for_zoom_in.get(), 1));
            update_for_zoom_in();
        });
        let scale_for_zoom_out = preview_scale.clone();
        let update_for_zoom_out = update_preview_controls.clone();
        preview_zoom_out_btn.connect_clicked(move |_| {
            scale_for_zoom_out.set(step_zoom(scale_for_zoom_out.get(), -1));
            update_for_zoom_out();
        });
        let scale_for_reset = preview_scale.clone();
        let rotation_for_reset = preview_rotation.clone();
        let update_for_reset = update_preview_controls.clone();
        preview_zoom_reset_btn.connect_clicked(move |_| {
            scale_for_reset.set(MIN_VIEWER_ZOOM);
            rotation_for_reset.set(0);
            update_for_reset();
        });
        let rotation_for_left = preview_rotation.clone();
        let update_for_left = update_preview_controls.clone();
        preview_rotate_left_btn.connect_clicked(move |_| {
            rotation_for_left.set((rotation_for_left.get() - 90).rem_euclid(360));
            update_for_left();
        });
        let rotation_for_right = preview_rotation.clone();
        let update_for_right = update_preview_controls.clone();
        preview_rotate_right_btn.connect_clicked(move |_| {
            rotation_for_right.set((rotation_for_right.get() + 90).rem_euclid(360));
            update_for_right();
        });
        let window_weak = window.downgrade();
        preview_restore_btn.connect_clicked(move |_| {
            if let Some(window) = window_weak.upgrade() {
                window.close();
            }
        });

        let key = gtk::EventControllerKey::new();
        let window_weak = window.downgrade();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(window) = window_weak.upgrade() {
                    window.close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key);

        let weak = self.downgrade();
        let handler_id_for_close = paintable_handler_id.clone();
        window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_DEBUG fullscreen_preview_close_request index={}",
                    this.imp().current_index.get()
                );
                if let Some(handler_id) = handler_id_for_close.borrow_mut().take() {
                    this.imp().picture.get().disconnect(handler_id);
                }
                this.imp().fullscreen_preview_window.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        window.connect_map(|window| {
            window.set_fullscreened(true);
            window.fullscreen();
        });

        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG fullscreen_preview_open index={} title={}",
            self.imp().current_index.get(),
            title
        );
        window.present();
        window.set_fullscreened(true);
        window.fullscreen();
        // Entrance fade: the picture starts at opacity 0 (CSS); adding
        // .fade-shown on the next idle lets GTK paint the hidden state first,
        // then the CSS transition fades the preview in.
        let _ = glib::idle_add_local_once(move || {
            if let Some(p) = pic_for_fade.upgrade() {
                p.add_css_class("fade-shown");
            }
        });
        *self.imp().fullscreen_preview_window.borrow_mut() = Some(window);
    }

    fn update_fullscreen_button(&self) {
        let button = self.imp().fullscreen_btn.get();
        button.set_icon_name(VIEWER_FULLSCREEN_ICON);
        button.set_tooltip_text(Some(&tr("viewer.tooltip.fullscreen")));
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

        let weak = self.downgrade();
        imp.rotate_left_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.rotate_viewer_image(-90);
            }
        });

        let weak = self.downgrade();
        imp.rotate_right_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.rotate_viewer_image(90);
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

    pub(super) fn reset_viewer_transform(&self) {
        self.imp().viewer_rotation_degrees.set(0);
        self.set_viewer_zoom(MIN_VIEWER_ZOOM, 0.0, 0.0);
    }

    fn rotate_viewer_image(&self, delta_degrees: i32) {
        let current = self.imp().viewer_rotation_degrees.get();
        let next = (current + delta_degrees).rem_euclid(360);
        self.imp().viewer_rotation_degrees.set(next);
        self.update_zoom_transform();
        self.update_zoom_buttons();
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
        let rotation = imp.viewer_rotation_degrees.get();
        if let Some(provider) = imp.zoom_provider.borrow().as_ref() {
            provider.load_from_data(&format!(
                "picture.viewer-image-frame {{ transform: translate({pan_x}px, {pan_y}px) rotate({rotation}deg) scale({scale}); }}"
            ));
        }
        imp.picture.get().queue_draw();
    }

    pub(super) fn set_zoom_controls_visible(&self, visible: bool) {
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
        imp.rotate_left_btn.get().set_visible(!zoomed);
        imp.rotate_right_btn.get().set_visible(!zoomed);
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
        let media_id = MediaId::from(item.id);
        let weak = self.downgrade();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let repo = MediaRepository::new(pool);
            let result = repo.rename_media_file(media_id, &requested_name);
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

    /// Display the item at `index`, decode the **original** image off the
    /// main thread, and preload its immediate neighbours. Safe to call
    /// multiple times.
    #[tracing::instrument(name = "viewer:show_at", skip(self))]
    pub fn show_at(&self, index: u32) {
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG show_at requested_index={} current_before={} details_revealed={}",
            index,
            self.imp().current_index.get(),
            self.imp().details_split_view.get().shows_sidebar()
        );
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
        if self.imp().current_index.get() != index {
            self.imp().current_index.set(index);
        }
        if self.imp().current_media_id.get() != item.id {
            self.imp().current_media_id.set(item.id);
        }
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

fn step_zoom(current: f64, direction: i32) -> f64 {
    let factor = if direction >= 0 {
        VIEWER_ZOOM_STEP
    } else {
        1.0 / VIEWER_ZOOM_STEP
    };
    (current * factor).clamp(MIN_VIEWER_ZOOM, MAX_VIEWER_ZOOM)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::cell::Cell;
    use std::time::Duration;

    // ── filmstrip window calculations ──────────────────────────────────

    #[test]
    fn video_stage_click_toggles_above_builtin_controls() {
        assert!(should_toggle_video_from_stage_click(240.0, 600.0));
    }

    #[test]
    fn video_stage_click_leaves_builtin_controls_alone() {
        assert!(!should_toggle_video_from_stage_click(570.0, 600.0));
        assert!(!should_toggle_video_from_stage_click(-1.0, 600.0));
        assert!(!should_toggle_video_from_stage_click(0.0, 40.0));
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
    fn right_growth_uses_append_update_instead_of_rebuilding_strip() {
        assert_eq!(
            classify_thumb_window_update(0, 23, 0, 27),
            ThumbWindowUpdateKind::AppendRight
        );
    }

    #[test]
    fn left_growth_is_classified_as_prepend_update() {
        assert_eq!(
            classify_thumb_window_update(20, 31, 16, 31),
            ThumbWindowUpdateKind::PrependLeft
        );
    }

    #[test]
    fn sliding_window_still_rebuilds_because_existing_indices_change() {
        assert_eq!(
            classify_thumb_window_update(50, 90, 54, 94),
            ThumbWindowUpdateKind::Rebuild
        );
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
    fn extend_at_window_cap_slides_left_without_growing_live_items() {
        let (new_start, new_end) =
            compute_extended_thumb_window(-1, 50, 90, 100, THUMB_WINDOW_MAX as usize).unwrap();

        assert_eq!(new_start, 50 - THUMB_LAZY_HALF);
        assert_eq!(new_end, 90 - THUMB_LAZY_HALF);
        assert_eq!(new_end - new_start, THUMB_WINDOW_MAX);
    }

    #[test]
    fn extend_at_window_cap_slides_right_without_growing_live_items() {
        let (new_start, new_end) =
            compute_extended_thumb_window(1, 50, 90, 100, THUMB_WINDOW_MAX as usize).unwrap();

        assert_eq!(new_start, 50 + THUMB_LAZY_HALF);
        assert_eq!(new_end, 90 + THUMB_LAZY_HALF);
        assert_eq!(new_end - new_start, THUMB_WINDOW_MAX);
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
    }

    #[test]
    fn current_edge_extend_continues_at_window_cap_by_sliding_window() {
        assert_eq!(
            compute_current_thumb_extend_direction(86, 50, 90, 100, THUMB_WINDOW_MAX as usize),
            Some(1),
            "window cap should not make the carousel feel paged when more items exist to the right"
        );
        assert_eq!(
            compute_current_thumb_extend_direction(53, 50, 90, 100, THUMB_WINDOW_MAX as usize),
            Some(-1),
            "window cap should not make the carousel feel paged when more items exist to the left"
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
    fn animated_scroll_value_eases_between_current_and_target() {
        assert_eq!(
            compute_thumb_animated_scroll_value(100.0, 260.0, 0.0),
            100.0
        );
        assert_eq!(
            compute_thumb_animated_scroll_value(100.0, 260.0, 1.0),
            260.0
        );

        let halfway = compute_thumb_animated_scroll_value(100.0, 260.0, 0.5);
        assert!(halfway > 100.0);
        assert!(halfway < 260.0);
        assert!(
            halfway > 180.0,
            "ease-out should move past the linear midpoint by halfway through the animation"
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
    fn filmstrip_placeholder_width_uses_media_dimensions_when_available() {
        assert_eq!(thumb_width_for_media_dimensions(Some(1170), Some(250)), 131);
        assert_eq!(thumb_width_for_media_dimensions(Some(1224), Some(824)), 83);
        assert_eq!(thumb_width_for_media_dimensions(Some(228), Some(256)), 50);
        assert_eq!(
            thumb_width_for_media_dimensions(None, Some(824)),
            THUMB_MIN_WIDTH
        );
        assert_eq!(
            thumb_width_for_media_dimensions(Some(1224), None),
            THUMB_MIN_WIDTH
        );
    }

    #[test]
    fn item_geometry_uses_sequence_when_current_allocation_x_is_stale() {
        let widths = [118.0, 118.0, 118.0, 118.0, 112.0, 118.0, 118.0];
        let (content_x, width, content_width) =
            thumb_item_content_geometry(&widths, 4, THUMB_STRIP_SPACING).unwrap();

        assert_eq!(
            content_x,
            THUMB_EDGE_INSET + 4.0 * (118.0 + THUMB_STRIP_SPACING)
        );
        assert_eq!(width, 112.0);
        assert_eq!(
            content_width,
            THUMB_EDGE_INSET * 2.0
                + widths.iter().sum::<f64>()
                + (widths.len() - 1) as f64 * THUMB_STRIP_SPACING
        );

        let (_, _, transform) = compute_thumb_positioning(content_x, width, 1140.0, 1140.0, 1140.0);
        assert!(
            transform.abs() < 100.0,
            "current index 4 must not be positioned from a stale allocation x near the left edge"
        );
    }

    #[test]
    fn item_geometry_includes_filmstrip_edge_inset() {
        let widths = [60.0, 60.0, 60.0];
        let (content_x, _, content_width) =
            thumb_item_content_geometry(&widths, 0, THUMB_STRIP_SPACING).unwrap();

        assert_eq!(content_x, THUMB_EDGE_INSET);
        assert_eq!(
            content_width,
            THUMB_EDGE_INSET * 2.0
                + widths.iter().sum::<f64>()
                + (widths.len() - 1) as f64 * THUMB_STRIP_SPACING
        );
    }

    #[test]
    fn item_geometry_rejects_transient_tiny_allocations_after_rebuild() {
        let widths = [2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 28.0, 2.0, 2.0];

        assert!(
            thumb_item_content_geometry(&widths, 6, THUMB_STRIP_SPACING).is_none(),
            "2px allocations logged immediately after rebuild are not stable enough for centering"
        );
    }

    #[test]
    fn item_geometry_accepts_small_loaded_thumbnail_allocations() {
        let widths = [38.0, 84.0, 110.0, 84.0, 38.0];
        let (content_x, width, _) =
            thumb_item_content_geometry(&widths, 2, THUMB_STRIP_SPACING).unwrap();

        assert_eq!(
            content_x,
            THUMB_EDGE_INSET + 38.0 + THUMB_STRIP_SPACING + 84.0 + THUMB_STRIP_SPACING
        );
        assert_eq!(width, 110.0);
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
            bottom_bar_classes
                .iter()
                .any(|class| class == "glass-raised"),
            "carousel thumbnail strip should reuse the shared raised glass material"
        );
        assert!(
            bottom_bar_classes
                .iter()
                .any(|class| class == "viewer-thumb-carousel"),
            "carousel thumbnail strip should expose a dedicated class for edge-fade styling"
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
    fn video_error_background_exists_in_viewer_overlay() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

        assert!(
            widget_tree_has_class(&viewer.imp().image_overlay.get(), "viewer-video-error"),
            "viewer should provide an app-owned video error background instead of exposing GtkVideo's default broken frame"
        );
        assert!(
            !viewer.imp().video_error_box.get().is_visible(),
            "video error background should stay hidden until playback reports an error"
        );
        assert_eq!(
            viewer.imp().video_error_title.get().label(),
            tr("viewer.video_error.title")
        );
    }

    #[gtk::test]
    fn video_error_background_hides_default_video_error_surface() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

        viewer.imp().video.get().set_visible(true);
        viewer.imp().picture.get().set_visible(true);
        viewer.set_spinner_visible(true);
        viewer.show_video_error_background();

        assert!(viewer.imp().video_error_box.get().is_visible());
        assert!(
            !viewer.imp().video.get().is_visible(),
            "GtkVideo should be hidden so its default broken-frame graphic is not exposed"
        );
        assert!(!viewer.imp().picture.get().is_visible());
        assert!(
            viewer
                .imp()
                .spinner
                .get()
                .has_css_class("viewer-spinner-hidden"),
            "spinner should be opacity-hidden (not removed from layout) so it fades"
        );

        viewer.show_image_stage();
        assert!(
            !viewer.imp().video_error_box.get().is_visible(),
            "leaving the failed video should clear the error background"
        );
    }

    #[gtk::test]
    fn fullscreen_preview_opens_separate_window_without_changing_viewer_layout() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        let texture = test_texture();
        viewer.imp().picture.get().set_paintable(Some(&texture));
        let parent = gtk::Window::builder()
            .title("Viewer parent")
            .default_width(900)
            .default_height(700)
            .build();
        parent.set_child(Some(&viewer));
        parent.present();
        while glib::MainContext::default().iteration(false) {}

        viewer.open_fullscreen_preview_window();

        let preview = viewer
            .imp()
            .fullscreen_preview_window
            .borrow()
            .as_ref()
            .cloned()
            .expect("fullscreen preview should keep a separate top-level window");
        assert!(
            preview.is_fullscreened(),
            "fullscreen preview should request fullscreen as its initial window state"
        );
        assert!(
            !preview.is_decorated(),
            "fullscreen preview should be borderless and hide the system titlebar controls"
        );
        assert!(
            preview.transient_for().is_none(),
            "fullscreen preview should be an independent top-level window so compositors honor fullscreen"
        );
        let preview_overlay = preview
            .child()
            .expect("fullscreen preview should have overlay content")
            .downcast::<gtk::Overlay>()
            .expect("fullscreen preview content should be a GtkOverlay");
        assert!(
            widget_tree_has_class(&preview_overlay, "viewer-fullscreen-preview-picture"),
            "fullscreen preview should render the media picture inside the overlay"
        );
        assert!(
            widget_tree_has_class(&preview_overlay, "viewer-overlay-nav"),
            "fullscreen preview should include the image-stage previous/next controls"
        );
        assert!(
            widget_tree_has_class(&preview_overlay, "viewer-zoom-controls"),
            "fullscreen preview should include the image-stage zoom/rotate controls"
        );
        assert!(
            widget_tree_has_button_icon(&preview_overlay, "view-restore-symbolic"),
            "fullscreen preview should include a restore button to close the enlarged window"
        );
        assert!(
            !widget_tree_has_class(&preview_overlay, "glass-header"),
            "fullscreen preview should not include the main viewer header actions"
        );
        assert!(
            viewer.imp().header_bar.get().is_visible(),
            "opening fullscreen preview must not hide the viewer header or disturb NavigationView"
        );
        assert!(viewer.imp().viewer_bottom_stack.get().is_visible());
        assert!(viewer.can_pop());
        assert_eq!(
            viewer.imp().fullscreen_btn.get().icon_name().as_deref(),
            Some(VIEWER_FULLSCREEN_ICON),
            "main viewer button should remain an open-preview command"
        );

        preview.close();
        parent.close();
        while glib::MainContext::default().iteration(false) {}
        assert!(
            viewer.imp().fullscreen_preview_window.borrow().is_none(),
            "closing the transient preview window should clear the stored handle"
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

    #[gtk::test]
    fn image_overlay_has_no_touch_zoom_or_pan_gestures() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        let controllers = viewer.imp().image_overlay.get().observe_controllers();
        let has_zoom = controllers
            .snapshot()
            .into_iter()
            .any(|controller| controller.downcast::<gtk::GestureZoom>().is_ok());
        let has_drag = controllers
            .snapshot()
            .into_iter()
            .any(|controller| controller.downcast::<gtk::GestureDrag>().is_ok());

        assert!(
            !has_zoom,
            "image overlay should not install touch pinch zoom while buttons own zoom actions"
        );
        assert!(
            !has_drag,
            "image overlay should not install touch drag pan while buttons own zoom/navigation actions"
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
    fn zoom_controls_live_in_top_right_with_reset_out_rotate_fullscreen_increase_order() {
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
            Some(imp.rotate_left_btn.get().upcast::<gtk::Widget>()),
            "rotate-left should follow zoom-out"
        );
        assert_eq!(
            imp.rotate_left_btn.get().next_sibling(),
            Some(imp.rotate_right_btn.get().upcast::<gtk::Widget>()),
            "rotate-right should follow rotate-left"
        );
        assert_eq!(
            imp.rotate_right_btn.get().next_sibling(),
            Some(imp.fullscreen_btn.get().upcast::<gtk::Widget>()),
            "fullscreen should follow rotate-right"
        );
        assert_eq!(
            imp.fullscreen_btn.get().next_sibling(),
            Some(imp.zoom_in_btn.get().upcast::<gtk::Widget>()),
            "zoom-in should follow fullscreen"
        );

        for (name, button) in [
            ("zoom_in_btn", imp.zoom_in_btn.get()),
            ("zoom_out_btn", imp.zoom_out_btn.get()),
            ("zoom_reset_btn", imp.zoom_reset_btn.get()),
            ("rotate_left_btn", imp.rotate_left_btn.get()),
            ("rotate_right_btn", imp.rotate_right_btn.get()),
            ("fullscreen_btn", imp.fullscreen_btn.get()),
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
        assert!(imp.rotate_left_btn.get().is_visible());
        assert!(imp.rotate_right_btn.get().is_visible());
        assert!(imp.fullscreen_btn.get().is_visible());

        viewer.set_viewer_zoom_for_tests(1.25, 0.0, 0.0);
        assert!(imp.zoom_in_btn.get().is_visible());
        assert!(imp.zoom_out_btn.get().is_visible());
        assert!(imp.zoom_reset_btn.get().is_visible());
        assert!(!imp.rotate_left_btn.get().is_visible());
        assert!(!imp.rotate_right_btn.get().is_visible());
        assert!(imp.fullscreen_btn.get().is_visible());
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

    #[test]
    fn viewer_preview_uses_medium_thumbnail() {
        assert_eq!(viewer_preview_thumbnail_size(), ThumbnailSize::Medium);
    }

    #[test]
    fn animated_image_adds_half_second_pause_before_looping() {
        let texture = test_texture();
        let frames = vec![
            AnimatedImageFrame {
                texture: texture.clone(),
                delay: Duration::from_millis(80),
            },
            AnimatedImageFrame {
                texture,
                delay: Duration::from_millis(120),
            },
        ];

        assert_eq!(
            animated_image_next_delay(&frames, 0),
            Duration::from_millis(80)
        );
        assert_eq!(
            animated_image_next_delay(&frames, 1),
            Duration::from_millis(120 + ANIMATED_IMAGE_LOOP_PAUSE_MS)
        );
    }

    #[test]
    fn viewer_plays_misnamed_gif_even_when_db_row_is_stale() {
        let mut item = sample_media_item();
        item.path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/media/gif_with_jpg_extension.jpg");
        item.uri = format!("file://{}", item.path.display());
        item.mime_type = "image/jpeg".into();
        item.media_attributes = "{}".into();

        assert!(
            should_play_animated_image(&item),
            "viewer should probe the current file header so unchanged stale DB rows still animate"
        );
    }

    #[gtk::test]
    fn viewer_starts_frame_timer_for_animated_gif() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/media/gif_with_jpg_extension.jpg");

        assert!(viewer.start_animated_image_playback(&path, 1));
        assert!(viewer.imp().picture.get().paintable().is_some());
        assert!(viewer.imp().animated_image_source.borrow().is_some());

        viewer.stop_animated_image_playback();
    }

    #[test]
    fn video_stage_reveals_only_for_current_prepared_stream() {
        assert!(
            should_reveal_prepared_video_stage(7, 7, true),
            "current prepared streams should switch from thumbnail preview to video"
        );
        assert!(
            !should_reveal_prepared_video_stage(7, 8, true),
            "stale streams from previous navigation must not reveal the video layer"
        );
        assert!(
            !should_reveal_prepared_video_stage(7, 7, false),
            "unprepared streams should keep the thumbnail preview visible"
        );
    }

    #[gtk::test]
    fn viewer_keyboard_action_navigates_and_closes() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

        let events = Rc::new(RefCell::new(Vec::new()));
        let events_for_cb = events.clone();
        viewer.connect_navigation(move |delta| {
            events_for_cb.borrow_mut().push(delta);
        });

        assert_eq!(
            viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerNext),
            crate::ui::keyboard::KeyboardResult::Handled
        );
        assert_eq!(
            viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerPrevious),
            crate::ui::keyboard::KeyboardResult::Handled
        );
        assert_eq!(
            viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::CancelOrClose),
            crate::ui::keyboard::KeyboardResult::Handled
        );

        assert_eq!(events.borrow().as_slice(), &[1, -1, NAV_POP]);
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

    fn test_texture() -> gdk::Texture {
        let bytes = glib::Bytes::from_owned(vec![255_u8, 0, 0, 255]);
        gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::R8g8b8a8, &bytes, 4).upcast()
    }

    fn widget_tree_has_class<W: IsA<gtk::Widget>>(widget: &W, class_name: &str) -> bool {
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

    fn widget_tree_has_button_icon<W: IsA<gtk::Widget>>(widget: &W, icon_name: &str) -> bool {
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

        assert_eq!(
            viewer.handle_keyboard_action(KeyboardAction::CancelOrClose),
            KeyboardResult::Handled,
            "Escape action should be consumed when details are visible"
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

    #[gtk::test]
    fn initial_open_guard_ignores_navigation_pop_action() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        let nav = adw::NavigationView::new();
        nav.push(&viewer);
        let nav_weak = nav.downgrade();
        viewer.connect_navigation(move |delta| {
            if delta == NAV_POP {
                if let Some(nav) = nav_weak.upgrade() {
                    nav.pop();
                }
            }
        });

        viewer.guard_initial_navigation_pop();
        let _ = viewer.activate_action("navigation.pop", None);

        assert_eq!(
            nav.visible_page().map(|page| page.title()).as_deref(),
            Some(viewer.title().as_str()),
            "initial open guard should swallow immediate navigation.pop events"
        );
    }

    #[gtk::test]
    fn initial_open_guard_ignores_keyboard_cancel() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);
        let pop_count = Rc::new(Cell::new(0));
        let pop_count_for_cb = pop_count.clone();
        viewer.connect_navigation(move |delta| {
            if delta == NAV_POP {
                pop_count_for_cb.set(pop_count_for_cb.get() + 1);
            }
        });

        viewer.guard_initial_navigation_pop();
        assert_eq!(
            viewer.handle_keyboard_action(KeyboardAction::CancelOrClose),
            KeyboardResult::Handled
        );

        assert_eq!(
            pop_count.get(),
            0,
            "initial open guard should swallow immediate keyboard close events"
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
    fn current_media_item_stays_anchored_when_startup_scan_inserts_before_it() {
        init_viewer_test();
        let list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let mut opened = sample_media_item();
        opened.id = 20;
        opened.uri = "file:///tmp/opened.jpg".into();
        opened.path = PathBuf::from("/tmp/opened.jpg");
        list.append(&glib::BoxedAnyObject::new(opened));

        let viewer =
            ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(20), list.clone());

        let mut inserted = sample_media_item();
        inserted.id = 10;
        inserted.uri = "file:///tmp/inserted.jpg".into();
        inserted.path = PathBuf::from("/tmp/inserted.jpg");
        list.insert(0, &glib::BoxedAnyObject::new(inserted));

        let current = viewer
            .current_media_item()
            .expect("viewer should still resolve the opened item");
        assert_eq!(current.id, 20);
        assert_eq!(viewer.current_index(), 1);
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

    #[gtk::test]
    fn stop_video_playback_retires_stream_until_next_idle() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

        let stream = gtk::MediaFile::for_filename("/tmp/photo-viewer-test.mp4");
        viewer.imp().video.get().set_media_stream(Some(&stream));
        assert!(
            viewer.imp().video.get().media_stream().is_some(),
            "precondition: a stream is attached"
        );

        viewer.stop_video_playback();

        // Detached from the widget at once, but NOT finalized: the retire slot
        // holds the only remaining reference so GstPlay can finish its terminal
        // state-changed signal against a live object. Releasing it synchronously
        // here is exactly what crashed the GstPlay thread.
        assert!(
            viewer.imp().video.get().media_stream().is_none(),
            "stream must detach from the video widget right away"
        );
        assert!(
            viewer.imp().retired_video_stream.borrow().is_some(),
            "stream must be retained past teardown to avoid the GstPlay-thread UAF"
        );

        // Pumping the default main context fires the idle callback that drops it.
        while glib::MainContext::default().iteration(false) {}
        assert!(
            viewer.imp().retired_video_stream.borrow().is_none(),
            "retired stream is released after the idle cycle"
        );
    }

    #[gtk::test]
    fn show_at_keeps_video_stream_when_startup_scan_re_anchors_same_item() {
        init_viewer_test();
        let dir = tempfile::tempdir().unwrap();
        let video_path = dir.path().join("clip.mp4");
        std::fs::write(&video_path, b"fake mp4").unwrap();

        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let mut video = sample_media_item();
        video.id = 20;
        video.mime_type = "video/mp4".into();
        video.uri = format!("file://{}", video_path.display());
        video.path = video_path;
        media_list.append(&glib::BoxedAnyObject::new(video));

        let viewer =
            ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(20), media_list.clone());

        viewer.show_at(0);
        assert!(
            viewer.imp().video.get().media_stream().is_some(),
            "first show_at for a video should attach a stream"
        );
        assert!(
            viewer.imp().retired_video_stream.borrow().is_none(),
            "precondition: nothing retired on first show"
        );

        // Startup scan inserts a media row before the current one. The viewer
        // re-resolves the render index to 1, but the media id is unchanged, so
        // show_at must reuse the live stream instead of rebuilding it.
        let mut inserted = sample_media_item();
        inserted.id = 10;
        media_list.insert(0, &glib::BoxedAnyObject::new(inserted));
        viewer.show_at(1);

        // If show_video_stage had rebuilt, its first call (stop_video_playback)
        // would have moved the previous stream into retired_video_stream, which
        // is only released on a later idle this test never pumps. An empty slot
        // plus a still-attached stream proves the live stream was reused.
        assert!(
            viewer.imp().video.get().media_stream().is_some(),
            "the live video stream should still be attached"
        );
        assert!(
            viewer.imp().retired_video_stream.borrow().is_none(),
            "same-id re-show must not tear down and rebuild the live GstPlay stream"
        );
    }

    #[gtk::test]
    fn show_at_rebuilds_video_stream_after_optimistic_navigation_to_different_video() {
        init_viewer_test();
        let dir = tempfile::tempdir().unwrap();
        let first_path = dir.path().join("first.mp4");
        let second_path = dir.path().join("second.mp4");
        std::fs::write(&first_path, b"fake first mp4").unwrap();
        std::fs::write(&second_path, b"fake second mp4").unwrap();

        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        let mut first = sample_media_item();
        first.id = 20;
        first.mime_type = "video/mp4".into();
        first.uri = format!("file://{}", first_path.display());
        first.path = first_path;
        media_list.append(&glib::BoxedAnyObject::new(first));

        let mut second = sample_media_item();
        second.id = 30;
        second.mime_type = "video/mp4".into();
        second.uri = format!("file://{}", second_path.display());
        second.path = second_path;
        media_list.append(&glib::BoxedAnyObject::new(second));

        let viewer =
            ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(20), media_list.clone());
        viewer.show_at(0);
        let first_stream = viewer
            .imp()
            .video
            .get()
            .media_stream()
            .expect("first video should attach a stream");

        // navigate_by_delta advances current_media_id optimistically before
        // the deferred show_at paints. show_at must still rebuild the video
        // stage for the target video instead of treating that optimistic id as
        // proof that the attached stream already belongs to the target.
        viewer.imp().current_media_id.set(30);
        viewer.show_at(1);

        let second_stream = viewer
            .imp()
            .video
            .get()
            .media_stream()
            .expect("second video should attach a stream");
        assert!(
            first_stream.as_ptr() != second_stream.as_ptr(),
            "switching to a different video must replace the attached stream"
        );
        assert!(
            viewer.imp().retired_video_stream.borrow().is_some(),
            "old video stream should be retired when navigating to a different video"
        );
    }
}
