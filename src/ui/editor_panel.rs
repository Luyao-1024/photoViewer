//! EditorPanel — 编辑控制面板，嵌入 ViewerPage 右侧滑出
//!
//! 从原 EditorPanel (Gtk.Box) 迁移为 Gtk.Box 子类，
//! 通过回调与 ViewerPage 通信：
//! - `connect_texture_ready`: 渲染完成后回调，宿主据此更新预览图片
//! - `connect_spinner`: 控制宿主的 loading spinner
//! - `connect_close`: 用户取消或保存完成后回调，宿主据此收起面板
//! - `connect_toast`: 显示 toast 消息（成功/错误）
//!
//! 生命周期：
//! 1. 模板初始化 → `constructed` vfunc 连接信号 + i18n + 保存菜单
//! 2. 宿主调用 `configure(item, pool)` → 重置状态、加载原图、首次渲染
//! 3. 用户操作（旋转 / 调色滑块）→ 修改 `EditState` → `schedule_preview_update`
//! 4. 30fps 节流：`glib::timeout_add_local_once(33ms)`，新请求取消旧 timer
//! 5. `render_preview` → `gio::spawn_blocking` 跑 `apply_all`
//! 6. 完成后回到主线程，将 `DynamicImage → Pixbuf → Texture`，回调宿主

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt, PreferencesGroupExt, PreferencesRowExt};

use gdk_pixbuf::{Colorspace, Pixbuf};

use crate::core::db::DbPool;
use crate::core::edit::{apply_all, EditRegistry, EditState};
use crate::core::i18n::{tr, trf};
use crate::core::media::MediaItem;

type TextureCallback = Rc<dyn Fn(gdk::Texture)>;
type SpinnerCallback = Rc<dyn Fn(bool)>;
type CloseCallback = Rc<dyn Fn()>;
type ToastCallback = Rc<dyn Fn(&str, ToastKind)>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/editor-panel.ui")]
    pub struct EditorPanel {
        pub media_item: RefCell<Option<MediaItem>>,
        pub pool: RefCell<Option<DbPool>>,
        pub registry: RefCell<Option<EditRegistry>>,
        pub state: RefCell<EditState>,
        pub source_image: RefCell<Option<image::DynamicImage>>,
        pub render_token: Cell<u64>,
        pub load_token: Cell<u64>,
        #[template_child]
        pub editor_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub editor_close_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub rotate_90_cw: TemplateChild<gtk::Button>,
        #[template_child]
        pub rotate_180: TemplateChild<gtk::Button>,
        #[template_child]
        pub rotate_90_ccw: TemplateChild<gtk::Button>,
        #[template_child]
        pub rotate_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub adjust_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub crop_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub brightness_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub contrast_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub saturation_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub brightness_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub contrast_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub saturation_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub start_crop_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub cancel_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_copy_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_menu_btn: TemplateChild<gtk::MenuButton>,
        pub debounce_id: RefCell<Option<glib::SourceId>>,
        pub on_texture_ready: RefCell<Option<TextureCallback>>,
        pub on_spinner: RefCell<Option<SpinnerCallback>>,
        pub on_close: RefCell<Option<CloseCallback>>,
        pub on_toast: RefCell<Option<ToastCallback>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditorPanel {
        const NAME: &'static str = "EditorPanel";
        type Type = super::EditorPanel;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EditorPanel {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.apply_i18n();
            obj.setup_scales();
            obj.connect_signals();
            obj.setup_save_menu();
        }
    }
    impl WidgetImpl for EditorPanel {}
    impl BoxImpl for EditorPanel {}
}

glib::wrapper! {
    pub struct EditorPanel(ObjectSubclass<imp::EditorPanel>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl EditorPanel {
    fn apply_i18n(&self) {
        let imp = self.imp();
        imp.editor_title.get().set_label(&tr("page.editor.title"));
        imp.editor_close_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.details.close")));
        imp.rotate_group.get().set_title(&tr("editor.panel.rotate"));
        imp.adjust_group.get().set_title(&tr("editor.panel.adjust"));
        imp.crop_group.get().set_title(&tr("editor.panel.crop"));
        imp.brightness_row
            .get()
            .set_title(&tr("editor.adjust.brightness"));
        imp.contrast_row
            .get()
            .set_title(&tr("editor.adjust.contrast"));
        imp.saturation_row
            .get()
            .set_title(&tr("editor.adjust.saturation"));
        imp.rotate_90_cw.get().set_label(&tr("editor.rotate.90"));
        imp.rotate_180.get().set_label(&tr("editor.rotate.180"));
        imp.rotate_90_ccw
            .get()
            .set_label(&tr("editor.rotate.90_ccw"));
        imp.start_crop_btn.get().set_label(&tr("editor.crop.start"));
        imp.cancel_btn.get().set_label(&tr("button.cancel"));
        imp.save_copy_btn
            .get()
            .set_label(&tr("editor.menu.save_copy"));
        imp.save_menu_btn.get().set_label(&tr("button.save"));
    }

    fn setup_scales(&self) {
        let imp = self.imp();
        for scale in [
            imp.brightness_scale.get(),
            imp.contrast_scale.get(),
            imp.saturation_scale.get(),
        ] {
            scale.set_range(-100.0, 100.0);
            scale.set_value(0.0);
        }
    }

    /// Configure the panel for a new editing session: reset state, set
    /// media item / pool / registry, and kick off the async source load.
    pub fn configure(&self, media_item: MediaItem, pool: DbPool) {
        let imp = self.imp();
        *imp.media_item.borrow_mut() = Some(media_item.clone());
        *imp.pool.borrow_mut() = Some(pool);
        *imp.registry.borrow_mut() = Some(EditRegistry::new_with_v1());
        *imp.state.borrow_mut() = EditState::default();
        *imp.source_image.borrow_mut() = None;

        imp.brightness_scale.get().set_value(0.0);
        imp.contrast_scale.get().set_value(0.0);
        imp.saturation_scale.get().set_value(0.0);

        let tok = imp.load_token.get() + 1;
        imp.load_token.set(tok);

        self.load_source_async(media_item.path.clone());
    }

    pub fn connect_texture_ready<F: Fn(gdk::Texture) + 'static>(&self, f: F) {
        *self.imp().on_texture_ready.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_spinner<F: Fn(bool) + 'static>(&self, f: F) {
        *self.imp().on_spinner.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_close<F: Fn() + 'static>(&self, f: F) {
        *self.imp().on_close.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_toast<F: Fn(&str, ToastKind) + 'static>(&self, f: F) {
        *self.imp().on_toast.borrow_mut() = Some(Rc::new(f));
    }

    fn fire_texture(&self, texture: gdk::Texture) {
        if let Some(cb) = self.imp().on_texture_ready.borrow().clone() {
            cb(texture);
        }
    }

    fn fire_spinner(&self, visible: bool) {
        if let Some(cb) = self.imp().on_spinner.borrow().clone() {
            cb(visible);
        }
    }

    fn fire_close(&self) {
        if let Some(cb) = self.imp().on_close.borrow().clone() {
            cb();
        }
    }

    fn fire_toast(&self, msg: &str, kind: ToastKind) {
        if let Some(cb) = self.imp().on_toast.borrow().clone() {
            cb(msg, kind);
        }
    }

    fn load_source_async(&self, path: std::path::PathBuf) {
        let weak = self.downgrade();
        let token = self.imp().load_token.get();
        glib::spawn_future_local(async move {
            let loaded: std::thread::Result<Option<image::DynamicImage>> =
                gio::spawn_blocking(move || image::open(&path).ok()).await;

            if let Ok(Some(img)) = loaded {
                let downsampled = if img.width() * img.height() > 8_000_000 {
                    let scale = (8_000_000.0_f64 / (img.width() * img.height()) as f64).sqrt();
                    img.resize(
                        (img.width() as f64 * scale) as u32,
                        (img.height() as f64 * scale) as u32,
                        image::imageops::FilterType::Triangle,
                    )
                } else {
                    img
                };

                if let Some(this) = weak.upgrade() {
                    if this.imp().load_token.get() != token {
                        return;
                    }
                    *this.imp().source_image.borrow_mut() = Some(downsampled);
                    this.schedule_preview_update();
                }
            } else {
                tracing::warn!("EditorPanel: failed to load source image");
            }
        });
    }

    fn connect_signals(&self) {
        let imp = self.imp();

        imp.editor_close_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.fire_close();
            }));

        imp.cancel_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.fire_close();
            }));

        imp.rotate_90_cw
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.apply_rotation_delta(90);
            }));
        imp.rotate_180
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.apply_rotation_delta(180);
            }));
        imp.rotate_90_ccw
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.apply_rotation_delta(-90);
            }));

        imp.brightness_scale.get().connect_value_changed(
            glib::clone!(@weak self as this => move |s| {
                this.imp().state.borrow_mut().brightness = s.value() as i32;
                this.schedule_preview_update();
            }),
        );
        imp.contrast_scale.get().connect_value_changed(
            glib::clone!(@weak self as this => move |s| {
                this.imp().state.borrow_mut().contrast = s.value() as i32;
                this.schedule_preview_update();
            }),
        );
        imp.saturation_scale.get().connect_value_changed(
            glib::clone!(@weak self as this => move |s| {
                this.imp().state.borrow_mut().saturation = s.value() as i32;
                this.schedule_preview_update();
            }),
        );

        imp.save_copy_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.save_as_copy();
            }));

        imp.start_crop_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.fire_toast(&tr("editor.crop_placeholder"), ToastKind::Success);
            }));
    }

    fn setup_save_menu(&self) {
        let menu = gio::Menu::new();
        menu.append(Some(&tr("editor.menu.save_copy")), Some("editor.save-copy"));
        menu.append(
            Some(&tr("editor.menu.save_overwrite")),
            Some("editor.save-overwrite"),
        );

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        popover.add_css_class("glass-menu");
        self.imp().save_menu_btn.get().set_popover(Some(&popover));

        let group = gio::SimpleActionGroup::new();
        let save_copy_action = gio::SimpleAction::new("save-copy", None);
        save_copy_action.connect_activate(glib::clone!(@weak self as this => move |_, _| {
            this.save_as_copy();
        }));
        group.add_action(&save_copy_action);

        let save_overwrite_action = gio::SimpleAction::new("save-overwrite", None);
        save_overwrite_action.connect_activate(glib::clone!(@weak self as this => move |_, _| {
            this.save_overwrite_with_confirm();
        }));
        group.add_action(&save_overwrite_action);

        self.insert_action_group("editor", Some(&group));
    }

    fn save_as_copy(&self) {
        let imp = self.imp();
        let item = match imp.media_item.borrow().clone() {
            Some(i) => i,
            None => {
                tracing::warn!("EditorPanel.save_as_copy: no media_item");
                return;
            }
        };
        let state = imp.state.borrow().clone();
        let pool = match imp.pool.borrow().clone() {
            Some(p) => p,
            None => return,
        };
        let registry = match imp.registry.borrow().clone() {
            Some(r) => r,
            None => return,
        };

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result: std::thread::Result<
                std::result::Result<crate::core::media::MediaItem, crate::core::error::AppError>,
            > = gio::spawn_blocking(move || {
                crate::core::edit::save_as_copy(&item, &state, &pool, &registry)
            })
            .await;

            if let Some(this) = weak.upgrade() {
                match result {
                    Ok(Ok(_)) => {
                        this.fire_toast(&tr("editor.toast.saved_copy"), ToastKind::Success);
                        this.fire_close();
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Save Copy failed: {}", e);
                        this.fire_toast(
                            &trf(
                                "editor.toast.save_copy_failed",
                                &[("error", &e.to_string())],
                            ),
                            ToastKind::Error,
                        );
                    }
                    Err(_) => {
                        tracing::error!("Save Copy worker panicked");
                    }
                }
            }
        });
    }

    fn save_overwrite_with_confirm(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("editor.overwrite_title"))
            .body(tr("editor.overwrite_body"))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("button.cancel"));
        dialog.add_response("overwrite", &tr("dialog.overwrite"));
        dialog.set_response_appearance("overwrite", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(None, move |_, response| {
            if response == "overwrite" {
                if let Some(this) = weak.upgrade() {
                    this.perform_save_overwrite();
                }
            }
        });
        dialog.present(self);
    }

    fn perform_save_overwrite(&self) {
        let imp = self.imp();
        let item = match imp.media_item.borrow().clone() {
            Some(i) => i,
            None => return,
        };
        let state = imp.state.borrow().clone();
        let pool = match imp.pool.borrow().clone() {
            Some(p) => p,
            None => return,
        };
        let registry = match imp.registry.borrow().clone() {
            Some(r) => r,
            None => return,
        };

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result: std::thread::Result<std::result::Result<(), crate::core::error::AppError>> =
                gio::spawn_blocking(move || {
                    crate::core::edit::save_overwrite(&item, &state, &pool, &registry)
                })
                .await;

            if let Some(this) = weak.upgrade() {
                match result {
                    Ok(Ok(())) => {
                        this.fire_toast(&tr("editor.toast.overwritten"), ToastKind::Success);
                        this.fire_close();
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Save Overwrite failed: {}", e);
                        this.fire_toast(
                            &trf(
                                "editor.toast.overwrite_failed",
                                &[("error", &e.to_string())],
                            ),
                            ToastKind::Error,
                        );
                    }
                    Err(_) => {
                        tracing::error!("Save Overwrite worker panicked");
                    }
                }
            }
        });
    }

    fn apply_rotation_delta(&self, delta: i32) {
        let imp = self.imp();
        let item = match imp.media_item.borrow().clone() {
            Some(i) => i,
            None => return,
        };

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result: std::thread::Result<std::result::Result<(), crate::core::error::AppError>> =
                gio::spawn_blocking(move || crate::core::edit::rotate_in_place(&item.path, delta))
                    .await;

            if let Some(this) = weak.upgrade() {
                match result {
                    Ok(Ok(())) => {
                        this.show_undo_toast(delta);
                    }
                    Ok(Err(e)) => tracing::error!("旋转失败: {}", e),
                    Err(_) => tracing::error!("旋转 worker 异常终止"),
                }
            }
        });
    }

    fn show_undo_toast(&self, delta: i32) {
        let item = match self.imp().media_item.borrow().clone() {
            Some(i) => i,
            None => return,
        };
        let weak = self.downgrade();
        let path = item.path.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || {
            if let Some(_this) = weak.upgrade() {
                if let Err(e) = crate::core::edit::rotate_in_place(&path, -delta) {
                    tracing::error!("撤销旋转失败: {}", e);
                }
            }
        });
        tracing::info!("已旋转 {}°，5 秒后撤销", delta);
    }

    fn schedule_preview_update(&self) {
        let imp = self.imp();
        if let Some(id) = imp.debounce_id.borrow_mut().take() {
            id.remove();
        }
        let weak = self.downgrade();
        imp.debounce_id
            .borrow_mut()
            .replace(glib::timeout_add_local_once(
                Duration::from_millis(33),
                move || {
                    if let Some(this) = weak.upgrade() {
                        this.render_preview();
                    }
                },
            ));
    }

    fn render_preview(&self) {
        let imp = self.imp();
        let source = match imp.source_image.borrow().clone() {
            Some(s) => s,
            None => return,
        };
        let state = imp.state.borrow().clone();
        let registry = match imp.registry.borrow().as_ref().cloned() {
            Some(r) => r,
            None => return,
        };

        let token = {
            let t = imp.render_token.get().saturating_add(1);
            imp.render_token.set(t);
            t
        };

        self.fire_spinner(true);

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let rendered: std::thread::Result<std::result::Result<image::DynamicImage, String>> =
                gio::spawn_blocking(move || apply_all(&registry, source, &state)).await;

            if let Some(this) = weak.upgrade() {
                if this.imp().render_token.get() != token {
                    return;
                }
                match rendered {
                    Ok(Ok(img)) => {
                        let rgb = img.to_rgb8();
                        let (width, height) = (rgb.width() as i32, rgb.height() as i32);
                        let rowstride = width * 3;
                        let bytes = glib::Bytes::from_owned(rgb.into_raw());
                        let pixbuf = Pixbuf::from_bytes(
                            &bytes,
                            Colorspace::Rgb,
                            false,
                            8,
                            width,
                            height,
                            rowstride,
                        );
                        let texture = gdk::Texture::for_pixbuf(&pixbuf);
                        this.fire_texture(texture);
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("EditorPanel: render failed: {}", e);
                    }
                    Err(_) => {
                        tracing::warn!("EditorPanel: spawn_blocking panicked");
                    }
                }
                this.fire_spinner(false);
            }
        });
    }
}
