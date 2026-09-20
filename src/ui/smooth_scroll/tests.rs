use super::*;

const WIDGET_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Virtual 60fps frame clock step (us) for deterministic glide driving.
const FRAME_STEP_US: i64 = 16_667;

fn build_scrollable_window(content_height: i32) -> (gtk::Window, gtk::ScrolledWindow) {
    let _ = gtk::init();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_height_request(content_height);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&content));
    scroller.set_vexpand(true);
    let window = gtk::Window::builder()
        .default_width(900)
        .default_height(500)
        .child(&scroller)
        .build();
    window.present();
    (window, scroller)
}

/// Pump the default main context until `predicate` holds. The one-shot
/// timeout source keeps `iteration(true)` from blocking forever once events
/// stop flowing.
fn pump_until(mut predicate: impl FnMut() -> bool) -> bool {
    let context = glib::MainContext::default();
    let deadline_reached = std::rc::Rc::new(std::cell::Cell::new(false));
    let deadline_callback = deadline_reached.clone();
    glib::timeout_add_local_once(WIDGET_TEST_TIMEOUT, move || {
        deadline_callback.set(true);
    });
    while !deadline_reached.get() && !predicate() {
        context.iteration(true);
    }
    predicate()
}

/// Wait for the fixture's first allocation and return its viewport page size.
fn wait_for_scrollable_allocation(scroller: &gtk::ScrolledWindow) -> f64 {
    let adjustment = scroller.vadjustment();
    assert!(
        pump_until(|| adjustment.upper() > adjustment.page_size()),
        "test scroller must receive a scrollable allocation"
    );
    adjustment.page_size()
}

/// Drive the glide on a synthetic 60fps frame clock until it rests (or
/// `max_frames` pass), returning the rested adjustment value. Deterministic:
/// no dependency on the real frame clock, which stalls under xvfb.
fn run_glide_to_rest(
    smooth: &SmoothScroller,
    scroller: &gtk::ScrolledWindow,
    max_frames: usize,
) -> f64 {
    let mut frame_time_us: i64 = 1_000_000;
    for _ in 0..max_frames {
        if let glib::ControlFlow::Break = smooth.glide_frame(scroller, frame_time_us) {
            break;
        }
        frame_time_us += FRAME_STEP_US;
    }
    scroller.vadjustment().value()
}

#[test]
fn wheel_step_follows_gtk_detent_formula_with_vscode_floor() {
    // Small viewports clamp to the 125px floor.
    assert_eq!(wheel_step_for_page(0.0), WHEEL_MIN_NOTCH_SCROLL_PX);
    assert_eq!(wheel_step_for_page(400.0), WHEEL_MIN_NOTCH_SCROLL_PX);
    assert_eq!(wheel_step_for_page(1000.0), WHEEL_MIN_NOTCH_SCROLL_PX);
    // Large viewports keep GTK's scaled step once it exceeds the floor.
    // Exact cubes keep the expected powers exact: 20^3 -> 20^2.
    assert!((wheel_step_for_page(8000.0) - 400.0).abs() < 1e-9);
    // The crossover sits at page = 125^1.5 ≈ 1397.5.
    assert!(wheel_step_for_page(1500.0) > WHEEL_MIN_NOTCH_SCROLL_PX);
    // Monotone above the floor (below it every page clamps to the same 125).
    assert!(wheel_step_for_page(8000.0) < wheel_step_for_page(8800.0));
}

#[test]
fn accumulated_target_adds_notches_in_burst_direction() {
    let (lower, max_value) = (0.0, 10_000.0);
    assert_eq!(
        accumulated_wheel_target(500.0, 1.0, 93.0, lower, max_value),
        593.0
    );
    assert_eq!(
        accumulated_wheel_target(500.0, -1.0, 93.0, lower, max_value),
        407.0
    );
    // Hi-res wheels emit fractional wheel-unit deltas.
    assert_eq!(
        accumulated_wheel_target(500.0, 0.5, 93.0, lower, max_value),
        546.5
    );
    // A burst compounds from its accumulated target, not from the viewport.
    let first = accumulated_wheel_target(500.0, 1.0, 93.0, lower, max_value);
    assert_eq!(
        accumulated_wheel_target(first, 1.0, 93.0, lower, max_value),
        686.0
    );
}

#[test]
fn accumulated_target_clamps_to_adjustment_bounds() {
    assert_eq!(
        accumulated_wheel_target(5_000.0, 40.0, 93.0, 0.0, 5_000.0),
        5_000.0
    );
    assert_eq!(
        accumulated_wheel_target(50.0, -40.0, 93.0, 0.0, 5_000.0),
        0.0
    );
    // Degenerate bounds (max < lower) must not panic.
    assert_eq!(accumulated_wheel_target(10.0, 1.0, 93.0, 50.0, 20.0), 50.0);
}

#[test]
fn eased_scroll_value_matches_vscode_ease_out_cubic() {
    assert_eq!(eased_scroll_value(120.0, 320.0, 0.0), 120.0);
    // Lands exactly on the target at completion.
    assert_eq!(eased_scroll_value(120.0, 320.0, 1.0), 320.0);
    // Progress is clamped, so overshooting time still snaps to the target.
    assert_eq!(eased_scroll_value(120.0, 320.0, 2.0), 320.0);
    assert_eq!(eased_scroll_value(120.0, 320.0, -1.0), 120.0);
    // Ease-out cubic at the midpoint covers 1 - (1 - 0.5)^3 = 0.875.
    let mid = eased_scroll_value(120.0, 320.0, 0.5);
    assert!((mid - 295.0).abs() < 1e-9);

    let mut previous = 120.0;
    for step in 1..=5 {
        let next = eased_scroll_value(120.0, 320.0, f64::from(step) * 0.2);
        assert!(next > previous, "the ease must be monotone");
        previous = next;
    }

    // Works when the target lies below the current value.
    let downward = eased_scroll_value(320.0, 120.0, 0.5);
    assert!((downward - 145.0).abs() < 1e-9);
}

#[test]
fn momentum_impulse_travels_one_detent_step() {
    // 8000 crosses the 125px floor so the scaled branch is covered too.
    for page_size in [400.0, 700.0, 1200.0, 8000.0] {
        let distance = momentum_impulse_for_page(page_size) * (MOMENTUM_FAST_TAU_MS / 1000.0);
        assert!(
            (distance - wheel_step_for_page(page_size)).abs() < 1e-9,
            "a single notch must travel exactly one detent step"
        );
    }
}

#[test]
fn momentum_tau_grows_with_speed_and_saturates() {
    let impulse = momentum_impulse_for_page(700.0);
    assert_eq!(
        momentum_tau_ms(0.0, impulse, 300.0),
        MOMENTUM_FAST_TAU_MS,
        "slow scrolling decays at the fast constant"
    );
    assert_eq!(
        momentum_tau_ms(impulse * MOMENTUM_SLOW_GAIN_END, impulse, 300.0),
        300.0
    );
    assert_eq!(
        momentum_tau_ms(impulse * 100.0, impulse, 300.0),
        300.0,
        "the slow regime saturates instead of growing forever"
    );
    let mid = momentum_tau_ms(impulse * 2.0, impulse, 300.0);
    assert!(mid > MOMENTUM_FAST_TAU_MS && mid < 300.0);
    // A degenerate impulse stays in the fast regime instead of dividing by
    // zero.
    assert_eq!(momentum_tau_ms(500.0, 0.0, 300.0), MOMENTUM_FAST_TAU_MS);
}

#[gtk::test]
fn install_adds_one_capture_vertical_scroll_controller() {
    let (window, scroller) = build_scrollable_window(2000);
    SmoothScroller::install(&scroller);

    let controllers: Vec<gtk::EventControllerScroll> = scroller
        .observe_controllers()
        .snapshot()
        .into_iter()
        .filter_map(|controller| controller.downcast::<gtk::EventControllerScroll>().ok())
        .filter(|controller| controller.name().as_deref() == Some("photo-viewer-smooth-scroll"))
        .collect();
    assert_eq!(
        controllers.len(),
        1,
        "install() adds exactly one named smooth-scroll controller"
    );
    assert_eq!(
        controllers[0].propagation_phase(),
        gtk::PropagationPhase::Capture
    );
    assert_eq!(
        controllers[0].flags(),
        gtk::EventControllerScrollFlags::VERTICAL
    );
    window.close();
}

#[gtk::test]
fn momentum_notch_glides_one_detent_step_and_rests() {
    let (window, scroller) = build_scrollable_window(2000);
    let page_size = wait_for_scrollable_allocation(&scroller);
    let step = wheel_step_for_page(page_size);
    let adjustment = scroller.vadjustment();

    let smooth = SmoothScroller::new(&scroller);
    // Pin the browser-referenced default explicitly; it is the shipped feel.
    smooth.set_momentum_glide(300.0);
    smooth.handle_wheel_delta(1.0);
    assert!(
        adjustment.value() < 1.0,
        "a consumed notch must not teleport the viewport"
    );

    // The exact per-frame integral telescopes to one detent step; only the
    // sub-cutoff velocity tail goes untraveled.
    let rested = run_glide_to_rest(&smooth, &scroller, 600);
    assert!(
        (rested - step).abs() <= 1.0,
        "momentum glide should settle one detent step away: rested={} step={}",
        rested,
        step
    );
    // At rest the glide stays down and nothing further moves the viewport.
    assert!(matches!(
        smooth.glide_frame(&scroller, 20_000_000),
        glib::ControlFlow::Break
    ));
    assert!((adjustment.value() - rested).abs() < 0.01);
    window.close();
}

#[gtk::test]
fn eased_burst_accumulates_exact_notch_sum() {
    let (window, scroller) = build_scrollable_window(2000);
    let page_size = wait_for_scrollable_allocation(&scroller);
    let step = wheel_step_for_page(page_size);

    let smooth = SmoothScroller::new(&scroller);
    smooth.handle_wheel_delta(1.0);
    // The head start makes the very first frame move: at 10ms of a 125ms
    // window the ease already covers ~22% of the distance.
    assert!(matches!(
        smooth.glide_frame(&scroller, 1_000_000),
        glib::ControlFlow::Continue
    ));
    assert!(
        scroller.vadjustment().value() > 1.0,
        "the first frame must show movement, not a dead frame"
    );
    smooth.handle_wheel_delta(1.0);

    let expected = 2.0 * step;
    let rested = run_glide_to_rest(&smooth, &scroller, 600);
    assert!(
        (rested - expected).abs() <= 0.5,
        "eased bursts land exactly on the notch sum: rested={} expected={}",
        rested,
        expected
    );
    window.close();
}

#[gtk::test]
fn up_and_down_notches_are_mirror_symmetric() {
    let (window, scroller) = build_scrollable_window(4000);
    let page_size = wait_for_scrollable_allocation(&scroller);
    let step = wheel_step_for_page(page_size);
    let adjustment = scroller.vadjustment();

    // Start deep enough that neither adjustment bound can interfere.
    let origin = 1500.0;
    adjustment.set_value(origin);

    let smooth = SmoothScroller::new(&scroller);

    // Record the per-frame values of a single downward notch.
    smooth.handle_wheel_delta(1.0);
    let mut down_curve = Vec::new();
    let mut frame_time_us: i64 = 1_000_000;
    for _ in 0..600 {
        if let glib::ControlFlow::Break = smooth.glide_frame(&scroller, frame_time_us) {
            break;
        }
        down_curve.push(adjustment.value());
        frame_time_us += FRAME_STEP_US;
    }
    assert!(
        down_curve.len() > 2,
        "a notch must animate over several frames"
    );
    assert!(
        (adjustment.value() - (origin + step)).abs() <= 0.5,
        "a down notch must land one step below the origin: value={}",
        adjustment.value()
    );

    // A single upward notch from the landing position must mirror the down
    // curve frame for frame: value(t) = 2*origin + step - down_curve(t).
    adjustment.set_value(origin + step);
    smooth.handle_wheel_delta(-1.0);
    let mut frame_time_us: i64 = 1_000_000;
    let mut frames = 0;
    for _ in 0..600 {
        if let glib::ControlFlow::Break = smooth.glide_frame(&scroller, frame_time_us) {
            break;
        }
        let mirrored = 2.0 * origin + step - down_curve[frames];
        assert!(
            (adjustment.value() - mirrored).abs() < 1e-6,
            "up frame {} must mirror the down curve: value={} mirrored={}",
            frames,
            adjustment.value(),
            mirrored
        );
        frames += 1;
        frame_time_us += FRAME_STEP_US;
    }
    assert_eq!(
        frames,
        down_curve.len(),
        "both directions must need the same number of frames"
    );
    assert!(
        (adjustment.value() - origin).abs() <= 0.5,
        "an up notch must land one step above its start: value={}",
        adjustment.value()
    );
    window.close();
}

#[gtk::test]
fn momentum_burst_carries_beyond_notch_sum() {
    // Taller content than the other fixtures: a 5-notch momentum burst can
    // carry thousands of pixels and must not hit the adjustment edge.
    let (window, scroller) = build_scrollable_window(6000);
    let page_size = wait_for_scrollable_allocation(&scroller);
    let step = wheel_step_for_page(page_size);

    let smooth = SmoothScroller::new(&scroller);
    // Pin the browser-referenced default explicitly; it is the shipped feel.
    smooth.set_momentum_glide(300.0);
    // Five synchronous notches stack velocity deep into the slow regime.
    for _ in 0..5 {
        smooth.handle_wheel_delta(1.0);
    }

    let notch_sum = 5.0 * step;
    let rested = run_glide_to_rest(&smooth, &scroller, 600);
    assert!(
        rested > notch_sum * 1.6,
        "rapid notches must stack into a longer carry: rested={} notch_sum={}",
        rested,
        notch_sum
    );
    assert!(
        rested < notch_sum * 4.5,
        "the browser-referenced default must stay restrained: rested={} notch_sum={}",
        rested,
        notch_sum
    );
    window.close();
}

#[gtk::test]
fn external_adjustment_write_cancels_glide() {
    let (window, scroller) = build_scrollable_window(2000);
    wait_for_scrollable_allocation(&scroller);
    let adjustment = scroller.vadjustment();

    let smooth = SmoothScroller::new(&scroller);
    smooth.handle_wheel_delta(1.0);
    let moving = run_glide_to_rest(&smooth, &scroller, 3);
    assert!(
        moving > 1.0 && moving < 300.0,
        "the glide should be under way, not finished: value={}",
        moving
    );

    adjustment.set_value(300.0);
    let rested = run_glide_to_rest(&smooth, &scroller, 30);
    assert!(
        (rested - 300.0).abs() <= 0.5,
        "an external write must cancel the glide: rested={}",
        rested
    );
    window.close();
}

#[gtk::test]
fn wheel_delta_without_scrollable_range_is_inert() {
    let (window, scroller) = build_scrollable_window(100);
    let adjustment = scroller.vadjustment();
    let _ = pump_until(|| adjustment.upper() > 0.0);

    let smooth = SmoothScroller::new(&scroller);
    smooth.handle_wheel_delta(1.0);
    smooth.handle_wheel_delta(-1.0);

    let rested = run_glide_to_rest(&smooth, &scroller, 30);
    assert!(
        rested.abs() <= 0.5,
        "a scroller without overflow must stay put: rested={}",
        rested
    );
    window.close();
}

#[gtk::test]
fn handle_scroll_without_a_current_event_proceeds() {
    let (window, scroller) = build_scrollable_window(2000);
    let smooth = SmoothScroller::new(&scroller);
    // A controller with no current event (never attached, or queried outside
    // an emission) must fall through to native handling.
    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    assert!(matches!(
        smooth.handle_scroll(&controller, 1.0),
        glib::Propagation::Proceed
    ));
    assert!(
        scroller.vadjustment().value() < 0.5,
        "no glide may start without a real wheel event"
    );
    window.close();
}
