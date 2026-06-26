//! Main window: sidebar + content area
use std::cell::RefCell;
use std::fs;
use std::sync::Arc;

use glib::subclass::types::ObjectSubclassIsExt;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::ListBoxRow;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt, NavigationPageExt};
use serde_json::{Map, Value};

use crate::config;
use crate::core::db::DbPool;
use crate::core::i18n::{locale, tr, trf};
use crate::core::prefs;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::grid_css;
use crate::ui::{AlbumsPage, TrashPage};

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
        #[template_child]
        pub sidebar_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub nav_view: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub sidebar_page: TemplateChild<adw::NavigationPage>,
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

    /// Populate the sidebar ListBox with section rows.
    /// Photos / Albums / Trash — only Photos is wired up in M1; others are placeholders.
    pub fn populate_sidebar(&self) {
        self.imp()
            .sidebar_page
            .get()
            .set_title(&tr("window.sidebar"));
        let list = self.imp().sidebar_list.get();
        let sidebar_rows = [
            (tr("sidebar.photos"), "photos"),
            (tr("sidebar.albums"), "albums"),
            (tr("sidebar.trash"), "trash"),
            (tr("sidebar.settings"), "settings"),
        ];
        for (label, _target) in &sidebar_rows {
            let row = ListBoxRow::new();
            row.add_css_class("glass-sidebar-row");
            let lbl = gtk::Label::builder()
                .label(label.clone())
                .halign(gtk::Align::Start)
                .css_classes(["glass-sidebar-label"])
                .build();
            row.set_child(Some(&lbl));
            list.append(&row);
        }
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

    /// Wire the sidebar `ListBox` `row-selected` signal to push the
    /// corresponding page onto `nav_view`. Sidebar rows are:
    ///   0 → Photos   (pop to root page)
    ///   1 → Albums   (pushes `AlbumsPage`)
    ///   2 → Trash    (pushes `TrashPage`)
    ///
    /// Requires `set_resources` to have been called first; if the resources
    /// are missing the closure silently no-ops.
    pub fn connect_sidebar(&self, nav_view: &adw::NavigationView) {
        let list = self.imp().sidebar_list.get();
        let gesture = gtk::GestureSwipe::new();
        gesture.connect_swipe(
            glib::clone!(@weak self as window, @weak nav_view => move |_gesture, velocity_x, _velocity_y| {
                if velocity_x.abs() > 450.0 && !visible_page_is_settings(&nav_view) {
                    window.show_settings_page(&nav_view);
                }
            }),
        );
        self.imp().nav_view.get().add_controller(gesture);

        list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else { return };
                match row.index() {
                    0 => {
                        pop_to_photos_root(&nav_view);
                    }
                    1 => {
                        // Albums: if Trash is stacked on Albums, just pop Trash.
                        // Otherwise reuse an existing Albums page in the stack, or
                        // create a fresh one from the current DB snapshot.
                        if visible_page_is_albums(&nav_view) {
                            return;
                        }
                        if pop_to_visible_page(&nav_view, is_albums_page) {
                            return;
                        }
                        let Some(page) = window.build_albums_page(&nav_view) else {
                            return;
                        };
                        pop_to_photos_root(&nav_view);
                        nav_view.push(&page);
                    }
                    2 => {
                        if visible_page_is_trash(&nav_view) {
                            return;
                        }
                        // If we are somewhere inside Albums, return to the
                        // top-level Albums page before stacking Trash on it.
                        let _ = pop_to_visible_page(&nav_view, is_albums_page);
                        let Some(page) = window.build_trash_page() else {
                            return;
                        };
                        nav_view.push(&page);
                    }
                    3 => {
                        window.show_settings_page(&nav_view);
                    }
                    _ => {}
                }
            }),
        );
    }

    fn build_albums_page(&self, nav_view: &adw::NavigationView) -> Option<AlbumsPage> {
        let pool = self.imp().pool.borrow().clone()?;
        let loader = self.imp().loader.borrow().clone()?;
        let albums = crate::core::albums::list_with_favorites(&pool).unwrap_or_default();
        let media_list = self.imp().media_list.borrow().clone()?;
        let page = AlbumsPage::new(albums, loader);
        page.set_nav_target(nav_view, media_list, pool);
        Some(page)
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

    fn show_settings_page(&self, nav_view: &adw::NavigationView) {
        if visible_page_is_settings(nav_view) {
            return;
        }
        let page = self.build_settings_page();
        nav_view.push(&page);
    }

    fn build_settings_page(&self) -> adw::NavigationPage {
        let page = adw::NavigationPage::builder()
            .title(tr("setting.page.title"))
            .build();

        let current = locale().to_string();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();

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

        let page_zh = page.clone();
        let page_en = page.clone();
        let btn_zh_ref = btn_zh.clone();
        let btn_en_ref = btn_en.clone();
        let btn_zh_ref2 = btn_zh.clone();
        let btn_en_ref2 = btn_en.clone();

        btn_zh.connect_clicked(move |_| match persist_locale("zh-CN") {
            Ok(()) => {
                show_settings_restart_dialog(&page_zh, true, None);
                btn_zh_ref.set_sensitive(false);
                btn_en_ref.set_sensitive(true);
            }
            Err(err) => {
                show_settings_restart_dialog(&page_zh, false, Some(err));
            }
        });

        btn_en.connect_clicked(move |_| match persist_locale("en") {
            Ok(()) => {
                show_settings_restart_dialog(&page_en, true, None);
                btn_zh_ref2.set_sensitive(true);
                btn_en_ref2.set_sensitive(false);
            }
            Err(err) => {
                show_settings_restart_dialog(&page_en, false, Some(err));
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

        let page_for_glass = page.clone();
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
                        &page_for_glass,
                        &trf("setting.liquid_glass_save_failed", &[("error", &err)]),
                    );
                }
            }
        });

        // ── Storage: Clear Cache ────────────────────────────────────────────
        // Buttons to clear thumbnail cache and reset the database.
        let storage_title = gtk::Label::new(Some(&tr("setting.section.storage")));
        storage_title.set_xalign(0.0);
        content.append(&storage_title);

        let storage_desc = gtk::Label::new(Some(&tr("setting.section.storage_description")));
        storage_desc.set_wrap(true);
        storage_desc.set_xalign(0.0);
        content.append(&storage_desc);

        // Clear thumbnails button
        let btn_clear_thumbs = gtk::Button::with_label(&tr("setting.clear_thumbnails"));
        btn_clear_thumbs.add_css_class("destructive-action");
        content.append(&btn_clear_thumbs);

        let page_for_thumbs = page.clone();
        let loader_for_thumbs = self.imp().loader.borrow().clone();
        btn_clear_thumbs.connect_clicked(move |_| {
            let loader_clone = loader_for_thumbs.clone();
            show_clear_confirm_dialog(
                &page_for_thumbs,
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
                            show_clear_success_toast(&trf("setting.clear_thumbnails_success", &[("count", &count.to_string())]));
                        }
                        Err(err) => {
                            show_clear_error_toast(&trf("setting.clear_failed", &[("error", &err.to_string())]));
                        }
                    }
                },
            );
        });

        // Reset database button
        let btn_clear_db = gtk::Button::with_label(&tr("setting.clear_database"));
        btn_clear_db.add_css_class("destructive-action");
        content.append(&btn_clear_db);

        let page_for_db = page.clone();
        let pool_for_db = self.imp().pool.borrow().clone();
        let loader_for_db = self.imp().loader.borrow().clone();
        let media_list_for_db = self.imp().media_list.borrow().clone();
        btn_clear_db.connect_clicked(move |_| {
            let pool_clone = pool_for_db.clone();
            let loader_clone = loader_for_db.clone();
            let media_list_clone = media_list_for_db.clone();
            show_clear_confirm_dialog(
                &page_for_db,
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
                                show_clear_success_toast(&trf("setting.clear_database_success", &[("count", &count.to_string())]));
                            }
                            Err(err) => {
                                show_clear_error_toast(&trf("setting.clear_failed", &[("error", &err.to_string())]));
                            }
                        }
                    }
                },
            );
        });

        page.set_child(Some(&content));
        page
    }
}

fn show_settings_restart_dialog(
    parent: &adw::NavigationPage,
    success: bool,
    error: Option<String>,
) {
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
fn show_settings_error_dialog(parent: &adw::NavigationPage, body: &str) {
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

fn visible_page_is_albums(nav_view: &adw::NavigationView) -> bool {
    nav_view
        .visible_page()
        .map(|page| is_albums_page(&page))
        .unwrap_or(false)
}

fn visible_page_is_trash(nav_view: &adw::NavigationView) -> bool {
    nav_view
        .visible_page()
        .map(|page| is_trash_page(&page))
        .unwrap_or(false)
}

fn visible_page_is_settings(nav_view: &adw::NavigationView) -> bool {
    nav_view
        .visible_page()
        .map(|page| is_settings_page(&page))
        .unwrap_or(false)
}

fn is_albums_page(page: &adw::NavigationPage) -> bool {
    page.clone().downcast::<AlbumsPage>().is_ok()
}

fn is_trash_page(page: &adw::NavigationPage) -> bool {
    page.clone().downcast::<TrashPage>().is_ok()
}

fn is_settings_page(page: &adw::NavigationPage) -> bool {
    page.title() == tr("setting.page.title")
}

/// Show a confirmation dialog for clearing cache/database.
fn show_clear_confirm_dialog<F: Fn() + 'static>(
    parent: &adw::NavigationPage,
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

fn pop_to_visible_page(
    nav_view: &adw::NavigationView,
    is_target: fn(&adw::NavigationPage) -> bool,
) -> bool {
    loop {
        let Some(visible) = nav_view.visible_page() else {
            return false;
        };
        if is_target(&visible) {
            return true;
        }
        let Some(previous) = nav_view.previous_page(&visible) else {
            return false;
        };
        if is_target(&previous) {
            return nav_view.pop();
        }
        if !nav_view.pop() {
            return false;
        }
    }
}
