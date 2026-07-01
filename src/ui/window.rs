//! Main window: sidebar + content area
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

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
use crate::core::i18n::{locale, tr, trf};
use crate::core::media::MediaItem;
use crate::core::repository::MediaMutation;
use crate::core::repository::MediaQuery;
use crate::core::thumbnails::ThumbnailLoader;
use crate::core::{prefs, runtime_config};
use crate::ui::album_detail_page::{media_query_for_album, AlbumDetailPage};
use crate::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
use crate::ui::TrashPage;
use crate::ui::{grid_css, theme};

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

/// Icon shown beside each album row, matching the screenshot's per-album glyph.
fn album_icon_name(album: &Album) -> &'static str {
    if album.is_favorites_album() {
        "emblem-favorite-symbolic"
    } else if album.is_images_album() {
        "image-x-generic-symbolic"
    } else if album.is_videos_album() {
        "video-x-generic-symbolic"
    } else if album.is_motion_photos_album() {
        "media-playback-start-symbolic"
    } else {
        "folder-symbolic"
    }
}

mod imp {
    use super::*;
    use adw::subclass::prelude::*;

    #[derive(gtk::CompositeTemplate, gtk::glib::Properties, Default)]
    #[properties(wrapper_type = super::MainWindow)]
    #[template(file = "../../data/ui/window.ui")]
    pub struct MainWindow {
        pub pool: RefCell<Option<DbPool>>,
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
        gtk::glib::Object::builder()
            .property("application", app)
            .property("title", tr("app.title"))
            .build()
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
        self.rebuild_album_rows();
        self.rebuild_media_type_rows();
    }

    fn rebuild_album_rows(&self) {
        let started = std::time::Instant::now();
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let album_list = self.imp().album_list.get();

        while let Some(child) = album_list.first_child() {
            album_list.remove(&child);
        }
        self.imp().album_rows.borrow_mut().clear();
        self.imp().album_targets.borrow_mut().clear();

        let albums = list_with_favorites(&pool).unwrap_or_default();
        let album_count = albums.len();
        let expanded = self.imp().albums_expanded.get();
        self.imp().album_scroll.set_visible(expanded);

        for album in albums {
            let row = build_album_row(&album);
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
            "SIDEBAR_ALBUM_REBUILD rows={} elapsed_ms={}",
            album_count,
            started.elapsed().as_millis()
        );
    }

    fn rebuild_media_type_rows(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let media_type_list = self.imp().media_type_list.get();

        while let Some(child) = media_type_list.first_child() {
            media_type_list.remove(&child);
        }
        self.imp().media_type_rows.borrow_mut().clear();
        self.imp().media_type_targets.borrow_mut().clear();

        let albums = list_media_type_albums(&pool).unwrap_or_default();
        let expanded = self.imp().media_types_expanded.get();
        self.imp().media_type_header_list.set_visible(true);
        self.imp().media_type_scroll.set_visible(expanded);

        for album in albums {
            let row = build_album_row(&album);
            row.set_visible(true);
            media_type_list.append(&row);
            self.imp().media_type_rows.borrow_mut().push(row);
            self.imp().media_type_targets.borrow_mut().push(album);
        }

        self.reselect_active_album_row();
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
        let expanded = !self.imp().albums_expanded.get();
        self.imp().albums_expanded.set(expanded);
        if let Some(arrow) = self.imp().albums_arrow.borrow().clone() {
            arrow.set_icon_name(if expanded {
                Some("pan-down-symbolic")
            } else {
                Some("pan-end-symbolic")
            });
        }
        self.imp().album_scroll.set_visible(expanded);
        self.imp().album_trash_wrapper.set_vexpand(expanded);
        self.imp().sidebar_spacer.set_vexpand(!expanded);
    }

    pub fn toggle_media_types_expanded(&self) {
        let expanded = !self.imp().media_types_expanded.get();
        self.imp().media_types_expanded.set(expanded);
        if let Some(arrow) = self.imp().media_types_arrow.borrow().clone() {
            arrow.set_icon_name(if expanded {
                Some("pan-down-symbolic")
            } else {
                Some("pan-end-symbolic")
            });
        }
        self.imp().media_type_scroll.set_visible(expanded);
    }

    /// Rebuild the sidebar album rows from the current DB snapshot so counts
    /// stay live after favorites/trash changes.
    pub fn refresh_album_rows(&self) {
        self.update_photos_count_label();
        self.rebuild_album_rows();
        self.rebuild_media_type_rows();
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

    fn update_photos_count_label(&self) {
        let Some(label) = self.imp().photos_count_label.borrow().as_ref().cloned() else {
            return;
        };
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
        label.set_label(&count.to_string());
        label.set_visible(true);
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
        let list = self.imp().sidebar_list.get();
        let trash_list = self.imp().trash_list.get();
        let album_list = self.imp().album_list.get();
        let media_type_list = self.imp().media_type_list.get();
        let gesture = gtk::GestureSwipe::new();
        gesture.connect_swipe(
            glib::clone!(@weak self as window, @weak nav_view => move |_gesture, velocity_x, _velocity_y| {
                if velocity_x.abs() > 450.0 {
                    window.show_settings_dialog();
                }
            }),
        );
        self.imp().nav_view.get().add_controller(gesture);

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
                window.open_album(&nav_view, album);
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
                window.open_album(&nav_view, album);
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

    pub(crate) fn open_album(&self, nav_view: &adw::NavigationView, album: Album) {
        let total_start = Instant::now();
        let album_name = album.display_name();
        let album_path = album.folder_path.to_string_lossy().into_owned();
        let album_is_virtual = album.is_virtual;
        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = album_is_virtual,
            expected_count = album.photo_count,
            "album_switch: begin"
        );

        // Already viewing this album → no-op (avoids rebuilding/pushing a
        // duplicate detail page on a re-select).
        let visible_check_start = Instant::now();
        let already_visible = nav_view
            .visible_page()
            .and_then(|page| page.downcast::<AlbumDetailPage>().ok())
            .is_some_and(|detail| {
                detail.album_folder_path().as_deref() == Some(album.folder_path.as_path())
            });
        if already_visible {
            tracing::info!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                visible_check_ms = visible_check_start.elapsed().as_millis(),
                total_ms = total_start.elapsed().as_millis(),
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
        let pop_start = Instant::now();
        pop_to_photos_root(nav_view);
        let pop_ms = pop_start.elapsed().as_millis();

        // 文件夹相册和虚拟相册都从数据库加载，不受 UI_MEDIA_LIST_CAP
        // 或启动时 master GTK 列表窗口限制。切换路径只同步加载首个可渲染窗口；
        // 大相册剩余项后台补齐，避免打开相册时阻塞主线程。
        let load_start = Instant::now();
        let query = media_query_for_album(&album);
        let initial_limit = album_initial_load_limit(album.photo_count);
        let page_result = crate::core::repository::MediaRepository::new(pool.clone()).page(
            query.clone(),
            0,
            initial_limit,
        );
        let (items, total_items) = match page_result {
            Ok(page) => {
                let total = page.total;
                (page.items, total)
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
        };
        let load_ms = load_start.elapsed().as_millis();
        let item_count = items.len();

        let store_start = Instant::now();
        let filtered = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for item in items {
            filtered.append(&glib::BoxedAnyObject::new(item));
        }
        let store_ms = store_start.elapsed().as_millis();

        let page_start = Instant::now();
        let page = AlbumDetailPage::new(album, filtered.clone(), master, pool.clone(), loader);
        let page_ms = page_start.elapsed().as_millis();
        page.set_nav_target(nav_view);
        let push_start = Instant::now();
        nav_view.push(&page);
        let push_ms = push_start.elapsed().as_millis();

        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = album_is_virtual,
            item_count,
            pop_ms,
            load_ms,
            store_ms,
            page_ms,
            push_ms,
            total_ms = total_start.elapsed().as_millis(),
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

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let worker_result =
                gtk::gio::spawn_blocking(move || delete_albums_to_trash_worker(pool, albums)).await;

            let Some(window) = weak.upgrade() else {
                return;
            };
            match worker_result {
                Ok(result) => {
                    if let Err(err) = &result.operation {
                        tracing::warn!("failed to delete album to trash: {err}");
                    }
                    if let Some(media_list) = window.imp().media_list.borrow().as_ref() {
                        remove_deleted_album_media_from_media_list(
                            media_list,
                            &result.deleted_paths,
                            &result.remaining_live_uris,
                            &result.unknown_remaining_live_paths,
                        );
                    }
                    window.refresh_album_rows();

                    let active_should_close = window
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
                Err(err) => {
                    tracing::warn!("album delete worker failed: {err:?}");
                    window.refresh_album_rows();
                }
            }
        });
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
        dialog.connect_closed(move |_| {
            if let Some(window) = weak.upgrade() {
                window
                    .imp()
                    .nav_view
                    .remove_css_class("settings-background-blur");
                window.imp().settings_dialog.borrow_mut().take();
            }
        });
        dialog.present(self);
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

fn album_initial_load_limit(total: i64) -> u32 {
    let total = u32::try_from(total.max(0)).unwrap_or(u32::MAX);
    let initial = crate::core::runtime_config::max_rendered_grid_items();
    total.min(u32::try_from(initial).unwrap_or(u32::MAX))
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
    glib::spawn_future_local(async move {
        let query_for_worker = query.clone();
        let fetch_started = Instant::now();
        let result = gtk::gio::spawn_blocking(move || {
            crate::core::repository::MediaRepository::new(pool)
                .page(query_for_worker, start, u32::MAX)
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
                    fetch_ms = fetch_started.elapsed().as_millis(),
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
                    fetch_ms = fetch_started.elapsed().as_millis(),
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
            fetched = items.len(),
            fetch_ms = fetch_started.elapsed().as_millis(),
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
    let append_started = Instant::now();
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
                append_ms = append_started.elapsed().as_millis(),
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

/// An album sub-row: indented under the Albums header, with a per-kind icon,
/// the album name, and a right-aligned count badge.
fn build_album_row(album: &Album) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");
    row.add_css_class("glass-sidebar-subrow");

    let icon = gtk::Image::from_icon_name(album_icon_name(album));
    icon.add_css_class("glass-sidebar-icon");

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
    box_.append(&icon);
    box_.append(&name);
    box_.append(&count);
    row.set_child(Some(&box_));
    row
}

pub fn build_album_context_menu_for_tests(album: &Album) -> gtk::Box {
    glass_context_menu::build_menu_panel_for_tests(build_album_context_menu_items(
        album, None, None, None,
    ))
}

fn build_album_context_menu_items(
    album: &Album,
    on_manage: Option<Box<dyn Fn() + 'static>>,
    on_delete: Option<Box<dyn Fn() + 'static>>,
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
        window.refresh_album_rows();
    }
}

struct AlbumDeleteUiResult {
    operation: std::result::Result<MediaMutation, String>,
    deleted_paths: Vec<PathBuf>,
    remaining_live_uris: HashSet<String>,
    remaining_live_folder_paths: HashSet<PathBuf>,
    unknown_remaining_live_paths: HashSet<PathBuf>,
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
