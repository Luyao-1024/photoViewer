use crate::core::edit::op::apply_color_adjust;
use crate::core::edit::{EditCategory, EditOperation, ParamValue};
use image::DynamicImage;

pub struct BrightnessOp;

impl EditOperation for BrightnessOp {
    fn id(&self) -> &'static str {
        "brightness"
    }
    fn display_name(&self) -> &'static str {
        "Brightness"
    }
    fn icon_name(&self) -> &'static str {
        "weather-clear-symbolic"
    }
    fn category(&self) -> EditCategory {
        EditCategory::Color
    }

    fn default_params(&self) -> ParamValue {
        ParamValue::Int(0)
    }

    fn apply(&self, img: &DynamicImage, params: ParamValue) -> Result<DynamicImage, String> {
        if let ParamValue::Int(b) = params {
            Ok(apply_color_adjust(img, b, 0, 0))
        } else {
            Err(format!("brightness expected Int, got {:?}", params))
        }
    }
}
