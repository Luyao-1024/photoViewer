//! EditorPage - 实时预览 + 旋转/调色控制面板
//!
//! 生命周期：
//! 1. `new(media_item, pool)` 同步建立 widget 树与状态（空 EditState）
//! 2. `glib::spawn_future_local` 异步加载原图（>8MP 自动降采样到 ~8MP）
//! 3. 加载完成 → `schedule_preview_update` → 33ms 后首次渲染
//! 4. 用户操作（旋转 / 调色滑块）→ 修改 `EditState` → `schedule_preview_update`
//! 5. 30fps 节流：`glib::timeout_add_local_once(33ms)`，新请求取消旧 timer
//! 6. `render_preview` → `gio::spawn_blocking` 在工作线程上跑 `apply_all`
//! 7. 完成后回到主线程，将 `DynamicImage → Pixbuf → Texture` 贴到 `GtkPicture`
//!
//! Crop UI V1 仅占位（按钮回调打日志），实际裁剪面板留到 V2。
//! Save Copy 实现留到 M4-T4，本任务只接回调。
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{
    AdwDialogExt, AlertDialogExt, NavigationPageExt, PreferencesGroupExt, PreferencesRowExt,
};
use libadwaita::subclass::prelude::*;

use gdk_pixbuf::{Colorspace, Pixbuf};

use crate::core::db::DbPool;
use crate::core::edit::{apply_all, EditRegistry, EditState};
use crate::core::i18n::{tr, trf};
use crate::core::media::MediaItem;

mod imp {
    use super::*;

    /// We do NOT derive `gtk::CompositeTemplate` here for the `Default`
    /// fields — `Default::default()` for `RefCell<Option<...>>` works but
    /// `Default` on the entire struct needs each field to implement it.
    /// `Option<DbPool>`, `Option<EditRegistry>`, `Option<DynamicImage>` are
    /// all `Default`, so this compiles.
    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/editor-page.ui")]
    pub struct EditorPage {
        pub media_item: RefCell<Option<MediaItem>>,
        pub pool: RefCell<Option<DbPool>>,
        pub registry: RefCell<Option<EditRegistry>>,
        pub state: RefCell<EditState>,
        pub source_image: RefCell<Option<image::DynamicImage>>,
        /// Token to invalidate stale `spawn_blocking` responses: each new
        /// render bumps it; on result arrival we compare to the current
        /// token and drop if it doesn't match (a newer render started).
        pub render_token: RefCell<u64>,
        #[template_child]
        pub preview_picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub cancel_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_copy_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_menu_btn: TemplateChild<gtk::MenuButton>,
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
        pub debounce_id: RefCell<Option<glib::SourceId>>,
        /// Optional callback fired by `cancel_btn`. Wired by the host
        /// (typically pops the `EditorPage` from the nav stack).
        pub on_cancel: RefCell<Option<Rc<dyn Fn()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditorPage {
        const NAME: &'static str = "EditorPage";
        type Type = super::EditorPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EditorPage {}
    impl WidgetImpl for EditorPage {}
    impl NavigationPageImpl for EditorPage {}
}

glib::wrapper! {
    pub struct EditorPage(ObjectSubclass<imp::EditorPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl EditorPage {
    /// Build a new EditorPage for `media_item`. The source image is loaded
    /// asynchronously on a blocking worker (down-sampled to ~8MP if the
    /// original is larger); the preview is rendered once the load
    /// completes. `pool` is stored for downstream M4-T4 save logic.
    pub fn new(media_item: MediaItem, pool: DbPool) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&tr("page.editor.title"));
        obj.apply_i18n();
        *obj.imp().media_item.borrow_mut() = Some(media_item.clone());
        *obj.imp().pool.borrow_mut() = Some(pool);
        *obj.imp().registry.borrow_mut() = Some(EditRegistry::new_with_v1());

        obj.connect_signals();
        obj.load_source_async(media_item.path.clone());

        obj
    }

    fn apply_i18n(&self) {
        let imp = self.imp();
        imp.cancel_btn.get().set_label(&tr("button.cancel"));
        imp.save_copy_btn
            .get()
            .set_label(&tr("editor.menu.save_copy"));
        imp.save_menu_btn.get().set_label(&tr("button.save"));
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
    }

    /// Register a callback fired when the user presses the Cancel button.
    /// The host typically wires this to `nav_view.pop()`.
    pub fn connect_cancel<F: Fn() + 'static>(&self, f: F) {
        *self.imp().on_cancel.borrow_mut() = Some(Rc::new(f));
    }

    /// Current edit state (useful for save-into-DB in M4-T4).
    pub fn state(&self) -> EditState {
        self.imp().state.borrow().clone()
    }

    /// 异步加载原图。>8MP 时降采样到 ~8MP（Triangle filter），减少后续预览
    /// 计算量。`spawn_blocking` 在工作线程上做 `image::open`。
    fn load_source_async(&self, path: std::path::PathBuf) {
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            // `gio::spawn_blocking` returns `JoinHandle<Option<DynamicImage>>`;
            // on `.await` we get `thread::Result<Option<DynamicImage>>`
            // (`Err` only if the worker panicked). The closure itself yields
            // `Option` because we already swallow decode errors with `.ok()`.
            let loaded: std::thread::Result<Option<image::DynamicImage>> =
                gio::spawn_blocking(move || image::open(&path).ok()).await;

            if let Ok(Some(img)) = loaded {
                // >8MP 降采样到 ~8MP（保持宽高比）
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
                    *this.imp().source_image.borrow_mut() = Some(downsampled);
                    this.schedule_preview_update();
                }
            } else {
                tracing::warn!("EditorPage: failed to load source image");
            }
        });
    }

    fn connect_signals(&self) {
        let imp = self.imp();

        // Cancel: 委托给 host 提供的回调
        imp.cancel_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                let cb = this.imp().on_cancel.borrow().clone();
                if let Some(cb) = cb {
                    cb();
                }
            }));

        // 旋转按钮
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

        // 调色滑块
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

        // Save Copy（默认）：渲染 → 写新文件 → 插新 DB 行
        imp.save_copy_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.save_as_copy();
            }));

        // Save ▼ 菜单：Save Copy / Save Overwrite
        self.setup_save_menu();

        // Crop 占位：V1 显示 toast
        imp.start_crop_btn
            .get()
            .connect_clicked(glib::clone!(@weak self as this => move |_| {
                this.show_toast(&tr("editor.crop_placeholder"));
            }));
    }

    /// Build the Save ▼ popover menu (Save Copy / Save Overwrite) and attach
    /// it to `save_menu_btn`. Each entry dispatches to the same methods the
    /// toolbar buttons call.
    fn setup_save_menu(&self) {
        let menu = gio::Menu::new();
        menu.append(Some(&tr("editor.menu.save_copy")), Some("editor.save-copy"));
        menu.append(
            Some(&tr("editor.menu.save_overwrite")),
            Some("editor.save-overwrite"),
        );

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        self.imp().save_menu_btn.get().set_popover(Some(&popover));

        // Action group carrying the two menu actions
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

    /// `Save Copy` 流程：异步渲染当前 `EditState` 到 `{stem}_edited.{ext}`，
    /// 插入新的 `media_items` 行，完成后弹 toast 并导航返回。
    fn save_as_copy(&self) {
        let imp = self.imp();
        let item = match imp.media_item.borrow().clone() {
            Some(i) => i,
            None => {
                tracing::warn!("EditorPage.save_as_copy: no media_item");
                return;
            }
        };
        let state = imp.state.borrow().clone();
        let pool = match imp.pool.borrow().clone() {
            Some(p) => p,
            None => {
                tracing::warn!("EditorPage.save_as_copy: no pool");
                return;
            }
        };
        let registry = match imp.registry.borrow().clone() {
            Some(r) => r,
            None => {
                tracing::warn!("EditorPage.save_as_copy: no registry");
                return;
            }
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
                    Ok(Ok(_new_item)) => {
                        this.show_toast(&tr("editor.toast.saved_copy"));
                        // 导航回上一页（host 通过 connect_cancel 注册的回调
                        // 通常就是 pop；用 action 名走 nav stack 同样安全）。
                        let _ = this.activate_action("navigation.pop", None);
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Save Copy failed: {}", e);
                        this.show_toast(&trf(
                            "editor.toast.save_copy_failed",
                            &[("error", &e.to_string())],
                        ));
                    }
                    Err(_) => {
                        tracing::error!("Save Copy worker panicked");
                    }
                }
            }
        });
    }

    /// `Save Overwrite` 流程：先弹 `AdwAlertDialog` 二次确认，用户确认后
    /// 调度 `perform_save_overwrite` 在工作线程上完成实际渲染与 DB 更新。
    fn save_overwrite_with_confirm(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("editor.overwrite_title"))
            .body(tr("editor.overwrite_body"))
            .build();
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

    /// 实际执行 `save_overwrite`：备份 → 渲染 → 写回原文件 → 更新 DB。
    fn perform_save_overwrite(&self) {
        let imp = self.imp();
        let item = match imp.media_item.borrow().clone() {
            Some(i) => i,
            None => {
                tracing::warn!("EditorPage.perform_save_overwrite: no media_item");
                return;
            }
        };
        let state = imp.state.borrow().clone();
        let pool = match imp.pool.borrow().clone() {
            Some(p) => p,
            None => {
                tracing::warn!("EditorPage.perform_save_overwrite: no pool");
                return;
            }
        };
        let registry = match imp.registry.borrow().clone() {
            Some(r) => r,
            None => {
                tracing::warn!("EditorPage.perform_save_overwrite: no registry");
                return;
            }
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
                        this.show_toast(&tr("editor.toast.overwritten"));
                        let _ = this.activate_action("navigation.pop", None);
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Save Overwrite failed: {}", e);
                        this.show_toast(&trf(
                            "editor.toast.overwrite_failed",
                            &[("error", &e.to_string())],
                        ));
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
            None => {
                tracing::warn!("EditorPage.apply_rotation_delta: no media_item");
                return;
            }
        };

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            // `gio::spawn_blocking` 在工作线程上执行破坏性旋转，覆盖原文件
            // （保留 .jpg.bak 备份）；返回 `Result<()>` 包装为
            // `thread::Result`（Err 仅在 worker panic 时）。
            let result: std::thread::Result<std::result::Result<(), crate::core::error::AppError>> =
                gio::spawn_blocking(move || crate::core::edit::rotate_in_place(&item.path, delta))
                    .await;

            if let Some(this) = weak.upgrade() {
                match result {
                    Ok(Ok(())) => {
                        this.show_undo_toast(delta);
                    }
                    Ok(Err(e)) => tracing::error!("旋转失败: {}", e),
                    Err(_) => {
                        tracing::error!("旋转 worker 异常终止");
                    }
                }
            }
        });
    }

    /// Schedule an auto-reverse rotation 5 seconds after the user pressed a
    /// rotate button. The inverse rotation re-applies `rotate_in_place` so the
    /// `.jpg.bak` backup is restored to the original path. A real UI with a
    /// visible toast button + ToastOverlay would replace this later.
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

    /// 30fps 节流预览重算：用 `glib::timeout_add_local_once(33ms)` 延迟一次
    /// 渲染。期间任何新的状态变更都会取消旧 timer 并安排新的 — 多次连按
    /// 旋转按钮或拖动滑块时只渲染最后一帧。
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

    /// 触发一次预览渲染。`spawn_blocking` 跑 `apply_all`，结果回到主线程
    /// 转为 `Texture` 贴到 `GtkPicture`。`render_token` 用来丢弃过期结果：
    /// 多个 render 并发时只有最后一个会落盘。
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

        // Bump token so any in-flight render from a previous state will
        // be discarded on arrival.
        let token = {
            let t = imp.render_token.borrow().saturating_add(1);
            *imp.render_token.borrow_mut() = t;
            t
        };

        imp.spinner.get().set_visible(true);

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            // `apply_all` returns `Result<DynamicImage, String>`. Wrapped by
            // `JoinHandle`'s `thread::Result` (Err only on worker panic), so
            // the outer `Result` distinguishes a panic from an app-level
            // image error.
            let rendered: std::thread::Result<std::result::Result<image::DynamicImage, String>> =
                gio::spawn_blocking(move || apply_all(&registry, source, &state)).await;

            if let Some(this) = weak.upgrade() {
                // Another render started after us — drop this stale result.
                if *this.imp().render_token.borrow() != token {
                    return;
                }
                match rendered {
                    Ok(Ok(img)) => {
                        let rgb = img.to_rgb8();
                        let (width, height) = (rgb.width() as i32, rgb.height() as i32);
                        let rowstride = width * 3;
                        // `Pixbuf::from_bytes` requires `&glib::Bytes` (zero-copy
                        // view into the underlying buffer). `into_vec` then
                        // wrap keeps the data alive for the lifetime of the
                        // resulting `Pixbuf`.
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
                        this.imp()
                            .preview_picture
                            .get()
                            .set_paintable(Some(&texture));
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("EditorPage: render failed: {}", e);
                    }
                    Err(_) => {
                        tracing::warn!("EditorPage: spawn_blocking panicked");
                    }
                }
                this.imp().spinner.get().set_visible(false);
            }
        });
    }

    /// V1 占位：仅记录日志。正式实现应接 `AdwToastOverlay`（需要外层包装）。
    fn show_toast(&self, msg: &str) {
        tracing::info!("EditorPage toast: {}", msg);
    }
}

impl Default for EditorPage {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}
