use super::super::*;

#[test]
fn thumbnail_request_window_includes_viewport_and_one_page_overscan() {
    assert!(tile_intersects_request_window(0.0, 270.0, 900.0, 1.0));
    assert!(tile_intersects_request_window(1_700.0, 270.0, 900.0, 1.0));
    assert!(!tile_intersects_request_window(1_900.0, 270.0, 900.0, 1.0));
    assert!(tile_intersects_request_window(-250.0, 270.0, 900.0, 1.0));
    assert!(!tile_intersects_request_window(-1_200.0, 270.0, 900.0, 1.0));
}
