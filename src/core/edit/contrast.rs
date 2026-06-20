use crate::core::edit::op::apply_color_adjust;
use crate::core::edit::{EditCategory, EditOperation, ParamValue};
use image::DynamicImage;

pub struct ContrastOp;

impl EditOperation for ContrastOp {
    fn id(&self) -> &'static str {
        "contrast"
    }
    fn display_name(&self) -> &'static str {
        "Contrast"
    }
    fn icon_name(&self) -> &'static str {
        "preferences-contrast-symbolic"
    }
    fn category(&self) -> EditCategory {
        EditCategory::Color
    }

    fn default_params(&self) -> ParamValue {
        ParamValue::Int(0)
    }

    fn apply(&self, img: &DynamicImage, params: ParamValue) -> Result<DynamicImage, String> {
        if let ParamValue::Int(c) = params {
            Ok(apply_color_adjust(img, 0, c, 0))
        } else {
            Err(format!("contrast expected Int, got {:?}", params))
        }
    }
}