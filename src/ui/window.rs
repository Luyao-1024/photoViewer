//! Main window: sidebar + content area
mod albums;
mod settings;
mod sidebar;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use glib::subclass::types::ObjectSubclassIsExt;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{
    ActionRowExt, AdwDialogExt, AlertDialogExt, NavigationPageExt, PreferencesGroupExt,
    PreferencesRowExt,
};
use serde_json::{Map, Value};

use crate::config;
use crate::core::albums::{list_media_type_albums, list_with_favorites, Album};
use crate::core::db::DbPool;
use crate::core::db_actor::DbActorHandle;
use crate::core::i18n::{locale, tr, trf};
use crate::core::media::MediaItem;
use crate::core::repository::MediaMutation;
use crate::core::repository::MediaQuery;
use crate::core::thumbnails::ThumbnailLoader;
use crate::core::{prefs, runtime_config};
use crate::ui::album_detail_page::{media_query_for_album, AlbumDetailPage};
use crate::ui::TrashPage;
use crate::ui::{grid_css, keyboard, theme, PhotosPage, SearchPage, ViewerPage};
pub use albums::build_album_context_menu_for_tests;
pub(crate) use albums::refresh_after_album_operation;
#[cfg(test)]
use albums::remove_deleted_album_media_from_media_list;
use albums::{album_backfill_fetch_limit, album_initial_load_limit};
#[cfg(test)]
use settings::restart_spec_from_for_tests;
use settings::{add_excluded_scan_path, show_settings_error_dialog};
pub(crate) use sidebar::refresh_albums_sidebar;
use sidebar::{
    build_album_row, build_albums_header_row, build_nav_row, find_sidebar_album_identity_index,
    load_sidebar_album_snapshot, same_sidebar_album_identities, same_sidebar_album_identity,
    sidebar_album_identities_are_ordered_subset, sidebar_album_identity_for_log,
    sidebar_album_summary, sidebar_list_child_count, update_album_row_in_place,
};

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

    /// Insert the folder + virtual albums in the dedicated album list, fetched
    /// from the current DB snapshot. Called once after `set_resources` (and
    /// again by [`Self::refresh_album_rows`] on live changes). Safe to call
    /// before `connect_sidebar`; it only touches album rows + album targets.
    pub fn populate_album_rows(&self) {
        self.update_photos_count_label_from_db();
        self.rebuild_album_rows();
        self.rebuild_media_type_rows();
    }

    #[tracing::instrument(name = "sidebar:rebuild_album_rows", skip(self))]
    fn rebuild_album_rows(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let albums = list_with_favorites(&pool).unwrap_or_default();
        self.apply_album_rows(albums);
    }

    #[tracing::instrument(name = "sidebar:apply_album_rows", skip(self, albums))]
    fn apply_album_rows(&self, albums: Vec<Album>) {
        let album_list = self.imp().album_list.get();
        let current_targets = self.imp().album_targets.borrow().clone();
        let current_rows = self.imp().album_rows.borrow().clone();
        let same_identities = same_sidebar_album_identities(&current_targets, &albums);
        let ordered_subset = current_rows.len() == current_targets.len()
            && sidebar_album_identities_are_ordered_subset(&current_targets, &albums);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE album_rows_begin current_targets={} current_rows={} list_children={} incoming={} same_identities={} ordered_subset={} expanded={} scroll_visible={} scroll_height={} wrapper_height={} current=[{}] incoming=[{}]",
            current_targets.len(),
            current_rows.len(),
            sidebar_list_child_count(&album_list),
            albums.len(),
            same_identities,
            ordered_subset,
            self.imp().albums_expanded.get(),
            self.imp().album_scroll.is_visible(),
            self.imp().album_scroll.height(),
            self.imp().album_trash_wrapper.height(),
            sidebar_album_summary(&current_targets),
            sidebar_album_summary(&albums)
        );
        if same_identities && current_rows.len() == albums.len() {
            let loader = self.imp().loader.borrow().as_ref().cloned();
            for ((row, previous), album) in current_rows
                .iter()
                .zip(current_targets.iter())
                .zip(albums.iter())
            {
                update_album_row_in_place(row, previous, album, loader.clone());
            }
            let album_count = albums.len();
            *self.imp().album_targets.borrow_mut() = albums;
            self.reselect_active_album_row();
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_ALBUM_UPDATE_IN_PLACE rows={}",
                album_count
            );
            self.log_sidebar_layout_state("album_rows_same_identities_after");
            self.log_sidebar_layout_state_next_idle("album_rows_same_identities_after");
            return;
        }
        if ordered_subset {
            let loader = self.imp().loader.borrow().as_ref().cloned();
            let mut next_rows = Vec::with_capacity(albums.len());
            let mut previous_targets = Vec::with_capacity(albums.len());
            for album in &albums {
                if let Some(index) = find_sidebar_album_identity_index(&current_targets, album) {
                    next_rows.push(current_rows[index].clone());
                    previous_targets.push(current_targets[index].clone());
                }
            }
            for ((row, previous), album) in next_rows
                .iter()
                .zip(previous_targets.iter())
                .zip(albums.iter())
            {
                update_album_row_in_place(row, previous, album, loader.clone());
            }
            for (index, row) in current_rows.iter().enumerate().rev() {
                if !albums
                    .iter()
                    .any(|album| same_sidebar_album_identity(&current_targets[index], album))
                {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SIDEBAR_TRACE album_rows_remove_missing index={} identity={}",
                        index,
                        sidebar_album_identity_for_log(&current_targets[index])
                    );
                    album_list.remove(row);
                }
            }
            let album_count = albums.len();
            *self.imp().album_rows.borrow_mut() = next_rows;
            *self.imp().album_targets.borrow_mut() = albums;
            self.reselect_active_album_row();
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_ALBUM_REMOVE_IN_PLACE rows={}",
                album_count
            );
            self.log_sidebar_layout_state("album_rows_ordered_subset_after");
            self.log_sidebar_layout_state_next_idle("album_rows_ordered_subset_after");
            return;
        }

        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE album_rows_rebuild_clear begin list_children={} current_rows={} incoming={} expanded={} scroll_visible_before={} scroll_height_before={} reason=identity_insert_or_reorder current=[{}] incoming=[{}]",
            sidebar_list_child_count(&album_list),
            current_rows.len(),
            albums.len(),
            self.imp().albums_expanded.get(),
            self.imp().album_scroll.is_visible(),
            self.imp().album_scroll.height(),
            sidebar_album_summary(&current_targets),
            sidebar_album_summary(&albums)
        );
        while let Some(child) = album_list.first_child() {
            album_list.remove(&child);
        }
        self.imp().album_rows.borrow_mut().clear();
        self.imp().album_targets.borrow_mut().clear();

        let album_count = albums.len();
        let expanded = self.imp().albums_expanded.get();
        self.imp().album_scroll.set_visible(expanded);

        for album in albums {
            let row = build_album_row(&album, self.imp().loader.borrow().as_ref().cloned());
            row.set_visible(true);
            self.attach_album_dnd(&row, album.folder_path.to_string_lossy().into_owned());
            self.attach_album_context_menu(&row, album.clone());
            album_list.append(&row);
            self.imp().album_rows.borrow_mut().push(row);
            self.imp().album_targets.borrow_mut().push(album);
        }

        self.reselect_active_album_row();
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_ALBUM_REBUILD rows={}",
            album_count
        );
        self.log_sidebar_layout_state("album_rows_rebuild_after");
        self.log_sidebar_layout_state_next_idle("album_rows_rebuild_after");
    }

    fn rebuild_media_type_rows(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let albums = list_media_type_albums(&pool).unwrap_or_default();
        self.apply_media_type_rows(albums);
    }

    fn apply_media_type_rows(&self, albums: Vec<Album>) {
        let media_type_list = self.imp().media_type_list.get();

        let has_media_types = !albums.is_empty();
        let expanded = self.imp().media_types_expanded.get();
        let header_visible_before = self.imp().media_type_header_list.is_visible();
        let scroll_visible_before = self.imp().media_type_scroll.is_visible();
        self.imp()
            .media_type_header_list
            .set_visible(has_media_types);
        self.imp()
            .media_type_scroll
            .set_visible(has_media_types && expanded);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE media_type_visibility has_media_types={} expanded={} header_visible_before={} header_visible_after={} scroll_visible_before={} scroll_visible_after={} scroll_height={}",
            has_media_types,
            expanded,
            header_visible_before,
            self.imp().media_type_header_list.is_visible(),
            scroll_visible_before,
            self.imp().media_type_scroll.is_visible(),
            self.imp().media_type_scroll.height()
        );

        let current_targets = self.imp().media_type_targets.borrow().clone();
        let current_rows = self.imp().media_type_rows.borrow().clone();
        let same_identities = same_sidebar_album_identities(&current_targets, &albums);
        let ordered_subset = current_rows.len() == current_targets.len()
            && sidebar_album_identities_are_ordered_subset(&current_targets, &albums);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE media_type_rows_begin current_targets={} current_rows={} list_children={} incoming={} same_identities={} ordered_subset={} current=[{}] incoming=[{}]",
            current_targets.len(),
            current_rows.len(),
            sidebar_list_child_count(&media_type_list),
            albums.len(),
            same_identities,
            ordered_subset,
            sidebar_album_summary(&current_targets),
            sidebar_album_summary(&albums)
        );
        if same_identities && current_rows.len() == albums.len() {
            let loader = self.imp().loader.borrow().as_ref().cloned();
            for ((row, previous), album) in current_rows
                .iter()
                .zip(current_targets.iter())
                .zip(albums.iter())
            {
                update_album_row_in_place(row, previous, album, loader.clone());
            }
            *self.imp().media_type_targets.borrow_mut() = albums;
            self.reselect_active_album_row();
            self.log_sidebar_layout_state("media_type_rows_same_identities_after");
            self.log_sidebar_layout_state_next_idle("media_type_rows_same_identities_after");
            return;
        }
        if ordered_subset {
            let loader = self.imp().loader.borrow().as_ref().cloned();
            let mut next_rows = Vec::with_capacity(albums.len());
            let mut previous_targets = Vec::with_capacity(albums.len());
            for album in &albums {
                if let Some(index) = find_sidebar_album_identity_index(&current_targets, album) {
                    next_rows.push(current_rows[index].clone());
                    previous_targets.push(current_targets[index].clone());
                }
            }
            for ((row, previous), album) in next_rows
                .iter()
                .zip(previous_targets.iter())
                .zip(albums.iter())
            {
                update_album_row_in_place(row, previous, album, loader.clone());
            }
            for (index, row) in current_rows.iter().enumerate().rev() {
                if !albums
                    .iter()
                    .any(|album| same_sidebar_album_identity(&current_targets[index], album))
                {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SIDEBAR_TRACE media_type_rows_remove_missing index={} identity={}",
                        index,
                        sidebar_album_identity_for_log(&current_targets[index])
                    );
                    media_type_list.remove(row);
                }
            }
            *self.imp().media_type_rows.borrow_mut() = next_rows;
            *self.imp().media_type_targets.borrow_mut() = albums;
            self.reselect_active_album_row();
            self.log_sidebar_layout_state("media_type_rows_ordered_subset_after");
            self.log_sidebar_layout_state_next_idle("media_type_rows_ordered_subset_after");
            return;
        }
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE media_type_rows_rebuild_clear begin list_children={} current_rows={} incoming={} scroll_visible={} scroll_height={} reason=identity_insert_or_reorder current=[{}] incoming=[{}]",
            sidebar_list_child_count(&media_type_list),
            current_rows.len(),
            albums.len(),
            self.imp().media_type_scroll.is_visible(),
            self.imp().media_type_scroll.height(),
            sidebar_album_summary(&current_targets),
            sidebar_album_summary(&albums)
        );
        while let Some(child) = media_type_list.first_child() {
            media_type_list.remove(&child);
        }
        self.imp().media_type_rows.borrow_mut().clear();
        self.imp().media_type_targets.borrow_mut().clear();

        for album in albums {
            let row = build_album_row(&album, self.imp().loader.borrow().as_ref().cloned());
            row.set_visible(true);
            media_type_list.append(&row);
            self.imp().media_type_rows.borrow_mut().push(row);
            self.imp().media_type_targets.borrow_mut().push(album);
        }

        self.reselect_active_album_row();
        self.log_sidebar_layout_state("media_type_rows_rebuild_after");
        self.log_sidebar_layout_state_next_idle("media_type_rows_rebuild_after");
    }

    #[tracing::instrument(name = "sidebar:apply_album_snapshot", skip(self, snapshot))]
    fn apply_sidebar_album_snapshot(&self, snapshot: SidebarAlbumSnapshot) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE snapshot_apply_begin live_count={:?} albums={} media_types={} albums_summary=[{}] media_types_summary=[{}]",
            snapshot.live_count,
            snapshot.albums.len(),
            snapshot.media_type_albums.len(),
            sidebar_album_summary(&snapshot.albums),
            sidebar_album_summary(&snapshot.media_type_albums)
        );
        self.log_sidebar_layout_state("snapshot_apply_begin");
        if let Some(live_count) = snapshot.live_count {
            self.set_photos_count_label(live_count);
        }
        self.apply_album_rows(snapshot.albums);
        self.apply_media_type_rows(snapshot.media_type_albums);
        self.log_sidebar_layout_state("snapshot_apply_end");
        self.log_sidebar_layout_state_next_idle("snapshot_apply_end");
    }

    fn install_sidebar_layout_trace(&self) {
        if self.imp().sidebar_layout_trace_installed.replace(true) {
            return;
        }
        self.connect_sidebar_widget_trace(
            "album_trash_wrapper",
            self.imp().album_trash_wrapper.upcast_ref(),
        );
        self.connect_sidebar_widget_trace("album_scroll", self.imp().album_scroll.upcast_ref());
        self.connect_sidebar_widget_trace("album_list", self.imp().album_list.upcast_ref());
        self.connect_sidebar_widget_trace(
            "media_type_header_list",
            self.imp().media_type_header_list.upcast_ref(),
        );
        self.connect_sidebar_widget_trace(
            "media_type_scroll",
            self.imp().media_type_scroll.upcast_ref(),
        );
        self.connect_sidebar_widget_trace(
            "media_type_list",
            self.imp().media_type_list.upcast_ref(),
        );
        self.connect_sidebar_widget_trace("trash_list", self.imp().trash_list.upcast_ref());
        self.connect_sidebar_widget_trace("sidebar_spacer", self.imp().sidebar_spacer.upcast_ref());
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE layout_trace_installed"
        );
        self.log_sidebar_layout_state("layout_trace_installed");
    }

    fn connect_sidebar_widget_trace(&self, name: &'static str, widget: &gtk::Widget) {
        widget.connect_notify_local(
            Some("height"),
            glib::clone!(@weak self as window => move |widget, _| {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE widget_height name={} visible={} mapped={} width={} height={}",
                    name,
                    widget.is_visible(),
                    widget.is_mapped(),
                    widget.width(),
                    widget.height()
                );
                window.log_sidebar_layout_state(&format!("height_notify:{name}"));
            }),
        );
        widget.connect_notify_local(
            Some("visible"),
            glib::clone!(@weak self as window => move |widget, _| {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE widget_visible name={} visible={} mapped={} width={} height={}",
                    name,
                    widget.is_visible(),
                    widget.is_mapped(),
                    widget.width(),
                    widget.height()
                );
                window.log_sidebar_layout_state(&format!("visible_notify:{name}"));
                window.log_sidebar_layout_state_next_idle(format!("visible_notify:{name}"));
            }),
        );
    }

    fn log_sidebar_layout_state_next_idle(&self, stage: impl Into<String>) {
        let stage = stage.into();
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(window) = weak.upgrade() {
                window.log_sidebar_layout_state(&format!("{stage}:idle"));
            }
        });
    }

    fn log_sidebar_layout_state(&self, stage: &str) {
        let active_album = self
            .imp()
            .active_album
            .borrow()
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".into());
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE layout stage={} albums_expanded={} media_types_expanded={} album_scroll_visible={} album_scroll_mapped={} album_scroll_wh={}x{} album_list_children={} album_list_wh={}x{} album_targets={} media_type_header_visible={} media_type_scroll_visible={} media_type_scroll_mapped={} media_type_scroll_wh={}x{} media_type_list_children={} media_type_targets={} wrapper_vexpand={} wrapper_wh={}x{} spacer_vexpand={} spacer_visible={} spacer_wh={}x{} trash_children={} active_album={}",
            stage,
            self.imp().albums_expanded.get(),
            self.imp().media_types_expanded.get(),
            self.imp().album_scroll.is_visible(),
            self.imp().album_scroll.is_mapped(),
            self.imp().album_scroll.width(),
            self.imp().album_scroll.height(),
            sidebar_list_child_count(&self.imp().album_list),
            self.imp().album_list.width(),
            self.imp().album_list.height(),
            self.imp().album_targets.borrow().len(),
            self.imp().media_type_header_list.is_visible(),
            self.imp().media_type_scroll.is_visible(),
            self.imp().media_type_scroll.is_mapped(),
            self.imp().media_type_scroll.width(),
            self.imp().media_type_scroll.height(),
            sidebar_list_child_count(&self.imp().media_type_list),
            self.imp().media_type_targets.borrow().len(),
            self.imp().album_trash_wrapper.vexpands(),
            self.imp().album_trash_wrapper.width(),
            self.imp().album_trash_wrapper.height(),
            self.imp().sidebar_spacer.vexpands(),
            self.imp().sidebar_spacer.is_visible(),
            self.imp().sidebar_spacer.width(),
            self.imp().sidebar_spacer.height(),
            sidebar_list_child_count(&self.imp().trash_list),
            active_album
        );
    }

    fn reselect_active_album_row(&self) {
        let active = match self.imp().active_album.borrow().clone() {
            Some(path) => path,
            None => return,
        };
        let album_list = self.imp().album_list.get();
        let idx = self
            .imp()
            .album_targets
            .borrow()
            .iter()
            .position(|album| album.folder_path == active);
        if let Some(i) = idx {
            if let Some(row) = album_list.row_at_index(i as i32) {
                self.imp().selecting_programmatically.set(true);
                album_list.select_row(Some(&row));
                self.imp().selecting_programmatically.set(false);
            }
        }
        let media_type_list = self.imp().media_type_list.get();
        let idx = self
            .imp()
            .media_type_targets
            .borrow()
            .iter()
            .position(|album| album.folder_path == active);
        if let Some(i) = idx {
            if let Some(row) = media_type_list.row_at_index(i as i32) {
                self.imp().selecting_programmatically.set(true);
                media_type_list.select_row(Some(&row));
                self.imp().selecting_programmatically.set(false);
            }
        }
    }

    /// Collapse/expand the Albums group: swap the disclosure arrow and toggle
    /// the dedicated album scroll region. Rows remain mounted in `album_list`
    /// so their drag order and album target indices stay stable.
    /// When expanded, the album-trash wrapper fills available space; the
    /// scrolled window sizes to content (no vexpand). When collapsed, the
    /// spacer expands so Settings stays pinned to the bottom.
    pub fn toggle_albums_expanded(&self) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_albums_expanded begin current_expanded={}",
            self.imp().albums_expanded.get()
        );
        self.log_sidebar_layout_state("toggle_albums_expanded_before");
        let expanded = !self.imp().albums_expanded.get();
        self.imp().albums_expanded.set(expanded);
        if let Some(arrow) = self.imp().albums_arrow.borrow().clone() {
            // Animate the disclosure arrow via CSS rotation instead of an icon
            // swap (which snaps). The icon stays pan-down-symbolic; the
            // .collapsed class rotates it -90deg (see grid_css.rs).
            if expanded {
                arrow.remove_css_class("collapsed");
            } else {
                arrow.add_css_class("collapsed");
            }
        }
        self.imp().album_scroll.set_visible(expanded);
        self.imp().album_trash_wrapper.set_vexpand(expanded);
        self.imp().sidebar_spacer.set_vexpand(!expanded);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_albums_expanded end expanded={}",
            expanded
        );
        self.log_sidebar_layout_state("toggle_albums_expanded_after");
        self.log_sidebar_layout_state_next_idle("toggle_albums_expanded_after");
    }

    pub fn toggle_media_types_expanded(&self) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_media_types_expanded begin current_expanded={}",
            self.imp().media_types_expanded.get()
        );
        self.log_sidebar_layout_state("toggle_media_types_expanded_before");
        let expanded = !self.imp().media_types_expanded.get();
        self.imp().media_types_expanded.set(expanded);
        if let Some(arrow) = self.imp().media_types_arrow.borrow().clone() {
            // CSS rotation drives the arrow animation (see
            // toggle_albums_expanded for the rationale).
            if expanded {
                arrow.remove_css_class("collapsed");
            } else {
                arrow.add_css_class("collapsed");
            }
        }
        self.imp().media_type_scroll.set_visible(expanded);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_media_types_expanded end expanded={}",
            expanded
        );
        self.log_sidebar_layout_state("toggle_media_types_expanded_after");
        self.log_sidebar_layout_state_next_idle("toggle_media_types_expanded_after");
    }

    /// Rebuild the sidebar album rows from the current DB snapshot so counts
    /// stay live after favorites/trash changes.
    pub fn refresh_album_rows(&self) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE refresh_album_rows_sync_begin"
        );
        self.log_sidebar_layout_state("refresh_album_rows_sync_begin");
        self.update_photos_count_label_from_db();
        self.rebuild_album_rows();
        self.rebuild_media_type_rows();
        self.log_sidebar_layout_state("refresh_album_rows_sync_end");
        self.log_sidebar_layout_state_next_idle("refresh_album_rows_sync_end");
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

    fn update_photos_count_label(&self) {
        let count = self
            .imp()
            .media_list
            .borrow()
            .as_ref()
            .map(|list| list.n_items())
            .unwrap_or(0);
        self.set_photos_count_label(count);
    }

    fn set_photos_count_label(&self, count: u32) {
        let Some(label) = self.imp().photos_count_label.borrow().as_ref().cloned() else {
            return;
        };
        label.set_label(&count.to_string());
        label.set_visible(true);
    }

    fn update_photos_count_label_from_db(&self) {
        let count = self
            .imp()
            .pool
            .borrow()
            .as_ref()
            .and_then(|pool| {
                crate::core::repository::MediaRepository::new(pool.clone())
                    .count(crate::core::repository::MediaQuery::LiveAll)
                    .ok()
            })
            .unwrap_or_else(|| {
                self.imp()
                    .media_list
                    .borrow()
                    .as_ref()
                    .map(|list| list.n_items())
                    .unwrap_or(0)
            });
        self.set_photos_count_label(count);
    }

    pub fn refresh_sidebar_snapshot_async(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let trace_id = SIDEBAR_SNAPSHOT_TRACE_ID.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE snapshot_request id={} album_targets={} media_type_targets={}",
            trace_id,
            self.imp().album_targets.borrow().len(),
            self.imp().media_type_targets.borrow().len()
        );
        self.log_sidebar_layout_state(&format!("snapshot_request#{trace_id}"));
        self.log_sidebar_layout_state_next_idle(format!("snapshot_request#{trace_id}"));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || {
                let snapshot = load_sidebar_album_snapshot(&pool);
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE snapshot_loaded id={} live_count={:?} albums={} media_types={} albums_summary=[{}] media_types_summary=[{}]",
                    trace_id,
                    snapshot.live_count,
                    snapshot.albums.len(),
                    snapshot.media_type_albums.len(),
                    sidebar_album_summary(&snapshot.albums),
                    sidebar_album_summary(&snapshot.media_type_albums)
                );
                snapshot
            })
            .await;
            let Some(window) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(snapshot) => {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SIDEBAR_TRACE snapshot_deliver id={}",
                        trace_id
                    );
                    window.apply_sidebar_album_snapshot(snapshot);
                    window.log_sidebar_layout_state(&format!("snapshot_deliver#{trace_id}_after"));
                    window.log_sidebar_layout_state_next_idle(format!(
                        "snapshot_deliver#{trace_id}_after"
                    ));
                }
                Err(err) => tracing::warn!("sidebar snapshot refresh failed: {err:?}"),
            }
        });
    }

    /// Wire the sidebar `ListBox` row-selected signal to navigate by row
    /// identity (`targets[index]`), not a hardcoded index:
    ///   - Photos → pop back to the root Photos page.
    ///   - Trash → push the `TrashPage`.
    ///
    /// Album rows live in `album_list` and are wired separately below.
    ///
    /// The Albums header is non-selectable, so it never lands here; its collapse
    /// toggle is driven by its own `GestureClick`.
    ///
    /// Requires `set_resources` to have been called first; if the resources are
    /// missing the closures silently no-op.
    pub fn connect_sidebar(&self, nav_view: &adw::NavigationView) {
        self.install_sidebar_layout_trace();
        let list = self.imp().sidebar_list.get();
        let trash_list = self.imp().trash_list.get();
        let album_list = self.imp().album_list.get();
        let media_type_list = self.imp().media_type_list.get();

        list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let target = {
                    let targets = window.imp().targets.borrow();
                    let Some(target) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    target
                };
                match target {
                    SidebarTarget::Photos => {
                        *window.imp().active_album.borrow_mut() = None;
                        window.imp().album_list.get().unselect_all();
                        window.imp().media_type_list.get().unselect_all();
                        window.imp().trash_list.get().unselect_all();
                        pop_to_photos_root(&nav_view);
                    }
                    SidebarTarget::AlbumsHeader => {}
                    SidebarTarget::Trash => {}
                }
            }),
        );

        trash_list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let target = {
                    let targets = window.imp().trash_targets.borrow();
                    let Some(target) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    target
                };
                if let SidebarTarget::Trash = target {
                    *window.imp().active_album.borrow_mut() = None;
                    window.imp().sidebar_list.get().unselect_all();
                    window.imp().album_list.get().unselect_all();
                    window.imp().media_type_list.get().unselect_all();
                    window.show_trash_page(&nav_view);
                }
            }),
        );

        album_list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                if window.imp().album_selection_mode.get() {
                    window.sync_selected_album_paths();
                    return;
                }
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let album = {
                    let targets = window.imp().album_targets.borrow();
                    let Some(album) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    album
                };
                *window.imp().active_album.borrow_mut() = Some(album.folder_path.clone());
                window.imp().sidebar_list.get().unselect_all();
                window.imp().media_type_list.get().unselect_all();
                window.imp().trash_list.get().unselect_all();
                // Defer open_album to idle so navigation (pop/push) cannot
                // re-enter these row-selected handlers and panic on a
                // double RefCell borrow.
                window.schedule_album_open_from_sidebar(&nav_view, album, "album_list", row.index());
            }),
        );

        media_type_list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let album = {
                    let targets = window.imp().media_type_targets.borrow();
                    let Some(album) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    album
                };
                *window.imp().active_album.borrow_mut() = Some(album.folder_path.clone());
                window.imp().sidebar_list.get().unselect_all();
                window.imp().album_list.get().unselect_all();
                window.imp().trash_list.get().unselect_all();
                // Defer open_album to idle so navigation (pop/push) cannot
                // re-enter these row-selected handlers and panic on a
                // double RefCell borrow.
                window.schedule_album_open_from_sidebar(
                    &nav_view,
                    album,
                    "media_type_list",
                    row.index(),
                );
            }),
        );

        let settings_btn = self.imp().settings_button.get();
        settings_btn.connect_clicked(glib::clone!(@weak self as window => move |_| {
            window.show_settings_dialog();
        }));

        self.imp()
            .album_selection_cancel_btn
            .set_label(&tr("common.cancel"));
        self.imp()
            .album_selection_delete_btn
            .set_label(&tr("album.selection.delete_selected"));
        self.imp()
            .album_selection_delete_btn
            .get()
            .set_sensitive(false);

        self.imp().album_selection_cancel_btn.connect_clicked(
            glib::clone!(@weak self as window => move |_| {
                window.exit_album_selection_mode();
            }),
        );
        self.imp().album_selection_delete_btn.connect_clicked(
            glib::clone!(@weak self as window => move |_| {
                window.confirm_delete_selected_albums();
            }),
        );
    }

    fn schedule_album_open_from_sidebar(
        &self,
        nav_view: &adw::NavigationView,
        album: Album,
        source: &'static str,
        row_index: i32,
    ) {
        let selected_at = Instant::now();
        let album_name = album.display_name();
        let album_path = album.folder_path.to_string_lossy().into_owned();
        let is_virtual = album.is_virtual;
        let expected_count = album.photo_count;
        let select_span = tracing::info_span!(
            "album:select_row",
            source,
            row_index,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual,
            expected_count
        );
        let _select = select_span.enter();

        let weak = self.downgrade();
        let nav_view = nav_view.clone();
        glib::idle_add_local_once(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let idle_wait_ms = selected_at.elapsed().as_millis() as u64;
            let idle_span = tracing::info_span!(
                "album:open_idle",
                source,
                row_index,
                album_name = %album_name,
                album_path = %album_path,
                is_virtual,
                expected_count,
                idle_wait_ms
            );
            let _idle = idle_span.enter();
            window.open_album(&nav_view, album);
        });
    }

    #[tracing::instrument(name = "album:open", skip(self, nav_view, album))]
    pub(crate) fn open_album(&self, nav_view: &adw::NavigationView, album: Album) {
        let album_name = album.display_name();
        let album_path = album.folder_path.to_string_lossy().into_owned();
        let album_is_virtual = album.is_virtual;
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = album_is_virtual,
            expected_count = album.photo_count,
            "album_switch: begin"
        );

        // Already viewing this album → no-op (avoids rebuilding/pushing a
        // duplicate detail page on a re-select).
        let already_visible = {
            let check_span = tracing::info_span!(
                "album:already_visible_check",
                album_name = %album_name,
                album_path = %album_path
            );
            let _check = check_span.enter();
            nav_view
                .visible_page()
                .and_then(|page| page.downcast::<AlbumDetailPage>().ok())
                .is_some_and(|detail| {
                    detail.album_folder_path().as_deref() == Some(album.folder_path.as_path())
                })
        };
        if already_visible {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: already_visible"
            );
            return;
        }

        let Some(pool) = self.imp().pool.borrow().clone() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: missing_db_pool"
            );
            return;
        };
        let Some(loader) = self.imp().loader.borrow().clone() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: missing_thumbnail_loader"
            );
            return;
        };
        let Some(master) = self.imp().media_list.borrow().clone() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: missing_master_media_list"
            );
            return;
        };

        // Albums are top-level destinations: drop any stacked pages back to the
        // Photos root, then push a fresh detail page so the back stack stays
        // shallow and consistent.
        {
            let pop_span = tracing::info_span!(
                "album:pop",
                album_name = %album_name,
                album_path = %album_path
            );
            let _pop = pop_span.enter();
            pop_to_photos_root(nav_view);
        }

        // 文件夹相册和虚拟相册都从数据库加载，不受 UI_MEDIA_LIST_CAP
        // 或启动时 master GTK 列表窗口限制。切换路径只同步加载首个可渲染窗口；
        // 大相册剩余项后台补齐，避免打开相册时阻塞主线程。
        let query = media_query_for_album(&album);
        let initial_limit = album_initial_load_limit(album.photo_count);
        let (items, total_items) = {
            let load_span = tracing::info_span!(
                "album:load",
                album_name = %album_name,
                album_path = %album_path,
                is_virtual = album_is_virtual,
                expected_count = album.photo_count,
                initial_limit,
                ?query
            );
            let _load = load_span.enter();
            match crate::core::repository::MediaRepository::new(pool.clone()).page(
                query.clone(),
                0,
                initial_limit,
            ) {
                Ok(page) => {
                    tracing::debug!(
                        target: crate::core::log_targets::ALBUMS,
                        album_name = %album_name,
                        album_path = %album_path,
                        is_virtual = album_is_virtual,
                        expected_count = album.photo_count,
                        initial_limit,
                        item_count = page.items.len(),
                        total_items = page.total,
                        "album_switch: initial_page_loaded"
                    );
                    (page.items, page.total)
                }
                Err(err) => {
                    tracing::warn!(
                        target: crate::core::log_targets::ALBUMS,
                        album_name = %album_name,
                        album_path = %album_path,
                        ?query,
                        "album_switch: initial_page_failed error={err}"
                    );
                    (Vec::new(), 0)
                }
            }
        };
        let item_count = items.len();

        let filtered = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        {
            let store_span = tracing::info_span!(
                "album:store",
                album_name = %album_name,
                album_path = %album_path,
                item_count,
                total_items
            );
            let _store = store_span.enter();
            for item in items {
                filtered.append(&glib::BoxedAnyObject::new(item));
            }
        }

        let page = {
            let page_span = tracing::info_span!(
                "album:page_build",
                album_name = %album_name,
                album_path = %album_path,
                item_count,
                total_items
            );
            let _page = page_span.enter();
            AlbumDetailPage::new(album, filtered.clone(), master, pool.clone(), loader)
        };
        {
            let bind_span = tracing::info_span!(
                "album:bind_page",
                album_name = %album_name,
                album_path = %album_path
            );
            let _bind = bind_span.enter();
            if let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() {
                page.set_db_actor(db_actor);
            }
            page.set_nav_target(nav_view);
        }
        {
            let push_span = tracing::info_span!(
                "album:push",
                album_name = %album_name,
                album_path = %album_path,
                item_count,
                total_items
            );
            let _push = push_span.enter();
            nav_view.push(&page);
        }

        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = album_is_virtual,
            item_count,
            "album_switch: end"
        );

        if total_items > item_count as u32 {
            backfill_album_media_list(
                filtered,
                pool,
                query,
                item_count as u32,
                total_items,
                album_name,
                album_path,
            );
        }
    }

    fn show_trash_page(&self, nav_view: &adw::NavigationView) {
        if visible_page_is_trash(nav_view) {
            return;
        }
        let Some(page) = self.build_trash_page() else {
            return;
        };
        pop_to_photos_root(nav_view);
        nav_view.push(&page);
    }

    fn build_trash_page(&self) -> Option<TrashPage> {
        let pool = self.imp().pool.borrow().clone()?;
        let loader = self.imp().loader.borrow().clone()?;
        let media_list = self.imp().media_list.borrow().clone()?;
        Some(TrashPage::with_media_list(pool, loader, media_list))
    }

    /// 若当前可见页面是回收站页，重读 DB 刷新它。供 `TrashChanged` 事件调用——
    /// 文件管理器改了系统回收站后，watcher 已对账 DB，这里让打开着的回收站页实时
    /// 跟着变，无需用户切换页面。
    pub fn refresh_visible_trash_page(&self) {
        let nav = self.imp().nav_view.get();
        let Some(page) = nav.visible_page() else {
            return;
        };
        if let Some(trash) = page.downcast_ref::<TrashPage>() {
            trash.refresh();
        }
    }

    /// Refresh an already-open album detail page after live media membership
    /// changes. Album rows and counts refresh through the sidebar snapshot;
    /// this keeps the page's own filtered `ListStore` in sync while the user
    /// stays on that album.
    pub fn refresh_visible_album_detail_page(&self) {
        let nav = self.imp().nav_view.get();
        let Some(page) = nav.visible_page() else {
            return;
        };
        if let Some(album_detail) = page.downcast_ref::<AlbumDetailPage>() {
            album_detail.refresh_media_list_from_repository();
        }
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

    fn open_search_page(&self) -> bool {
        let nav = self.imp().nav_view.get();
        if let Some(search) = nav
            .visible_page()
            .and_then(|page| page.downcast::<SearchPage>().ok())
        {
            search.focus_search_entry();
            return true;
        }
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return false;
        };
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return false;
        };
        let page = SearchPage::new(pool, loader);
        if let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() {
            page.set_db_actor(db_actor);
        }
        page.set_nav_target(&nav);
        nav.push(&page);
        true
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
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashSet;
    use std::path::Path;
    use std::rc::Rc;

    fn collect_labels(widget: &gtk::Widget, labels: &mut Vec<String>) {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            labels.push(label.label().to_string());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_labels(&current, labels);
        }
    }

    fn collect_scales(widget: &gtk::Widget, scales: &mut Vec<gtk::Scale>) {
        if let Some(scale) = widget.downcast_ref::<gtk::Scale>() {
            scales.push(scale.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_scales(&current, scales);
        }
    }

    fn collect_check_buttons(widget: &gtk::Widget, buttons: &mut Vec<gtk::CheckButton>) {
        if let Some(btn) = widget.downcast_ref::<gtk::CheckButton>() {
            buttons.push(btn.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_check_buttons(&current, buttons);
        }
    }

    fn collect_preference_titles(widget: &gtk::Widget, titles: &mut Vec<String>) {
        if let Some(row) = widget.downcast_ref::<adw::PreferencesRow>() {
            titles.push(row.title().to_string());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            collect_preference_titles(&current, titles);
        }
    }

    #[test]
    fn album_switch_trace_points_cover_selection_to_backfill() {
        let source = include_str!("window.rs");
        let production_source = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("window.rs must contain production code");
        for trace_name in [
            "album:select_row",
            "album:open_idle",
            "album:already_visible_check",
            "album:bind_page",
            "album:backfill_schedule",
        ] {
            assert!(
                production_source.contains(trace_name),
                "missing album switch trace point {trace_name}"
            );
        }
    }

    #[test]
    fn high_frequency_album_progress_logs_stay_debug() {
        let source = include_str!("window.rs");
        let production_source = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("window.rs must contain production code");

        for message in [
            "album_switch: initial_page_loaded",
            "album_backfill: skipped_at_cap",
            "album_backfill: fetched",
            "album_backfill: appended",
        ] {
            let message_index = production_source
                .find(message)
                .unwrap_or_else(|| panic!("missing log message {message}"));
            let before = &production_source[..message_index];
            let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
                .iter()
                .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
                .max_by_key(|(index, _)| *index)
                .map(|(_, candidate)| candidate)
                .expect("log message should be inside a tracing macro");
            assert_eq!(
                actual_macro, "tracing::debug!(",
                "{message} should stay out of default logs"
            );
        }
    }

    #[test]
    fn sidebar_trace_logs_stay_debug() {
        let source = include_str!("window.rs");
        let production_source = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("window.rs must contain production code");

        let mut search_from = 0;
        while let Some(relative_index) = production_source[search_from..].find("SIDEBAR_TRACE") {
            let message_index = search_from + relative_index;
            let before = &production_source[..message_index];
            let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
                .iter()
                .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
                .max_by_key(|(index, _)| *index)
                .map(|(_, candidate)| candidate)
                .expect("SIDEBAR_TRACE message should be inside a tracing macro");
            assert_eq!(
                actual_macro, "tracing::debug!(",
                "SIDEBAR_TRACE messages are diagnostic noise and should stay out of default INFO logs"
            );
            search_from = message_index + "SIDEBAR_TRACE".len();
        }
    }

    #[test]
    fn sidebar_album_row_summary_logs_stay_debug() {
        let source = include_str!("window.rs");
        let production_source = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("window.rs must contain production code");

        for message in [
            "SIDEBAR_ALBUM_UPDATE_IN_PLACE",
            "SIDEBAR_ALBUM_REMOVE_IN_PLACE",
            "SIDEBAR_ALBUM_REBUILD",
        ] {
            let message_index = production_source
                .find(message)
                .unwrap_or_else(|| panic!("missing log message {message}"));
            let before = &production_source[..message_index];
            let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
                .iter()
                .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
                .max_by_key(|(index, _)| *index)
                .map(|(_, candidate)| candidate)
                .expect("log message should be inside a tracing macro");
            assert_eq!(
                actual_macro, "tracing::debug!(",
                "{message} is high-volume sidebar row diagnostics and should stay out of default INFO logs"
            );
        }
    }

    fn find_action_row_subtitle(widget: &gtk::Widget, title: &str) -> Option<String> {
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

    fn media_item(id: i64, folder_path: &str, name: &str) -> MediaItem {
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

    fn sidebar_album(path: &str, name: &str, photo_count: i64) -> Album {
        Album {
            folder_path: PathBuf::from(path),
            name: name.into(),
            cover_uri: None,
            photo_count,
            last_modified: Utc::now(),
            is_virtual: false,
        }
    }

    fn keyboard_media_item(id: i64) -> MediaItem {
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

    fn keyboard_media_list() -> gtk::gio::ListStore {
        let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        media_list.append(&glib::BoxedAnyObject::new(keyboard_media_item(1)));
        media_list
    }

    fn keyboard_thumbnail_loader() -> (tempfile::TempDir, Arc<ThumbnailLoader>) {
        let tmp = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-scope.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
        (tmp, loader)
    }

    #[gtk::test]
    fn shared_media_projection_refresh_emits_pure_addition_for_new_item() {
        let _ = gtk::init();
        let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        let existing = media_item(1, "/tmp/shared-refresh", "one.jpg");
        list.append(&glib::BoxedAnyObject::new(existing.clone()));
        let changes = Rc::new(RefCell::new(Vec::<(u32, u32, u32)>::new()));
        let changes_for_signal = changes.clone();
        list.connect_items_changed(move |_, position, removed, added| {
            changes_for_signal
                .borrow_mut()
                .push((position, removed, added));
        });

        let added = media_item(2, "/tmp/shared-refresh", "two.jpg");
        apply_media_projection_to_list(&list, vec![added, existing]);

        assert_eq!(
            *changes.borrow(),
            vec![(0, 0, 1)],
            "shared Photos refresh should insert new media without replacing existing tiles"
        );
    }

    fn emit_key_for_tests<W: IsA<gtk::Widget>>(
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

    #[gtk::test]
    fn main_window_installs_single_keyboard_router() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardRouter")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        let window = MainWindow::new(&app);

        let key_controllers: Vec<gtk::EventControllerKey> = window
            .observe_controllers()
            .snapshot()
            .into_iter()
            .filter_map(|controller| controller.downcast::<gtk::EventControllerKey>().ok())
            .filter(|controller| {
                controller.name().as_deref() == Some("photo-viewer-keyboard-router")
            })
            .filter(|controller| controller.propagation_phase() == gtk::PropagationPhase::Capture)
            .collect();

        assert_eq!(
            key_controllers.len(),
            1,
            "MainWindow should own one capture-phase keyboard router"
        );
        assert_eq!(
            key_controllers[0].propagation_phase(),
            gtk::PropagationPhase::Capture
        );
    }

    #[gtk::test]
    fn sidebar_album_snapshot_updates_stable_rows_in_place() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.SidebarStableAlbumRows")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        let window = MainWindow::new(&app);

        let albums = vec![
            sidebar_album("/tmp/camera", "Camera", 2),
            sidebar_album("/tmp/screenshots", "Screenshots", 1),
        ];
        window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
            albums: albums.clone(),
            media_type_albums: Vec::new(),
            live_count: Some(3),
        });
        let album_list = window.imp().album_list.get();
        let first_before = album_list
            .row_at_index(0)
            .expect("first album row should exist");
        let second_before = album_list
            .row_at_index(1)
            .expect("second album row should exist");

        let mut refreshed = albums;
        refreshed[0].photo_count = 1;
        refreshed[1].photo_count = 1;
        window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
            albums: refreshed,
            media_type_albums: Vec::new(),
            live_count: Some(2),
        });

        assert!(
            album_list.row_at_index(0).as_ref() == Some(&first_before),
            "same album/order refresh should update the first row in place instead of replacing it"
        );
        assert!(
            album_list.row_at_index(1).as_ref() == Some(&second_before),
            "same album/order refresh should update the second row in place instead of replacing it"
        );
        assert_eq!(
            window.imp().album_targets.borrow()[0].photo_count,
            1,
            "target snapshot should still update to the latest count"
        );
    }

    #[gtk::test]
    fn sidebar_album_snapshot_removes_missing_row_without_replacing_survivors() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.SidebarStableAlbumRemoval")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        let window = MainWindow::new(&app);

        let albums = vec![
            sidebar_album("/tmp/camera", "Camera", 2),
            sidebar_album("/tmp/downloads", "Downloads", 1),
            sidebar_album("/tmp/screenshots", "Screenshots", 3),
        ];
        window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
            albums: albums.clone(),
            media_type_albums: Vec::new(),
            live_count: Some(6),
        });
        let album_list = window.imp().album_list.get();
        let first_before = album_list
            .row_at_index(0)
            .expect("first album row should exist");
        let third_before = album_list
            .row_at_index(2)
            .expect("third album row should exist");

        window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
            albums: vec![
                sidebar_album("/tmp/camera", "Camera", 1),
                sidebar_album("/tmp/screenshots", "Screenshots", 3),
            ],
            media_type_albums: Vec::new(),
            live_count: Some(4),
        });

        assert!(
            album_list.row_at_index(0).as_ref() == Some(&first_before),
            "removing one album should keep the first surviving row mounted"
        );
        assert!(
            album_list.row_at_index(1).as_ref() == Some(&third_before),
            "removing one album should keep the later surviving row mounted"
        );
        assert_eq!(
            window.imp().album_targets.borrow().len(),
            2,
            "target snapshot should remove only the missing album"
        );
        assert_eq!(
            window.imp().album_targets.borrow()[0].photo_count,
            1,
            "surviving row targets should still update to latest counts"
        );
    }

    #[gtk::test]
    fn sidebar_media_type_snapshot_removes_missing_row_without_replacing_survivors() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.SidebarStableMediaTypeRemoval")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        let window = MainWindow::new(&app);

        let mut dynamic = sidebar_album("/virtual/dynamic", "Dynamic Photos", 2);
        dynamic.is_virtual = true;
        let mut raw = sidebar_album("/virtual/raw", "Raw", 1);
        raw.is_virtual = true;
        let mut panoramas = sidebar_album("/virtual/panoramas", "Panoramas", 3);
        panoramas.is_virtual = true;

        window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
            albums: Vec::new(),
            media_type_albums: vec![dynamic.clone(), raw, panoramas.clone()],
            live_count: Some(6),
        });
        let media_type_list = window.imp().media_type_list.get();
        let first_before = media_type_list
            .row_at_index(0)
            .expect("first media type row should exist");
        let third_before = media_type_list
            .row_at_index(2)
            .expect("third media type row should exist");

        dynamic.photo_count = 1;
        window.apply_sidebar_album_snapshot(SidebarAlbumSnapshot {
            albums: Vec::new(),
            media_type_albums: vec![dynamic, panoramas],
            live_count: Some(4),
        });

        assert!(
            media_type_list.row_at_index(0).as_ref() == Some(&first_before),
            "removing one media type should keep the first surviving row mounted"
        );
        assert!(
            media_type_list.row_at_index(1).as_ref() == Some(&third_before),
            "removing one media type should keep the later surviving row mounted"
        );
        assert_eq!(
            window.imp().media_type_targets.borrow().len(),
            2,
            "target snapshot should remove only the missing media type"
        );
        assert_eq!(
            window.imp().media_type_targets.borrow()[0].photo_count,
            1,
            "surviving media type targets should still update to latest counts"
        );
    }

    #[gtk::test]
    fn navigation_view_has_no_touch_swipe_controller() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.NoSwipeRouter")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        let window = MainWindow::new(&app);
        window.populate_sidebar();

        let nav = window.nav_view();
        window.connect_sidebar(&nav);

        let has_swipe = nav
            .observe_controllers()
            .snapshot()
            .into_iter()
            .any(|controller| controller.downcast::<gtk::GestureSwipe>().is_ok());
        assert!(
            !has_swipe,
            "NavigationView should not install a touch swipe controller that competes with buttons and keyboard actions"
        );
    }

    #[gtk::test]
    fn keyboard_scope_is_viewer_when_viewer_page_is_visible() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardScopeViewer")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();

        let viewer = crate::ui::ViewerPage::new(keyboard_media_list(), 0);
        nav.push(&viewer);

        assert_eq!(
            window.keyboard_scope_for_tests(),
            crate::ui::keyboard::KeyboardScope::Viewer
        );
    }

    #[gtk::test]
    fn keyboard_scope_is_browsing_when_photos_page_is_visible() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardScopeBrowsing")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let (_tmp, loader) = keyboard_thumbnail_loader();
        let photos = PhotosPage::new(keyboard_media_list(), loader);
        nav.push(&photos);

        assert_eq!(
            window.keyboard_scope_for_tests(),
            crate::ui::keyboard::KeyboardScope::Browsing
        );
    }

    #[gtk::test]
    fn ctrl_f_opens_search_from_photos_page() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardSearch")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let media_list = keyboard_media_list();
        let (tmp, loader) = keyboard_thumbnail_loader();
        let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-search.db")).unwrap();
        window.set_resources(pool.clone(), loader.clone(), media_list.clone());
        let photos = PhotosPage::new(media_list, loader);
        photos.set_nav_target(&nav);
        photos.set_db_pool(pool);
        nav.push(&photos);

        let handled = emit_key_for_tests(
            &window,
            gtk::gdk::Key::f,
            gtk::gdk::ModifierType::CONTROL_MASK,
        );

        assert!(handled, "Ctrl+F should be handled from PhotosPage");
        assert!(
            nav.visible_page().and_downcast::<SearchPage>().is_some(),
            "Ctrl+F should push SearchPage from PhotosPage"
        );
    }

    #[gtk::test]
    fn ctrl_a_selects_visible_photos_grid_items() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardBrowseSelectAll")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let media_list = keyboard_media_list();
        let (_tmp, loader) = keyboard_thumbnail_loader();
        let photos = PhotosPage::new(media_list, loader);
        nav.push(&photos);

        let handled = emit_key_for_tests(
            &window,
            gtk::gdk::Key::a,
            gtk::gdk::ModifierType::CONTROL_MASK,
        );

        assert!(
            handled,
            "Ctrl+A should be handled by the visible PhotosPage"
        );
        assert_eq!(
            photos.selected_count_for_tests(),
            1,
            "Ctrl+A should select the currently rendered item"
        );
    }

    #[gtk::test]
    fn escape_clears_photos_grid_selection_before_navigation_back() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardBrowseEscapeSelection")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let media_list = keyboard_media_list();
        let (_tmp, loader) = keyboard_thumbnail_loader();
        let photos = PhotosPage::new(media_list, loader);
        nav.push(&photos);

        assert!(emit_key_for_tests(
            &window,
            gtk::gdk::Key::a,
            gtk::gdk::ModifierType::CONTROL_MASK,
        ));
        assert_eq!(photos.selected_count_for_tests(), 1);

        let handled = emit_key_for_tests(
            &window,
            gtk::gdk::Key::Escape,
            gtk::gdk::ModifierType::empty(),
        );

        assert!(
            handled,
            "Escape should be consumed by PhotosPage while selection is active"
        );
        assert_eq!(
            photos.selected_count_for_tests(),
            0,
            "Escape should clear selected photos"
        );
        assert!(
            nav.visible_page().and_downcast::<PhotosPage>().is_some(),
            "Escape should not navigate away while it is clearing selection"
        );
    }

    #[gtk::test]
    fn ctrl_f_opens_search_from_trash_page() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardSearchTrash")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let media_list = keyboard_media_list();
        let (tmp, loader) = keyboard_thumbnail_loader();
        let pool =
            crate::core::db::init_pool(&tmp.path().join("keyboard-search-trash.db")).unwrap();
        window.set_resources(pool.clone(), loader.clone(), media_list.clone());
        let trash = TrashPage::with_media_list(pool, loader, media_list);
        nav.push(&trash);

        let handled = emit_key_for_tests(
            &window,
            gtk::gdk::Key::f,
            gtk::gdk::ModifierType::CONTROL_MASK,
        );

        assert!(handled, "Ctrl+F should be handled from TrashPage");
        assert!(
            nav.visible_page().and_downcast::<SearchPage>().is_some(),
            "Ctrl+F should push SearchPage from non-Photos pages when resources are available"
        );
    }

    #[gtk::test]
    fn settings_modal_blocks_global_search_and_closes_on_escape() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardSettingsModal")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let media_list = keyboard_media_list();
        let (tmp, loader) = keyboard_thumbnail_loader();
        let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-settings.db")).unwrap();
        window.set_resources(pool.clone(), loader.clone(), media_list.clone());
        let photos = PhotosPage::new(media_list, loader);
        photos.set_nav_target(&nav);
        photos.set_db_pool(pool);
        nav.push(&photos);

        let opened = emit_key_for_tests(
            &window,
            gtk::gdk::Key::comma,
            gtk::gdk::ModifierType::CONTROL_MASK,
        );
        assert!(opened, "Ctrl+, should open Settings");
        assert!(window.imp().settings_dialog.borrow().is_some());
        assert_eq!(
            window.keyboard_scope_for_tests(),
            crate::ui::keyboard::KeyboardScope::Modal
        );

        let leaked = emit_key_for_tests(
            &window,
            gtk::gdk::Key::f,
            gtk::gdk::ModifierType::CONTROL_MASK,
        );
        assert!(!leaked, "Ctrl+F must not leak through Settings modal");
        assert!(
            nav.visible_page().and_downcast::<SearchPage>().is_none(),
            "SearchPage should not open behind Settings"
        );
        let settings_reopen = emit_key_for_tests(
            &window,
            gtk::gdk::Key::comma,
            gtk::gdk::ModifierType::CONTROL_MASK,
        );
        assert!(
            !settings_reopen,
            "Ctrl+, must not re-enter Settings while modal is open"
        );
        assert!(window.imp().settings_dialog.borrow().is_some());

        let closed = emit_key_for_tests(
            &window,
            gtk::gdk::Key::Escape,
            gtk::gdk::ModifierType::empty(),
        );
        assert!(closed, "Escape should close Settings modal");
        assert!(window.imp().settings_dialog.borrow().is_none());
    }

    #[gtk::test]
    fn glass_menu_modal_escape_does_not_pop_visible_page() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardGlassMenuModal")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let root = adw::NavigationPage::builder()
            .title("Root")
            .child(&gtk::Label::new(Some("root")))
            .build();
        let pushed = adw::NavigationPage::builder()
            .title("Pushed")
            .child(&gtk::Label::new(Some("pushed")))
            .build();
        nav.push(&root);
        nav.push(&pushed);

        let layer = gtk::Fixed::builder()
            .can_focus(true)
            .css_classes(["glass-context-menu-layer"])
            .build();
        let button = gtk::Button::with_label("Menu item");
        layer.put(&button, 0.0, 0.0);
        window.imp().root_overlay.get().add_overlay(&layer);
        window.present();
        button.grab_focus();
        while glib::MainContext::default().iteration(false) {}
        assert_eq!(
            window.keyboard_scope_for_tests(),
            crate::ui::keyboard::KeyboardScope::Modal
        );

        let handled = emit_key_for_tests(
            &window,
            gtk::gdk::Key::Escape,
            gtk::gdk::ModifierType::empty(),
        );

        assert!(
            !handled,
            "Window router should let glass menu Escape reach the menu-local handler"
        );
        assert_eq!(
            nav.visible_page().map(|page| page.title().to_string()),
            Some("Pushed".to_string()),
            "Modal Escape must not pop the page underneath"
        );
        window.imp().root_overlay.get().remove_overlay(&layer);
        window.close();
        while glib::MainContext::default().iteration(false) {}
    }

    #[gtk::test]
    fn ctrl_f_reuses_visible_search_page() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardSearchReuse")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();
        let media_list = keyboard_media_list();
        let (tmp, loader) = keyboard_thumbnail_loader();
        let pool =
            crate::core::db::init_pool(&tmp.path().join("keyboard-search-reuse.db")).unwrap();
        window.set_resources(pool.clone(), loader.clone(), media_list);
        let search = SearchPage::new(pool, loader);
        search.set_nav_target(&nav);
        nav.push(&search);
        let page_before = nav.visible_page().expect("search visible");

        let handled = emit_key_for_tests(
            &window,
            gtk::gdk::Key::f,
            gtk::gdk::ModifierType::CONTROL_MASK,
        );

        assert!(handled, "Ctrl+F should focus/reuse visible SearchPage");
        let page_after = nav.visible_page().expect("search still visible");
        assert!(
            page_before == page_after,
            "Ctrl+F should not push a duplicate SearchPage"
        );
    }

    #[gtk::test]
    fn viewer_right_key_navigates_when_focus_is_on_header_button() {
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.KeyboardViewerFocus")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        crate::ui::grid_css::install();
        let window = MainWindow::new(&app);
        let nav = window.nav_view();

        let viewer = crate::ui::ViewerPage::new(keyboard_media_list(), 0);
        let events = Rc::new(RefCell::new(Vec::new()));
        let events_for_cb = events.clone();
        viewer.connect_navigation(move |delta| {
            events_for_cb.borrow_mut().push(delta);
        });
        nav.push(&viewer);

        viewer.imp().details_btn.get().grab_focus();
        let handled = emit_key_for_tests(
            &window,
            gtk::gdk::Key::Right,
            gtk::gdk::ModifierType::empty(),
        );

        assert!(handled, "viewer Right shortcut should stop propagation");
        assert_eq!(events.borrow().as_slice(), &[1]);
    }

    fn media_list_uris(list: &gtk::gio::ListStore) -> Vec<String> {
        (0..list.n_items())
            .filter_map(|index| {
                list.item(index)
                    .and_downcast::<glib::BoxedAnyObject>()
                    .map(|boxed| boxed.borrow::<MediaItem>().uri.clone())
            })
            .collect()
    }

    #[test]
    fn album_delete_pruning_removes_deleted_folder_rows_except_remaining_live_uris() {
        let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        let deleted = media_item(1, "/tmp/Camera", "deleted.jpg");
        let still_live = media_item(2, "/tmp/Camera", "still-live.jpg");
        let other = media_item(3, "/tmp/Other", "keep.jpg");
        list.append(&glib::BoxedAnyObject::new(deleted.clone()));
        list.append(&glib::BoxedAnyObject::new(still_live.clone()));
        list.append(&glib::BoxedAnyObject::new(other.clone()));

        remove_deleted_album_media_from_media_list(
            &list,
            &[Path::new("/tmp/Camera").to_path_buf()],
            &HashSet::from([still_live.uri.clone()]),
            &HashSet::new(),
        );

        assert_eq!(media_list_uris(&list), vec![still_live.uri, other.uri]);
    }

    #[test]
    fn album_delete_pruning_preserves_unknown_remaining_live_folder_rows() {
        let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        let unknown = media_item(1, "/tmp/Camera", "unknown.jpg");
        let deleted = media_item(2, "/tmp/Trips", "deleted.jpg");
        let other = media_item(3, "/tmp/Other", "keep.jpg");
        list.append(&glib::BoxedAnyObject::new(unknown.clone()));
        list.append(&glib::BoxedAnyObject::new(deleted.clone()));
        list.append(&glib::BoxedAnyObject::new(other.clone()));

        remove_deleted_album_media_from_media_list(
            &list,
            &[
                Path::new("/tmp/Camera").to_path_buf(),
                Path::new("/tmp/Trips").to_path_buf(),
            ],
            &HashSet::new(),
            &HashSet::from([Path::new("/tmp/Camera").to_path_buf()]),
        );

        assert_eq!(media_list_uris(&list), vec![unknown.uri, other.uri]);
    }

    #[test]
    fn album_initial_load_limit_caps_large_albums_to_render_window() {
        assert_eq!(album_initial_load_limit(2), 2);
        assert_eq!(
            album_initial_load_limit(100_000),
            crate::core::runtime_config::DEFAULT_MAX_RENDERED_GRID_ITEMS as u32
        );
        assert_eq!(album_initial_load_limit(-1), 0);
    }

    #[test]
    fn album_backfill_limit_caps_large_albums_to_ui_window() {
        let initial = album_initial_load_limit(100_000);
        let limit = album_backfill_fetch_limit(initial, 100_000);

        assert!(
            initial + limit <= crate::core::runtime_config::DEFAULT_UI_MEDIA_LIST_CAP as u32,
            "album backfill must not materialize the full album into the GTK ListStore"
        );
        assert!(
            limit < 100_000 - initial,
            "large album backfill should fetch only a bounded continuation window"
        );
    }

    #[test]
    fn restart_spec_uses_current_executable_and_preserves_args() {
        let exe = PathBuf::from("/tmp/photo-viewer");
        let args = vec!["--profile".into(), "debug".into()];

        let spec = restart_spec_from_for_tests(exe.clone(), args.clone());

        assert_eq!(spec.program, exe);
        assert_eq!(spec.args, args);
    }

    #[test]
    fn restart_spec_requires_current_process_exit_after_spawn() {
        let spec = restart_spec_from_for_tests(PathBuf::from("/tmp/photo-viewer"), Vec::new());

        assert!(spec.exit_current_process_after_spawn);
    }

    #[gtk::test]
    fn settings_page_exposes_video_default_mute_without_volume_control() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowSettings")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let page = window.build_settings_page(&host);

        let mut labels = Vec::new();
        collect_labels(&page.upcast::<gtk::Widget>(), &mut labels);

        assert!(
            labels
                .iter()
                .any(|label| label == &tr("setting.section.video")),
            "settings page should contain the video playback section, got {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|label| label == &tr("setting.video_default_muted")),
            "settings page should expose the default mute setting, got {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|label| label == &tr("setting.auto_play_motion_photo")),
            "settings page should expose the motion-photo auto-play setting, got {labels:?}"
        );
        assert!(
            !labels
                .iter()
                .any(|label| label == &tr("setting.video_volume")),
            "volume should be persisted from playback, not configured in settings"
        );
    }

    #[gtk::test]
    fn settings_page_exposes_liquid_glass_transparency_slider() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowGlassTransparency")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let page = window.build_settings_page(&host);
        let page = page.upcast::<gtk::Widget>();

        let mut labels = Vec::new();
        collect_labels(&page, &mut labels);
        assert!(
            labels
                .iter()
                .any(|label| label == &tr("setting.liquid_glass_transparency")),
            "settings page should expose the generic transparency label, got {labels:?}"
        );
        for mark in [
            "0", "10", "20", "30", "40", "50", "60", "70", "80", "90", "100",
        ] {
            assert!(
                labels.iter().any(|label| label == mark),
                "transparency scale should expose mark {mark}, got {labels:?}"
            );
        }

        let mut scales = Vec::new();
        collect_scales(&page, &mut scales);
        assert!(
            scales.iter().any(|scale| {
                scale.adjustment().lower() == 0.0 && scale.adjustment().upper() == 100.0
            }),
            "settings page should expose a 0-100 transparency scale"
        );
    }

    #[gtk::test]
    fn settings_page_exposes_theme_selector() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowThemeSelector")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let page = window.build_settings_page(&host);
        let page = page.upcast::<gtk::Widget>();

        let mut labels = Vec::new();
        collect_labels(&page, &mut labels);
        assert!(
            labels.iter().any(|label| label == &tr("setting.theme")),
            "settings page should expose a theme label, got {labels:?}"
        );

        let mut check_buttons = Vec::new();
        collect_check_buttons(&page, &mut check_buttons);
        let theme_labels: Vec<String> = check_buttons
            .iter()
            .filter_map(|btn| btn.label().map(|label| label.to_string()))
            .collect();
        assert!(
            theme_labels.contains(&tr("setting.theme.system")),
            "settings page should expose Follow System theme option, got {theme_labels:?}"
        );
        assert!(
            theme_labels.contains(&tr("setting.theme.light")),
            "settings page should expose Light theme option, got {theme_labels:?}"
        );
        assert!(
            theme_labels.contains(&tr("setting.theme.dark")),
            "settings page should expose Dark theme option, got {theme_labels:?}"
        );

        let mut titles = Vec::new();
        collect_preference_titles(&page, &mut titles);
        assert!(
            titles.iter().any(|title| title == &tr("setting.theme")),
            "theme selector should live in a PreferencesRow, got {titles:?}"
        );
        assert!(
            titles
                .iter()
                .any(|title| title == &tr("setting.liquid_glass")),
            "Liquid Glass toggle should live in a PreferencesRow, got {titles:?}"
        );
        assert!(
            titles
                .iter()
                .any(|title| title == &tr("setting.video_default_muted")),
            "video mute toggle should live in a PreferencesRow, got {titles:?}"
        );
    }

    #[gtk::test]
    fn settings_page_exposes_thumbnail_generation_speed_selector() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowThumbnailSpeed")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let page = window.build_settings_page(&host);
        let page = page.upcast::<gtk::Widget>();

        let mut titles = Vec::new();
        collect_preference_titles(&page, &mut titles);
        assert!(
            titles
                .iter()
                .any(|title| title == &tr("setting.thumbnail_generation_speed")),
            "settings page should expose thumbnail generation speed, got {titles:?}"
        );

        let mut check_buttons = Vec::new();
        collect_check_buttons(&page, &mut check_buttons);
        let speed_labels: Vec<String> = check_buttons
            .iter()
            .filter_map(|btn| btn.label().map(|l| l.to_string()))
            .collect();
        assert!(
            speed_labels.contains(&tr("setting.thumbnail_generation_speed.slow")),
            "should have Slow radio button, got {speed_labels:?}"
        );
        assert!(
            speed_labels.contains(&tr("setting.thumbnail_generation_speed.normal")),
            "should have Normal radio button, got {speed_labels:?}"
        );
        assert!(
            speed_labels.contains(&tr("setting.thumbnail_generation_speed.fast")),
            "should have Fast radio button, got {speed_labels:?}"
        );
        assert!(
            speed_labels.contains(&tr("setting.thumbnail_generation_speed.fastest")),
            "should have Fastest radio button, got {speed_labels:?}"
        );
    }

    #[gtk::test]
    fn settings_page_exposes_scan_path_management() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowScanPaths")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let page = window.build_settings_page(&host);
        let page = page.upcast::<gtk::Widget>();

        let mut titles = Vec::new();
        collect_preference_titles(&page, &mut titles);
        assert!(
            titles
                .iter()
                .any(|title| title == &tr("setting.scan_paths.custom")),
            "settings page should expose custom scan path management, got {titles:?}"
        );
        assert!(
            titles
                .iter()
                .any(|title| title == &tr("setting.scan_paths.excluded")),
            "settings page should expose excluded scan path management, got {titles:?}"
        );
    }

    #[gtk::test]
    fn settings_page_exposes_trash_backend_controls() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowTrashBackend")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let page = window.build_settings_page(&host);
        let page = page.upcast::<gtk::Widget>();

        let mut labels = Vec::new();
        collect_labels(&page, &mut labels);
        assert!(
            labels
                .iter()
                .any(|label| label == &tr("setting.section.trash")),
            "settings page should expose the trash settings section, got {labels:?}"
        );

        let mut titles = Vec::new();
        collect_preference_titles(&page, &mut titles);
        assert!(
            titles
                .iter()
                .any(|title| title == &tr("setting.trash.backend")),
            "settings page should expose the trash backend row, got {titles:?}"
        );
        assert!(
            titles
                .iter()
                .any(|title| title == &tr("setting.trash.system_available_title")),
            "settings page should include the system-trash migration recommendation row, got {titles:?}"
        );
    }

    #[gtk::test]
    fn settings_storage_rows_defer_size_calculation() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowStorageUsage")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let page = window.build_settings_page(&host);
        let page = page.upcast::<gtk::Widget>();
        let pending = tr("setting.storage_usage_calculating");

        assert_eq!(
            find_action_row_subtitle(&page, &tr("setting.clear_thumbnails")).as_deref(),
            Some(pending.as_str()),
            "thumbnail cache size must not be calculated while constructing Settings"
        );
        assert_eq!(
            find_action_row_subtitle(&page, &tr("setting.clear_database")).as_deref(),
            Some(pending.as_str()),
            "database size must not be calculated while constructing Settings"
        );
    }

    #[gtk::test]
    fn settings_dialog_uses_bounded_scroll_child() {
        let _ = gtk::init();
        let app = adw::Application::builder()
            .application_id("io.github.luyao_1024.photoviewer.WindowSettingsDialogBounds")
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");

        let window = MainWindow::new(&app);
        let host = window.clone().upcast::<gtk::Widget>();
        let dialog = window.build_settings_dialog(&host);

        assert!(
            dialog.content_height() <= 700,
            "settings dialog content height should stay below an 800px window; got {}",
            dialog.content_height()
        );
        assert!(
            dialog
                .child()
                .is_some_and(|child| child.is::<gtk::ScrolledWindow>()),
            "settings dialog should use a ScrolledWindow so tall content does not over-request sheet height"
        );
    }
}
