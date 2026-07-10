//! Main window: sidebar + content area
mod albums;
mod navigation;
mod settings;
mod sidebar;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use glib::subclass::types::ObjectSubclassIsExt;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt, NavigationPageExt};

use crate::core::albums::Album;
use crate::core::db::DbPool;
use crate::core::db_actor::DbActorHandle;
use crate::core::i18n::tr;
use crate::core::media::MediaItem;
use crate::core::repository::MediaQuery;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::album_detail_page::AlbumDetailPage;
use crate::ui::TrashPage;
use crate::ui::{keyboard, PhotosPage, SearchPage, ViewerPage};
use albums::album_backfill_fetch_limit;
pub use albums::build_album_context_menu_for_tests;
pub(crate) use albums::refresh_after_album_operation;
pub(crate) use sidebar::refresh_albums_sidebar;
use sidebar::{build_albums_header_row, build_nav_row};

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    use chrono::Utc;

    use libadwaita::prelude::{ActionRowExt, PreferencesRowExt};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    pub(super) fn production_source(path: &str) -> String {
        let source = fs::read_to_string(path).unwrap_or_else(|err| panic!("read {path}: {err}"));
        ["\n#[cfg(test)]\nmod tests;"]
            .into_iter()
            .filter_map(|marker| source.find(marker))
            .min()
            .map(|index| source[..index].to_string())
            .unwrap_or(source)
    }

    pub(super) fn collect_labels(widget: &gtk::Widget, labels: &mut Vec<String>) {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            labels.push(label.label().to_string());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_labels(&current, labels);
        }
    }

    pub(super) fn collect_scales(widget: &gtk::Widget, scales: &mut Vec<gtk::Scale>) {
        if let Some(scale) = widget.downcast_ref::<gtk::Scale>() {
            scales.push(scale.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_scales(&current, scales);
        }
    }

    pub(super) fn collect_check_buttons(widget: &gtk::Widget, buttons: &mut Vec<gtk::CheckButton>) {
        if let Some(btn) = widget.downcast_ref::<gtk::CheckButton>() {
            buttons.push(btn.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_check_buttons(&current, buttons);
        }
    }

    pub(super) fn collect_preference_titles(widget: &gtk::Widget, titles: &mut Vec<String>) {
        if let Some(row) = widget.downcast_ref::<adw::PreferencesRow>() {
            titles.push(row.title().to_string());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_preference_titles(&current, titles);
        }
    }

    pub(super) fn find_action_row_subtitle(widget: &gtk::Widget, title: &str) -> Option<String> {
        if let Some(row) = widget.downcast_ref::<adw::ActionRow>() {
            if row.title() == title {
                return row.subtitle().map(|subtitle| subtitle.to_string());
            }
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            if let Some(subtitle) = find_action_row_subtitle(&current, title) {
                return Some(subtitle);
            }
        }
        None
    }

    pub(super) fn media_item(id: i64, folder_path: &str, name: &str) -> MediaItem {
        let folder_path = PathBuf::from(folder_path);
        let path = folder_path.join(name);
        MediaItem {
            id,
            uri: format!("file://{}", path.display()),
            path,
            folder_path,
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(100),
            height: Some(100),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 10,
            blake3_hash: format!("hash-{id}"),
            is_favorite: false,
            trashed_at: None,
        }
    }

    pub(super) fn sidebar_album(path: &str, name: &str, photo_count: i64) -> Album {
        Album {
            folder_path: PathBuf::from(path),
            name: name.into(),
            cover_uri: None,
            photo_count,
            last_modified: Utc::now(),
            is_virtual: false,
        }
    }

    pub(super) fn keyboard_media_item(id: i64) -> MediaItem {
        MediaItem {
            id,
            uri: format!("file:///tmp/keyboard-{id}.jpg"),
            path: PathBuf::from(format!("/tmp/keyboard-{id}.jpg")),
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
            blake3_hash: format!("keyboard-hash-{id}"),
            is_favorite: false,
            trashed_at: None,
        }
    }

    pub(super) fn keyboard_media_list() -> gtk::gio::ListStore {
        let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(keyboard_media_item(1)));
        media_list
    }

    pub(super) fn keyboard_thumbnail_loader() -> (tempfile::TempDir, Arc<ThumbnailLoader>) {
        let tmp = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-scope.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
        (tmp, loader)
    }

    pub(super) fn emit_key_for_tests<W: IsA<gtk::Widget>>(
        widget: &W,
        key: gtk::gdk::Key,
        state: gtk::gdk::ModifierType,
    ) -> bool {
        let controller = widget
            .observe_controllers()
            .snapshot()
            .into_iter()
            .find_map(|controller| controller.downcast::<gtk::EventControllerKey>().ok())
            .filter(|controller| {
                controller.name().as_deref() == Some("photo-viewer-keyboard-router")
            })
            .expect("keyboard router should be installed");
        controller.emit_by_name("key-pressed", &[&key, &0_u32, &state])
    }

    pub(super) fn media_list_uris(list: &gtk::gio::ListStore) -> Vec<String> {
        (0..list.n_items())
            .filter_map(|index| {
                list.item(index)
                    .and_downcast::<glib::BoxedAnyObject>()
                    .map(|boxed| boxed.borrow::<MediaItem>().uri.clone())
            })
            .collect()
    }
}

#[derive(Debug, Default)]
struct SidebarAlbumSnapshot {
    albums: Vec<Album>,
    media_type_albums: Vec<Album>,
    live_count: Option<u32>,
}

static SIDEBAR_SNAPSHOT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

/// What a sidebar row navigates to. The top list uses `targets[index]`, while
/// the stable bottom Trash list uses `trash_targets[index]`. The albums group
/// is a non-selectable header that only collapses/expands the dedicated album
/// list.
#[derive(Clone, Debug)]
pub enum SidebarTarget {
    Photos,
    AlbumsHeader,
    Trash,
}

mod imp {
    use super::*;
    use adw::subclass::prelude::*;

    #[derive(gtk::CompositeTemplate, gtk::glib::Properties, Default)]
    #[properties(wrapper_type = super::MainWindow)]
    #[template(file = "../../data/ui/window.ui")]
    pub struct MainWindow {
        pub pool: RefCell<Option<DbPool>>,
        pub db_actor: RefCell<Option<DbActorHandle>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        /// Index→target mirror of the sidebar ListBox, so the `row-selected`
        /// handler can dispatch by identity rather than a hardcoded index.
        pub targets: RefCell<Vec<SidebarTarget>>,
        /// Index→target mirror of the bottom Trash ListBox.
        pub trash_targets: RefCell<Vec<SidebarTarget>>,
        /// Index→album mirror of the dedicated album ListBox.
        pub album_targets: RefCell<Vec<Album>>,
        /// Index→virtual media type mirror of the dedicated media type ListBox.
        pub media_type_targets: RefCell<Vec<Album>>,
        /// The album rows nested under the "Albums" group header — kept so the
        /// live refresh and tests can inspect/rebuild them precisely.
        pub album_rows: RefCell<Vec<gtk::ListBoxRow>>,
        /// Rows nested under the "Media Types" group header.
        pub media_type_rows: RefCell<Vec<gtk::ListBoxRow>>,
        /// Right-aligned total live-media count on the Photos sidebar row.
        pub photos_count_label: RefCell<Option<gtk::Label>>,
        /// The disclosure arrow on the Albums header; swapped between
        /// `pan-down-symbolic` (expanded) and `pan-end-symbolic` (collapsed).
        pub albums_arrow: RefCell<Option<gtk::Image>>,
        /// Disclosure arrow on the Media Types header.
        pub media_types_arrow: RefCell<Option<gtk::Image>>,
        /// Whether the Albums group is currently expanded.
        pub albums_expanded: Cell<bool>,
        /// Whether the Media Types group is currently expanded.
        pub media_types_expanded: Cell<bool>,
        /// folder_path of the album whose `AlbumDetailPage` is on top of the
        /// stack, so a live refresh can re-select its sidebar row.
        pub active_album: RefCell<Option<PathBuf>>,
        /// Whether the sidebar album list is in batch-selection mode.
        pub album_selection_mode: Cell<bool>,
        /// Real album folder paths selected for batch delete. Virtual albums
        /// are deliberately excluded because they are saved views, not folders.
        pub selected_album_paths: RefCell<HashSet<PathBuf>>,
        /// Set while we programmatically `select_row`, so the `row-selected`
        /// handler does not re-enter navigation during a refresh.
        pub selecting_programmatically: Cell<bool>,
        /// Guard so diagnostic sidebar layout notify logging is connected once.
        pub sidebar_layout_trace_installed: Cell<bool>,
        pub settings_dialog: RefCell<Option<adw::Dialog>>,
        #[template_child]
        pub root_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub sidebar_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub trash_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub album_trash_wrapper: TemplateChild<gtk::Box>,
        #[template_child]
        pub sidebar_spacer: TemplateChild<gtk::Box>,
        #[template_child]
        pub album_scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub media_type_header_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub media_type_scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub album_selection_bar: TemplateChild<gtk::ActionBar>,
        #[template_child]
        pub album_selection_cancel_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub album_selection_delete_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub album_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub media_type_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub nav_view: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub sidebar_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub settings_button: TemplateChild<gtk::Button>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "PhotoViewerWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::glib::derived_properties]
    impl ObjectImpl for MainWindow {}
    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}
    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}
}

gtk::glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends adw::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Self {
        let window: Self = gtk::glib::Object::builder()
            .property("application", app)
            .property("title", tr("app.title"))
            .build();
        keyboard::router::install(
            &window,
            {
                let weak = window.downgrade();
                move || {
                    weak.upgrade()
                        .map(|window| window.resolve_keyboard_scope())
                        .unwrap_or(keyboard::KeyboardScope::Global)
                }
            },
            {
                let weak = window.downgrade();
                move |action| {
                    weak.upgrade()
                        .map(|window| window.handle_keyboard_action(action))
                        .unwrap_or(keyboard::KeyboardResult::Ignored)
                }
            },
        );
        window
    }

    fn resolve_keyboard_scope(&self) -> keyboard::KeyboardScope {
        let root_scope = keyboard::router::scope_for_focus(self.upcast_ref());
        if root_scope == keyboard::KeyboardScope::TextInput {
            return root_scope;
        }
        if self.imp().settings_dialog.borrow().is_some() {
            return keyboard::KeyboardScope::Modal;
        }
        if widget_tree_has_visible_class(
            self.imp().root_overlay.get().upcast_ref(),
            "glass-context-menu-layer",
        ) {
            return keyboard::KeyboardScope::Modal;
        }

        let Some(page) = self.imp().nav_view.get().visible_page() else {
            return keyboard::KeyboardScope::Global;
        };

        if let Ok(viewer) = page.clone().downcast::<ViewerPage>() {
            if viewer.is_editing_keyboard_scope() {
                return keyboard::KeyboardScope::Editor;
            }
            return keyboard::KeyboardScope::Viewer;
        }

        if page.clone().downcast::<PhotosPage>().is_ok()
            || page.clone().downcast::<AlbumDetailPage>().is_ok()
            || page.clone().downcast::<SearchPage>().is_ok()
            || page.clone().downcast::<TrashPage>().is_ok()
        {
            return keyboard::KeyboardScope::Browsing;
        }

        keyboard::KeyboardScope::Global
    }

    fn handle_keyboard_action(&self, action: keyboard::KeyboardAction) -> keyboard::KeyboardResult {
        let settings_dialog = self.imp().settings_dialog.borrow().as_ref().cloned();
        if let Some(dialog) = settings_dialog {
            return match action {
                keyboard::KeyboardAction::CancelOrClose => {
                    self.close_settings_dialog(dialog);
                    keyboard::KeyboardResult::Handled
                }
                _ => keyboard::KeyboardResult::Ignored,
            };
        }
        if self.resolve_keyboard_scope() == keyboard::KeyboardScope::Modal {
            return keyboard::KeyboardResult::Ignored;
        }

        if let Some(page) = self.imp().nav_view.get().visible_page() {
            if let Ok(viewer) = page.clone().downcast::<ViewerPage>() {
                let result = viewer.handle_keyboard_action(action);
                if result.is_handled() {
                    return result;
                }
            }
            if let Ok(photos) = page.clone().downcast::<PhotosPage>() {
                let result = photos.handle_keyboard_action(action);
                if result.is_handled() {
                    return result;
                }
            }
        }

        match action {
            keyboard::KeyboardAction::Search => {
                if self.open_search_page() {
                    keyboard::KeyboardResult::Handled
                } else {
                    keyboard::KeyboardResult::Ignored
                }
            }
            keyboard::KeyboardAction::OpenSettings => {
                self.show_settings_dialog();
                keyboard::KeyboardResult::Handled
            }
            keyboard::KeyboardAction::NavigateBack | keyboard::KeyboardAction::CancelOrClose => {
                if self.imp().nav_view.get().pop() {
                    keyboard::KeyboardResult::Handled
                } else {
                    keyboard::KeyboardResult::Ignored
                }
            }
            _ => keyboard::KeyboardResult::Ignored,
        }
    }

    #[cfg(test)]
    pub(crate) fn keyboard_scope_for_tests(&self) -> keyboard::KeyboardScope {
        self.resolve_keyboard_scope()
    }

    /// Build the static sidebar skeleton: Photos, the collapsible Albums group
    /// header, and Trash. The album rows nested under the header are
    /// inserted later by [`Self::populate_album_rows`] once the DB pool is
    /// available (the app calls it after `set_resources`).
    pub fn populate_sidebar(&self) {
        self.imp()
            .sidebar_page
            .get()
            .set_title(&tr("window.sidebar"));
        self.imp().albums_expanded.set(true);
        self.imp().media_types_expanded.set(true);
        let list = self.imp().sidebar_list.get();
        let trash_list = self.imp().trash_list.get();
        let mut targets = Vec::new();
        let mut trash_targets = Vec::new();

        let (photos_row, photos_count_label) =
            build_nav_row(&tr("sidebar.photos"), "view-grid-symbolic", true);
        *self.imp().photos_count_label.borrow_mut() = photos_count_label;
        list.append(&photos_row);
        targets.push(SidebarTarget::Photos);

        // Albums group header: non-selectable (so it never claims the
        // single-selection slot) and toggles its children via a click gesture.
        let (header_row, arrow) = build_albums_header_row(&tr("sidebar.albums"));
        header_row.set_selectable(false);
        {
            let weak = self.downgrade();
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                if let Some(window) = weak.upgrade() {
                    window.toggle_albums_expanded();
                }
            });
            header_row.add_controller(gesture);
        }
        *self.imp().albums_arrow.borrow_mut() = Some(arrow);
        list.append(&header_row);
        targets.push(SidebarTarget::AlbumsHeader);

        let media_type_header_list = self.imp().media_type_header_list.get();
        let (media_types_row, media_types_arrow) =
            build_albums_header_row(&tr("sidebar.media_types"));
        media_types_row.set_selectable(false);
        {
            let weak = self.downgrade();
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                if let Some(window) = weak.upgrade() {
                    window.toggle_media_types_expanded();
                }
            });
            media_types_row.add_controller(gesture);
        }
        *self.imp().media_types_arrow.borrow_mut() = Some(media_types_arrow);
        media_type_header_list.append(&media_types_row);

        let (trash_row, _) = build_nav_row(&tr("sidebar.trash"), "user-trash-symbolic", false);
        trash_list.append(&trash_row);
        trash_targets.push(SidebarTarget::Trash);

        self.imp()
            .settings_button
            .set_tooltip_text(Some(&tr("sidebar.settings")));

        *self.imp().targets.borrow_mut() = targets;
        *self.imp().trash_targets.borrow_mut() = trash_targets;

        // Highlight Photos as the default root view. Done before
        // connect_sidebar wires row-selected, so this never triggers navigation.
        self.imp().selecting_programmatically.set(true);
        list.select_row(Some(&photos_row));
        self.imp().selecting_programmatically.set(false);
    }

    /// Accessor for the content area's NavigationView (used by later tasks).
    pub fn nav_view(&self) -> adw::NavigationView {
        self.imp().nav_view.get()
    }

    /// Inject the DB pool and thumbnail loader so the sidebar can construct
    /// pages on demand. Called from `app::build_app` once initialization
    /// (DB + scan) has completed.
    pub fn set_resources(
        &self,
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
        media_list: gtk::gio::ListStore,
    ) {
        *self.imp().pool.borrow_mut() = Some(pool);
        *self.imp().loader.borrow_mut() = Some(loader);
        *self.imp().media_list.borrow_mut() = Some(media_list);
        self.update_photos_count_label();
    }

    pub fn set_db_actor(&self, db_actor: DbActorHandle) {
        *self.imp().db_actor.borrow_mut() = Some(db_actor);
    }
    fn build_trash_page(&self) -> Option<TrashPage> {
        let pool = self.imp().pool.borrow().clone()?;
        let loader = self.imp().loader.borrow().clone()?;
        let media_list = self.imp().media_list.borrow().clone()?;
        let db_actor = self.imp().db_actor.borrow().as_ref().cloned()?;
        Some(TrashPage::with_media_list_and_actor(
            pool, loader, media_list, db_actor,
        ))
    }

    /// Reload the bounded live-media window backing the Photos page. This is
    /// used by album operations that mutate DB rows outside the DB actor event
    /// stream, such as Copy/Move from the album picker.
    pub fn refresh_shared_media_list_from_repository(&self) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let limit =
            u32::try_from(crate::ui::apply_to_media_list::ui_media_list_cap()).unwrap_or(u32::MAX);
        let items = crate::core::repository::MediaRepository::new(pool)
            .items(crate::core::repository::MediaQuery::LiveAll, 0, limit)
            .unwrap_or_default();
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "PHOTO_REFRESH_TRACE shared_refresh_loaded current_len={} refreshed_len={} limit={}",
            media_list.n_items(),
            items.len(),
            limit
        );
        apply_media_projection_to_list(&media_list, items);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "PHOTO_REFRESH_TRACE shared_refresh_applied final_len={}",
            media_list.n_items()
        );
        self.update_photos_count_label_from_db();
    }

    fn show_settings_dialog(&self) {
        if self.imp().settings_dialog.borrow().is_some() {
            return;
        }

        self.imp()
            .nav_view
            .add_css_class("settings-background-blur");
        let host = self.clone().upcast::<gtk::Widget>();
        let dialog = self.build_settings_dialog(&host);
        self.imp()
            .settings_dialog
            .borrow_mut()
            .replace(dialog.clone());
        let weak = self.downgrade();
        dialog.connect_closed(move |dialog| {
            if let Some(window) = weak.upgrade() {
                window.close_settings_dialog_state(dialog);
            }
        });
        dialog.present(self);
    }

    fn close_settings_dialog(&self, dialog: adw::Dialog) {
        self.close_settings_dialog_state(&dialog);
        dialog.close();
    }

    fn close_settings_dialog_state(&self, dialog: &adw::Dialog) {
        self.imp()
            .nav_view
            .remove_css_class("settings-background-blur");
        let should_take = self
            .imp()
            .settings_dialog
            .borrow()
            .as_ref()
            .is_some_and(|current| current == dialog);
        if should_take {
            self.imp().settings_dialog.borrow_mut().take();
        }
    }
}

fn apply_media_projection_to_list(list: &gtk::gio::ListStore, refreshed: Vec<MediaItem>) {
    let current_len = list.n_items();
    if apply_pure_media_insertions(list, &refreshed) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "PHOTO_REFRESH_TRACE projection_strategy=pure_insert before_len={} refreshed_len={} after_len={}",
            current_len,
            refreshed.len(),
            list.n_items()
        );
        return;
    }
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "PHOTO_REFRESH_TRACE projection_strategy=full_replace before_len={} refreshed_len={}",
        current_len,
        refreshed.len()
    );
    let additions: Vec<glib::BoxedAnyObject> = refreshed
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect();
    list.splice(0, list.n_items(), &additions);
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "PHOTO_REFRESH_TRACE projection_full_replace_applied after_len={}",
        list.n_items()
    );
}

fn apply_pure_media_insertions(list: &gtk::gio::ListStore, refreshed: &[MediaItem]) -> bool {
    let current = media_items_from_store(list);
    if refreshed.len() <= current.len() {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "PHOTO_REFRESH_TRACE pure_insert_rejected reason=not_growth current_len={} refreshed_len={}",
            current.len(),
            refreshed.len()
        );
        return false;
    }

    let mut prefix = 0usize;
    while prefix < current.len()
        && prefix < refreshed.len()
        && same_media_identity(&current[prefix], &refreshed[prefix])
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < current.len().saturating_sub(prefix)
        && same_media_identity(
            &current[current.len() - 1 - suffix],
            &refreshed[refreshed.len() - 1 - suffix],
        )
    {
        suffix += 1;
    }

    if prefix + suffix != current.len() {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "PHOTO_REFRESH_TRACE pure_insert_rejected reason=identity_mismatch current_len={} refreshed_len={} prefix={} suffix={}",
            current.len(),
            refreshed.len(),
            prefix,
            suffix
        );
        return false;
    }

    let insert_end = refreshed.len() - suffix;
    if insert_end <= prefix {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "PHOTO_REFRESH_TRACE pure_insert_rejected reason=empty_insert current_len={} refreshed_len={} prefix={} suffix={}",
            current.len(),
            refreshed.len(),
            prefix,
            suffix
        );
        return false;
    }
    let additions: Vec<glib::BoxedAnyObject> = refreshed[prefix..insert_end]
        .iter()
        .cloned()
        .map(glib::BoxedAnyObject::new)
        .collect();
    let inserted_ids: Vec<i64> = refreshed[prefix..insert_end]
        .iter()
        .take(8)
        .map(|item| item.id)
        .collect();
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "PHOTO_REFRESH_TRACE pure_insert_apply position={} added={} current_len={} refreshed_len={} inserted_ids_first8={:?}",
        prefix,
        insert_end - prefix,
        current.len(),
        refreshed.len(),
        inserted_ids
    );
    list.splice(prefix as u32, 0, &additions);
    true
}

fn media_items_from_store(list: &gtk::gio::ListStore) -> Vec<MediaItem> {
    let mut items = Vec::with_capacity(list.n_items() as usize);
    for idx in 0..list.n_items() {
        let Some(obj) = list.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        items.push((*boxed.borrow::<MediaItem>()).clone());
    }
    items
}

fn same_media_identity(a: &MediaItem, b: &MediaItem) -> bool {
    a.id == b.id && a.uri == b.uri
}

fn widget_or_ancestor_has_class(widget: &gtk::Widget, class_name: &str) -> bool {
    let mut current = Some(widget.clone());
    while let Some(w) = current {
        if w.css_classes()
            .iter()
            .any(|class| class.as_str() == class_name)
        {
            return true;
        }
        current = w.parent();
    }
    false
}

fn widget_tree_has_visible_class(widget: &gtk::Widget, class_name: &str) -> bool {
    if widget.is_visible()
        && widget
            .css_classes()
            .iter()
            .any(|class| class.as_str() == class_name)
    {
        return true;
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        if widget_tree_has_visible_class(&current, class_name) {
            return true;
        }
        child = current.next_sibling();
    }

    false
}

fn build_about_label() -> gtk::Label {
    let text = format!(
        "{} {} - Wang Luyao - {}",
        tr("app.title"),
        env!("CARGO_PKG_VERSION"),
        tr("setting.about.license_value")
    );
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .css_classes(["settings-about-text"])
        .build()
}

fn backfill_album_media_list(
    list: gtk::gio::ListStore,
    pool: DbPool,
    query: MediaQuery,
    start: u32,
    total: u32,
    album_name: String,
    album_path: String,
) {
    let schedule_span = tracing::info_span!(
        "album:backfill_schedule",
        album_name = %album_name,
        album_path = %album_path,
        ?query,
        start,
        total
    );
    let _schedule = schedule_span.enter();
    let limit = album_backfill_fetch_limit(start, total);
    if limit == 0 {
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            ?query,
            start,
            total,
            "album_backfill: skipped_at_cap"
        );
        return;
    }
    glib::spawn_future_local(async move {
        let fetch_span = tracing::info_span!(
            "album:backfill_fetch",
            album_name = %album_name,
            album_path = %album_path,
            ?query,
            start,
            total,
            limit
        );
        let _fetch = fetch_span.enter();
        let query_for_worker = query.clone();
        let result = gtk::gio::spawn_blocking(move || {
            crate::core::repository::MediaRepository::new(pool)
                .page(query_for_worker, start, limit)
                .map(|page| page.items)
        })
        .await;

        let items = match result {
            Ok(Ok(items)) => items,
            Ok(Err(err)) => {
                tracing::warn!(
                    target: crate::core::log_targets::ALBUMS,
                    album_name = %album_name,
                    album_path = %album_path,
                    ?query,
                    start,
                    total,
                    limit,
                    "album_backfill: fetch_failed error={err}"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(
                    target: crate::core::log_targets::ALBUMS,
                    album_name = %album_name,
                    album_path = %album_path,
                    ?query,
                    start,
                    total,
                    limit,
                    "album_backfill: join_failed error={err:?}"
                );
                return;
            }
        };

        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            ?query,
            start,
            total,
            limit,
            fetched = items.len(),
            "album_backfill: fetched"
        );
        append_album_items_in_chunks(list, items, album_name, album_path, start, total);
    });
}

fn append_album_items_in_chunks(
    list: gtk::gio::ListStore,
    items: Vec<crate::core::media::MediaItem>,
    album_name: String,
    album_path: String,
    start: u32,
    total: u32,
) {
    const CHUNK_SIZE: usize = 500;
    let mut chunks = items.into_iter();
    let mut appended = 0usize;
    glib::idle_add_local(move || {
        let chunk: Vec<glib::BoxedAnyObject> = chunks
            .by_ref()
            .take(CHUNK_SIZE)
            .map(glib::BoxedAnyObject::new)
            .collect();
        if chunk.is_empty() {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                start,
                total,
                appended,
                list_items = list.n_items(),
                "album_backfill: appended"
            );
            return glib::ControlFlow::Break;
        }
        let old_len = list.n_items();
        appended += chunk.len();
        list.splice(old_len, 0, &chunk);
        glib::ControlFlow::Continue
    });
}

fn show_trash_operation_error_dialog(parent: &gtk::Widget, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("trash.operation_failed"))
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
}

fn pop_to_photos_root(nav_view: &adw::NavigationView) {
    while nav_view.pop() {}
}

fn visible_page_is_trash(nav_view: &adw::NavigationView) -> bool {
    nav_view
        .visible_page()
        .map(|page| is_trash_page(&page))
        .unwrap_or(false)
}

fn is_trash_page(page: &adw::NavigationPage) -> bool {
    page.clone().downcast::<TrashPage>().is_ok()
}

#[cfg(test)]
mod tests;
