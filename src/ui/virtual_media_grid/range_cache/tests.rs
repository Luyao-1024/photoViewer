use super::*;

#[test]
fn adjacent_resident_ranges_are_merged() {
    let mut cache = RangeCoordinator::default();
    cache.mark_resident(MediaRange::new(10, 20));
    cache.mark_resident(MediaRange::new(20, 30));
    cache.mark_resident(MediaRange::new(4, 8));

    assert_eq!(
        cache.resident(),
        &[MediaRange::new(4, 8), MediaRange::new(10, 30)]
    );
}

#[test]
fn request_applies_in_flight_overlap_before_retargeting_latest_intent() {
    let mut cache = RangeCoordinator::default();
    let first = match cache.request(MediaRange::new(0, 20)) {
        RequestDisposition::Started(request) => request,
        disposition => panic!("unexpected disposition: {disposition:?}"),
    };
    assert_eq!(
        cache.request(MediaRange::new(80, 100)),
        RequestDisposition::Coalesced
    );

    let completion = cache.finish(first.generation);
    assert!(completion.apply_result);
    assert_eq!(
        completion.next,
        Some(RangeRequest {
            generation: 1,
            range: MediaRange::new(80, 100),
        })
    );

    cache.mark_resident(first.range);
    assert_eq!(
        cache.request(completion.next.unwrap().range),
        RequestDisposition::Started(RangeRequest {
            generation: 2,
            range: MediaRange::new(80, 100),
        })
    );
}

#[test]
fn retarget_after_in_flight_landing_requests_only_the_uncovered_tail() {
    let mut cache = RangeCoordinator::default();
    let first = match cache.request(MediaRange::new(0, 48)) {
        RequestDisposition::Started(request) => request,
        disposition => panic!("unexpected disposition: {disposition:?}"),
    };
    assert_eq!(
        cache.request(MediaRange::new(3, 51)),
        RequestDisposition::Coalesced
    );

    let completion = cache.finish(first.generation);
    assert!(completion.apply_result);
    cache.mark_resident(first.range);
    assert_eq!(
        cache.request(completion.next.unwrap().range),
        RequestDisposition::Started(RangeRequest {
            generation: 2,
            range: MediaRange::new(48, 51),
        })
    );
}

#[test]
fn covered_range_does_not_start_a_second_query() {
    let mut cache = RangeCoordinator::default();
    cache.mark_resident(MediaRange::new(10, 40));

    assert_eq!(
        cache.request(MediaRange::new(12, 30)),
        RequestDisposition::Covered
    );
    assert_eq!(cache.in_flight(), None);
}

#[test]
fn shifted_range_requests_only_the_missing_trailing_edge() {
    let mut cache = RangeCoordinator::default();
    cache.mark_resident(MediaRange::new(0, 48));

    assert_eq!(
        cache.request(MediaRange::new(3, 51)),
        RequestDisposition::Started(RangeRequest {
            generation: 1,
            range: MediaRange::new(48, 51),
        })
    );
}

#[test]
fn shifted_range_requests_only_the_missing_leading_edge() {
    let mut cache = RangeCoordinator::default();
    cache.mark_resident(MediaRange::new(3, 51));

    assert_eq!(
        cache.request(MediaRange::new(0, 48)),
        RequestDisposition::Started(RangeRequest {
            generation: 1,
            range: MediaRange::new(0, 3),
        })
    );
}

#[test]
fn invalidate_makes_in_flight_result_stale() {
    let mut cache = RangeCoordinator::default();
    let request = match cache.request(MediaRange::new(0, 20)) {
        RequestDisposition::Started(request) => request,
        disposition => panic!("unexpected disposition: {disposition:?}"),
    };
    cache.invalidate();

    assert!(!cache.finish(request.generation).apply_result);
    assert!(cache.resident().is_empty());
}

#[test]
fn directional_overscan_favors_scroll_direction() {
    let visible = MediaRange::new(50, 60);
    assert_eq!(
        expanded_visible_range(visible, 100, true),
        MediaRange::new(40, 80)
    );
    assert_eq!(
        expanded_visible_range(visible, 100, false),
        MediaRange::new(30, 70)
    );
}
