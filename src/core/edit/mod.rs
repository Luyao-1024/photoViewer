//! 编辑操作 trait + 注册表 + 共享状态
pub mod brightness;
pub mod contrast;
pub mod crop;
pub mod destructive_rotate;
pub mod op;
pub mod rotate;
pub mod saturation;
pub mod save;

pub use destructive_rotate::rotate_in_place;
pub use save::{save_as_copy, save_overwrite};

use std::collections::HashMap;
use std::sync::Arc;

use gtk4::glib;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    None,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn as_degrees(self) -> i32 {
        match self {
            Self::None => 0,
            Self::R90 => 90,
            Self::R180 => 180,
            Self::R270 => 270,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default)]
pub struct EditState {
    pub rotation: Rotation,
    pub brightness: i32, // -100..+100
    pub contrast: i32,
    pub saturation: i32,
    /// Optional crop as (x, y, width, height) in pixels.
    pub crop: Option<(u32, u32, u32, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCategory {
    Transform, // Rotate
    Color,     // Brightness, Contrast, Saturation
    Crop,
    Filter,    // V2: Grayscale, Blur, etc.
    Effect,    // V2: Sticker, RedEye, etc.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamValue {
    Int(i32),
    Uint(u32),
    Bool(bool),
    /// Crop as (x, y, width, height); `None` means "cleared".
    Crop(Option<(u32, u32, u32, u32)>),
    Rotation(Rotation),
}

pub trait EditOperation: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn icon_name(&self) -> &'static str;
    fn category(&self) -> EditCategory;
    fn default_params(&self) -> ParamValue;
    fn validate_params(&self, _params: ParamValue) -> Result<(), String> {
        Ok(())
    }
    fn apply(
        &self,
        img: &image::DynamicImage,
        params: ParamValue,
    ) -> Result<image::DynamicImage, String>;
}

#[derive(Clone)]
pub struct EditRegistry {
    ops: HashMap<&'static str, Arc<dyn EditOperation>>,
}

impl EditRegistry {
    pub fn new() -> Self {
        Self { ops: HashMap::new() }
    }

    pub fn new_with_v1() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(rotate::RotateOp));
        r.register(Arc::new(crop::CropOp));
        r.register(Arc::new(brightness::BrightnessOp));
        r.register(Arc::new(contrast::ContrastOp));
        r.register(Arc::new(saturation::SaturationOp));
        r
    }

    pub fn register(&mut self, op: Arc<dyn EditOperation>) {
        self.ops.insert(op.id(), op);
    }

    pub fn list(&self) -> Vec<Arc<dyn EditOperation>> {
        let mut v: Vec<_> = self.ops.values().cloned().collect();
        v.sort_by_key(|op| (op.category() as u32, op.id()));
        v
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn EditOperation>> {
        self.ops.get(id).cloned()
    }
}

impl Default for EditRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply the full `EditState` pipeline (rotation → brightness → contrast →
/// saturation → crop) to `img`. Each op is fetched from the registry by id;
/// non-zero (or non-None for crop) params are applied in order. No-op
/// params are skipped to avoid the cost of re-running the same op.
pub fn apply_all(
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

// Workaround: gtk::glib re-export for ParamValue::to_variant if needed
impl glib::variant::ToVariant for ParamValue {
    fn to_variant(&self) -> glib::Variant {
        match self {
            Self::Int(i) => glib::Variant::from(*i),
            Self::Uint(u) => glib::Variant::from(*u as i64),
            Self::Bool(b) => glib::Variant::from(*b),
            Self::Rotation(r) => glib::Variant::from(r.as_degrees()),
            Self::Crop(opt) => match opt {
                Some((x, y, w, h)) => glib::Variant::from((*x, *y, *w, *h)),
                None => glib::Variant::from((0u32, 0u32, 0u32, 0u32)),
            },
        }
    }
}
