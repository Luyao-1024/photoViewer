//! 裁剪操作
use crate::core::edit::op::apply_crop;
use crate::core::edit::{CropRect, EditCategory, EditOperation, ParamValue};
use image::DynamicImage;

pub struct CropOp;

impl EditOperation for CropOp {
    fn id(&self) -> &'static str {
        "crop"
    }
    fn display_name(&self) -> &'static str {
        "Crop"
    }
    fn icon_name(&self) -> &'static str {
        "edit-cut-symbolic"
    }
    fn category(&self) -> EditCategory {
        EditCategory::Crop
    }

    fn default_params(&self) -> ParamValue {
        ParamValue::Crop(None)
    }

    fn apply(&self, img: &DynamicImage, params: ParamValue) -> Result<DynamicImage, String> {
        if let ParamValue::Crop(opt) = params {
            match opt {
                None => Ok(img.clone()),
                Some((x, y, w, h)) => Ok(apply_crop(
                    img,
                    CropRect {
                        x,
                        y,
                        width: w,
                        height: h,
                    },
                )),
            }
        } else {
            Err(format!("crop expected Crop, got {:?}", params))
        }
    }
}
