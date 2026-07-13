use super::*;

#[gtk::test]
fn square_tile_exposes_shared_thumbnail_css_class() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    assert!(tile.has_css_class("thumb-tile"));
}

// Task 4: the new three-state CSS in `grid_css` targets
// `.glass-thumb-card`, which the legacy `.thumb-tile` selector no
// longer covers. The tile wrapper must carry the new class.
#[gtk::test]
fn square_tile_carries_glass_thumb_card_class() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    assert!(tile.has_css_class("glass-thumb-card"));
}

#[gtk::test]
fn square_tile_clips_thumbnail_to_glass_card_radius() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    assert_eq!(tile.overflow(), gtk::Overflow::Hidden);
    let picture = tile
        .imp()
        .picture
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its picture child")
        .clone();
    assert!(picture.has_css_class("thumb-image"));
}

// The translucent-white selection checkmark is parented atop the
// picture in every tile and revealed by CSS on flowboxchild:selected.
#[gtk::test]
fn square_tile_has_selection_checkmark_atop_picture() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    let checkmark = tile
        .imp()
        .checkmark
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its checkmark child")
        .clone();
    assert!(checkmark.has_css_class("thumb-checkmark"));
    // Parented (always allocated; visibility driven by CSS opacity)
    // and drawn above the picture.
    assert!(checkmark.parent().is_some());
}

#[gtk::test]
fn square_tile_has_motion_badge_for_dynamic_photos() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    let badge = tile
        .imp()
        .motion_badge
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its motion badge")
        .clone();
    assert!(badge.has_css_class("thumb-motion-badge"));
    assert!(!badge.is_visible());

    tile.set_motion_badge_visible(true);
    assert!(badge.is_visible());
}

#[gtk::test]
fn square_tile_has_duration_and_favorite_badges() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    let duration = tile
        .imp()
        .duration_badge
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its duration badge")
        .clone();
    assert!(duration.has_css_class("thumb-video-duration"));
    assert!(!duration.is_visible());

    tile.set_video_duration(Some("01:23"));
    assert!(duration.is_visible());
    assert_eq!(duration.label(), "01:23");

    let favorite = tile
        .imp()
        .favorite_badge
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its favorite badge")
        .clone();
    assert!(favorite.has_css_class("thumb-favorite-badge"));
    assert!(!favorite.is_visible());

    tile.set_favorite_badge_visible(true);
    assert!(favorite.is_visible());
}

#[gtk::test]
fn square_tile_runs_registered_thumbnail_request() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    let called = Rc::new(std::cell::Cell::new(false));
    let called_for_request = called.clone();

    tile.set_thumbnail_request(Rc::new(move || {
        called_for_request.set(true);
    }));
    tile.request_thumbnail();

    assert!(called.get());
}

#[gtk::test]
fn square_tile_clear_for_rebind_drops_previous_media_visual_state() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    tile.set_background_is_light(true);
    tile.set_cache_key(Some("old-cache-key".into()));
    tile.set_motion_badge_visible(true);
    tile.set_video_duration(Some("01:23"));
    tile.set_favorite_badge_visible(true);
    tile.add_css_class("thumb-loading");
    tile.add_css_class("thumb-placeholder");
    tile.add_css_class("media-selected");
    tile.set_can_target(false);

    tile.clear_for_rebind();

    assert_eq!(tile.background_is_light(), None);
    assert_eq!(tile.cache_key(), None);
    assert!(!tile.has_css_class("thumb-loading"));
    assert!(!tile.has_css_class("thumb-placeholder"));
    assert!(!tile.has_css_class("media-selected"));
    assert!(tile.can_target());
}

#[gtk::test]
fn square_tile_loading_placeholder_is_visible_until_a_paintable_arrives() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    tile.show_loading_placeholder();

    assert!(tile.has_css_class("thumb-loading"));
    assert!(tile.has_css_class("thumb-placeholder"));
}

#[gtk::test]
fn square_tile_accepts_zero_allocation_while_a_gridview_recycles_it() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    // GtkGridView can issue this transient deallocation before calculating the
    // next fixed column layout. It must not trip the badge-size clamps.
    tile.size_allocate(&gtk::Allocation::new(0, 0, 0, 0), -1);
}
