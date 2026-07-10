use super::super::test_support::*;
use super::super::*;
use super::*;

const SCROLL_PAGE_SIZE: f64 = 300.0;
const SCROLL_BTN_W: f64 = 60.0;
const SCROLL_SPACING: f64 = 6.0;
const SCROLL_UPPER: f64 = 720.0;

#[test]
fn initial_window_centred_on_current_in_middle_of_album() {
    // 100 photos, current = 50 -> +/-5 items centred, no clipping.
    let (start, end) = compute_initial_thumb_window(50, 100);
    assert_eq!(start, 45);
    assert_eq!(end, 56);
    assert_eq!(end - start, THUMB_DEFAULT_WINDOW_LEN);
}

#[test]
fn initial_window_clips_at_album_start() {
    // current near 0 -> start clamped to 0, missing left-side items are
    // backfilled on the right so the strip still has a full window.
    let (start, end) = compute_initial_thumb_window(2, 100);
    assert_eq!(start, 0);
    assert_eq!(end, THUMB_DEFAULT_WINDOW_LEN);
    assert!(end > 2);
}

#[test]
fn initial_window_clips_at_album_end() {
    // current near the end -> end clamped to n_items, with a full window
    // backfilled on the left when enough items exist.
    let n = 100u32;
    let current = n - 2;
    let (start, end) = compute_initial_thumb_window(current, n);
    assert_eq!(end, n);
    assert_eq!(end - start, THUMB_DEFAULT_WINDOW_LEN);
    assert!(start <= current);
}

#[test]
fn initial_window_is_empty_for_empty_album() {
    assert_eq!(compute_initial_thumb_window(0, 0), (0, 0));
    assert_eq!(compute_initial_thumb_window(5, 0), (0, 0));
}

#[test]
fn extend_left_grows_window_without_changing_end() {
    // 100 photos, window [30, 40], extend left by LAZY_HALF.
    let (new_start, new_end) = compute_extended_thumb_window(-1, 30, 40, 100, 10).unwrap();
    assert_eq!(new_start, 30 - THUMB_LAZY_HALF);
    assert_eq!(new_end, 40);
}

#[test]
fn extend_right_grows_window_without_changing_start() {
    let (new_start, new_end) = compute_extended_thumb_window(1, 30, 40, 100, 10).unwrap();
    assert_eq!(new_start, 30);
    assert_eq!(new_end, 40 + THUMB_LAZY_HALF);
}

#[test]
fn right_growth_uses_append_update_instead_of_rebuilding_strip() {
    assert_eq!(
        classify_thumb_window_update(0, 23, 0, 27),
        ThumbWindowUpdateKind::AppendRight
    );
}

#[test]
fn left_growth_is_classified_as_prepend_update() {
    assert_eq!(
        classify_thumb_window_update(20, 31, 16, 31),
        ThumbWindowUpdateKind::PrependLeft
    );
}

#[test]
fn sliding_window_still_rebuilds_because_existing_indices_change() {
    assert_eq!(
        classify_thumb_window_update(50, 90, 54, 94),
        ThumbWindowUpdateKind::Rebuild
    );
}

#[test]
fn extend_left_returns_none_at_album_start() {
    // Already at 0, can't go further left.
    assert!(compute_extended_thumb_window(-1, 0, 10, 100, 10).is_none());
}

#[test]
fn extend_right_returns_none_at_album_end() {
    // Window already touches the end of the album.
    assert!(compute_extended_thumb_window(1, 90, 100, 100, 10).is_none());
}

#[test]
fn extend_at_window_cap_slides_left_without_growing_live_items() {
    let (new_start, new_end) =
        compute_extended_thumb_window(-1, 50, 90, 100, THUMB_WINDOW_MAX as usize).unwrap();

    assert_eq!(new_start, 50 - THUMB_LAZY_HALF);
    assert_eq!(new_end, 90 - THUMB_LAZY_HALF);
    assert_eq!(new_end - new_start, THUMB_WINDOW_MAX);
}

#[test]
fn extend_at_window_cap_slides_right_without_growing_live_items() {
    let (new_start, new_end) =
        compute_extended_thumb_window(1, 50, 90, 100, THUMB_WINDOW_MAX as usize).unwrap();

    assert_eq!(new_start, 50 + THUMB_LAZY_HALF);
    assert_eq!(new_end, 90 + THUMB_LAZY_HALF);
    assert_eq!(new_end - new_start, THUMB_WINDOW_MAX);
}

#[test]
fn extend_left_clamps_to_zero_not_negative() {
    // start is small but non-zero -> new_start must not underflow.
    let (new_start, _) = compute_extended_thumb_window(-1, 2, 12, 100, 10).unwrap();
    assert_eq!(new_start, 0);
}

#[test]
fn extend_right_clamps_to_n_items() {
    let (_, new_end) = compute_extended_thumb_window(1, 92, 99, 100, 10).unwrap();
    assert_eq!(new_end, 100);
}

#[test]
fn current_near_right_edge_triggers_right_thumb_extend() {
    assert_eq!(
        compute_current_thumb_extend_direction(7, 0, 11, 100, 11),
        Some(1),
        "current at offset 7 leaves only 3 thumbnails to the right, so preload more"
    );
}

#[test]
fn current_near_left_edge_triggers_left_thumb_extend() {
    assert_eq!(
        compute_current_thumb_extend_direction(23, 20, 31, 100, 11),
        Some(-1),
        "current at offset 3 leaves only 3 thumbnails to the left, so preload more"
    );
}

#[test]
fn current_in_middle_does_not_extend_thumb_window() {
    assert_eq!(
        compute_current_thumb_extend_direction(50, 45, 56, 100, 11),
        None
    );
}

#[test]
fn current_edge_extend_respects_album_edges_and_window_cap() {
    assert_eq!(
        compute_current_thumb_extend_direction(2, 0, 11, 100, 11),
        None,
        "near the left edge cannot extend when already at album start"
    );
    assert_eq!(
        compute_current_thumb_extend_direction(97, 89, 100, 100, 11),
        None,
        "near the right edge cannot extend when already at album end"
    );
}

#[test]
fn current_edge_extend_continues_at_window_cap_by_sliding_window() {
    assert_eq!(
        compute_current_thumb_extend_direction(86, 50, 90, 100, THUMB_WINDOW_MAX as usize),
        Some(1),
        "window cap should not make the carousel feel paged when more items exist to the right"
    );
    assert_eq!(
        compute_current_thumb_extend_direction(53, 50, 90, 100, THUMB_WINDOW_MAX as usize),
        Some(-1),
        "window cap should not make the carousel feel paged when more items exist to the left"
    );
}

#[test]
fn initial_window_total_item_count_matches_docstring() {
    // Regression: the fallback count remains 11 when no viewport
    // allocation is available yet.
    for n in [11u32, 100, 1000] {
        let current = n / 2;
        let (start, end) = compute_initial_thumb_window(current, n);
        let actual = end - start;
        assert!(actual <= THUMB_DEFAULT_WINDOW_LEN, "n={n} actual={actual}");
    }
}

#[test]
fn residual_centres_first_thumbnail_without_layout_padding() {
    let (value, residual) =
        compute_thumb_scroll_and_residual(0.0, SCROLL_BTN_W, SCROLL_PAGE_SIZE, SCROLL_UPPER);
    assert_eq!(value, 0.0);
    assert!(residual > 0.0);
    assert!(
        (0.0 + SCROLL_BTN_W / 2.0 - value + residual - SCROLL_PAGE_SIZE / 2.0).abs() < 0.5,
        "first thumbnail center should align with viewport center without layout padding"
    );
}

#[test]
fn residual_is_suppressed_when_content_does_not_exceed_viewport() {
    let (value, residual) =
        compute_thumb_scroll_and_residual(0.0, SCROLL_BTN_W, SCROLL_PAGE_SIZE, SCROLL_PAGE_SIZE);
    assert_eq!(value, 0.0);
    assert_eq!(residual, 0.0);
}

#[test]
fn visual_transform_uses_css_offset_when_adjustment_has_no_scroll_range() {
    assert_eq!(
        compute_thumb_visual_transform(240.0, 0.0, 300.0, 300.0),
        -240.0
    );
}

#[test]
fn visual_transform_uses_only_residual_when_adjustment_can_scroll() {
    assert_eq!(
        compute_thumb_visual_transform(240.0, 12.0, 720.0, 300.0),
        12.0
    );
}

#[test]
fn animated_scroll_value_eases_between_current_and_target() {
    assert_eq!(
        compute_thumb_animated_scroll_value(100.0, 260.0, 0.0),
        100.0
    );
    assert_eq!(
        compute_thumb_animated_scroll_value(100.0, 260.0, 1.0),
        260.0
    );

    let halfway = compute_thumb_animated_scroll_value(100.0, 260.0, 0.5);
    assert!(halfway > 100.0);
    assert!(halfway < 260.0);
    assert!(
        halfway > 180.0,
        "ease-out should move past the linear midpoint by halfway through the animation"
    );
}

#[test]
fn positioning_centres_current_when_content_is_narrower_than_viewport() {
    let (target, residual, transform) =
        compute_thumb_positioning(-10.0, 56.0, 7081.0, 7081.0, 2826.0);
    assert_eq!(target, 0.0);
    assert_eq!(residual, transform);
    assert!(
        (-10.0 + 56.0 / 2.0 + transform - 7081.0 / 2.0).abs() < 0.5,
        "current thumbnail should be visually centred even when the loaded strip is narrower than the viewport"
    );
}

#[test]
fn filmstrip_thumbnail_width_is_clamped_to_reasonable_aspect_ratio() {
    assert_eq!(clamped_thumb_width_for_texture(2100, 900), 131);
    assert_eq!(clamped_thumb_width_for_texture(4200, 900), 131);
    assert_eq!(clamped_thumb_width_for_texture(900, 2100), 36);
    assert_eq!(clamped_thumb_width_for_texture(900, 900), 56);
    assert_eq!(clamped_thumb_width_for_texture(0, 900), 36);
}

#[test]
fn filmstrip_placeholder_width_uses_media_dimensions_when_available() {
    assert_eq!(thumb_width_for_media_dimensions(Some(1170), Some(250)), 131);
    assert_eq!(thumb_width_for_media_dimensions(Some(1224), Some(824)), 83);
    assert_eq!(thumb_width_for_media_dimensions(Some(228), Some(256)), 50);
    assert_eq!(
        thumb_width_for_media_dimensions(None, Some(824)),
        THUMB_MIN_WIDTH
    );
    assert_eq!(
        thumb_width_for_media_dimensions(Some(1224), None),
        THUMB_MIN_WIDTH
    );
}

#[test]
fn item_geometry_uses_sequence_when_current_allocation_x_is_stale() {
    let widths = [118.0, 118.0, 118.0, 118.0, 112.0, 118.0, 118.0];
    let (content_x, width, content_width) =
        thumb_item_content_geometry(&widths, 4, THUMB_STRIP_SPACING).unwrap();

    assert_eq!(
        content_x,
        THUMB_EDGE_INSET + 4.0 * (118.0 + THUMB_STRIP_SPACING)
    );
    assert_eq!(width, 112.0);
    assert_eq!(
        content_width,
        THUMB_EDGE_INSET * 2.0
            + widths.iter().sum::<f64>()
            + (widths.len() - 1) as f64 * THUMB_STRIP_SPACING
    );

    let (_, _, transform) = compute_thumb_positioning(content_x, width, 1140.0, 1140.0, 1140.0);
    assert!(
        transform.abs() < 100.0,
        "current index 4 must not be positioned from a stale allocation x near the left edge"
    );
}

#[test]
fn item_geometry_includes_filmstrip_edge_inset() {
    let widths = [60.0, 60.0, 60.0];
    let (content_x, _, content_width) =
        thumb_item_content_geometry(&widths, 0, THUMB_STRIP_SPACING).unwrap();

    assert_eq!(content_x, THUMB_EDGE_INSET);
    assert_eq!(
        content_width,
        THUMB_EDGE_INSET * 2.0
            + widths.iter().sum::<f64>()
            + (widths.len() - 1) as f64 * THUMB_STRIP_SPACING
    );
}

#[test]
fn item_geometry_rejects_transient_tiny_allocations_after_rebuild() {
    let widths = [2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 28.0, 2.0, 2.0];

    assert!(
        thumb_item_content_geometry(&widths, 6, THUMB_STRIP_SPACING).is_none(),
        "2px allocations logged immediately after rebuild are not stable enough for centering"
    );
}

#[test]
fn item_geometry_accepts_small_loaded_thumbnail_allocations() {
    let widths = [38.0, 84.0, 110.0, 84.0, 38.0];
    let (content_x, width, _) =
        thumb_item_content_geometry(&widths, 2, THUMB_STRIP_SPACING).unwrap();

    assert_eq!(
        content_x,
        THUMB_EDGE_INSET + 38.0 + THUMB_STRIP_SPACING + 84.0 + THUMB_STRIP_SPACING
    );
    assert_eq!(width, 110.0);
}

#[test]
fn scroll_value_centres_middle_thumbnail_without_residual() {
    let middle_btn_x = 5.0 * (SCROLL_BTN_W + SCROLL_SPACING);
    let (value, residual) = compute_thumb_scroll_and_residual(
        middle_btn_x,
        SCROLL_BTN_W,
        SCROLL_PAGE_SIZE,
        SCROLL_UPPER,
    );
    assert_eq!(residual, 0.0);
    assert!(
        (middle_btn_x + SCROLL_BTN_W / 2.0 - value + residual - SCROLL_PAGE_SIZE / 2.0).abs() < 0.5,
        "middle thumbnail center should align with viewport center after scrolling"
    );
}

#[test]
fn residual_centres_last_thumbnail_without_layout_padding() {
    let last_btn_x = 10.0 * (SCROLL_BTN_W + SCROLL_SPACING);
    let (value, residual) =
        compute_thumb_scroll_and_residual(last_btn_x, SCROLL_BTN_W, SCROLL_PAGE_SIZE, SCROLL_UPPER);
    assert_eq!(value, SCROLL_UPPER - SCROLL_PAGE_SIZE);
    assert!(residual < 0.0);
    assert!(
        (last_btn_x + SCROLL_BTN_W / 2.0 - value + residual - SCROLL_PAGE_SIZE / 2.0).abs() < 0.5,
        "last thumbnail center should align with viewport center without layout padding"
    );
}

#[test]
fn thumb_centering_retries_until_allocation_is_ready() {
    assert!(
        should_retry_thumb_centering(false, 3),
        "initial viewer entry can run before thumbnail allocation; it must retry"
    );
    assert!(
        !should_retry_thumb_centering(true, 3),
        "successful centering should stop the tick callback"
    );
    assert!(
        !should_retry_thumb_centering(false, 0),
        "retry loop must have a hard stop"
    );
}

#[gtk::test]
fn thumb_strip_template_starts_without_layout_spacers() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    assert!(
        !viewer.imp().thumb_scrolled.get().propagates_natural_width(),
        "thumb scroller must not propagate the filmstrip child width into the viewer window"
    );
    let (hpolicy, vpolicy) = viewer.imp().thumb_scrolled.get().policy();
    assert_eq!(
        hpolicy,
        gtk::PolicyType::External,
        "thumb scroller needs a horizontal adjustment but no visible scrollbar; Never lets child width resize the viewer"
    );
    assert_eq!(
        vpolicy,
        gtk::PolicyType::Never,
        "thumb scroller should not expose vertical scrolling"
    );
    let bottom_bar = viewer
        .imp()
        .thumb_scrolled
        .get()
        .parent()
        .expect("thumb scroller should be inside viewer bottom bar");
    let bottom_bar_classes = bottom_bar.css_classes();
    assert!(
        bottom_bar_classes
            .iter()
            .any(|class| class == "viewer-thumb-bar"),
        "thumb scroller parent should keep the viewer-thumb-bar layout class"
    );
    assert!(
        bottom_bar_classes
            .iter()
            .any(|class| class == "glass-raised"),
        "carousel thumbnail strip should reuse the shared raised glass material"
    );
    assert!(
        bottom_bar_classes
            .iter()
            .any(|class| class == "viewer-thumb-carousel"),
        "carousel thumbnail strip should expose a dedicated class for edge-fade styling"
    );
    assert!(
        viewer
            .imp()
            .thumb_strip
            .get()
            .css_classes()
            .iter()
            .any(|class| class == "viewer-thumb-strip"),
        "thumb_strip must carry viewer-thumb-strip so CSS can suppress natural-width growth"
    );

    let strip = viewer.imp().thumb_strip.get();
    let mut count = 0;
    let mut child = strip.first_child();
    while let Some(widget) = child {
        count += 1;
        child = widget.next_sibling();
    }

    assert_eq!(
        count, 0,
        "thumb_strip must not contain template spacer children because viewport-sized children feed back into ScrolledWindow allocation"
    );
}
