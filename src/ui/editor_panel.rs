//! EditorPanel — 编辑控制面板，嵌入 ViewerPage 右侧滑出
//!
//! 从原 EditorPanel (Gtk.Box) 迁移为 Gtk.Box 子类，
//! 通过回调与 ViewerPage 通信：
//! - `connect_texture_ready`: 渲染完成后回调，宿主据此更新预览图片
//! - `connect_spinner`: 控制宿主的 loading spinner
//! - `connect_close`: 用户取消或保存成功时回调，宿主据此收起面板
//! - `connect_save_result`: 保存完成后回调，宿主据此弹出结果对话框
//! - `connect_toast`: 显示 toast 消息（成功/错误）
//!
//! 生命周期：
//! 1. 模板初始化 → `constructed` vfunc 连接信号 + i18n
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
use crate::core::edit::{
    apply_all, centered_crop_rect_for_aspect, CropRect, EditRegistry, EditState,
};
use crate::core::i18n::{tr, trf};
use crate::core::media::MediaItem;

type TextureCallback = Rc<dyn Fn(gdk::Texture)>;
type SpinnerCallback = Rc<dyn Fn(bool)>;
type CloseCallback = Rc<dyn Fn()>;
type ToastCallback = Rc<dyn Fn(&str, ToastKind)>;
type SaveResultCallback = Rc<dyn Fn(SaveResultKind, String, String)>;
type CropOverlayCallback = Rc<dyn Fn(CropOverlayUpdate)>;

const CROP_RATIOS: [CropRatioChoice; 6] = [
    CropRatioChoice {
        id: "source",
        label_key: "editor.crop.ratio.source",
        aspect: None,
    },
    CropRatioChoice {
        id: "1:1",
        label_key: "editor.crop.ratio.square",
        aspect: Some((1, 1)),
    },
    CropRatioChoice {
        id: "4:3",
        label_key: "editor.crop.ratio.4_3",
        aspect: Some((4, 3)),
    },
    CropRatioChoice {
        id: "3:2",
        label_key: "editor.crop.ratio.3_2",
        aspect: Some((3, 2)),
    },
    CropRatioChoice {
        id: "16:9",
        label_key: "editor.crop.ratio.16_9",
        aspect: Some((16, 9)),
    },
    CropRatioChoice {
        id: "free",
        label_key: "editor.crop.ratio.free",
        aspect: None,
    },
];

#[derive(Clone, Copy)]
struct CropRatioChoice {
    id: &'static str,
    label_key: &'static str,
    aspect: Option<(u32, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveResultKind {
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropOverlayUpdate {
    pub active: bool,
    pub rect: Option<(u32, u32, u32, u32)>,
    pub image_dimensions: (u32, u32),
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
        pub source_dimensions: Cell<(u32, u32)>,
        pub preview_scale: Cell<f64>,
        pub crop_mode_active: Cell<bool>,
        pub crop_ratio_index: Cell<usize>,
        pub render_token: Cell<u64>,
        pub load_token: Cell<u64>,
        #[template_child]
        pub editor_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub editor_close_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub reset_btn: TemplateChild<gtk::Button>,
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
        pub crop_ratio_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub crop_ratio_prev_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub crop_ratio_next_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub crop_ratio_preview: TemplateChild<gtk::DrawingArea>,
        #[template_child]
        pub crop_ratio_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub start_crop_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub cancel_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_copy_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_overwrite_btn: TemplateChild<gtk::Button>,
        pub debounce_id: RefCell<Option<glib::SourceId>>,
        pub on_texture_ready: RefCell<Option<TextureCallback>>,
        pub on_spinner: RefCell<Option<SpinnerCallback>>,
        pub on_close: RefCell<Option<CloseCallback>>,
        pub on_save_result: RefCell<Option<SaveResultCallback>>,
        pub on_toast: RefCell<Option<ToastCallback>>,
        pub on_crop_overlay: RefCell<Option<CropOverlayCallback>>,
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
        imp.reset_btn
            .get()
            .set_tooltip_text(Some(&tr("editor.reset.tooltip")));
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
        imp.save_overwrite_btn
            .get()
            .set_label(&tr("editor.save_overwrite"));
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

        self.update_crop_ratio_preview();
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
        imp.source_dimensions.set((0, 0));
        imp.preview_scale.set(1.0);
        imp.crop_mode_active.set(false);
        imp.crop_ratio_index.set(0);

        imp.brightness_scale.get().set_value(0.0);
        imp.contrast_scale.get().set_value(0.0);
        imp.saturation_scale.get().set_value(0.0);
        self.update_crop_ratio_preview();
        self.update_crop_controls();
        self.update_reset_button();
        self.fire_crop_overlay_update();

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

    pub fn connect_save_result<F: Fn(SaveResultKind, String, String) + 'static>(&self, f: F) {
        *self.imp().on_save_result.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_toast<F: Fn(&str, ToastKind) + 'static>(&self, f: F) {
        *self.imp().on_toast.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_crop_overlay<F: Fn(CropOverlayUpdate) + 'static>(&self, f: F) {
        *self.imp().on_crop_overlay.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_crop_rect_from_overlay(&self, rect: (u32, u32, u32, u32)) {
        let (width, height) = self.current_crop_space_dimensions();
        if width == 0 || height == 0 {
            return;
        }
        let ratio_id = self.active_crop_ratio_id();
        let rect = normalized_crop_rect(
            rect.0,
            rect.1,
            rect.2,
            rect.3,
            width,
            height,
            Some(ratio_id),
        );
        self.imp().state.borrow_mut().crop = Some((rect.x, rect.y, rect.width, rect.height));
        self.update_reset_button();
        self.update_crop_controls();
        self.fire_crop_overlay_update();
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

    fn fire_save_result(&self, kind: SaveResultKind, heading: String, body: String) {
        if let Some(cb) = self.imp().on_save_result.borrow().clone() {
            cb(kind, heading, body);
        }
    }

    fn fire_toast(&self, msg: &str, kind: ToastKind) {
        if let Some(cb) = self.imp().on_toast.borrow().clone() {
            cb(msg, kind);
        }
    }

    fn fire_crop_overlay_update(&self) {
        if let Some(cb) = self.imp().on_crop_overlay.borrow().clone() {
            cb(CropOverlayUpdate {
                active: self.imp().crop_mode_active.get(),
                rect: self.imp().state.borrow().crop,
                image_dimensions: self.current_crop_space_dimensions(),
            });
        }
    }

    fn load_source_async(&self, path: std::path::PathBuf) {
        let weak = self.downgrade();
        let token = self.imp().load_token.get();
        glib::spawn_future_local(async move {
            let loaded: std::thread::Result<Option<image::DynamicImage>> =
                gio::spawn_blocking(move || {
                    crate::core::orientation::load_oriented_pixbuf(&path)
                        .ok()
                        .and_then(dynamic_image_from_pixbuf)
                })
                .await;

            if let Ok(Some(img)) = loaded {
                let original_dimensions = (img.width(), img.height());
                let (downsampled, preview_scale) = if img.width() * img.height() > 8_000_000 {
                    let scale = (8_000_000.0_f64 / (img.width() * img.height()) as f64).sqrt();
                    (
                        img.resize(
                            (img.width() as f64 * scale) as u32,
                            (img.height() as f64 * scale) as u32,
                            image::imageops::FilterType::Triangle,
                        ),
                        scale,
                    )
                } else {
                    (img, 1.0)
                };

                if let Some(this) = weak.upgrade() {
                    if this.imp().load_token.get() != token {
                        return;
                    }
                    *this.imp().source_image.borrow_mut() = Some(downsampled);
                    this.imp().source_dimensions.set(original_dimensions);
                    this.imp().preview_scale.set(preview_scale);
                    this.fire_crop_overlay_update();
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

        imp.reset_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.reset_edits();
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
                this.update_reset_button();
                this.schedule_preview_update();
            }),
        );
        imp.contrast_scale.get().connect_value_changed(
            glib::clone!(@weak self as this => move |s| {
                this.imp().state.borrow_mut().contrast = s.value() as i32;
                this.update_reset_button();
                this.schedule_preview_update();
            }),
        );
        imp.saturation_scale.get().connect_value_changed(
            glib::clone!(@weak self as this => move |s| {
                this.imp().state.borrow_mut().saturation = s.value() as i32;
                this.update_reset_button();
                this.schedule_preview_update();
            }),
        );

        imp.crop_ratio_preview.get().set_draw_func(
            glib::clone!(@weak self as this => move |_, cr, width, height| {
                this.draw_crop_ratio_preview(cr, width, height);
            }),
        );
        imp.crop_ratio_prev_btn.get().connect_clicked(
            glib::clone!(@weak self as this => move |_| {
                this.step_crop_ratio(-1);
            }),
        );
        imp.crop_ratio_next_btn.get().connect_clicked(
            glib::clone!(@weak self as this => move |_| {
                this.step_crop_ratio(1);
            }),
        );

        imp.save_copy_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.save_as_copy();
            }));

        imp.save_overwrite_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.save_overwrite_with_confirm();
            }));

        imp.start_crop_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.toggle_crop_mode();
            }));
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
                        this.fire_save_result(
                            SaveResultKind::Success,
                            tr("editor.save_result.copy_success_title"),
                            tr("editor.save_result.copy_success_body"),
                        );
                        this.fire_close();
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Save Copy failed: {}", e);
                        this.fire_save_result(
                            SaveResultKind::Error,
                            tr("editor.save_result.copy_failed_title"),
                            trf(
                                "editor.save_result.copy_failed_body",
                                &[("error", &e.to_string())],
                            ),
                        );
                    }
                    Err(_) => {
                        tracing::error!("Save Copy worker panicked");
                        this.fire_save_result(
                            SaveResultKind::Error,
                            tr("editor.save_result.copy_failed_title"),
                            tr("editor.save_result.worker_failed_body"),
                        );
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
                        this.fire_save_result(
                            SaveResultKind::Success,
                            tr("editor.save_result.overwrite_success_title"),
                            tr("editor.save_result.overwrite_success_body"),
                        );
                        this.fire_close();
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Save Overwrite failed: {}", e);
                        this.fire_save_result(
                            SaveResultKind::Error,
                            tr("editor.save_result.overwrite_failed_title"),
                            trf(
                                "editor.save_result.overwrite_failed_body",
                                &[("error", &e.to_string())],
                            ),
                        );
                    }
                    Err(_) => {
                        tracing::error!("Save Overwrite worker panicked");
                        this.fire_save_result(
                            SaveResultKind::Error,
                            tr("editor.save_result.overwrite_failed_title"),
                            tr("editor.save_result.worker_failed_body"),
                        );
                    }
                }
            }
        });
    }

    fn apply_rotation_delta(&self, delta: i32) {
        let mut state = self.imp().state.borrow_mut();
        state.rotation = state.rotation.rotated_by(delta);
        drop(state);
        if self.imp().crop_mode_active.get() {
            self.ensure_crop_rect();
            self.fire_crop_overlay_update();
        }
        self.update_reset_button();
        tracing::info!("ROTATE_TRACE editor_memory_rotate delta={}", delta);
        self.schedule_preview_update();
    }

    fn reset_edits(&self) {
        self.imp().state.borrow_mut().reset();
        self.imp().brightness_scale.get().set_value(0.0);
        self.imp().contrast_scale.get().set_value(0.0);
        self.imp().saturation_scale.get().set_value(0.0);
        self.imp().crop_mode_active.set(false);
        self.update_crop_controls();
        self.update_reset_button();
        self.fire_crop_overlay_update();
        self.schedule_preview_update();
    }

    fn update_reset_button(&self) {
        let has_pending_edits = self.imp().state.borrow().has_pending_edits();
        self.imp().reset_btn.get().set_sensitive(has_pending_edits);
    }

    fn toggle_crop_mode(&self) {
        let active = !self.imp().crop_mode_active.get();
        if active && !self.ensure_crop_rect() {
            self.fire_toast(&tr("editor.crop.no_image"), ToastKind::Error);
            return;
        }
        self.imp().crop_mode_active.set(active);
        self.update_crop_controls();
        self.update_reset_button();
        self.fire_crop_overlay_update();
        self.schedule_preview_update();
    }

    fn update_crop_controls(&self) {
        let active = self.imp().crop_mode_active.get();
        self.imp().crop_ratio_box.get().set_visible(active);
        let label = if active {
            tr("editor.crop.done")
        } else {
            tr("editor.crop.start")
        };
        self.imp().start_crop_btn.get().set_label(&label);
    }

    fn active_crop_ratio(&self) -> CropRatioChoice {
        CROP_RATIOS[self.imp().crop_ratio_index.get().min(CROP_RATIOS.len() - 1)]
    }

    fn active_crop_ratio_id(&self) -> &'static str {
        self.active_crop_ratio().id
    }

    fn step_crop_ratio(&self, delta: i32) {
        let current = self.imp().crop_ratio_index.get() as i32;
        let next = (current + delta).rem_euclid(CROP_RATIOS.len() as i32) as usize;
        self.imp().crop_ratio_index.set(next);
        self.update_crop_ratio_preview();
        self.apply_crop_ratio(Some(self.active_crop_ratio_id()));
    }

    fn update_crop_ratio_preview(&self) {
        let ratio = self.active_crop_ratio();
        self.imp()
            .crop_ratio_label
            .get()
            .set_label(&tr(ratio.label_key));
        self.imp().crop_ratio_preview.get().queue_draw();
    }

    fn draw_crop_ratio_preview(&self, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let ratio = self.active_crop_ratio();
        let area_width = width.max(1) as f64;
        let area_height = height.max(1) as f64;
        let max_width = (area_width - 12.0).max(1.0);
        let max_height = (area_height - 8.0).max(1.0);
        let aspect = if ratio.id == "source" {
            let (width, height) = self.current_crop_space_dimensions();
            (width > 0 && height > 0).then_some((width, height))
        } else {
            ratio.aspect
        };
        let (rect_width, rect_height) = match aspect {
            Some((aspect_width, aspect_height)) => {
                let target = aspect_width as f64 / aspect_height as f64;
                if max_width / max_height > target {
                    (max_height * target, max_height)
                } else {
                    (max_width, max_width / target)
                }
            }
            None => (max_width * 0.82, max_height * 0.7),
        };
        let x = (area_width - rect_width) / 2.0;
        let y = (area_height - rect_height) / 2.0;

        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.rectangle(x, y, rect_width, rect_height);
        let _ = cr.fill();

        cr.set_source_rgba(1.0, 1.0, 1.0, 0.84);
        cr.set_line_width(2.0);
        cr.rectangle(x, y, rect_width, rect_height);
        let _ = cr.stroke();

        if ratio.id == "free" {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.52);
            cr.set_line_width(1.0);
            cr.move_to(x + rect_width * 0.25, y + rect_height * 0.22);
            cr.line_to(x + rect_width * 0.75, y + rect_height * 0.78);
            let _ = cr.stroke();
        }
    }

    fn ensure_crop_rect(&self) -> bool {
        let (width, height) = self.current_crop_space_dimensions();
        if width == 0 || height == 0 {
            return false;
        }
        if self.imp().state.borrow().crop.is_none() {
            let rect = crop_rect_for_ratio_id(Some(self.active_crop_ratio_id()), width, height)
                .unwrap_or_else(|| default_free_crop_rect(width, height));
            self.imp().state.borrow_mut().crop = Some((rect.x, rect.y, rect.width, rect.height));
        }
        true
    }

    fn apply_crop_ratio(&self, ratio_id: Option<&str>) {
        if !self.imp().crop_mode_active.get() {
            return;
        }
        let (width, height) = self.current_crop_space_dimensions();
        let Some(rect) = crop_rect_for_ratio_id(ratio_id, width, height) else {
            self.fire_crop_overlay_update();
            return;
        };
        self.imp().state.borrow_mut().crop = Some((rect.x, rect.y, rect.width, rect.height));
        self.update_reset_button();
        self.update_crop_controls();
        self.fire_crop_overlay_update();
        self.schedule_preview_update();
    }

    fn current_crop_space_dimensions(&self) -> (u32, u32) {
        let (width, height) = self.imp().source_dimensions.get();
        if matches!(
            self.imp().state.borrow().rotation,
            crate::core::edit::Rotation::R90 | crate::core::edit::Rotation::R270
        ) {
            (height, width)
        } else {
            (width, height)
        }
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
                        this.imp().debounce_id.borrow_mut().take();
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
        let state = imp.state.borrow().for_preview(imp.crop_mode_active.get());
        let state = scaled_preview_state(&state, imp.preview_scale.get());
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

fn dynamic_image_from_pixbuf(pb: Pixbuf) -> Option<image::DynamicImage> {
    let width = pb.width() as usize;
    let height = pb.height() as usize;
    let n_channels = pb.n_channels() as usize;
    let rowstride = pb.rowstride() as usize;
    let bytes = pb.read_pixel_bytes();
    let src: &[u8] = bytes.as_ref();

    match n_channels {
        3 => {
            let mut out = Vec::with_capacity(width * height * 3);
            for y in 0..height {
                let start = y * rowstride;
                let end = start + width * 3;
                out.extend_from_slice(src.get(start..end)?);
            }
            image::RgbImage::from_raw(width as u32, height as u32, out)
                .map(image::DynamicImage::ImageRgb8)
        }
        4 => {
            let mut out = Vec::with_capacity(width * height * 4);
            for y in 0..height {
                let start = y * rowstride;
                let end = start + width * 4;
                out.extend_from_slice(src.get(start..end)?);
            }
            image::RgbaImage::from_raw(width as u32, height as u32, out)
                .map(image::DynamicImage::ImageRgba8)
        }
        _ => None,
    }
}

fn default_free_crop_rect(width: u32, height: u32) -> CropRect {
    let crop_width = ((width as f64) * 0.9).round() as u32;
    let crop_height = ((height as f64) * 0.9).round() as u32;
    CropRect {
        x: (width.saturating_sub(crop_width)) / 2,
        y: (height.saturating_sub(crop_height)) / 2,
        width: crop_width.max(1),
        height: crop_height.max(1),
    }
}

fn crop_rect_for_ratio_id(ratio_id: Option<&str>, width: u32, height: u32) -> Option<CropRect> {
    match ratio_id {
        Some("source") => Some(CropRect {
            x: 0,
            y: 0,
            width,
            height,
        }),
        Some("1:1") => centered_crop_rect_for_aspect(width, height, 1, 1),
        Some("4:3") => centered_crop_rect_for_aspect(width, height, 4, 3),
        Some("3:2") => centered_crop_rect_for_aspect(width, height, 3, 2),
        Some("16:9") => centered_crop_rect_for_aspect(width, height, 16, 9),
        _ => None,
    }
}

fn normalized_crop_rect(
    x: u32,
    y: u32,
    requested_width: u32,
    requested_height: u32,
    image_width: u32,
    image_height: u32,
    ratio_id: Option<&str>,
) -> CropRect {
    let x = x.min(image_width.saturating_sub(1));
    let y = y.min(image_height.saturating_sub(1));
    let max_width = image_width.saturating_sub(x).max(1);
    let max_height = image_height.saturating_sub(y).max(1);
    let mut width = requested_width.clamp(1, max_width);
    let mut height = requested_height.clamp(1, max_height);

    if let Some((aspect_width, aspect_height)) = ratio_pair(ratio_id, image_width, image_height) {
        height = ((width as f64 * aspect_height as f64 / aspect_width as f64).round() as u32)
            .clamp(1, max_height);
        width = ((height as f64 * aspect_width as f64 / aspect_height as f64).round() as u32)
            .clamp(1, max_width);
    }

    CropRect {
        x,
        y,
        width,
        height,
    }
}

fn ratio_pair(ratio_id: Option<&str>, image_width: u32, image_height: u32) -> Option<(u32, u32)> {
    match ratio_id {
        Some("source") => Some((image_width, image_height)),
        Some("1:1") => Some((1, 1)),
        Some("4:3") => Some((4, 3)),
        Some("3:2") => Some((3, 2)),
        Some("16:9") => Some((16, 9)),
        _ => None,
    }
}

fn scaled_preview_state(state: &EditState, preview_scale: f64) -> EditState {
    if (preview_scale - 1.0).abs() < f64::EPSILON {
        return state.clone();
    }

    let mut preview_state = state.clone();
    preview_state.crop = state.crop.map(|(x, y, width, height)| {
        (
            scale_dimension(x, preview_scale),
            scale_dimension(y, preview_scale),
            scale_dimension(width, preview_scale),
            scale_dimension(height, preview_scale),
        )
    });
    preview_state
}

fn scale_dimension(value: u32, scale: f64) -> u32 {
    ((value as f64) * scale).round().max(1.0) as u32
}
