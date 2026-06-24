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
use crate::core::{albums, trash};
use crate::ui::album_picker::AlbumPickerDialog;
use crate::ui::editor_page::EditorPage;
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
type FavoriteStateCallback = Rc<dyn Fn(i64, bool)>;

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
        /// DB pool injected by host (needed to construct `EditorPage`).
        pub pool: RefCell<Option<DbPool>>,
        /// Navigation view used to push `EditorPage` when the user clicks Edit.
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        /// Dynamic EXIF rows currently appended to `exif_group`.
        pub exif_rows: RefCell<Vec<adw::ActionRow>>,
        /// 当前图片收藏状态（用于按钮即时渲染）。
        pub is_favorite: Cell<bool>,
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
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ViewerPage {
        const NAME: &'static str = "ViewerPage";
        type Type = super::ViewerPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
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
        obj.setup_delete_button();
        obj.setup_details_panel();
        obj.setup_favorite_button();
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
    /// `EditorPage` when the Edit button is pressed. Call this after
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

    /// Wire the Edit button: build an `EditorPage` for the currently
    /// displayed item, push it onto the host nav view, and wire its
    /// Cancel callback to pop the editor back to the viewer.
    fn setup_edit_button(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.edit_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let nav = match this.imp().nav_view.borrow().as_ref() {
                Some(n) => n.clone(),
                None => {
                    tracing::warn!("ViewerPage: Edit pressed but nav_view not set");
                    return;
                }
            };
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
            let editor = EditorPage::new(item, pool);
            // When the user presses Cancel in the editor, pop it from the
            // nav view and return to the viewer.
            let nav_for_cancel = nav.downgrade();
            editor.connect_cancel(move || {
                if let Some(n) = nav_for_cancel.upgrade() {
                    n.pop();
                }
            });
            nav.push(&editor);
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
            tracing::debug!(
                "VIEWER_DEBUG navigation.pop action index={} details_revealed={}",
                this.imp().current_index.get(),
                details_split_view.shows_sidebar()
            );
            if details_split_view.shows_sidebar() {
                this.set_details_revealed(false, "navigation.pop");
                tracing::debug!(
                    "VIEWER_DEBUG navigation.pop consumed_by_details index={} after_revealed={}",
                    this.imp().current_index.get(),
                    details_split_view.shows_sidebar()
                );
            } else {
                tracing::debug!(
                    "VIEWER_DEBUG navigation.pop forwarding NAV_POP index={}",
                    this.imp().current_index.get()
                );
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
        if idx >= list.n_items() {
            return None;
        }
        let obj = list.item(idx)?;
        // Clone the `MediaItem` out of the `BoxedAnyObject` while both the
        // outer `Ref` (held by `list`) and `obj`/`boxed` are alive. Using
        // `match` (rather than `?`) makes the drop order explicit so the
        // borrow of `boxed` does not extend past its lifetime.
        let boxed = match obj.downcast::<glib::BoxedAnyObject>() {
            Ok(b) => b,
            Err(_) => return None,
        };
        let item = (*boxed.borrow::<MediaItem>()).clone();
        Some(item)
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

        // Decode the current image off the main thread. `Pixbuf::from_file`
        // dispatches via gdk-pixbuf loaders (JPEG/PNG/HEIC/AVIF/...) and is
        // CPU-bound for big images — `spawn_blocking` keeps the UI responsive.
        // We use `gio::spawn_blocking` (matches `editor_page.rs`) rather than
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
                    tracing::debug!(
                        "VIEWER_DEBUG key Right index={} details_revealed={}",
                        this.imp().current_index.get(),
                        this.imp().details_split_view.get().shows_sidebar()
                    );
                    this.fire_nav(1);
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Left => {
                if let Some(this) = weak.upgrade() {
                    tracing::debug!(
                        "VIEWER_DEBUG key Left index={} details_revealed={}",
                        this.imp().current_index.get(),
                        this.imp().details_split_view.get().shows_sidebar()
                    );
                    this.fire_nav(-1);
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Escape => {
                if let Some(this) = weak.upgrade() {
                    let details_split_view = this.imp().details_split_view.get();
                    tracing::debug!(
                        "VIEWER_DEBUG key Escape index={} details_revealed={}",
                        this.imp().current_index.get(),
                        details_split_view.shows_sidebar()
                    );
                    if details_split_view.shows_sidebar() {
                        this.set_details_revealed(false, "key Escape");
                        tracing::debug!(
                            "VIEWER_DEBUG key Escape consumed_by_details index={} after_revealed={}",
                            this.imp().current_index.get(),
                            details_split_view.shows_sidebar()
                        );
                        return glib::Propagation::Stop;
                    }
                    tracing::debug!(
                        "VIEWER_DEBUG key Escape forwarding NAV_POP index={}",
                        this.imp().current_index.get()
                    );
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

    #[gtk::test]
    fn escape_closes_details_panel_without_navigation_pop() {
        let _ = gtk::init();
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
        let _ = gtk::init();
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
        let _ = gtk::init();
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
        let _ = gtk::init();
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
