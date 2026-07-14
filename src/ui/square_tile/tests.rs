use super::*;

#[gtk::test]
fn square_tile_exposes_shared_thumbnail_css_class() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    assert!(tile.has_css_class("thumb-tile"));
}

#[gtk::test]
fn square_tile_declares_height_for_width_for_responsive_grid_cells() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    tile.set_height_for_width(true);

    assert_eq!(tile.request_mode(), gtk::SizeRequestMode::HeightForWidth);
}

#[gtk::test]
fn fixed_size_square_tiles_do_not_propagate_row_width_as_height() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    assert_eq!(tile.request_mode(), gtk::SizeRequestMode::ConstantSize);
    let (minimum, natural, min_baseline, natural_baseline) =
        tile.measure(gtk::Orientation::Vertical, 174);
    assert_eq!(minimum, natural);
    assert_ne!(natural, 174);
    assert_eq!((min_baseline, natural_baseline), (-1, -1));
}

#[gtk::test]
fn square_tile_can_relax_its_horizontal_minimum_for_virtual_grid_cells() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    tile.set_target(270);

    let (fixed_minimum, fixed_natural, _, _) = tile.measure(gtk::Orientation::Horizontal, -1);
    assert_eq!(fixed_minimum, fixed_natural);
    assert!(!tile.allows_width_shrink());

    tile.set_allow_width_shrink(true);
    let (relaxed_minimum, relaxed_natural, _, _) = tile.measure(gtk::Orientation::Horizontal, -1);

    assert!(tile.allows_width_shrink());
    assert!(
        relaxed_minimum < relaxed_natural,
        "a virtual GridView cell must accept a final width below its preferred target"
    );
    assert_eq!(relaxed_natural, fixed_natural);
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

// The translucent-white selection checkmark is parented by the shared overlay
// atop the picture in every tile and revealed by CSS on flowboxchild:selected.
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
    // Parented (always allocated; visibility driven by CSS opacity).
    assert!(checkmark.parent().is_some());
}

#[gtk::test]
fn square_tile_uses_one_managed_overlay_child_tree() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    let content = tile
        .imp()
        .content
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its overlay content root")
        .clone();
    let picture = tile
        .imp()
        .picture
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its picture child")
        .clone();
    let checkmark = tile
        .imp()
        .checkmark
        .borrow()
        .as_ref()
        .expect("SquareTile should construct its checkmark child")
        .clone();

    assert_eq!(tile.first_child(), Some(content.clone().upcast()));
    assert_eq!(content.parent(), Some(tile.clone().upcast()));
    assert_eq!(picture.parent(), Some(content.clone().upcast()));
    assert_eq!(checkmark.parent(), Some(content.upcast()));
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
fn square_tile_only_restores_opacity_on_a_legacy_flowbox_wrapper() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    let generic_parent = gtk::Box::new(gtk::Orientation::Vertical, 0);
    generic_parent.append(&tile);
    generic_parent.set_opacity(0.25);

    tile.set_paintable(None::<&gtk::gdk::Paintable>);

    assert!(
        (generic_parent.opacity() - 0.25).abs() < 0.01,
        "a non-FlowBox parent must not be changed by set_paintable"
    );

    generic_parent.remove(&tile);
    let flow = gtk::FlowBox::new();
    flow.append(&tile);
    let flow_child = tile
        .parent()
        .and_downcast::<gtk::FlowBoxChild>()
        .expect("FlowBox should wrap the tile in a FlowBoxChild");
    flow_child.set_opacity(0.25);

    tile.set_paintable(None::<&gtk::gdk::Paintable>);

    assert_eq!(flow_child.opacity(), 1.0);
}

#[gtk::test]
fn square_tile_accepts_zero_allocation_while_a_gridview_recycles_it() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    // GtkGridView can issue this transient deallocation before calculating the
    // next fixed column layout. It must not trip the badge-size clamps.
    tile.size_allocate(&gtk::Allocation::new(0, 0, 0, 0), -1);
}
