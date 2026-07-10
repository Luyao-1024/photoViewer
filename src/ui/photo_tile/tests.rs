use super::*;

#[gtk::test]
fn photo_tile_template_loads_picture_child() {
    let _ = gtk::init();
    let tile = PhotoTile::new();
    assert!(tile.imp().picture.get().is::<gtk::Picture>());
}

#[gtk::test]
fn photo_tile_picture_can_receive_selection_outline() {
    let _ = gtk::init();
    let tile = PhotoTile::new();

    assert!(tile.has_css_class("thumb-tile"));
    assert!(tile.imp().picture.get().has_css_class("photo-tile-picture"));
}

#[gtk::test]
fn photo_tile_template_loads_motion_badge_child() {
    let _ = gtk::init();
    let tile = PhotoTile::new();

    assert!(tile
        .imp()
        .motion_badge
        .get()
        .has_css_class("thumb-motion-badge"));
    assert!(!tile.imp().motion_badge.get().is_visible());
}
