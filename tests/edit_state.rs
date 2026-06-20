use photo_viewer::core::edit::{EditCategory, EditState};

#[test]
fn default_state_is_zero() {
    let s = EditState::default();
    assert_eq!(s.brightness, 0);
    assert_eq!(s.contrast, 0);
    assert_eq!(s.saturation, 0);
    assert!(s.crop.is_none());
}

#[test]
fn state_can_be_modified() {
    let s = EditState {
        brightness: 50,
        crop: Some((10, 20, 100, 200)),
        ..Default::default()
    };
    assert_eq!(s.brightness, 50);
    assert_eq!(s.crop, Some((10, 20, 100, 200)));
}

#[test]
fn category_variants_exist() {
    // Compile-time only check
    let _transform: EditCategory = EditCategory::Transform;
    let _color: EditCategory = EditCategory::Color;
    let _crop: EditCategory = EditCategory::Crop;
    let _filter: EditCategory = EditCategory::Filter;
    let _effect: EditCategory = EditCategory::Effect;
}
