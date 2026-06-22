//! 旋转操作
use crate::core::edit::op::apply_rotation;
use crate::core::edit::{EditCategory, EditOperation, ParamValue, Rotation};
use image::DynamicImage;

pub struct RotateOp;

impl EditOperation for RotateOp {
    fn id(&self) -> &'static str {
        "rotate"
    }
    fn display_name(&self) -> &'static str {
        "Rotate"
    }
    fn icon_name(&self) -> &'static str {
        "object-rotate-right-symbolic"
    }
    fn category(&self) -> EditCategory {
        EditCategory::Transform
    }

    fn default_params(&self) -> ParamValue {
        ParamValue::Rotation(Rotation::None)
    }

    fn apply(&self, img: &DynamicImage, params: ParamValue) -> Result<DynamicImage, String> {
        if let ParamValue::Rotation(r) = params {
            Ok(apply_rotation(img, r))
        } else {
            Err(format!("rotate expected Rotation, got {:?}", params))
        }
    }
}
