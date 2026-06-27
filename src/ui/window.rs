//! Main window: sidebar + content area
use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

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
use crate::core::albums::{list_with_favorites, set_album_order, Album};
use crate::core::db::DbPool;
use crate::core::i18n::{locale, tr, trf};
use crate::core::prefs;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::album_detail_page::{filtered_items_for_album, AlbumDetailPage};
use crate::ui::grid_css;
use crate::ui::TrashPage;

/// What a sidebar row navigates to. The `Gtk.ListBox` row at index `i` maps to
/// `targets[i]` (kept in lock-step by [`MainWindow::populate_sidebar`] and
/// [`MainWindow::rebuild_album_rows`]). The albums group is a non-selectable
/// header that only collapses/expands its children; every other row is a real
/// navigation target.
#[derive(Clone, Debug)]
pub enum SidebarTarget {
    Photos,
    AlbumsHeader,
    Album(Album),
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
        /// The album rows nested under the "Albums" group header — kept so the
        /// collapse toggle and the live refresh can hide/rebuild them precisely.
        pub album_rows: RefCell<Vec<gtk::ListBoxRow>>,
        /// The disclosure arrow on the Albums header; swapped between
        /// `pan-down-symbolic` (expanded) and `pan-end-symbolic` (collapsed).
        pub albums_arrow: RefCell<Option<gtk::Image>>,
        /// Whether the Albums group is currently expanded.
        pub albums_expanded: Cell<bool>,
        /// folder_path of the album whose `AlbumDetailPage` is on top of the
        /// stack, so a live refresh can re-select its sidebar row.
        pub active_album: RefCell<Option<PathBuf>>,
        /// Set while we programmatically `select_row`, so the `row-selected`
        /// handler does not re-enter navigation during a refresh.
        pub selecting_programmatically: Cell<bool>,
        pub settings_dialog: RefCell<Option<adw::Dialog>>,
        #[template_child]
        pub sidebar_list: TemplateChild<gtk::ListBox>,
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
        let list = self.imp().sidebar_list.get();
        let mut targets = Vec::new();

        let photos_row = build_nav_row(&tr("sidebar.photos"), "view-grid-symbolic");
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

        let trash_row = build_nav_row(&tr("sidebar.trash"), "user-trash-symbolic");
        list.append(&trash_row);
        targets.push(SidebarTarget::Trash);

        self.imp()
            .settings_button
            .set_tooltip_text(Some(&tr("sidebar.settings")));

        *self.imp().targets.borrow_mut() = targets;

        // Highlight Photos as the default root view. Done before
        // connect_sidebar wires row-selected, so this never triggers navigation.
        self.imp().selecting_programmatically.set(true);
        list.select_row(Some(&photos_row));
        self.imp().selecting_programmatically.set(false);
    }

    /// Insert the folder + virtual albums under the Albums header, fetched from
    /// the current DB snapshot. Called once after `set_resources` (and again by
    /// [`Self::refresh_album_rows`] on live changes). Safe to call before
    /// `connect_sidebar`; it only touches rows + the `targets` mirror.
    pub fn populate_album_rows(&self) {
        self.rebuild_album_rows();
    }

    fn rebuild_album_rows(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let list = self.imp().sidebar_list.get();

        // Drop the existing album rows from the listbox and our bookkeeping.
        let old_rows = std::mem::take(&mut *self.imp().album_rows.borrow_mut());
        for row in &old_rows {
            list.remove(row);
        }

        // Keep `targets` in lock-step: after removal it should read
        // [Photos, AlbumsHeader, Trash]. The albums occupied the
        // indices strictly between the header (1) and Trash (len-1).
        {
            let mut targets = self.imp().targets.borrow_mut();
            if targets.len() >= 3 {
                let end = targets.len() - 1;
                if end > 2 {
                    targets.drain(2..end);
                }
            }
        }

        let albums = list_with_favorites(&pool).unwrap_or_default();
        let expanded = self.imp().albums_expanded.get();

        // Insert album rows starting at index 2 (after Photos + header),
        // pushing Trash back down so the order is preserved.
        for (pos, album) in (2_i32..).zip(albums.iter()) {
            let row = build_album_row(album);
            row.set_visible(expanded);
            self.attach_album_dnd(&row, album.folder_path.to_string_lossy().into_owned());
            list.insert(&row, pos);
            self.imp().album_rows.borrow_mut().push(row);
            self.imp()
                .targets
                .borrow_mut()
                .insert(pos as usize, SidebarTarget::Album(album.clone()));
        }

        self.reselect_active_album_row();
    }

    fn reselect_active_album_row(&self) {
        let active = match self.imp().active_album.borrow().clone() {
            Some(path) => path,
            None => return,
        };
        let list = self.imp().sidebar_list.get();
        let idx = self
            .imp()
            .targets
            .borrow()
            .iter()
            .position(|t| matches!(t, SidebarTarget::Album(a) if a.folder_path == active));
        if let Some(i) = idx {
            if let Some(row) = list.row_at_index(i as i32) {
                self.imp().selecting_programmatically.set(true);
                list.select_row(Some(&row));
                self.imp().selecting_programmatically.set(false);
            }
        }
    }

    /// Collapse/expand the Albums group: swap the disclosure arrow and toggle
    /// every album row's visibility. Hidden rows keep their ListBox index, so
    /// the `targets[index]` dispatch stays valid while collapsed.
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
        for row in self.imp().album_rows.borrow().iter() {
            row.set_visible(expanded);
        }
    }

    /// Rebuild the sidebar album rows from the current DB snapshot so counts
    /// stay live after favorites/trash changes. Preserves the collapse state
    /// and re-selects the album row whose detail page is on top of the stack.
    pub fn refresh_album_rows(&self) {
        self.rebuild_album_rows();
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
            .targets
            .borrow()
            .iter()
            .filter_map(|target| match target {
                SidebarTarget::Album(a) => Some(a.folder_path.to_string_lossy().into_owned()),
                _ => None,
            })
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
    }

    /// Wire the sidebar `ListBox` row-selected signal to navigate by row
    /// identity (`targets[index]`), not a hardcoded index:
    ///   - Photos → pop back to the root Photos page.
    ///   - An album row → push that album's `AlbumDetailPage` directly.
    ///   - Trash → push the `TrashPage`.
    ///
    /// The Albums header is non-selectable, so it never lands here; its collapse
    /// toggle is driven by its own `GestureClick`.
    ///
    /// Requires `set_resources` to have been called first; if the resources are
    /// missing the closures silently no-op.
    pub fn connect_sidebar(&self, nav_view: &adw::NavigationView) {
        let list = self.imp().sidebar_list.get();
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
                        pop_to_photos_root(&nav_view);
                    }
                    SidebarTarget::AlbumsHeader => {}
                    SidebarTarget::Album(album) => {
                        *window.imp().active_album.borrow_mut() = Some(album.folder_path.clone());
                        window.open_album(&nav_view, album);
                    }
                    SidebarTarget::Trash => {
                        *window.imp().active_album.borrow_mut() = None;
                        window.show_trash_page(&nav_view);
                    }
                }
            }),
        );

        let settings_btn = self.imp().settings_button.get();
        settings_btn.connect_clicked(glib::clone!(@weak self as window => move |_| {
            window.show_settings_dialog();
        }));
    }

    fn open_album(&self, nav_view: &adw::NavigationView, album: Album) {
        // Already viewing this album → no-op (avoids rebuilding/pushing a
        // duplicate detail page on a re-select).
        let already_visible = nav_view
            .visible_page()
            .and_then(|page| page.downcast::<AlbumDetailPage>().ok())
            .is_some_and(|detail| {
                detail.album_folder_path().as_deref() == Some(album.folder_path.as_path())
            });
        if already_visible {
            return;
        }

        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().clone() else {
            return;
        };
        let Some(master) = self.imp().media_list.borrow().clone() else {
            return;
        };

        // Albums are top-level destinations: drop any stacked pages back to the
        // Photos root, then push a fresh detail page so the back stack stays
        // shallow and consistent.
        pop_to_photos_root(nav_view);

        let items = filtered_items_for_album(&album, &master, &pool);
        let filtered = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for item in items {
            filtered.append(&glib::BoxedAnyObject::new(item));
        }
        let page = AlbumDetailPage::new(album, filtered, master, pool, loader);
        page.set_nav_target(nav_view);
        nav_view.push(&page);
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
        let dialog = adw::Dialog::builder()
            .title(tr("setting.page.title"))
            .content_width(540)
            .content_height(760)
            .child(&self.build_settings_page(host))
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
                show_settings_restart_dialog(&parent_for_zh, true, None);
                btn_zh_ref.set_sensitive(false);
                btn_en_ref.set_sensitive(true);
            }
            Err(err) => {
                show_settings_restart_dialog(&parent_for_zh, false, Some(err));
            }
        });

        btn_en.connect_clicked(move |_| match persist_locale("en") {
            Ok(()) => {
                show_settings_restart_dialog(&parent_for_en, true, None);
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

        // ── Appearance: Liquid Glass toggle ───────────────────────────────
        // The switch reflects the persisted pref and re-skins the whole app
        // live on toggle (no restart). See grid_css::reapply.
        let appearance_title = gtk::Label::new(Some(&tr("setting.section.appearance")));
        appearance_title.set_xalign(0.0);
        content.append(&appearance_title);

        let switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(prefs::liquid_glass_enabled())
            .build();

        let switch_label = gtk::Label::new(Some(&tr("setting.liquid_glass")));
        switch_label.set_halign(gtk::Align::Start);
        switch_label.set_hexpand(true);

        let glass_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();
        glass_row.append(&switch_label);
        glass_row.append(&switch);
        content.append(&glass_row);

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

        // ── Storage: Clear Cache ────────────────────────────────────────────
        // Show current storage usage with action rows matching the project's
        // Adw.PreferencesGroup + Adw.ActionRow design pattern.
        let storage_group = adw::PreferencesGroup::new();
        storage_group.set_title(&tr("setting.section.storage"));
        storage_group.set_description(Some(&tr("setting.section.storage_description")));
        content.append(&storage_group);

        // Show current storage usage
        let cache_dir = config::cache_dir();
        let thumb_dir = cache_dir.join("thumbnails");
        let thumb_size = crate::core::cache::dir_size(&thumb_dir);
        let db_path = crate::config::data_dir().join("photos.db");
        let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

        // Thumbnail cache row with size and clear button
        let thumb_row = adw::ActionRow::new();
        thumb_row.set_title(&tr("setting.clear_thumbnails"));
        thumb_row.set_subtitle(&format_size(thumb_size));
        thumb_row.set_activatable(false);

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
        db_row.set_title(&tr("setting.clear_database"));
        db_row.set_subtitle(&format_size(db_size));
        db_row.set_activatable(false);

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
fn build_nav_row(label: &str, icon_name: &str) -> gtk::ListBoxRow {
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
    row.set_child(Some(&box_));
    row
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
