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

    // The grid badge draws the same mark the favorite buttons draw. It used to
    // be a "♡" label at 18pt, i.e. a text glyph shaped by the system font
    // sitting in the same app as a themed vector heart.
    assert_eq!(
        favorite.icon_name().as_deref(),
        Some(crate::ui::favorite_icon::NAME),
        "the tile badge must use the shared favorite mark, not its own glyph",
    );
    assert_eq!(
        favorite.pixel_size(),
        crate::ui::favorite_icon::TILE_PIXEL_SIZE,
        "the tile badge box is the shared one, so the two overlays in that \
         corner keep the same weight",
    );

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
    tile.set_cloud_state(Some(CloudState::Synced));
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
    assert!(!tile
        .imp()
        .sync_badge
        .borrow()
        .as_ref()
        .unwrap()
        .is_visible());
    assert!(tile.can_target());
}

#[gtk::test]
fn square_tile_cloud_icons_are_bundled_and_switch_with_state() {
    let _ = gtk::init();
    crate::ensure_resources_registered();
    for state in [CloudState::Synced, CloudState::Off] {
        for dark in [true, false] {
            let path = cloud_badge::resource(state, dark);
            assert!(
                gtk::gio::resources_lookup_data(path, gtk::gio::ResourceLookupFlags::NONE).is_ok()
            );
        }
    }

    let tile = SquareTile::new();
    let badge = tile.imp().sync_badge.borrow().as_ref().unwrap().clone();
    tile.set_cloud_state(Some(CloudState::Off));
    assert!(badge.is_visible());
    assert_eq!(
        badge.resource().as_deref(),
        Some(cloud_badge::resource(CloudState::Off, true))
    );
    tile.set_cloud_state(Some(CloudState::Synced));
    assert_eq!(
        badge.resource().as_deref(),
        Some(cloud_badge::resource(CloudState::Synced, true))
    );
}

#[gtk::test]
fn cloud_icons_render_with_transparent_centers() {
    let _ = gtk::init();
    crate::ensure_resources_registered();
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None).unwrap();

    for (state, dark) in [
        (CloudState::Synced, true),
        (CloudState::Off, true),
        (CloudState::Synced, false),
        (CloudState::Off, false),
    ] {
        let image = gtk::Image::from_resource(cloud_badge::resource(state, dark));
        let paintable = image.paintable().unwrap();
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, 18.0, 18.0);
        let node = snapshot.to_node().unwrap();
        let texture = renderer.render_texture(&node, None);
        let stride = texture.width() as usize * 4;
        let mut pixels = vec![0; stride * texture.height() as usize];
        texture.download(&mut pixels, stride);

        // Sample from the rendered ink instead of hardcoded coordinates: this
        // test owns "the badge is an outline", not the artwork's exact curves.
        let alpha = |x: usize, y: usize| pixels[y * stride + x * 4 + 3];
        let (mut min_x, mut min_y) = (usize::MAX, usize::MAX);
        let (mut max_x, mut max_y) = (0usize, 0usize);
        for y in 0..texture.height() as usize {
            for x in 0..texture.width() as usize {
                if alpha(x, y) > 24 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(
            min_x < max_x && min_y < max_y,
            "{state:?} should render a visible outline"
        );

        // The badge has to stay an outline rather than a filled blob, and the
        // same rule has to hold for both states: `cloud-off` draws a diagonal
        // across the body, so sampling the exact center would only work for one
        // of them. Measure the whole interior instead — the stroke alone keeps
        // a few opaque pixels inside a 3 px inset, a filled cloud would fill
        // all of it.
        const INSET: usize = 3;
        let mut interior = 0usize;
        let mut opaque = 0usize;
        for y in (min_y + INSET)..(max_y + 1 - INSET) {
            for x in (min_x + INSET)..(max_x + 1 - INSET) {
                interior += 1;
                if alpha(x, y) > 200 {
                    opaque += 1;
                }
            }
        }
        let filled_fraction = opaque as f64 / interior.max(1) as f64;
        assert!(
            filled_fraction < 0.25,
            "{state:?} interior is {filled_fraction:.2} opaque; the badge should be an \
             outline, not a filled shape",
        );

        // The stroke crossing the top of the silhouette is its opaque core.
        // Sample the strongest pixel in the top rows rather than the first one:
        // the topmost row of a downscaled bitmap is partial coverage by
        // definition.
        let mut edge_base = 0usize;
        let mut edge_alpha = 0u8;
        for y in min_y..(min_y + 3).min(texture.height() as usize) {
            for x in min_x..=max_x {
                if alpha(x, y) > edge_alpha {
                    edge_alpha = alpha(x, y);
                    edge_base = y * stride + x * 4;
                }
            }
        }
        let edge_red = pixels[edge_base];
        assert!(
            edge_alpha > 128,
            "{state:?} top edge should remain visible; alpha={edge_alpha}"
        );
        assert!(
            if dark { edge_red > 200 } else { edge_red < 100 },
            "{state:?} outline should match the surface color"
        );
    }

    // A renderer that is still realized when it is finalized trips
    // `Gsk:ERROR:gskrenderer.c:gsk_renderer_dispose: assertion failed:
    // (!priv->is_realized)`, which is a `g_abort()` on the runner's GTK 4.14.5 —
    // it killed the entire lib test binary, so nothing after this test ever
    // reported. GTK 4.22 lets it pass, which is why a green local suite hid it.
    // Unrealizing is what the API asks for, on every version.
    renderer.unrealize();
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

/// P2-4: a thumbnail is a picture with no text, and the state badges were bare
/// glyphs, so to a screen reader the grid was anonymous boxes plus stray "▶"
/// and "♡" characters. The tile takes its name from whoever binds it, and every
/// state overlay names itself.
#[gtk::test]
fn the_tile_and_its_state_badges_are_named() {
    let _ = gtk::init();
    let tile = SquareTile::new();

    // The role is what makes the name reachable at all: GTK ignores an
    // aria-label pushed onto a plain container.
    assert_eq!(
        tile.accessible_role(),
        gtk::AccessibleRole::Img,
        "a thumbnail is a picture, so it must take the role that carries a name"
    );

    tile.set_accessible_name("IMG_0123.jpg");
    assert_eq!(
        tile.accessible_name_for_tests().as_deref(),
        Some("IMG_0123.jpg"),
        "the name pushed to AT must be readable back"
    );

    // A recycled virtual-grid cell must not keep answering to the previous
    // photo's name while it is still a placeholder.
    tile.clear_for_rebind();
    assert_eq!(
        tile.accessible_name_for_tests(),
        None,
        "clear_for_rebind has to drop the stale name with the stale thumbnail"
    );

    for key in ["tile.badge.motion", "tile.badge.favorite"] {
        let name = tr(key);
        assert!(
            !name.is_empty() && name != key,
            "{key} must resolve to a real word in the active locale, got {name:?}"
        );
    }

    let named = trf("tile.badge.duration", &[("duration", "01:23")]);
    assert!(
        named.contains("01:23") && named != "tile.badge.duration",
        "the duration badge keeps the visible number inside its name, got {named:?}"
    );

    // The badges and the tile all read their names from the same i18n path the
    // visible text uses, so a locale switch cannot desynchronise the two.
    let duration = tile
        .imp()
        .duration_badge
        .borrow()
        .as_ref()
        .expect("duration badge")
        .clone();
    tile.set_video_duration(Some("01:23"));
    assert_eq!(duration.label(), "01:23", "the caption stays the bare time");
    assert_eq!(
        tile.imp()
            .checkmark
            .borrow()
            .as_ref()
            .expect("checkmark")
            .accessible_role(),
        gtk::AccessibleRole::Presentation,
        "the tick is decoration: naming it would announce \
         \"selected\" on every tile"
    );
}
