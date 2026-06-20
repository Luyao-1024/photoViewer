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
use libadwaita::subclass::prelude::*;

use gdk_pixbuf::{Colorspace, Pixbuf};

use crate::core::db::DbPool;
use crate::core::edit::{
    EditRegistry, EditState, ParamValue, Rotation,
};
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
        pub brightness_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub contrast_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub saturation_scale: TemplateChild<gtk::Scale>,
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
        *obj.imp().media_item.borrow_mut() = Some(media_item.clone());
        *obj.imp().pool.borrow_mut() = Some(pool);
        *obj.imp().registry.borrow_mut() = Some(EditRegistry::new_with_v1());

        obj.connect_signals();
        obj.load_source_async(media_item.path.clone());

        obj
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
                    let scale =
                        (8_000_000.0_f64 / (img.width() * img.height()) as f64).sqrt();
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
        imp.cancel_btn.get().connect_clicked(glib::clone!(@weak self as this => move |_| {
            let cb = this.imp().on_cancel.borrow().clone();
            if let Some(cb) = cb {
                cb();
            }
        }));

        // 旋转按钮
        imp.rotate_90_cw.get().connect_clicked(glib::clone!(@weak self as this => move |_| {
            this.apply_rotation_delta(90);
        }));
        imp.rotate_180.get().connect_clicked(glib::clone!(@weak self as this => move |_| {
            this.apply_rotation_delta(180);
        }));
        imp.rotate_90_ccw.get().connect_clicked(glib::clone!(@weak self as this => move |_| {
            this.apply_rotation_delta(-90);
        }));

        // 调色滑块
        imp.brightness_scale.get().connect_value_changed(glib::clone!(@weak self as this => move |s| {
            this.imp().state.borrow_mut().brightness = s.value() as i32;
            this.schedule_preview_update();
        }));
        imp.contrast_scale.get().connect_value_changed(glib::clone!(@weak self as this => move |s| {
            this.imp().state.borrow_mut().contrast = s.value() as i32;
            this.schedule_preview_update();
        }));
        imp.saturation_scale.get().connect_value_changed(glib::clone!(@weak self as this => move |s| {
            this.imp().state.borrow_mut().saturation = s.value() as i32;
            this.schedule_preview_update();
        }));

        // Save Copy（默认）：M4-T4 实现，先占位
        imp.save_copy_btn.get().connect_clicked(glib::clone!(@weak self as this => move |_| {
            tracing::info!("EditorPage: Save Copy clicked (M4-T4 will implement)");
            this.show_toast("Save Copy 将在 M4-T4 实现");
        }));

        // Crop 占位：V1 显示 toast
        imp.start_crop_btn.get().connect_clicked(glib::clone!(@weak self as this => move |_| {
            this.show_toast("Crop UI 在 V2 实现");
        }));
    }

    fn apply_rotation_delta(&self, delta: i32) {
        let mut state = self.imp().state.borrow_mut();
        let cur = state.rotation.as_degrees();
        let new = cur.saturating_add(delta).rem_euclid(360);
        state.rotation = match new {
            90 => Rotation::R90,
            180 => Rotation::R180,
            270 => Rotation::R270,
            _ => Rotation::None,
        };
        drop(state);
        self.schedule_preview_update();
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
        imp.debounce_id.borrow_mut().replace(glib::timeout_add_local_once(
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
            let rendered: std::thread::Result<
                Result<image::DynamicImage, String>,
            > = gio::spawn_blocking(move || apply_all(&registry, source, &state)).await;

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
                        this.imp().preview_picture.get().set_paintable(Some(&texture));
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

/// Apply the full `EditState` pipeline (rotation → brightness → contrast →
/// saturation → crop) to `img`. Each op is fetched from the registry by id;
/// non-zero (or non-None for crop) params are applied in order. No-op
/// params are skipped to avoid the cost of re-running the same op.
fn apply_all(
    registry: &EditRegistry,
    mut img: image::DynamicImage,
    state: &EditState,
) -> Result<image::DynamicImage, String> {
    if state.rotation != Rotation::None {
        if let Some(op) = registry.get("rotate") {
            img = op.apply(&img, ParamValue::Rotation(state.rotation))?;
        }
    }
    if state.brightness != 0 {
        if let Some(op) = registry.get("brightness") {
            img = op.apply(&img, ParamValue::Int(state.brightness))?;
        }
    }
    if state.contrast != 0 {
        if let Some(op) = registry.get("contrast") {
            img = op.apply(&img, ParamValue::Int(state.contrast))?;
        }
    }
    if state.saturation != 0 {
        if let Some(op) = registry.get("saturation") {
            img = op.apply(&img, ParamValue::Int(state.saturation))?;
        }
    }
    if let Some(crop) = state.crop {
        if let Some(op) = registry.get("crop") {
            img = op.apply(&img, ParamValue::Crop(Some(crop)))?;
        }
    }
    Ok(img)
}
