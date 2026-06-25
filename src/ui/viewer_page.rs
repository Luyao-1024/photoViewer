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
use crate::core::db::{self, DbPool};
use crate::core::i18n::tr;
use crate::core::media::MediaItem;
use crate::core::metadata::{self, ExifField};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::core::{albums, trash};
use crate::ui::album_picker::AlbumPickerDialog;
use crate::ui::editor_panel::{EditorPanel, ToastKind};
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

/// 初次打开 viewer 时,缩略图栏向左右各加载的条数 —— "一栏" 大约能填满底部
/// 条可见宽度的项目数。中心对称,所以默认总条目数 = 2 * THUMB_INITIAL_HALF + 1。
/// Initial visible window per side: enough to fill ~one row of the bottom
/// strip. Total = 2*THUMB_INITIAL_HALF + 1 items centred on the current.
const THUMB_INITIAL_HALF: u32 = 5;

/// 用户滚动接近边缘时,每次懒加载追加的条数 —— "半栏"。滚动条触发后向一侧
/// 补这些,避免一次性预渲染全部缩略图导致 viewer 被撑大。
/// Lazy-load chunk per scroll-edge event: "half row" extension per side.
const THUMB_LAZY_HALF: u32 = 4;

/// 缩略图栏总条目硬上限,防止大图库场景下无限扩展。
/// Hard cap on total items kept in memory.
const THUMB_WINDOW_MAX: u32 = 40;

/// Direction hint the host receives from keyboard input. `i32::MIN` is the
/// "pop navigation" sentinel; other values are a delta on the current index.
pub type NavDelta = i32;
pub const NAV_POP: NavDelta = i32::MIN;

/// Callback the host registers for keyboard navigation. Shared via `Rc` so
/// closures capturing owned state can be cloned into GTK signal handlers.
pub type NavCallback = Rc<dyn Fn(NavDelta)>;
type ItemCallback = Rc<dyn Fn(i64)>;

/// Convert a `file://` URI stored on `MediaItem::uri` to a `PathBuf` for
/// the gdk-pixbuf loader. Anything without the `file://` prefix is treated
/// as a raw path (defensive — the scanner only emits `file://` URIs).
fn strip_file_uri(uri: &str) -> PathBuf {
    let stripped = uri.strip_prefix("file://").unwrap_or(uri);
    PathBuf::from(stripped)
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

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/viewer-page.ui")]
    pub struct ViewerPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub current_index: Cell<u32>,
        /// Per-`show_at` token: any older response is dropped on arrival.
        pub current_token: Cell<u64>,
        /// Cumulative zoom scale (1.0 = identity). GestureZoom multiplies into it.
        pub zoom_scale: Cell<f64>,
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
        /// Dynamic EXIF rows currently appended to `exif_group`.
        pub exif_rows: RefCell<Vec<adw::ActionRow>>,
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
        pub add_to_album_btn: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub favorite_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub name_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub path_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub mime_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub dimensions_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub size_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub modified_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub taken_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub exif_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub thumb_scrolled: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub thumb_strip: TemplateChild<gtk::Box>,
        #[template_child]
        pub prev_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub next_btn: TemplateChild<gtk::Button>,
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

    impl ObjectImpl for ViewerPage {}
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
        obj.apply_i18n();
        obj.setup_gesture();
        obj.setup_keyboard();
        obj.setup_edit_button();
        obj.setup_editor_callbacks();
        obj.setup_delete_button();
        obj.setup_details_panel();
        obj.setup_favorite_button();
        obj.setup_nav_buttons();
        obj.setup_thumb_strip_listener();
        obj.setup_navigation_pop_action();
        obj.setup_lifecycle_logging();
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
        imp.add_to_album_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.add_to_album")));
        imp.edit_btn.get().set_label(&tr("viewer.tooltip.edit"));
        imp.details_close_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.details.close")));
        imp.details_title
            .get()
            .set_label(&tr("viewer.details.title"));
    }

    /// Inject the `AdwNavigationView` and DB pool used to push an
    /// the editor panel when the Edit button is pressed. Call this after
    /// construction (mirrors `PhotosPage::set_nav_target`).
    pub fn set_edit_target(&self, nav: &adw::NavigationView, pool: DbPool) {
        tracing::debug!(
            "VIEWER_DEBUG set_edit_target index={} nav_visible={:?}",
            self.imp().current_index.get(),
            nav.visible_page().map(|page| page.title())
        );
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    /// Inject the `AdwNavigationView` and DB pool used by the "Add to Album"
    /// menu entry. The actual menu + handler are built lazily in `setup`
    /// once both are available, so this is idempotent.
    pub fn set_album_target(&self, nav: &adw::NavigationView, pool: DbPool) {
        tracing::debug!(
            "VIEWER_DEBUG set_album_target index={} nav_visible={:?}",
            self.imp().current_index.get(),
            nav.visible_page().map(|page| page.title())
        );
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
        *self.imp().pool.borrow_mut() = Some(pool);
        self.setup_album_menu();
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

    /// Build the menu shown by the "Add to Album" `MenuButton`. Constructed
    /// lazily after `set_album_target` is called (which is when the host
    /// nav view and DB pool are known). The menu currently has one item;
    /// more can be added without further wiring changes.
    fn setup_album_menu(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        let menu = gtk::gio::Menu::new();
        menu.append(Some(&tr("viewer.menu.add_to_album")), Some("album.add"));

        let action_group = gtk::gio::SimpleActionGroup::new();
        let add_action = gtk::gio::SimpleAction::new("add", None);
        add_action.connect_activate(move |_, _| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let nav = match this.imp().nav_view.borrow().as_ref() {
                Some(n) => n.clone(),
                None => {
                    tracing::warn!("ViewerPage: Add to Album pressed but nav_view not set");
                    return;
                }
            };
            let pool = match this.imp().pool.borrow().as_ref() {
                Some(p) => p.clone(),
                None => {
                    tracing::warn!("ViewerPage: Add to Album pressed but pool not set");
                    return;
                }
            };
            let Some(item) = this.current_media_item() else {
                return;
            };
            AlbumPickerDialog::present(&nav, pool, vec![item.id]);
        });
        action_group.add_action(&add_action);
        self.insert_action_group("album", Some(&action_group));

        imp.add_to_album_btn.get().set_menu_model(Some(&menu));
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
        self.imp().is_editing.set(true);
        self.imp().editor_split_view.get().set_show_sidebar(true);
        self.set_can_pop(false);
    }

    /// Hide the editor side-panel, restore the original image, and
    /// re-enable navigation gestures.
    fn stop_editing(&self) {
        let imp = self.imp();
        imp.is_editing.set(false);
        imp.editor_split_view.get().set_show_sidebar(false);

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
                    let result = trash::move_to_trash(&item_uri)
                        .and_then(|_| db::mark_trashed(&pool, item_id))
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
                let result = db::set_media_favorite(&pool, item_id, next_state);
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
        if is_favorite {
            button.set_label("★");
            button.add_css_class("favorite-active");
            button.set_tooltip_text(Some(&tr("viewer.button.favorite_active")));
        } else {
            button.set_label("☆");
            button.remove_css_class("favorite-active");
            button.set_tooltip_text(Some(&tr("viewer.button.favorite")));
        }
    }

    /// Wire the `<` / `>` filmstrip navigation buttons. They delegate to
    /// `fire_nav(±1)` so the host's navigation callback handles the actual
    /// index advance, exactly like keyboard arrow keys.
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
                this.fire_nav(-1);
            }
        });
        let weak = self.downgrade();
        imp.next_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.fire_nav(1);
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

        let in_window = end > start && current >= start && current < end;

        if in_window {
            self.update_thumb_highlight(current);
        } else {
            self.load_initial_thumb_window(current);
        }
        self.scroll_thumb_to_current();
    }

    /// First-time load: only `2*THUMB_INITIAL_HALF + 1` items centred on
    /// `current`. The strip's natural width is therefore bounded to ~one row
    /// of thumbnails regardless of album size, which prevents the viewer
    /// layer from being inflated by 65+ buttons as before.
    fn load_initial_thumb_window(&self, current: u32) {
        let Some(n_items) = self.list_n_items() else {
            return;
        };
        if n_items == 0 {
            return;
        }
        let (start, end) = compute_initial_thumb_window(current, n_items);
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
        self.rebuild_thumb_strip(new_start, new_end, current);

        // Clear the debounce flag on next idle so a subsequent scroll
        // past the new edge can extend again.
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.imp().thumb_pending_extend.set(None);
            }
        });
    }

    /// Tear down the existing strip and rebuild with `[start, end)`.
    /// Each item is a frame-less `GtkButton` wrapping a `GtkPicture` with
    /// `content-fit: contain` (preserves aspect ratio). After the thumbnail
    /// texture arrives, `width-request` is set so the button sizes to the
    /// image's aspect ratio at the fixed `THUMB_HEIGHT`.
    fn rebuild_thumb_strip(&self, start: u32, end: u32, current: u32) {
        let imp = self.imp();
        let strip = imp.thumb_strip.get();

        // Clear old items.
        let old = imp.thumb_items.borrow_mut().drain(..).collect::<Vec<_>>();
        for btn in &old {
            strip.remove(btn);
        }

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

        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .height_request(THUMB_HEIGHT)
            .can_shrink(true)
            .build();
        button.set_child(Some(&picture));

        // Request thumbnail. The ThumbnailLoader caches by `path + mtime`, so
        // extending the strip after the items were already requested once is a
        // cache hit (no extra decode).
        let item_uri = item.uri.clone();
        let item_mtime = std::time::SystemTime::from(item.file_mtime);
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(item_uri, ThumbnailSize::Small, Some(item_mtime), tx);

        let pic_weak = picture.downgrade();
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
            if tex_h > 0 {
                let w = ((THUMB_HEIGHT as f64) * tex_w as f64 / tex_h as f64).round() as i32;
                pic.set_width_request(w.max(36));
            }
        });

        // Click → navigate to this index.
        let weak = self.downgrade();
        button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let delta = idx as i32 - this.current_index() as i32;
                if delta != 0 {
                    this.fire_nav(delta);
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

    /// Compute the horizontal scroll value that centres the current item
    /// inside the visible strip. Returns `None` when the current item isn't
    /// in the loaded window yet (e.g. before the first `load_initial_thumb_window`).
    fn compute_thumb_target_value(&self) -> Option<f64> {
        let start = self.imp().thumb_window_start.get();
        let current = self.imp().current_index.get();
        if current < start {
            return None;
        }
        let offset = (current - start) as usize;
        let items = self.imp().thumb_items.borrow();
        let btn = items.get(offset)?;
        let scrolled = self.imp().thumb_scrolled.get();
        let hadj = scrolled.hadjustment();
        let alloc = btn.allocation();
        Some(compute_thumb_scroll_target(
            alloc.x() as f64,
            alloc.width() as f64,
            hadj.page_size(),
            hadj.upper(),
        ))
    }

    /// Move `thumb_scrolled`'s horizontal adjustment to reveal the current
    /// item. This intentionally snaps instead of animating: the adjustment's
    /// `value-changed` signal drives lazy loading, and animating that value
    /// during initial viewer allocation can produce high-frequency layout
    /// churn on GNOME.
    fn scroll_thumb_to_current(&self) {
        let Some(target) = self.compute_thumb_target_value() else {
            return;
        };
        let scrolled = self.imp().thumb_scrolled.get();
        let hadj = scrolled.hadjustment();
        let imp = self.imp();
        imp.thumb_programmatic_scroll.set(true);
        hadj.set_value(target);
        imp.thumb_programmatic_scroll.set(false);
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

    /// 从数据库异步同步当前图片收藏状态。与 `show_at()` 的 token 绑定，避免异步回写过期。
    fn sync_favorite_state(&self, item_id: i64) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            self.refresh_favorite_button(false);
            return;
        };

        let token = self.imp().current_token.get();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let result = db::is_media_favorite(&pool, item_id);
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
                "VIEWER_DEBUG details_btn clicked index={} before_revealed={} next_revealed={}",
                this.imp().current_index.get(),
                before,
                next
            );
            this.set_details_revealed(next, "details_btn");
            if next {
                if let Some(item) = this.current_media_item() {
                    tracing::debug!(
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
                "VIEWER_DEBUG details_close_btn clicked index={} before_revealed={}",
                this.imp().current_index.get(),
                split_view.shows_sidebar()
            );
            this.set_details_revealed(false, "details_close_btn");
            tracing::debug!(
                "VIEWER_DEBUG details_close_btn after set_reveal_child(false) revealed={}",
                split_view.shows_sidebar()
            );
            this.log_nav_state("details_close_btn immediate");
            let weak_after = this.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(this) = weak_after.upgrade() {
                    tracing::debug!(
                        "VIEWER_DEBUG details_close_btn idle_after revealed={} mapped={} visible={} root_is_some={}",
                        this.imp().details_split_view.get().shows_sidebar(),
                        this.is_mapped(),
                        this.is_visible(),
                        this.root().is_some()
                    );
                    this.log_nav_state("details_close_btn idle_after");
                } else {
                    tracing::debug!("VIEWER_DEBUG details_close_btn idle_after viewer_dropped");
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
            "VIEWER_DEBUG set_details_revealed reason={} index={} from={} to={} can_pop_before={}",
            reason,
            self.imp().current_index.get(),
            split_view.shows_sidebar(),
            revealed,
            self.can_pop()
        );

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
                    tracing::debug!("VIEWER_DEBUG restore_can_pop viewer_dropped");
                    return;
                };
                if !this.imp().details_split_view.get().shows_sidebar() {
                    this.set_can_pop(true);
                    tracing::debug!(
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
                        "VIEWER_DEBUG restore_can_pop skipped_details_open index={} can_pop={}",
                        this.imp().current_index.get(),
                        this.can_pop()
                    );
                }
            });
        }

        tracing::debug!(
            "VIEWER_DEBUG set_details_revealed done reason={} index={} revealed={} can_pop_after={}",
            reason,
            self.imp().current_index.get(),
            split_view.shows_sidebar(),
            self.can_pop()
        );
    }

    fn setup_lifecycle_logging(&self) {
        let weak = self.downgrade();
        self.connect_unmap(move |_| {
            if let Some(this) = weak.upgrade() {
                tracing::debug!(
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
            "VIEWER_DEBUG show_at requested_index={} current_before={} details_revealed={}",
            index,
            self.imp().current_index.get(),
            self.imp().details_split_view.get().shows_sidebar()
        );
        self.imp().current_index.set(index);
        self.imp().spinner.get().set_visible(true);

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
        self.set_title(item.display_name());
        self.sync_favorite_state(item.id);
        tracing::debug!(
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
        let path = strip_file_uri(&item.uri);
        tracing::debug!(
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
            let result = gdk_pixbuf::Pixbuf::from_file(&path)
                .map(|pb| gdk::Texture::for_pixbuf(&pb))
                .map_err(|e| format!("Pixbuf::from_file({path:?}) failed: {e}"));
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
            let uri = boxed.borrow::<MediaItem>().uri.clone();
            strip_file_uri(&uri)
        };
        gio::spawn_blocking(move || {
            // Result intentionally dropped — we only care that the file
            // got read into the page cache.
            let _ = gdk_pixbuf::Pixbuf::from_file(&path);
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
        gesture.connect_scale_changed(move |_, scale| {
            if let Some(this) = weak.upgrade() {
                let prev = this.imp().zoom_scale.get();
                let next = (prev * scale).clamp(0.25, 8.0);
                this.imp().zoom_scale.set(next);

                // GtkPicture has no first-class `set_transform` API; we
                // express the accumulated scale via the shared stylesheet.
                // The picture re-paints on the next frame.
                let picture = this.imp().picture.get();
                if let Some(provider) = this.imp().zoom_provider.borrow().as_ref() {
                    provider.load_from_data(&format!("picture {{ transform: scale({}); }}", next));
                }
                picture.queue_draw();
            }
        });
        self.imp().picture.get().add_controller(gesture);
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
                    this.fire_nav(1);
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Left => {
                if let Some(this) = weak.upgrade() {
                    if this.imp().is_editing.get() {
                        return glib::Propagation::Stop;
                    }
                    this.fire_nav(-1);
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
            "VIEWER_DEBUG update_details index={} name={} path={}",
            self.imp().current_index.get(),
            item.display_name(),
            item.path.display()
        );
        let imp = self.imp();
        imp.name_row.get().set_title(&tr("viewer.details.name"));
        imp.path_row.get().set_title(&tr("viewer.details.path"));
        imp.folder_row.get().set_title(&tr("viewer.details.folder"));
        imp.mime_row.get().set_title(&tr("viewer.details.type"));
        imp.dimensions_row
            .get()
            .set_title(&tr("viewer.details.dimensions"));
        imp.size_row.get().set_title(&tr("viewer.details.size"));
        imp.modified_row
            .get()
            .set_title(&tr("viewer.details.modified"));
        imp.taken_row
            .get()
            .set_title(&tr("viewer.details.captured"));
        imp.exif_group.get().set_title(&tr("viewer.details.exif"));
        imp.name_row.get().set_subtitle(item.display_name());
        imp.path_row
            .get()
            .set_subtitle(&item.path.to_string_lossy());
        imp.folder_row
            .get()
            .set_subtitle(&item.folder_path.to_string_lossy());
        imp.mime_row.get().set_subtitle(&item.mime_type);
        imp.dimensions_row
            .get()
            .set_subtitle(&format_dimensions(item.width, item.height));
        imp.size_row
            .get()
            .set_subtitle(&format_file_size(item.file_size));
        imp.modified_row
            .get()
            .set_subtitle(&format_datetime(Some(item.file_mtime)));
        imp.taken_row
            .get()
            .set_subtitle(&format_datetime(item.taken_at));

        self.set_exif_rows(vec![ExifField {
            tag: tr("viewer.exif.status"),
            value: tr("viewer.exif.loading"),
        }]);
        self.load_exif_details(item.path.clone(), self.imp().current_token.get());
    }

    fn load_exif_details(&self, path: PathBuf, token: u64) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let fields = metadata::extract(&path)
                .map(|m| m.exif_fields)
                .unwrap_or_default();
            let _ = tx.send(fields);
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(fields) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            if fields.is_empty() {
                this.set_exif_rows(vec![ExifField {
                    tag: tr("viewer.exif.status"),
                    value: tr("viewer.not_available"),
                }]);
            } else {
                this.set_exif_rows(fields);
            }
        });
    }

    fn set_exif_rows(&self, fields: Vec<ExifField>) {
        let imp = self.imp();
        let group = imp.exif_group.get();
        for row in imp.exif_rows.borrow_mut().drain(..) {
            group.remove(&row);
        }

        let mut rows = imp.exif_rows.borrow_mut();
        for field in fields {
            let row = adw::ActionRow::builder()
                .title(field.tag)
                .subtitle(field.value)
                .activatable(false)
                .build();
            group.add(&row);
            rows.push(row);
        }
    }
}

/// Pure calculation: scroll target that centres a thumbnail of width `btn_w`
/// at content x `btn_x` inside a viewport of width `page_size`, when the
/// total content width is `upper`. The result is clamped to the adjustment's
/// legal range `[0, upper - page_size]`.
fn compute_thumb_scroll_target(btn_x: f64, btn_w: f64, page_size: f64, upper: f64) -> f64 {
    let target = btn_x - page_size / 2.0 + btn_w / 2.0;
    let max_value = (upper - page_size).max(0.0);
    target.max(0.0).min(max_value)
}

/// Pure calculation: compute the initial `[start, end)` window centred on
/// `current`. The window is bounded by `[0, n_items)` and clips at the album
/// ends (no negative or out-of-bounds indices).
fn compute_initial_thumb_window(current: u32, n_items: u32) -> (u32, u32) {
    if n_items == 0 {
        return (0, 0);
    }
    let start = current.saturating_sub(THUMB_INITIAL_HALF);
    let end = current
        .saturating_add(THUMB_INITIAL_HALF)
        .saturating_add(1)
        .min(n_items);
    (start, end)
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
    fn initial_window_centred_on_current_in_middle_of_album() {
        // 100 photos, current = 50 → ±5 items centred, no clipping.
        let (start, end) = compute_initial_thumb_window(50, 100);
        assert_eq!(start, 45);
        assert_eq!(end, 56);
        assert_eq!(end - start, 2 * THUMB_INITIAL_HALF + 1);
    }

    #[test]
    fn initial_window_clips_at_album_start() {
        // current near 0 → start clamped to 0.
        let (start, end) = compute_initial_thumb_window(2, 100);
        assert_eq!(start, 0);
        assert_eq!(end, 2 + THUMB_INITIAL_HALF + 1);
        // Still centred enough to show current at index 2 of the window.
        assert!(end > 2);
    }

    #[test]
    fn initial_window_clips_at_album_end() {
        // current near the end → end clamped to n_items.
        let n = 100u32;
        let current = n - 2;
        let (start, end) = compute_initial_thumb_window(current, n);
        assert_eq!(end, n);
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
    fn initial_window_total_item_count_matches_docstring() {
        // Regression: ensures the "one row" count is what we promise — 11
        // items centred on the current image, regardless of album size.
        for n in [11u32, 100, 1000] {
            let current = n / 2;
            let (start, end) = compute_initial_thumb_window(current, n);
            // Bounded by n_items and never exceeds the half-window radius
            // on the constrained side.
            let radius = THUMB_INITIAL_HALF;
            let max_size = 2 * radius + 1;
            let actual = end - start;
            assert!(actual <= max_size, "n={n} actual={actual}");
        }
    }

    // ── scroll-to-current target calculation ─────────────────────────────

    /// Reasonable layout: page_size=300, 11 items, button width=60,
    /// spacing=6. Total upper = 720.
    const SCROLL_PAGE_SIZE: f64 = 300.0;
    const SCROLL_BTN_W: f64 = 60.0;
    const SCROLL_SPACING: f64 = 6.0;
    const SCROLL_UPPER: f64 = 11.0 * SCROLL_BTN_W + 10.0 * SCROLL_SPACING; // 720

    #[test]
    fn scroll_target_first_item_clamps_to_left_edge() {
        let target = compute_thumb_scroll_target(0.0, SCROLL_BTN_W, SCROLL_PAGE_SIZE, SCROLL_UPPER);
        assert_eq!(target, 0.0);
    }

    #[test]
    fn scroll_target_last_item_clamps_to_right_edge() {
        let btn_x = 10.0 * (SCROLL_BTN_W + SCROLL_SPACING);
        let target =
            compute_thumb_scroll_target(btn_x, SCROLL_BTN_W, SCROLL_PAGE_SIZE, SCROLL_UPPER);
        let max_value = SCROLL_UPPER - SCROLL_PAGE_SIZE;
        assert_eq!(target, max_value);
    }

    #[test]
    fn scroll_target_middle_item_centres_when_content_overflows_viewport() {
        let btn_x = 4.0 * (SCROLL_BTN_W + SCROLL_SPACING);
        let target =
            compute_thumb_scroll_target(btn_x, SCROLL_BTN_W, SCROLL_PAGE_SIZE, SCROLL_UPPER);
        assert!((target - 144.0).abs() < 0.5, "expected ~144, got {target}");
    }

    #[gtk::test]
    fn thumb_strip_template_starts_without_layout_spacers() {
        init_viewer_test();
        let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
        let viewer = ViewerPage::new(media_list, 0);

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

    fn sample_media_item() -> MediaItem {
        MediaItem {
            id: 1,
            uri: "file:///tmp/sample.jpg".into(),
            path: PathBuf::from("/tmp/sample.jpg"),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            width: Some(64),
            height: Some(48),
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1024,
            blake3_hash: "hash".into(),
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
}
