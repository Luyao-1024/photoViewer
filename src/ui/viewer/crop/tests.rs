use super::*;

#[test]
fn crop_overlay_contain_rect_centers_letterboxed_image() {
    let rect = compute_contained_image_rect(1000.0, 500.0, (400, 300)).unwrap();

    assert!((rect.x - 166.666).abs() < 0.01);
    assert_eq!(rect.y, 0.0);
    assert!((rect.width - 666.666).abs() < 0.01);
    assert_eq!(rect.height, 500.0);
}

#[test]
fn crop_overlay_drag_move_clamps_to_image_bounds() {
    let drag = CropDragState {
        mode: CropDragMode::Move,
        rect: (300, 220, 100, 80),
    };

    assert_eq!(drag_rect(drag, 60.0, 60.0, (400, 300)), (300, 220, 100, 80));
    assert_eq!(
        drag_rect(drag, -40.0, -20.0, (400, 300)),
        (260, 200, 100, 80)
    );
}

#[test]
fn crop_overlay_drag_corner_resizes_rect() {
    let drag = CropDragState {
        mode: CropDragMode::ResizeSe,
        rect: (50, 60, 120, 90),
    };

    assert_eq!(drag_rect(drag, 30.0, 20.0, (400, 300)), (50, 60, 150, 110));
}
