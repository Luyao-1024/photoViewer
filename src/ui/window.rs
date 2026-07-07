//! Main window: sidebar + content area
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use gdk_pixbuf::Pixbuf;
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
use crate::core::albums::{list_media_type_albums, list_with_favorites, set_album_order, Album};
use crate::core::db::DbPool;
use crate::core::db_actor::DbActorHandle;
use crate::core::i18n::{locale, tr, trf};
use crate::core::media::MediaItem;
use crate::core::prefs::TrashBackend;
use crate::core::repository::MediaMutation;
use crate::core::repository::MediaQuery;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::core::{prefs, runtime_config};
use crate::ui::album_detail_page::{media_query_for_album, AlbumDetailPage};
use crate::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
use crate::ui::media_grid::square_tile::SquareTile;
use crate::ui::TrashPage;
use crate::ui::{grid_css, keyboard, theme, PhotosPage, SearchPage, ViewerPage};

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
        tracing::info!(
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
            tracing::info!(
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
                    tracing::info!(
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
            tracing::info!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_ALBUM_REMOVE_IN_PLACE rows={}",
                album_count
            );
            self.log_sidebar_layout_state("album_rows_ordered_subset_after");
            self.log_sidebar_layout_state_next_idle("album_rows_ordered_subset_after");
            return;
        }

        tracing::info!(
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
        tracing::info!(
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
        tracing::info!(
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
        tracing::info!(
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
                    tracing::info!(
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
        tracing::info!(
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
        tracing::info!(
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
        tracing::info!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE layout_trace_installed"
        );
        self.log_sidebar_layout_state("layout_trace_installed");
    }

    fn connect_sidebar_widget_trace(&self, name: &'static str, widget: &gtk::Widget) {
        widget.connect_notify_local(
            Some("height"),
            glib::clone!(@weak self as window => move |widget, _| {
                tracing::info!(
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
                tracing::info!(
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
        tracing::info!(
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
        tracing::info!(
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
        tracing::info!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_albums_expanded end expanded={}",
            expanded
        );
        self.log_sidebar_layout_state("toggle_albums_expanded_after");
        self.log_sidebar_layout_state_next_idle("toggle_albums_expanded_after");
    }

    pub fn toggle_media_types_expanded(&self) {
        tracing::info!(
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
        tracing::info!(
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
        tracing::info!(
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

    pub fn enter_album_selection_mode(&self) {
        self.imp().album_selection_mode.set(true);
        self.imp()
            .album_list
            .get()
            .set_selection_mode(gtk::SelectionMode::Multiple);
        self.imp().album_list.get().unselect_all();
        self.imp().selected_album_paths.borrow_mut().clear();
        self.imp().album_selection_bar.get().set_revealed(true);
        self.update_album_selection_actions();
    }

    fn exit_album_selection_mode(&self) {
        self.imp().album_selection_mode.set(false);
        self.imp().album_list.get().unselect_all();
        self.imp()
            .album_list
            .get()
            .set_selection_mode(gtk::SelectionMode::Single);
        self.imp().selected_album_paths.borrow_mut().clear();
        self.imp().album_selection_bar.get().set_revealed(false);
        self.update_album_selection_actions();
    }

    pub fn selected_album_delete_count(&self) -> usize {
        self.imp().selected_album_paths.borrow().len()
    }

    fn sync_selected_album_paths(&self) {
        let album_list = self.imp().album_list.get();
        let targets = self.imp().album_targets.borrow().clone();
        let mut selected = HashSet::new();
        let mut virtual_rows = Vec::new();

        for row in album_list.selected_rows() {
            let Some(album) = targets.get(row.index() as usize) else {
                continue;
            };
            if album.is_virtual {
                virtual_rows.push(row);
            } else {
                selected.insert(album.folder_path.clone());
            }
        }

        for row in virtual_rows {
            album_list.unselect_row(&row);
        }

        *self.imp().selected_album_paths.borrow_mut() = selected;
        self.update_album_selection_actions();
    }

    fn update_album_selection_actions(&self) {
        self.imp()
            .album_selection_delete_btn
            .get()
            .set_sensitive(self.selected_album_delete_count() > 0);
    }

    fn selected_real_albums(&self) -> Vec<Album> {
        let selected = self.imp().selected_album_paths.borrow().clone();
        self.imp()
            .album_targets
            .borrow()
            .iter()
            .filter(|album| !album.is_virtual && selected.contains(&album.folder_path))
            .cloned()
            .collect()
    }

    /// Persist a drag-to-reorder: move the album at `source_path` so it lands
    /// just before (`drop_after = false`) or just after (`drop_after = true`)
    /// the album at `target_path`, then rebuild the rows so the sidebar matches.
    ///
    /// The new full order is derived from the currently displayed `targets`
    /// (the source of truth for what the user sees), written wholesale to
    /// `album_order`, then `rebuild_album_rows` re-fetches and re-applies it.
    fn reorder_album(&self, source_path: &str, target_path: &str, drop_after: bool) {
        if source_path == target_path {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };

        // Current top-to-bottom album order, minus the dragged album.
        let mut order: Vec<String> = self
            .imp()
            .album_targets
            .borrow()
            .iter()
            .map(|album| album.folder_path.to_string_lossy().into_owned())
            .filter(|p| p != source_path)
            .collect();

        let insert_at = match order.iter().position(|p| p == target_path) {
            Some(idx) => {
                if drop_after {
                    (idx + 1).min(order.len())
                } else {
                    idx
                }
            }
            None => order.len(),
        };
        order.insert(insert_at, source_path.to_string());

        if let Err(err) = set_album_order(&pool, &order) {
            tracing::warn!("failed to persist album order: {err}");
        }
        self.rebuild_album_rows();
    }

    /// Wire long-press-drag reorder onto an album row: a `DragSource` carries
    /// the row's `folder_path` as the drag payload (and dims the row while
    /// dragging), and a `DropTarget` accepts another album's path, showing an
    /// above/below insertion indicator and persisting the new order on drop.
    ///
    /// `Gtk.DragSource` only begins a drag after the pointer moves past the
    /// drag threshold, so a plain click still selects the row normally — only
    /// a press-and-drag reorders.
    fn attach_album_dnd(&self, row: &gtk::ListBoxRow, folder_path: String) {
        let drag = gtk::DragSource::new();
        drag.set_actions(gtk::gdk::DragAction::MOVE);
        let value = glib::Value::from(folder_path.as_str());
        drag.set_content(Some(&gtk::gdk::ContentProvider::for_value(&value)));

        let drag_row = row.downgrade();
        drag.connect_drag_begin(move |_, _| {
            if let Some(r) = drag_row.upgrade() {
                r.add_css_class("glass-sidebar-row-dragging");
            }
        });
        let drag_row = row.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(r) = drag_row.upgrade() {
                r.remove_css_class("glass-sidebar-row-dragging");
            }
        });
        row.add_controller(drag);

        let drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);

        let motion_row = row.downgrade();
        drop.connect_motion(move |_t, _x, y| {
            if let Some(r) = motion_row.upgrade() {
                let half = r.height().max(1) as f64 / 2.0;
                r.remove_css_class("glass-sidebar-row-drop-above");
                r.remove_css_class("glass-sidebar-row-drop-below");
                r.add_css_class(if y > half {
                    "glass-sidebar-row-drop-below"
                } else {
                    "glass-sidebar-row-drop-above"
                });
            }
            gtk::gdk::DragAction::MOVE
        });
        let leave_row = row.downgrade();
        drop.connect_leave(move |_t| {
            if let Some(r) = leave_row.upgrade() {
                r.remove_css_class("glass-sidebar-row-drop-above");
                r.remove_css_class("glass-sidebar-row-drop-below");
            }
        });

        let weak = self.downgrade();
        let drop_row = row.downgrade();
        let target_path = folder_path;
        drop.connect_drop(move |_t, value, _x, y| {
            let Some(window) = weak.upgrade() else {
                return false;
            };
            let Some(r) = drop_row.upgrade() else {
                return false;
            };
            r.remove_css_class("glass-sidebar-row-drop-above");
            r.remove_css_class("glass-sidebar-row-drop-below");
            let Ok(src) = value.get::<String>() else {
                return false;
            };
            let half = r.height().max(1) as f64 / 2.0;
            window.reorder_album(&src, &target_path, y > half);
            true
        });
        row.add_controller(drop);
    }

    fn attach_album_context_menu(&self, row: &gtk::ListBoxRow, album: Album) {
        let weak = self.downgrade();
        let row_weak = row.downgrade();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.connect_pressed(move |_gesture, n_press, x, y| {
            if n_press != 1 {
                return;
            }
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some(row) = row_weak.upgrade() else {
                return;
            };

            let manage_album = album.clone();
            let delete_album = album.clone();
            let ignore_album = album.clone();
            let select_album = album.clone();
            let nav_view = window.imp().nav_view.get();
            let items = build_album_context_menu_items(
                &album,
                Some(Box::new(glib::clone!(
                    @weak window,
                    @weak nav_view,
                    @weak row,
                    @strong manage_album => move || {
                        *window.imp().active_album.borrow_mut() =
                            Some(manage_album.folder_path.clone());
                        window.imp().selecting_programmatically.set(true);
                        window.imp().album_list.get().select_row(Some(&row));
                        window.imp().selecting_programmatically.set(false);
                        window.imp().sidebar_list.get().unselect_all();
                        window.imp().media_type_list.get().unselect_all();
                        window.imp().trash_list.get().unselect_all();
                        window.open_album(&nav_view, manage_album.clone());
                    }
                ))),
                Some(Box::new(glib::clone!(
                    @weak window,
                    @strong delete_album => move || {
                        window.confirm_delete_album(delete_album.clone());
                    }
                ))),
                Some(Box::new(glib::clone!(
                    @weak window,
                    @strong ignore_album => move || {
                        window.confirm_ignore_album(ignore_album.clone());
                    }
                ))),
                Some(Box::new(glib::clone!(
                    @weak window,
                    @weak row,
                    @strong select_album => move || {
                        window.enter_album_selection_mode();
                        if !select_album.is_virtual {
                            window.imp().album_list.get().select_row(Some(&row));
                        }
                    }
                ))),
            );
            glass_context_menu::show(
                &window.imp().root_overlay.get(),
                row.upcast_ref(),
                x,
                y,
                items,
            );
        });
        row.add_controller(gesture);
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
        tracing::info!(
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
                tracing::info!(
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
                    tracing::info!(
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
                    tracing::info!(
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

    fn confirm_delete_album(&self, album: Album) {
        if album.is_virtual {
            return;
        }

        let album_name = album.display_name();
        let dialog = adw::AlertDialog::builder()
            .heading(tr("album.delete.confirm_title"))
            .body(trf("album.delete.confirm_body", &[("album", &album_name)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("common.cancel"));
        dialog.add_response("delete", &tr("album.delete.confirm_action"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("delete"), move |_, _| {
            if let Some(window) = weak.upgrade() {
                window.delete_albums_to_trash_ui(vec![album.clone()]);
            }
        });

        dialog.present(self);
    }

    fn confirm_ignore_album(&self, album: Album) {
        if album.is_virtual {
            return;
        }

        let album_name = album.display_name();
        let dialog = adw::AlertDialog::builder()
            .heading(tr("album.ignore.confirm_title"))
            .body(trf("album.ignore.confirm_body", &[("album", &album_name)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("common.cancel"));
        dialog.add_response("ignore", &tr("album.ignore.confirm_action"));
        dialog.set_response_appearance("ignore", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("ignore"), move |_, _| {
            if let Some(window) = weak.upgrade() {
                window.ignore_album_ui(album.clone());
            }
        });

        dialog.present(self);
    }

    fn ignore_album_ui(&self, album: Album) {
        if album.is_virtual {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let worker_result =
                gtk::gio::spawn_blocking(move || ignore_album_worker(pool, album.folder_path))
                    .await;

            let Some(window) = weak.upgrade() else {
                return;
            };
            match worker_result {
                Ok(Ok(result)) => {
                    tracing::info!(
                        "ignored album {} and removed {} indexed media rows",
                        result.folder_path.display(),
                        result.removed_count
                    );
                    if let Some(media_list) = window.imp().media_list.borrow().as_ref() {
                        remove_ignored_album_media_from_media_list(media_list, &result.folder_path);
                    }
                    window.refresh_album_rows();

                    let active_should_close = window
                        .imp()
                        .active_album
                        .borrow()
                        .as_ref()
                        .is_some_and(|active| active == &result.folder_path);
                    if active_should_close {
                        *window.imp().active_album.borrow_mut() = None;
                        window.imp().album_list.get().unselect_all();
                        window.imp().trash_list.get().unselect_all();
                        window.imp().selecting_programmatically.set(true);
                        if let Some(row) = window.imp().sidebar_list.get().row_at_index(0) {
                            window.imp().sidebar_list.get().select_row(Some(&row));
                        }
                        window.imp().selecting_programmatically.set(false);
                        pop_to_photos_root(&window.imp().nav_view.get());
                    }
                }
                Ok(Err(err)) => {
                    tracing::warn!("failed to ignore album: {err}");
                    show_settings_error_dialog(
                        window.upcast_ref(),
                        &trf("setting.scan_paths.save_failed", &[("error", &err)]),
                    );
                    window.refresh_album_rows();
                }
                Err(err) => {
                    tracing::warn!("album ignore worker failed: {err:?}");
                    window.refresh_album_rows();
                }
            }
        });
    }

    fn confirm_delete_selected_albums(&self) {
        let selected = self.selected_real_albums();
        if selected.is_empty() {
            return;
        }

        let count = selected.len().to_string();
        let dialog = adw::AlertDialog::builder()
            .heading(tr("album.selection.confirm_title"))
            .body(trf("album.selection.confirm_body", &[("count", &count)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("common.cancel"));
        dialog.add_response("delete", &tr("album.delete.confirm_action"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("delete"), move |_, _| {
            if let Some(window) = weak.upgrade() {
                window.delete_albums_to_trash_ui(selected.clone());
                window.exit_album_selection_mode();
            }
        });

        dialog.present(self);
    }

    fn delete_albums_to_trash_ui(&self, albums: Vec<Album>) {
        if albums.is_empty() {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        tracing::info!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE delete_albums_to_trash_ui_begin albums={} summary=[{}]",
            albums.len(),
            sidebar_album_summary(&albums)
        );
        self.log_sidebar_layout_state("delete_albums_to_trash_ui_begin");

        let weak = self.downgrade();
        let retry_pool = pool.clone();
        let retry_albums = albums.clone();
        glib::spawn_future_local(async move {
            let worker_result =
                gtk::gio::spawn_blocking(move || delete_albums_to_trash_worker(pool, albums)).await;

            let Some(window) = weak.upgrade() else {
                return;
            };
            match worker_result {
                Ok(result) => {
                    let operation_error = result.operation.as_ref().err().cloned();
                    window.apply_album_delete_ui_result(&result);
                    if let Some(err) = operation_error {
                        tracing::warn!("failed to delete album to trash: {err}");
                        if prefs::trash_backend() == TrashBackend::System {
                            window.prompt_album_trash_backend_fallback(
                                retry_pool,
                                retry_albums,
                                err,
                            );
                        } else {
                            show_trash_operation_error_dialog(
                                window.upcast_ref(),
                                &tr("trash.move_failed"),
                            );
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("album delete worker failed: {err:?}");
                    window.refresh_album_rows();
                }
            }
        });
    }

    fn apply_album_delete_ui_result(&self, result: &AlbumDeleteUiResult) {
        tracing::info!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE apply_album_delete_result begin deleted_paths={} remaining_live_uris={} remaining_live_folders={} unknown_remaining_live_paths={}",
            result.deleted_paths.len(),
            result.remaining_live_uris.len(),
            result.remaining_live_folder_paths.len(),
            result.unknown_remaining_live_paths.len()
        );
        self.log_sidebar_layout_state("apply_album_delete_result_before_media_remove");
        if let Some(media_list) = self.imp().media_list.borrow().as_ref() {
            remove_deleted_album_media_from_media_list(
                media_list,
                &result.deleted_paths,
                &result.remaining_live_uris,
                &result.unknown_remaining_live_paths,
            );
        }
        self.log_sidebar_layout_state("apply_album_delete_result_before_refresh_album_rows");
        self.refresh_album_rows();

        let active_should_close = self
            .imp()
            .active_album
            .borrow()
            .as_ref()
            .is_some_and(|active| {
                result.deleted_paths.iter().any(|path| path == active)
                    && !result
                        .remaining_live_folder_paths
                        .iter()
                        .any(|path| path == active)
            });
        tracing::info!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE apply_album_delete_result active_should_close={}",
            active_should_close
        );
        if active_should_close {
            *self.imp().active_album.borrow_mut() = None;
            self.imp().album_list.get().unselect_all();
            self.imp().trash_list.get().unselect_all();
            self.imp().selecting_programmatically.set(true);
            if let Some(row) = self.imp().sidebar_list.get().row_at_index(0) {
                self.imp().sidebar_list.get().select_row(Some(&row));
            }
            self.imp().selecting_programmatically.set(false);
            pop_to_photos_root(&self.imp().nav_view.get());
        }
        self.log_sidebar_layout_state("apply_album_delete_result_end");
        self.log_sidebar_layout_state_next_idle("apply_album_delete_result_end");
    }

    fn prompt_album_trash_backend_fallback(&self, pool: DbPool, albums: Vec<Album>, error: String) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("trash.fallback.title"))
            .body(trf("trash.fallback.album_body", &[("error", &error)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("dialog.cancel"));
        dialog.add_response("switch", &tr("trash.fallback.switch_to_app"));
        dialog.set_response_appearance("switch", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("switch"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("switch"), move |_, _| {
            let pool = pool.clone();
            let albums = albums.clone();
            let weak = weak.clone();
            glib::spawn_future_local(async move {
                let worker_result = gtk::gio::spawn_blocking(move || {
                    crate::core::trash::switch_trash_backend(&pool, TrashBackend::App)
                        .map_err(|err| err.to_string())?;
                    Ok::<_, String>(delete_albums_to_trash_worker(pool, albums))
                })
                .await;

                let Some(window) = weak.upgrade() else {
                    return;
                };
                match worker_result {
                    Ok(Ok(result)) => {
                        let operation_error = result.operation.as_ref().err().cloned();
                        window.apply_album_delete_ui_result(&result);
                        if let Some(err) = operation_error {
                            tracing::warn!("failed to retry album trash delete: {err}");
                            show_trash_operation_error_dialog(
                                window.upcast_ref(),
                                &tr("trash.move_failed"),
                            );
                        }
                    }
                    Ok(Err(err)) => {
                        show_trash_operation_error_dialog(
                            window.upcast_ref(),
                            &trf("trash.fallback.switch_failed", &[("error", &err)]),
                        );
                    }
                    Err(err) => {
                        show_trash_operation_error_dialog(
                            window.upcast_ref(),
                            &trf(
                                "trash.fallback.switch_failed",
                                &[("error", &format!("{err:?}"))],
                            ),
                        );
                    }
                }
            });
        });

        dialog.present(self);
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

    fn build_settings_dialog(&self, host: &gtk::Widget) -> adw::Dialog {
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(false)
            .min_content_height(0)
            .max_content_height(700)
            .child(&self.build_settings_page(host))
            .build();

        let dialog = adw::Dialog::builder()
            .title(tr("setting.page.title"))
            .content_width(540)
            .content_height(700)
            .child(&scroller)
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_css_class("settings-dialog-backdrop");
        dialog.set_can_close(true);
        add_close_on_backdrop_click(&dialog);
        dialog
    }

    fn build_settings_page(&self, parent: &gtk::Widget) -> gtk::Box {
        let current = locale().to_string();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        content.add_css_class("settings-dialog-content");

        let title = gtk::Label::new(Some(&tr("setting.section.language")));
        title.set_xalign(0.0);
        content.append(&title);

        let description = gtk::Label::new(Some(&tr("setting.section.language_description")));
        description.set_wrap(true);
        description.set_xalign(0.0);
        content.append(&description);

        let lang_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let btn_zh = gtk::Button::with_label(&tr("setting.lang.zh"));
        let btn_en = gtk::Button::with_label(&tr("setting.lang.en"));

        btn_zh.set_sensitive(current != "zh-CN");
        btn_en.set_sensitive(current != "en");

        let parent_for_zh = parent.clone();
        let parent_for_en = parent.clone();
        let btn_zh_ref = btn_zh.clone();
        let btn_en_ref = btn_en.clone();
        let btn_zh_ref2 = btn_zh.clone();
        let btn_en_ref2 = btn_en.clone();

        btn_zh.connect_clicked(move |_| match persist_locale("zh-CN") {
            Ok(()) => {
                show_restart_required_dialog(&parent_for_zh);
                btn_zh_ref.set_sensitive(false);
                btn_en_ref.set_sensitive(true);
            }
            Err(err) => {
                show_settings_restart_dialog(&parent_for_zh, false, Some(err));
            }
        });

        btn_en.connect_clicked(move |_| match persist_locale("en") {
            Ok(()) => {
                show_restart_required_dialog(&parent_for_en);
                btn_zh_ref2.set_sensitive(true);
                btn_en_ref2.set_sensitive(false);
            }
            Err(err) => {
                show_settings_restart_dialog(&parent_for_en, false, Some(err));
            }
        });

        lang_box.append(&btn_zh);
        lang_box.append(&btn_en);
        content.append(&lang_box);

        // ── Appearance: theme + Liquid Glass controls ─────────────────────
        let appearance_group = adw::PreferencesGroup::new();
        appearance_group.set_title(&tr("setting.section.appearance"));
        appearance_group.add_css_class("settings-preferences-group");
        content.append(&appearance_group);

        let btn_theme_system = gtk::CheckButton::with_label(&tr("setting.theme.system"));
        let btn_theme_light = gtk::CheckButton::with_label(&tr("setting.theme.light"));
        let btn_theme_dark = gtk::CheckButton::with_label(&tr("setting.theme.dark"));
        btn_theme_light.set_group(Some(&btn_theme_system));
        btn_theme_dark.set_group(Some(&btn_theme_system));

        match prefs::theme_preference() {
            prefs::ThemePreference::System => btn_theme_system.set_active(true),
            prefs::ThemePreference::Light => btn_theme_light.set_active(true),
            prefs::ThemePreference::Dark => btn_theme_dark.set_active(true),
        }

        let theme_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        theme_box.set_valign(gtk::Align::Center);
        theme_box.append(&btn_theme_system);
        theme_box.append(&btn_theme_light);
        theme_box.append(&btn_theme_dark);

        let theme_row = adw::ActionRow::new();
        theme_row.add_css_class("settings-action-row");
        theme_row.set_title(&tr("setting.theme"));
        theme_row.set_activatable(false);
        theme_row.add_suffix(&theme_box);
        appearance_group.add(&theme_row);

        let parent_for_theme = parent.clone();
        let connect_theme_btn =
            move |btn: &gtk::CheckButton, preference: prefs::ThemePreference| {
                let parent = parent_for_theme.clone();
                btn.connect_toggled(move |btn| {
                    if !btn.is_active() {
                        return;
                    }
                    match prefs::set_theme_preference(preference) {
                        Ok(()) => theme::apply(preference),
                        Err(err) => show_settings_error_dialog(
                            &parent,
                            &trf("setting.theme_save_failed", &[("error", &err)]),
                        ),
                    }
                });
            };
        connect_theme_btn(&btn_theme_system, prefs::ThemePreference::System);
        connect_theme_btn(&btn_theme_light, prefs::ThemePreference::Light);
        connect_theme_btn(&btn_theme_dark, prefs::ThemePreference::Dark);

        let switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(prefs::liquid_glass_enabled())
            .build();

        let glass_row = adw::ActionRow::new();
        glass_row.add_css_class("settings-action-row");
        glass_row.set_title(&tr("setting.liquid_glass"));
        glass_row.set_activatable(false);
        glass_row.add_suffix(&switch);
        appearance_group.add(&glass_row);

        let parent_for_glass = parent.clone();
        switch.connect_notify_local(Some("active"), move |sw, _pspec| {
            let active = sw.is_active();
            match prefs::set_liquid_glass(active) {
                Ok(()) => {
                    // Live re-skin: swap the display CSS provider so every
                    // glass surface restyles immediately (no app restart).
                    grid_css::reapply(active);
                }
                Err(err) => {
                    show_settings_error_dialog(
                        &parent_for_glass,
                        &trf("setting.liquid_glass_save_failed", &[("error", &err)]),
                    );
                }
            }
        });

        let transparency_scale =
            gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        transparency_scale.set_hexpand(true);
        transparency_scale.set_size_request(300, -1);
        transparency_scale.set_digits(0);
        transparency_scale.set_value(prefs::liquid_glass_transparency() * 100.0);
        for mark in (0..=100).step_by(10) {
            let label = mark.to_string();
            transparency_scale.add_mark(mark as f64, gtk::PositionType::Bottom, Some(&label));
        }

        let transparency_row = adw::ActionRow::new();
        transparency_row.add_css_class("settings-action-row");
        transparency_row.set_title(&tr("setting.liquid_glass_transparency"));
        transparency_row.set_activatable(false);
        transparency_row.add_suffix(&transparency_scale);
        appearance_group.add(&transparency_row);

        let parent_for_transparency = parent.clone();
        transparency_scale.connect_value_changed(move |scale| {
            let transparency = scale.value() / 100.0;
            match prefs::set_liquid_glass_transparency(transparency) {
                Ok(()) => grid_css::reapply(prefs::liquid_glass_enabled()),
                Err(err) => {
                    show_settings_error_dialog(
                        &parent_for_transparency,
                        &trf(
                            "setting.liquid_glass_transparency_save_failed",
                            &[("error", &err)],
                        ),
                    );
                }
            }
        });

        // ── Video playback: startup mute preference ───────────────────────
        // Volume itself is persisted from the GtkMediaStream while watching a
        // video; settings only controls whether newly opened videos start muted.
        let video_group = adw::PreferencesGroup::new();
        video_group.set_title(&tr("setting.section.video"));
        video_group.add_css_class("settings-preferences-group");
        content.append(&video_group);

        let muted_switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(prefs::video_default_muted())
            .build();

        let muted_row = adw::ActionRow::new();
        muted_row.add_css_class("settings-action-row");
        muted_row.set_title(&tr("setting.video_default_muted"));
        muted_row.set_activatable(false);
        muted_row.add_suffix(&muted_switch);
        video_group.add(&muted_row);

        let parent_for_muted = parent.clone();
        muted_switch.connect_notify_local(Some("active"), move |sw, _pspec| {
            if let Err(err) = prefs::set_video_default_muted(sw.is_active()) {
                show_settings_error_dialog(
                    &parent_for_muted,
                    &trf(
                        "setting.video_default_muted_save_failed",
                        &[("error", &err)],
                    ),
                );
            }
        });

        let auto_play_motion_switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(prefs::auto_play_motion_photo())
            .build();

        let auto_play_motion_row = adw::ActionRow::new();
        auto_play_motion_row.add_css_class("settings-action-row");
        auto_play_motion_row.set_title(&tr("setting.auto_play_motion_photo"));
        auto_play_motion_row.set_activatable(false);
        auto_play_motion_row.add_suffix(&auto_play_motion_switch);
        video_group.add(&auto_play_motion_row);

        let parent_for_auto_play = parent.clone();
        auto_play_motion_switch.connect_notify_local(Some("active"), move |sw, _pspec| {
            if let Err(err) = prefs::set_auto_play_motion_photo(sw.is_active()) {
                show_settings_error_dialog(
                    &parent_for_auto_play,
                    &trf(
                        "setting.auto_play_motion_photo_save_failed",
                        &[("error", &err)],
                    ),
                );
            }
        });

        // ── Album management: custom and excluded scan folders ────────────
        let scan_paths_group = build_scan_paths_group(parent);
        content.append(&scan_paths_group);

        // ── Trash backend: system/app fallback and migration ───────────────
        content.append(&self.build_trash_settings_group(parent));

        // ── Storage: Clear Cache ────────────────────────────────────────────
        // Show current storage usage with action rows matching the project's
        // Adw.PreferencesGroup + Adw.ActionRow design pattern.
        let storage_group = adw::PreferencesGroup::new();
        storage_group.set_title(&tr("setting.section.storage"));
        storage_group.set_description(Some(&tr("setting.section.storage_description")));
        storage_group.add_css_class("settings-preferences-group");
        content.append(&storage_group);

        // ── Thumbnail generation speed: horizontal radio buttons ──────────
        let slow_label = tr("setting.thumbnail_generation_speed.slow");
        let normal_label = tr("setting.thumbnail_generation_speed.normal");
        let fast_label = tr("setting.thumbnail_generation_speed.fast");
        let fastest_label = tr("setting.thumbnail_generation_speed.fastest");

        let btn_slow = gtk::CheckButton::with_label(&slow_label);
        let btn_normal = gtk::CheckButton::with_label(&normal_label);
        let btn_fast = gtk::CheckButton::with_label(&fast_label);
        let btn_fastest = gtk::CheckButton::with_label(&fastest_label);

        // Set up radio group: Normal/Fast/Fastest join Slow's group.
        btn_normal.set_group(Some(&btn_slow));
        btn_fast.set_group(Some(&btn_slow));
        btn_fastest.set_group(Some(&btn_slow));

        // Select the radio button matching the current config.
        let current_speed = runtime_config::thumbnail_generation_speed();
        match current_speed {
            runtime_config::ThumbnailGenerationSpeed::Slow => btn_slow.set_active(true),
            runtime_config::ThumbnailGenerationSpeed::Normal => btn_normal.set_active(true),
            runtime_config::ThumbnailGenerationSpeed::Fast => btn_fast.set_active(true),
            runtime_config::ThumbnailGenerationSpeed::Fastest => btn_fastest.set_active(true),
        }

        let speed_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        speed_box.set_valign(gtk::Align::Center);
        speed_box.append(&btn_slow);
        speed_box.append(&btn_normal);
        speed_box.append(&btn_fast);
        speed_box.append(&btn_fastest);

        let speed_row = adw::ActionRow::new();
        speed_row.add_css_class("settings-action-row");
        speed_row.set_title(&tr("setting.thumbnail_generation_speed"));
        speed_row.set_subtitle(&tr("setting.thumbnail_generation_speed_description"));
        speed_row.set_activatable(false);
        speed_row.add_suffix(&speed_box);
        storage_group.add(&speed_row);

        let parent_for_speed = parent.clone();
        let connect_speed_btn =
            move |btn: &gtk::CheckButton, speed: runtime_config::ThumbnailGenerationSpeed| {
                let parent = parent_for_speed.clone();
                btn.connect_toggled(move |btn| {
                    if !btn.is_active() {
                        return;
                    }
                    if let Err(err) = runtime_config::set_thumbnail_generation_speed(speed) {
                        show_settings_error_dialog(
                            &parent,
                            &trf(
                                "setting.thumbnail_generation_speed_save_failed",
                                &[("error", &err)],
                            ),
                        );
                    } else {
                        show_restart_required_dialog(&parent);
                    }
                });
            };
        connect_speed_btn(&btn_slow, runtime_config::ThumbnailGenerationSpeed::Slow);
        connect_speed_btn(
            &btn_normal,
            runtime_config::ThumbnailGenerationSpeed::Normal,
        );
        connect_speed_btn(&btn_fast, runtime_config::ThumbnailGenerationSpeed::Fast);
        connect_speed_btn(
            &btn_fastest,
            runtime_config::ThumbnailGenerationSpeed::Fastest,
        );

        // Show current storage usage
        let cache_dir = config::cache_dir();
        let thumb_dir = cache_dir.join("thumbnails");
        let db_path = crate::config::data_dir().join("photos.db");

        // Thumbnail cache row with size and clear button
        let thumb_row = adw::ActionRow::new();
        thumb_row.add_css_class("settings-action-row");
        thumb_row.set_title(&tr("setting.clear_thumbnails"));
        thumb_row.set_activatable(false);
        update_storage_size_async(&thumb_row, move || crate::core::cache::dir_size(&thumb_dir));

        let btn_clear_thumbs = gtk::Button::new();
        btn_clear_thumbs.set_icon_name("user-trash-symbolic");
        btn_clear_thumbs.set_valign(gtk::Align::Center);
        btn_clear_thumbs.add_css_class("glass-toolbar-button");
        btn_clear_thumbs.add_css_class("glass-toolbar-danger");
        btn_clear_thumbs.set_tooltip_text(Some(&tr("setting.clear_thumbnails")));
        thumb_row.add_suffix(&btn_clear_thumbs);
        storage_group.add(&thumb_row);

        let parent_for_thumbs = parent.clone();
        let loader_for_thumbs = self.imp().loader.borrow().clone();
        let thumb_row_for_thumbs = thumb_row.clone();
        btn_clear_thumbs.connect_clicked(move |_| {
            let loader_clone = loader_for_thumbs.clone();
            let row_clone = thumb_row_for_thumbs.clone();
            show_clear_confirm_dialog(
                &parent_for_thumbs,
                &tr("setting.clear_thumbnails_confirm_title"),
                &tr("setting.clear_thumbnails_confirm_body"),
                move || {
                    let cache_dir = config::cache_dir();
                    let thumb_dir = cache_dir.join("thumbnails");
                    match crate::core::cache::enforce_size_limit(&thumb_dir, 0) {
                        Ok(count) => {
                            // Clear in-memory cache
                            if let Some(ref loader) = loader_clone {
                                loader.clear_mem_cache();
                            }
                            // Update subtitle
                            row_clone.set_subtitle(&format_size(0));
                            show_clear_success_toast(&trf(
                                "setting.clear_thumbnails_success",
                                &[("count", &count.to_string())],
                            ));
                        }
                        Err(err) => {
                            show_clear_error_toast(&trf(
                                "setting.clear_failed",
                                &[("error", &err.to_string())],
                            ));
                        }
                    }
                },
            );
        });

        // Database row with size and clear button
        let db_row = adw::ActionRow::new();
        db_row.add_css_class("settings-action-row");
        db_row.set_title(&tr("setting.clear_database"));
        db_row.set_activatable(false);
        update_storage_size_async(&db_row, move || {
            std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0)
        });

        let btn_clear_db = gtk::Button::new();
        btn_clear_db.set_icon_name("user-trash-symbolic");
        btn_clear_db.set_valign(gtk::Align::Center);
        btn_clear_db.add_css_class("glass-toolbar-button");
        btn_clear_db.add_css_class("glass-toolbar-danger");
        btn_clear_db.set_tooltip_text(Some(&tr("setting.clear_database")));
        db_row.add_suffix(&btn_clear_db);
        storage_group.add(&db_row);

        let parent_for_db = parent.clone();
        let pool_for_db = self.imp().pool.borrow().clone();
        let loader_for_db = self.imp().loader.borrow().clone();
        let media_list_for_db = self.imp().media_list.borrow().clone();
        let db_row_for_db = db_row.clone();
        btn_clear_db.connect_clicked(move |_| {
            let pool_clone = pool_for_db.clone();
            let loader_clone = loader_for_db.clone();
            let media_list_clone = media_list_for_db.clone();
            let row_clone = db_row_for_db.clone();
            show_clear_confirm_dialog(
                &parent_for_db,
                &tr("setting.clear_database_confirm_title"),
                &tr("setting.clear_database_confirm_body"),
                move || {
                    if let Some(ref pool) = pool_clone {
                        match crate::core::db::clear_all_media(pool) {
                            Ok(count) => {
                                // Clear in-memory thumbnail cache
                                if let Some(ref loader) = loader_clone {
                                    loader.clear_mem_cache();
                                }
                                // Clear the media list in UI
                                if let Some(ref media_list) = media_list_clone {
                                    media_list.remove_all();
                                }
                                // Update subtitle
                                row_clone.set_subtitle(&format_size(0));
                                show_clear_success_toast(&trf(
                                    "setting.clear_database_success",
                                    &[("count", &count.to_string())],
                                ));
                            }
                            Err(err) => {
                                show_clear_error_toast(&trf(
                                    "setting.clear_failed",
                                    &[("error", &err.to_string())],
                                ));
                            }
                        }
                    }
                },
            );
        });

        let spacer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        content.append(&spacer);
        content.append(&build_about_label());

        content
    }

    fn build_trash_settings_group(&self, parent: &gtk::Widget) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::new();
        group.set_title(&tr("setting.section.trash"));
        group.set_description(Some(&tr("setting.section.trash_description")));
        group.add_css_class("settings-preferences-group");

        let status_row = adw::ActionRow::new();
        status_row.add_css_class("settings-action-row");
        status_row.set_title(&tr("setting.trash.backend"));
        status_row.set_subtitle(&trash_backend_subtitle(prefs::trash_backend(), None));
        status_row.set_activatable(false);

        let switch_button =
            gtk::Button::with_label(&trash_backend_switch_label(prefs::trash_backend(), None));
        switch_button.set_valign(gtk::Align::Center);
        switch_button.add_css_class("glass-toolbar-button");
        status_row.add_suffix(&switch_button);
        group.add(&status_row);

        let suggestion_row = adw::ActionRow::new();
        suggestion_row.add_css_class("settings-action-row");
        suggestion_row.set_title(&tr("setting.trash.system_available_title"));
        suggestion_row.set_subtitle(&tr("setting.trash.system_available_subtitle"));
        suggestion_row.set_activatable(false);
        suggestion_row.set_visible(false);
        let suggestion_button = gtk::Button::with_label(&tr("setting.trash.migrate_to_system"));
        suggestion_button.set_valign(gtk::Align::Center);
        suggestion_button.add_css_class("glass-toolbar-button");
        suggestion_button.add_css_class("suggested-action");
        suggestion_row.add_suffix(&suggestion_button);
        group.add(&suggestion_row);

        let pool = self.imp().pool.borrow().clone();
        let parent_for_switch = parent.clone();
        let status_for_switch = status_row.clone();
        let switch_for_switch = switch_button.clone();
        let suggestion_for_switch = suggestion_row.clone();
        let suggestion_button_for_switch = suggestion_button.clone();
        switch_button.connect_clicked(move |_| {
            let Some(pool) = pool.clone() else {
                show_settings_error_dialog(
                    &parent_for_switch,
                    &tr("setting.trash.switch_unavailable_without_database"),
                );
                return;
            };
            let current = prefs::trash_backend();
            let target = match current {
                TrashBackend::System => TrashBackend::App,
                TrashBackend::App => TrashBackend::System,
            };
            run_trash_backend_switch(
                &parent_for_switch,
                pool,
                target,
                &status_for_switch,
                &switch_for_switch,
                &suggestion_for_switch,
                &suggestion_button_for_switch,
            );
        });

        let pool_for_suggestion = self.imp().pool.borrow().clone();
        let parent_for_suggestion = parent.clone();
        let status_for_suggestion = status_row.clone();
        let switch_for_suggestion = switch_button.clone();
        let suggestion_for_suggestion = suggestion_row.clone();
        let suggestion_button_for_suggestion = suggestion_button.clone();
        suggestion_button.connect_clicked(move |_| {
            let Some(pool) = pool_for_suggestion.clone() else {
                show_settings_error_dialog(
                    &parent_for_suggestion,
                    &tr("setting.trash.switch_unavailable_without_database"),
                );
                return;
            };
            run_trash_backend_switch(
                &parent_for_suggestion,
                pool,
                TrashBackend::System,
                &status_for_suggestion,
                &switch_for_suggestion,
                &suggestion_for_suggestion,
                &suggestion_button_for_suggestion,
            );
        });

        #[cfg(not(test))]
        {
            let status_for_probe = status_row.clone();
            let switch_for_probe = switch_button.clone();
            let suggestion_for_probe = suggestion_row.clone();
            let suggestion_button_for_probe = suggestion_button.clone();
            glib::spawn_future_local(async move {
                let probe = gtk::gio::spawn_blocking(crate::core::trash::probe_system_trash).await;
                let system_available = matches!(probe, Ok(Ok(())));
                update_trash_settings_state(
                    &status_for_probe,
                    &switch_for_probe,
                    &suggestion_for_probe,
                    &suggestion_button_for_probe,
                    Some(system_available),
                    false,
                );
            });
        }

        group
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

fn add_close_on_backdrop_click(dialog: &adw::Dialog) {
    let gesture = gtk::GestureClick::new();
    gesture.connect_released(glib::clone!(@weak dialog => move |_, _n_press, x, y| {
        let picked = dialog.pick(x, y, gtk::PickFlags::DEFAULT);
        if !picked
            .as_ref()
            .is_some_and(|widget| widget_or_ancestor_has_class(widget, "settings-dialog-content"))
        {
            let _ = dialog.close();
        }
    }));
    dialog.add_controller(gesture);
}

fn trash_backend_label(backend: TrashBackend) -> String {
    match backend {
        TrashBackend::System => tr("setting.trash.backend.system"),
        TrashBackend::App => tr("setting.trash.backend.app"),
    }
}

fn trash_backend_subtitle(backend: TrashBackend, system_available: Option<bool>) -> String {
    match (backend, system_available) {
        (TrashBackend::System, Some(false)) => tr("setting.trash.system_unavailable"),
        (TrashBackend::System, _) => tr("setting.trash.using_system"),
        (TrashBackend::App, Some(true)) => tr("setting.trash.using_app_system_available"),
        (TrashBackend::App, Some(false)) => tr("setting.trash.using_app_system_unavailable"),
        (TrashBackend::App, None) => tr("setting.trash.checking_system"),
    }
}

fn trash_backend_switch_label(backend: TrashBackend, system_available: Option<bool>) -> String {
    match backend {
        TrashBackend::System => tr("setting.trash.switch_to_app"),
        TrashBackend::App if system_available == Some(false) => {
            tr("setting.trash.system_unavailable_short")
        }
        TrashBackend::App => tr("setting.trash.switch_to_system"),
    }
}

fn update_trash_settings_state(
    status_row: &adw::ActionRow,
    switch_button: &gtk::Button,
    suggestion_row: &adw::ActionRow,
    suggestion_button: &gtk::Button,
    system_available: Option<bool>,
    busy: bool,
) {
    let backend = prefs::trash_backend();
    status_row.set_subtitle(&trash_backend_subtitle(backend, system_available));
    switch_button.set_label(&trash_backend_switch_label(backend, system_available));
    let switch_sensitive = !busy
        && match backend {
            TrashBackend::System => true,
            TrashBackend::App => system_available.unwrap_or(false),
        };
    switch_button.set_sensitive(switch_sensitive);
    suggestion_row.set_visible(backend == TrashBackend::App && system_available == Some(true));
    suggestion_button.set_sensitive(!busy && system_available == Some(true));
    if busy {
        status_row.set_subtitle(&tr("setting.trash.migrating"));
    }
}

fn run_trash_backend_switch(
    parent: &gtk::Widget,
    pool: DbPool,
    target: TrashBackend,
    status_row: &adw::ActionRow,
    switch_button: &gtk::Button,
    suggestion_row: &adw::ActionRow,
    suggestion_button: &gtk::Button,
) {
    update_trash_settings_state(
        status_row,
        switch_button,
        suggestion_row,
        suggestion_button,
        None,
        true,
    );
    let parent = parent.clone();
    let status_row = status_row.clone();
    let switch_button = switch_button.clone();
    let suggestion_row = suggestion_row.clone();
    let suggestion_button = suggestion_button.clone();
    glib::spawn_future_local(async move {
        let result = gtk::gio::spawn_blocking(move || {
            crate::core::trash::switch_trash_backend(&pool, target)
        })
        .await;
        match result {
            Ok(Ok(stats)) => {
                let probe = gtk::gio::spawn_blocking(crate::core::trash::probe_system_trash).await;
                let system_available = matches!(probe, Ok(Ok(())));
                update_trash_settings_state(
                    &status_row,
                    &switch_button,
                    &suggestion_row,
                    &suggestion_button,
                    Some(system_available),
                    false,
                );
                show_settings_info_dialog(
                    &parent,
                    &trf(
                        "setting.trash.switch_success",
                        &[
                            ("backend", &trash_backend_label(target)),
                            ("count", &stats.moved.to_string()),
                        ],
                    ),
                );
            }
            Ok(Err(err)) => {
                let probe = gtk::gio::spawn_blocking(crate::core::trash::probe_system_trash).await;
                let system_available = matches!(probe, Ok(Ok(())));
                update_trash_settings_state(
                    &status_row,
                    &switch_button,
                    &suggestion_row,
                    &suggestion_button,
                    Some(system_available),
                    false,
                );
                show_settings_error_dialog(
                    &parent,
                    &trf(
                        "setting.trash.switch_failed",
                        &[("error", &err.to_string())],
                    ),
                );
            }
            Err(err) => {
                update_trash_settings_state(
                    &status_row,
                    &switch_button,
                    &suggestion_row,
                    &suggestion_button,
                    None,
                    false,
                );
                show_settings_error_dialog(
                    &parent,
                    &trf(
                        "setting.trash.switch_failed",
                        &[("error", &format!("{err:?}"))],
                    ),
                );
            }
        }
    });
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

#[derive(Clone, Copy)]
enum ScanPathListKind {
    Custom,
    Excluded,
}

fn build_scan_paths_group(parent: &gtk::Widget) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&tr("setting.section.scan_paths"));
    group.set_description(Some(&tr("setting.section.scan_paths_description")));
    group.add_css_class("settings-preferences-group");

    add_scan_path_section(
        &group,
        parent,
        ScanPathListKind::Custom,
        &tr("setting.scan_paths.custom"),
        &tr("setting.scan_paths.custom_description"),
        prefs::custom_scan_roots(),
    );
    add_scan_path_section(
        &group,
        parent,
        ScanPathListKind::Excluded,
        &tr("setting.scan_paths.excluded"),
        &tr("setting.scan_paths.excluded_description"),
        prefs::excluded_scan_roots(),
    );

    group
}

fn add_scan_path_section(
    group: &adw::PreferencesGroup,
    parent: &gtk::Widget,
    kind: ScanPathListKind,
    title: &str,
    subtitle: &str,
    paths: Vec<PathBuf>,
) {
    let row = adw::ActionRow::new();
    row.add_css_class("settings-action-row");
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_activatable(false);

    let add_button = gtk::Button::new();
    add_button.set_icon_name("list-add-symbolic");
    add_button.set_valign(gtk::Align::Center);
    add_button.add_css_class("glass-toolbar-button");
    add_button.set_tooltip_text(Some(&tr("setting.scan_paths.add")));
    row.add_suffix(&add_button);
    group.add(&row);

    for path in paths {
        add_scan_path_value_row(group, parent, kind, path);
    }

    let parent_for_add = parent.clone();
    let group_for_add = group.clone();
    add_button.connect_clicked(move |_| {
        choose_scan_folder(&parent_for_add, {
            let parent = parent_for_add.clone();
            let group = group_for_add.clone();
            move |path| match append_scan_path(kind, path.clone()) {
                Ok(true) => {
                    add_scan_path_value_row(&group, &parent, kind, path);
                    show_restart_required_dialog(&parent);
                }
                Ok(false) => show_restart_required_dialog(&parent),
                Err(err) => show_settings_error_dialog(
                    &parent,
                    &trf("setting.scan_paths.save_failed", &[("error", &err)]),
                ),
            }
        });
    });
}

fn add_scan_path_value_row(
    group: &adw::PreferencesGroup,
    parent: &gtk::Widget,
    kind: ScanPathListKind,
    path: PathBuf,
) {
    let row = adw::ActionRow::new();
    row.add_css_class("settings-action-row");
    row.set_title(&path.to_string_lossy());
    row.set_activatable(false);

    let remove_button = gtk::Button::new();
    remove_button.set_icon_name("user-trash-symbolic");
    remove_button.set_valign(gtk::Align::Center);
    remove_button.add_css_class("glass-toolbar-button");
    remove_button.add_css_class("glass-toolbar-danger");
    remove_button.set_tooltip_text(Some(&tr("setting.scan_paths.remove")));
    row.add_suffix(&remove_button);
    group.add(&row);

    let parent_for_remove = parent.clone();
    let row_for_remove = row.clone();
    remove_button.connect_clicked(move |_| match remove_scan_path(kind, &path) {
        Ok(()) => {
            row_for_remove.set_visible(false);
            show_restart_required_dialog(&parent_for_remove);
        }
        Err(err) => show_settings_error_dialog(
            &parent_for_remove,
            &trf("setting.scan_paths.save_failed", &[("error", &err)]),
        ),
    });
}

fn choose_scan_folder<F>(parent: &gtk::Widget, on_selected: F)
where
    F: Fn(PathBuf) + 'static,
{
    let native = gtk::FileChooserNative::builder()
        .title(tr("setting.scan_paths.choose_folder"))
        .action(gtk::FileChooserAction::SelectFolder)
        .accept_label(tr("setting.scan_paths.choose"))
        .cancel_label(tr("button.cancel"))
        .build();
    if let Some(window) = parent.root().and_downcast::<gtk::Window>() {
        native.set_transient_for(Some(&window));
    }
    native.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(path) = dialog.file().and_then(|file| file.path()) {
                on_selected(path);
            }
        }
        dialog.destroy();
    });
    native.show();
}

fn append_scan_path(kind: ScanPathListKind, path: PathBuf) -> Result<bool, String> {
    let mut paths = scan_paths(kind);
    if paths.iter().any(|existing| existing == &path) {
        return Ok(false);
    }
    paths.push(path);
    set_scan_paths(kind, &paths)?;
    Ok(true)
}

fn remove_scan_path(kind: ScanPathListKind, path: &PathBuf) -> Result<(), String> {
    let mut paths = scan_paths(kind);
    paths.retain(|existing| existing != path);
    set_scan_paths(kind, &paths)
}

fn scan_paths(kind: ScanPathListKind) -> Vec<PathBuf> {
    match kind {
        ScanPathListKind::Custom => prefs::custom_scan_roots(),
        ScanPathListKind::Excluded => prefs::excluded_scan_roots(),
    }
}

fn set_scan_paths(kind: ScanPathListKind, paths: &[PathBuf]) -> Result<(), String> {
    match kind {
        ScanPathListKind::Custom => prefs::set_custom_scan_roots(paths),
        ScanPathListKind::Excluded => prefs::set_excluded_scan_roots(paths),
    }
}

fn album_initial_load_limit(total: i64) -> u32 {
    let total = u32::try_from(total.max(0)).unwrap_or(u32::MAX);
    let initial = crate::core::runtime_config::max_rendered_grid_items();
    total.min(u32::try_from(initial).unwrap_or(u32::MAX))
}

fn album_backfill_fetch_limit(current_len: u32, total: u32) -> u32 {
    if current_len >= total {
        return 0;
    }
    let ui_cap =
        u32::try_from(crate::core::runtime_config::ui_media_list_cap()).unwrap_or(u32::MAX);
    if current_len >= ui_cap {
        return 0;
    }
    total.saturating_sub(current_len).min(ui_cap - current_len)
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
        tracing::info!(
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

        tracing::info!(
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
            tracing::info!(
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestartSpec {
    program: PathBuf,
    args: Vec<OsString>,
    exit_current_process_after_spawn: bool,
}

fn restart_spec_from(program: PathBuf, args: Vec<OsString>) -> RestartSpec {
    RestartSpec {
        program,
        args,
        exit_current_process_after_spawn: true,
    }
}

fn current_restart_spec() -> Result<RestartSpec, String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    let args = std::env::args_os().skip(1).collect();
    Ok(restart_spec_from(program, args))
}

fn restart_application() -> Result<(), String> {
    let spec = current_restart_spec()?;
    Command::new("sh")
        .arg("-c")
        .arg("sleep 0.2; exec \"$@\"")
        .arg("photo-viewer-restart")
        .arg(&spec.program)
        .args(&spec.args)
        .spawn()
        .map_err(|e| e.to_string())?;
    if spec.exit_current_process_after_spawn {
        let app = gtk::Application::default();
        for window in app.windows() {
            window.close();
        }
        app.quit();
        std::process::exit(0);
    }
    Ok(())
}

fn show_restart_required_dialog(parent: &gtk::Widget) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("setting.restart_required_title"))
        .body(tr("setting.restart_required_body"))
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("later", &tr("button.no"));
    dialog.add_response("restart", &tr("button.yes"));
    dialog.set_default_response(Some("restart"));
    dialog.set_close_response("later");

    let parent_for_error = parent.clone();
    dialog.connect_response(Some("restart"), move |_, _| {
        if let Err(err) = restart_application() {
            show_settings_error_dialog(
                &parent_for_error,
                &trf("setting.restart_now_failed", &[("error", &err)]),
            );
        }
    });

    dialog.present(parent);
}

fn show_settings_restart_dialog(parent: &gtk::Widget, success: bool, error: Option<String>) {
    let heading = if success {
        tr("setting.locale.saved")
    } else {
        tr("setting.locale.failed")
    };
    let body = if let Some(error) = error {
        trf("setting.restart_failed", &[("error", &error)])
    } else {
        tr("setting.restart_hint")
    };
    let dialog = adw::AlertDialog::builder()
        .heading(&heading)
        .body(&body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
}

/// Surface a non-fatal settings error (e.g. failed to persist the Liquid
/// Glass pref) as a glass alert dialog. Mirrors the locale restart dialog.
fn show_settings_error_dialog(parent: &gtk::Widget, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("setting.save_failed"))
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
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

fn show_settings_info_dialog(parent: &gtk::Widget, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("setting.done"))
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
}

fn update_storage_size_async<F>(row: &adw::ActionRow, compute_size: F)
where
    F: FnOnce() -> u64 + Send + 'static,
{
    row.set_subtitle(&tr("setting.storage_usage_calculating"));

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(compute_size());
    });

    let row = row.downgrade();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(size) => {
                if let Some(row) = row.upgrade() {
                    row.set_subtitle(&format_size(size));
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => {
                if row.upgrade().is_some() {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn persist_locale(locale: &str) -> Result<(), String> {
    let path = config::config_dir().join("i18n.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut object = match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str::<Value>(&data)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default(),
        Err(_) => Map::new(),
    };
    object.insert("locale".to_string(), Value::String(locale.to_string()));
    let value = Value::Object(object);
    let json = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
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

/// Show a confirmation dialog for clearing cache/database.
fn show_clear_confirm_dialog<F: Fn() + 'static>(
    parent: &gtk::Widget,
    title: &str,
    body: &str,
    on_confirm: F,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(title)
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("cancel", &tr("button.cancel"));
    dialog.add_response("confirm", &tr("dialog.confirm"));
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.connect_response(Some("confirm"), move |_, _| {
        on_confirm();
    });

    dialog.present(parent);
}

/// Show a success toast notification.
fn show_clear_success_toast(message: &str) {
    let app = gtk::Application::default();
    if let Some(window) = app.active_window() {
        if let Ok(_win) = window.downcast::<MainWindow>() {
            let notification = gtk::gio::Notification::new(&tr("setting.clear_success"));
            notification.set_body(Some(message));
            app.send_notification(None, &notification);
        }
    }
}

/// Show an error toast notification.
fn show_clear_error_toast(message: &str) {
    let app = gtk::Application::default();
    if let Some(window) = app.active_window() {
        if let Ok(_win) = window.downcast::<MainWindow>() {
            let notification = gtk::gio::Notification::new(&tr("setting.clear_failed"));
            notification.set_body(Some(message));
            app.send_notification(None, &notification);
        }
    }
}

/// Format bytes into human-readable size (KB, MB, GB).
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

// ── Sidebar row builders ──────────────────────────────────────────────────
// Rows share the `.glass-sidebar-row` material (hover/selected glass veil from
// both glass modes); the per-kind classes below only own layout (indentation,
// count badge, section header weight).

/// A plain navigable sidebar row: leading symbolic icon + label.
fn build_nav_row(
    label: &str,
    icon_name: &str,
    include_count: bool,
) -> (gtk::ListBoxRow, Option<gtk::Label>) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("glass-sidebar-icon");

    let lbl = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .css_classes(["glass-sidebar-label"])
        .build();

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();
    box_.append(&icon);
    box_.append(&lbl);
    let count_label = if include_count {
        let count = gtk::Label::builder()
            .label("0")
            .visible(false)
            .halign(gtk::Align::End)
            .css_classes(["glass-sidebar-count", "photos-sidebar-count"])
            .build();
        box_.append(&count);
        Some(count)
    } else {
        None
    };
    row.set_child(Some(&box_));
    (row, count_label)
}

/// The "Albums" group header: a non-selectable disclosure row. The arrow is
/// returned so the collapse toggle can swap its icon.
fn build_albums_header_row(label: &str) -> (gtk::ListBoxRow, gtk::Image) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");
    row.add_css_class("glass-sidebar-section");

    let arrow = gtk::Image::from_icon_name("pan-down-symbolic");
    arrow.add_css_class("glass-sidebar-arrow");

    let lbl = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .css_classes(["glass-sidebar-section-label"])
        .build();

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();
    box_.append(&arrow);
    box_.append(&lbl);
    row.set_child(Some(&box_));
    (row, arrow)
}

/// An album sub-row: indented under the Albums header, with the album cover,
/// the album name, and a right-aligned count badge.
fn build_album_row(album: &Album, loader: Option<Arc<ThumbnailLoader>>) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");
    row.add_css_class("glass-sidebar-subrow");

    let cover = build_sidebar_album_cover(album, loader);

    let name = gtk::Label::builder()
        .label(album.display_name())
        .halign(gtk::Align::Start)
        .hexpand(true)
        .css_classes(["glass-sidebar-label"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(18)
        .build();

    let count = gtk::Label::builder()
        .label(trf(
            "album.count",
            &[("count", &album.photo_count.to_string())],
        ))
        .halign(gtk::Align::End)
        .css_classes(["glass-sidebar-count"])
        .build();

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();
    box_.append(&cover);
    box_.append(&name);
    box_.append(&count);
    row.set_child(Some(&box_));
    row
}

fn same_sidebar_album_identities(current: &[Album], next: &[Album]) -> bool {
    current.len() == next.len()
        && current
            .iter()
            .zip(next)
            .all(|(current, next)| same_sidebar_album_identity(current, next))
}

fn same_sidebar_album_identity(current: &Album, next: &Album) -> bool {
    current.folder_path == next.folder_path && current.is_virtual == next.is_virtual
}

fn sidebar_album_identities_are_ordered_subset(current: &[Album], next: &[Album]) -> bool {
    if next.len() > current.len() {
        return false;
    }
    let mut search_from = 0;
    for next_album in next {
        let Some(offset) = current[search_from..]
            .iter()
            .position(|current_album| same_sidebar_album_identity(current_album, next_album))
        else {
            return false;
        };
        search_from += offset + 1;
    }
    true
}

fn find_sidebar_album_identity_index(albums: &[Album], needle: &Album) -> Option<usize> {
    albums
        .iter()
        .position(|album| same_sidebar_album_identity(album, needle))
}

fn sidebar_list_child_count(list: &gtk::ListBox) -> u32 {
    list.observe_children().n_items()
}

fn sidebar_album_summary(albums: &[Album]) -> String {
    const MAX_ITEMS: usize = 8;
    let mut parts = albums
        .iter()
        .take(MAX_ITEMS)
        .map(sidebar_album_identity_for_log)
        .collect::<Vec<_>>();
    if albums.len() > MAX_ITEMS {
        parts.push(format!("...(+{})", albums.len() - MAX_ITEMS));
    }
    parts.join(" | ")
}

fn sidebar_album_identity_for_log(album: &Album) -> String {
    let kind = if album.is_virtual {
        "virtual"
    } else {
        "folder"
    };
    format!(
        "{}:{} count={} name={}",
        kind,
        album.folder_path.display(),
        album.photo_count,
        album.display_name()
    )
}

fn update_album_row_in_place(
    row: &gtk::ListBoxRow,
    previous: &Album,
    album: &Album,
    loader: Option<Arc<ThumbnailLoader>>,
) {
    if let Some(name) = find_label_with_css_class(row.upcast_ref(), "glass-sidebar-label") {
        name.set_label(&album.display_name());
    }
    if let Some(count) = find_label_with_css_class(row.upcast_ref(), "glass-sidebar-count") {
        count.set_label(&trf(
            "album.count",
            &[("count", &album.photo_count.to_string())],
        ));
    }
    if previous.cover_uri != album.cover_uri || previous.last_modified != album.last_modified {
        replace_sidebar_album_cover(row, album, loader);
    }
}

fn replace_sidebar_album_cover(
    row: &gtk::ListBoxRow,
    album: &Album,
    loader: Option<Arc<ThumbnailLoader>>,
) {
    let Some(cover) = find_sidebar_album_cover(row.upcast_ref()) else {
        return;
    };
    let Some(parent) = cover.parent().and_downcast::<gtk::Box>() else {
        return;
    };
    let replacement = build_sidebar_album_cover(album, loader);
    parent.remove(&cover);
    parent.prepend(&replacement);
}

fn find_label_with_css_class(widget: &gtk::Widget, class_name: &str) -> Option<gtk::Label> {
    if widget.has_css_class(class_name) {
        if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
            return Some(label);
        }
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_label_with_css_class(&current, class_name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn find_sidebar_album_cover(widget: &gtk::Widget) -> Option<SquareTile> {
    if widget.has_css_class("glass-sidebar-cover") {
        if let Ok(tile) = widget.clone().downcast::<SquareTile>() {
            return Some(tile);
        }
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_sidebar_album_cover(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn build_sidebar_album_cover(album: &Album, loader: Option<Arc<ThumbnailLoader>>) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(24);
    tile.set_halign(gtk::Align::Center);
    tile.set_valign(gtk::Align::Center);
    tile.set_hexpand(false);
    tile.set_vexpand(false);
    tile.add_css_class("glass-sidebar-cover");
    tile.set_paintable(Some(&sidebar_cover_placeholder_texture()));

    let Some(loader) = loader else {
        return tile;
    };
    let Some(cover_uri) = album.cover_uri.as_ref().cloned() else {
        return tile;
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request(
        cover_uri,
        ThumbnailSize::Small,
        Some(std::time::SystemTime::from(album.last_modified)),
        tx,
        crate::core::thumbnails::TIER_NORMAL,
    );
    let tile_weak = tile.downgrade();
    gtk::glib::spawn_future_local(async move {
        let Ok(loaded) = rx.await else {
            return;
        };
        if let Some(tile) = tile_weak.upgrade() {
            tile.set_paintable(Some(&loaded.texture));
        }
    });

    tile
}

fn sidebar_cover_placeholder_texture() -> gtk::gdk::Texture {
    let pixbuf = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 2, 2)
        .expect("allocate 2x2 sidebar album cover placeholder pixbuf");
    pixbuf.fill(0xC8C8C8FF);
    gtk::gdk::Texture::for_pixbuf(&pixbuf)
}

pub fn build_album_context_menu_for_tests(album: &Album) -> gtk::Box {
    glass_context_menu::build_menu_panel_for_tests(build_album_context_menu_items(
        album, None, None, None, None,
    ))
}

fn build_album_context_menu_items(
    album: &Album,
    on_manage: Option<Box<dyn Fn() + 'static>>,
    on_delete: Option<Box<dyn Fn() + 'static>>,
    on_ignore: Option<Box<dyn Fn() + 'static>>,
    on_select: Option<Box<dyn Fn() + 'static>>,
) -> Vec<GlassMenuItem> {
    let mut items = Vec::new();

    items.push(GlassMenuItem::new(
        tr("album.context.manage"),
        GlassMenuItemKind::Normal,
        move || {
            if let Some(on_manage) = &on_manage {
                on_manage();
            }
        },
    ));

    if let Some(on_select) = on_select {
        items.push(GlassMenuItem::new(
            tr("album.context.multi_select"),
            GlassMenuItemKind::Suggested,
            move || {
                on_select();
            },
        ));
    }

    if !album.is_virtual {
        items.push(GlassMenuItem::new(
            tr("album.context.ignore"),
            GlassMenuItemKind::Normal,
            move || {
                if let Some(on_ignore) = &on_ignore {
                    on_ignore();
                }
            },
        ));
        items.push(GlassMenuItem::new(
            tr("album.context.delete"),
            GlassMenuItemKind::Danger,
            move || {
                if let Some(on_delete) = &on_delete {
                    on_delete();
                }
            },
        ));
    }

    items
}

/// Find the `MainWindow` that owns `nav` and refresh its sidebar album rows.
/// Replaces the old "refresh the AlbumsPage grid" hook — the grid page is gone;
/// the albums now live directly in the sidebar, so a favorite/trash change must
/// refresh their counts here.
pub(crate) fn refresh_albums_sidebar(nav: &adw::NavigationView) {
    if let Some(window) = nav
        .ancestor(MainWindow::static_type())
        .and_downcast::<MainWindow>()
    {
        window.refresh_sidebar_snapshot_async();
    }
}

pub(crate) fn refresh_after_album_operation(nav: &adw::NavigationView) {
    if let Some(window) = nav
        .ancestor(MainWindow::static_type())
        .and_downcast::<MainWindow>()
    {
        let visible_page_type = nav
            .visible_page()
            .map(|page| page.type_().name().to_string())
            .unwrap_or_else(|| "<none>".to_string());
        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            "PHOTO_REFRESH_TRACE refresh_after_album_operation visible_page_type={}",
            visible_page_type
        );
        window.refresh_shared_media_list_from_repository();
        window.refresh_visible_album_detail_page();
        window.refresh_sidebar_snapshot_async();
    }
}

#[tracing::instrument(name = "sidebar:load_album_snapshot", skip(pool))]
fn load_sidebar_album_snapshot(pool: &DbPool) -> SidebarAlbumSnapshot {
    let live_count = crate::core::repository::MediaRepository::new(pool.clone())
        .count(crate::core::repository::MediaQuery::LiveAll)
        .ok();
    SidebarAlbumSnapshot {
        albums: list_with_favorites(pool).unwrap_or_default(),
        media_type_albums: list_media_type_albums(pool).unwrap_or_default(),
        live_count,
    }
}

struct AlbumDeleteUiResult {
    operation: std::result::Result<MediaMutation, String>,
    deleted_paths: Vec<PathBuf>,
    remaining_live_uris: HashSet<String>,
    remaining_live_folder_paths: HashSet<PathBuf>,
    unknown_remaining_live_paths: HashSet<PathBuf>,
}

struct AlbumIgnoreUiResult {
    folder_path: PathBuf,
    removed_count: usize,
}

fn ignore_album_worker(
    pool: DbPool,
    folder_path: PathBuf,
) -> std::result::Result<AlbumIgnoreUiResult, String> {
    append_scan_path(ScanPathListKind::Excluded, folder_path.clone())?;
    let removed_count = crate::core::db::delete_live_media_by_folder(&pool, &folder_path)
        .map_err(|err| err.to_string())?;
    crate::core::albums::refresh(&pool).map_err(|err| err.to_string())?;
    Ok(AlbumIgnoreUiResult {
        folder_path,
        removed_count,
    })
}

fn delete_albums_to_trash_worker(pool: DbPool, albums: Vec<Album>) -> AlbumDeleteUiResult {
    let deleted_paths = albums
        .iter()
        .filter(|album| !album.is_virtual)
        .map(|album| album.folder_path.clone())
        .collect::<Vec<_>>();
    let operation = crate::core::album_ops::delete_albums_to_trash(&pool, &albums)
        .map_err(|err| err.to_string());

    let mut remaining_live_uris = HashSet::new();
    let mut remaining_live_folder_paths = HashSet::new();
    let mut unknown_remaining_live_paths = HashSet::new();
    for path in &deleted_paths {
        match crate::core::db::list_media_by_folder(&pool, path) {
            Ok(items) => {
                if !items.is_empty() {
                    remaining_live_folder_paths.insert(path.clone());
                }
                remaining_live_uris.extend(items.into_iter().map(|item| item.uri));
            }
            Err(err) => {
                tracing::warn!(
                    "failed to query remaining live media for album {}: {err}",
                    path.display()
                );
                remaining_live_folder_paths.insert(path.clone());
                unknown_remaining_live_paths.insert(path.clone());
            }
        }
    }

    AlbumDeleteUiResult {
        operation,
        deleted_paths,
        remaining_live_uris,
        remaining_live_folder_paths,
        unknown_remaining_live_paths,
    }
}

fn remove_ignored_album_media_from_media_list(
    media_list: &gtk::gio::ListStore,
    ignored_path: &PathBuf,
) {
    let mut index = 0;
    while index < media_list.n_items() {
        let should_remove = media_list
            .item(index)
            .and_downcast::<glib::BoxedAnyObject>()
            .is_some_and(|boxed| {
                let item = boxed.borrow::<MediaItem>();
                item.folder_path == *ignored_path
            });
        if should_remove {
            media_list.remove(index);
        } else {
            index += 1;
        }
    }
}

fn remove_deleted_album_media_from_media_list(
    media_list: &gtk::gio::ListStore,
    deleted_paths: &[PathBuf],
    remaining_live_uris: &HashSet<String>,
    unknown_remaining_live_paths: &HashSet<PathBuf>,
) {
    if deleted_paths.is_empty() {
        return;
    }

    let deleted_paths: HashSet<&PathBuf> = deleted_paths.iter().collect();
    let mut index = 0;
    while index < media_list.n_items() {
        let should_remove = media_list
            .item(index)
            .and_downcast::<glib::BoxedAnyObject>()
            .is_some_and(|boxed| {
                let item = boxed.borrow::<MediaItem>();
                deleted_paths.contains(&item.folder_path)
                    && !unknown_remaining_live_paths.contains(&item.folder_path)
                    && !remaining_live_uris.contains(&item.uri)
            });
        if should_remove {
            media_list.remove(index);
        } else {
            index += 1;
        }
    }
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

        let spec = restart_spec_from(exe.clone(), args.clone());

        assert_eq!(spec.program, exe);
        assert_eq!(spec.args, args);
    }

    #[test]
    fn restart_spec_requires_current_process_exit_after_spawn() {
        let spec = restart_spec_from(PathBuf::from("/tmp/photo-viewer"), Vec::new());

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
