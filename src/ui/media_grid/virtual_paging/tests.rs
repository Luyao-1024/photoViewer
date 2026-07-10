use super::super::*;
use super::*;

#[gtk::test]
fn virtual_placeholder_flow_renders_loading_tiles_immediately() {
    let _ = gtk::init();
    let spec = ViewSpec {
        mode: GroupBy::Day,
        pixel_size: 270,
        thumb_size: ThumbnailSize::Large,
    };
    let flow = build_virtual_placeholder_flow(spec, 12);

    assert!(flow.has_css_class("virtual-placeholder-grid"));
    assert_eq!(flow.selection_mode(), gtk::SelectionMode::None);
    let mut count = 0;
    let mut child = flow.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        let tile = widget
            .downcast::<gtk::FlowBoxChild>()
            .ok()
            .and_then(|child| child.child())
            .and_then(|child| child.downcast::<SquareTile>().ok())
            .expect("placeholder flow children should wrap SquareTile");
        assert_eq!(tile.target(), 270);
        assert!(tile.has_css_class("thumb-loading"));
        assert!(tile.has_css_class("thumb-placeholder"));
        count += 1;
        child = next;
    }
    assert_eq!(count, 12);
}

#[test]
fn virtual_scroll_ratio_maps_to_full_library_offset() {
    assert_eq!(virtual_offset_for_ratio(0.0, 100_000, 500), 0);
    assert_eq!(virtual_offset_for_ratio(0.50, 100_000, 500), 50_000);
    assert_eq!(virtual_offset_for_ratio(0.99, 100_000, 500), 99_000);
    assert_eq!(virtual_offset_for_ratio(1.0, 100_000, 500), 99_500);
}

#[test]
fn virtual_scroll_prefetches_before_window_edge() {
    assert_eq!(
        virtual_page_start_for_offset(850, 0, 1_000, 100_000, 500),
        Some(600),
        "80%+ through the current window should prefetch ahead"
    );
    assert_eq!(
        virtual_page_start_for_offset(550, 0, 1_000, 100_000, 500),
        None,
        "middle of current window should not reload"
    );
    assert_eq!(
        virtual_page_start_for_offset(50_000, 0, 1_000, 100_000, 500),
        Some(49_750),
        "dragging the full-library scrollbar should jump near that global offset"
    );
    assert_eq!(
        virtual_page_start_for_offset(99_900, 99_000, 1_000, 100_000, 500),
        Some(99_500),
        "near the end should clamp to the last full page"
    );
}

#[test]
fn virtual_scroll_absolute_end_targets_last_page() {
    assert_eq!(
                virtual_page_start_for_offset(99_500, 0, 500, 100_000, 500),
                Some(99_500),
                "dragging to the absolute end must load the final page, not a centered window above bottom spacer"
            );
    assert_eq!(
        virtual_page_start_for_offset(99_593, 0, 500, 100_093, 500),
        Some(99_593),
        "non-page-aligned library totals must still land on the final partial boundary"
    );
}

#[test]
fn virtual_spacer_height_scales_with_unloaded_items() {
    let spec = ViewSpec {
        mode: GroupBy::Day,
        pixel_size: 270,
        thumb_size: ThumbnailSize::Large,
    };
    assert_eq!(virtual_spacer_height(0, 4, 1_000.0, spec), 0);
    assert!(
        virtual_spacer_height(1_000, 4, 1_000.0, spec)
            > virtual_spacer_height(100, 4, 1_000.0, spec)
    );
}

#[test]
fn virtual_loading_window_counts_placeholders_for_target_page() {
    assert_eq!(virtual_window_item_count(0, 100_000, 500), 500);
    assert_eq!(virtual_window_item_count(99_500, 100_000, 500), 500);
    assert_eq!(virtual_window_item_count(99_800, 100_000, 500), 200);
    assert_eq!(virtual_window_item_count(100_000, 100_000, 500), 0);
}

#[test]
fn scroll_ratio_tracks_latest_drag_value_while_page_is_loading() {
    assert_eq!(scroll_ratio_from_adjustment_value(0.0, 1_000.0, 100.0), 0.0);
    assert_eq!(
        scroll_ratio_from_adjustment_value(450.0, 1_000.0, 100.0),
        0.5
    );
    assert_eq!(
        scroll_ratio_from_adjustment_value(2_000.0, 1_000.0, 100.0),
        1.0
    );
    assert_eq!(scroll_ratio_from_adjustment_value(20.0, 100.0, 100.0), 0.0);
}

#[test]
fn programmatic_scroll_restore_does_not_request_virtual_page() {
    assert!(
        should_consider_virtual_page_load(false, 100_000, 500),
        "user-driven scrolling in a virtualized library should still consider page loads"
    );
    assert!(
        !should_consider_virtual_page_load(true, 100_000, 500),
        "restoring scroll after a rebuild must not recursively request another virtual page"
    );
    assert!(
        !should_consider_virtual_page_load(false, 500, 500),
        "fully loaded small libraries do not need virtual page loads"
    );
    assert!(
        !should_consider_virtual_page_load(false, 100_000, 0),
        "an empty current window cannot be used to target a virtual page"
    );
}

#[test]
fn coalesced_virtual_page_target_keeps_latest_drag_target() {
    let pending_start = std::cell::Cell::new(None);
    let pending_ratio = std::cell::Cell::new(None);

    replace_pending_virtual_page(&pending_start, &pending_ratio, 20_000, 0.20);
    replace_pending_virtual_page(&pending_start, &pending_ratio, 55_000, 0.55);

    assert_eq!(pending_start.get(), Some(55_000));
    assert_eq!(pending_ratio.get(), Some(0.55));
}
