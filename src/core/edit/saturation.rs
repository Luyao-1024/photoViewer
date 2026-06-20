use crate::core::edit::op::apply_color_adjust;
use crate::core::edit::{EditCategory, EditOperation, ParamValue};
use image::DynamicImage;

pub struct SaturationOp;

impl EditOperation for SaturationOp {
    fn id(&self) -> &'static str {
        "saturation"
    }
    fn display_name(&self) -> &'static str {
        "Saturation"
    }
    fn icon_name(&self) -> &'static str {
        "color-select-symbolic"
    }
    fn category(&self) -> EditCategory {
        EditCategory::Color
    }

    fn default_params(&self) -> ParamValue {
        ParamValue::Int(0)
    }

    fn apply(&self, img: &DynamicImage, params: ParamValue) -> Result<DynamicImage, String> {
        if let ParamValue::Int(s) = params {
            Ok(apply_color_adjust(img, 0, 0, s))
        } else {
            Err(format!("saturation expected Int, got {:?}", params))
        }
    }
}