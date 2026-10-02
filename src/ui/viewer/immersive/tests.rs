use super::super::test_support::*;
use super::super::*;
use super::*;
use gtk4 as gtk;
use gtk4::{gio, glib};

fn viewer() -> ViewerPage {
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    ViewerPage::new(media_list, 0)
}

/// The two refusals are the whole safety story of this feature: chrome that
/// disappears under someone mid-stroke, or a panel that folds away while it is
/// being read.
#[test]
fn immersive_is_refused_while_the_user_is_working_in_chrome() {
    assert!(immersive_allowed(false, false));
    assert!(
        !immersive_allowed(true, false),
        "editing must keep its header and sliders; an auto-hide mid-stroke loses them"
    );
    assert!(
        !immersive_allowed(false, true),
        "an open details or editor panel is chrome the user is looking at"
    );
    assert!(!immersive_allowed(true, true));
}

#[gtk::test]
fn entering_immersive_folds_chrome_and_leaving_restores_it() {
    init_viewer_test();
    let page = viewer();
    let imp = page.imp();
    assert!(
        imp.header_revealer.get().reveals_child()
            && imp.filmstrip_revealer.get().reveals_child()
            && imp.nav_buttons_revealer.get().reveals_child()
            && imp.zoom_controls_revealer.get().reveals_child(),
        "the viewer starts with its chrome present — immersion is opt-in"
    );

    page.toggle_immersive();
    assert!(page.is_immersive());
    assert!(
        !imp.header_revealer.get().reveals_child(),
        "the first thing F does is give the picture the header's rows"
    );

    // Stillness, not the press, is what hides it: activity brings it straight
    // back while immersion stays armed.
    page.note_immersive_activity("test");
    assert!(
        imp.header_revealer.get().reveals_child(),
        "moving the pointer must return the chrome immediately"
    );
    assert!(
        page.is_immersive(),
        "revealing the chrome is not the same as leaving immersive mode"
    );

    page.set_immersive(false);
    assert!(!page.is_immersive());
    assert!(
        imp.header_revealer.get().reveals_child()
            && imp.filmstrip_revealer.get().reveals_child()
            && imp.nav_buttons_revealer.get().reveals_child()
            && imp.zoom_controls_revealer.get().reveals_child(),
        "leaving immersive must restore every region, so no state hides chrome forever"
    );
}

#[gtk::test]
fn an_open_panel_refuses_immersive_instead_of_folding_away_behind_it() {
    init_viewer_test();
    let page = viewer();
    let imp = page.imp();
    imp.details_split_view.get().set_show_sidebar(true);
    page.toggle_immersive();
    assert!(
        !page.is_immersive(),
        "details is open, so F must not fold the chrome away behind it"
    );
    assert!(imp.header_revealer.get().reveals_child());
}

/// A stillness clock has to measure stillness.
///
/// GDK keeps delivering motion events for a pointer that has not gone anywhere,
/// because folding the chrome re-targets what sits under it. Counting each of
/// those as "the user moved" cancelled and re-armed the pending timer on every
/// event, so it could never reach its deadline: the chrome was pinned open for
/// as long as the pointer rested anywhere on the page, and immersive browsing
/// read as a flicker instead of a fold.
#[gtk::test]
fn a_pointer_that_did_not_move_is_not_activity() {
    init_viewer_test();
    let page = viewer();
    let imp = page.imp();
    let motion = page
        .observe_controllers()
        .snapshot()
        .into_iter()
        .find_map(|controller| controller.downcast::<gtk::EventControllerMotion>().ok())
        .expect("the page should watch pointer motion");

    // The user moved the pointer before pressing F, so the watcher has a
    // baseline to compare against — the real sequence, and the reason a resting
    // pointer's event storm can be told apart from an actual move.
    motion.emit_by_name::<()>("motion", &[&10.0f64, &20.0f64]);

    page.set_immersive(true);
    let pending_timer_id = || {
        imp.immersive_idle_source
            .borrow()
            .as_ref()
            .map(glib::SourceId::as_raw)
    };
    let armed_id = pending_timer_id();

    // The same coordinates, over and over — the shape of a resting pointer.
    for _ in 0..8 {
        motion.emit_by_name::<()>("motion", &[&10.0f64, &20.0f64]);
    }
    assert!(
        !imp.header_revealer.get().reveals_child(),
        "a pointer that did not move must leave the folded chrome folded"
    );
    assert_eq!(
        pending_timer_id(),
        armed_id,
        "a pointer that did not move must not re-arm the stillness timer, or the timer can never \
         reach its deadline and the chrome never actually folds"
    );

    // A real move still brings the chrome back and restarts the countdown.
    motion.emit_by_name::<()>("motion", &[&40.0f64, &20.0f64]);
    assert!(
        imp.header_revealer.get().reveals_child(),
        "moving the pointer must bring the chrome back"
    );
    assert_ne!(
        pending_timer_id(),
        armed_id,
        "a real move must re-arm the stillness timer, so the chrome folds again once the user stops"
    );

    // Sub-pixel steps accumulate: a slow drag has to keep registering even
    // though no single step crosses the threshold on its own.
    page.set_immersive(false);
    page.set_immersive(true);
    let slow_armed = pending_timer_id();
    for step in 1..=6 {
        let x = 40.0 + step as f64 * 0.4;
        motion.emit_by_name::<()>("motion", &[&x, &20.0f64]);
    }
    assert_ne!(
        pending_timer_id(),
        slow_armed,
        "a slow drag must still count as movement once it has travelled far enough, otherwise \
         filtering jitter would also filter a deliberate drag"
    );
}

/// The countdown is the only timer this page owns while immersed, and it must
/// not stack: every movement re-arms it, so a moving pointer never folds the
/// chrome and a still one folds it exactly once.
#[gtk::test]
fn activity_replaces_the_pending_stillness_timer_instead_of_stacking_one() {
    init_viewer_test();
    let page = viewer();
    let imp = page.imp();

    page.set_immersive(true);
    assert!(
        imp.immersive_idle_source.borrow().is_some(),
        "arming immersive mode schedules the stillness timer"
    );
    // GLib numbers sources from an increasing counter and never reuses an id, so
    // a different raw id is proof the pending one was replaced, not added to.
    let first = imp
        .immersive_idle_source
        .borrow()
        .as_ref()
        .map(glib::SourceId::as_raw);
    page.note_immersive_activity("test");
    let second = imp
        .immersive_idle_source
        .borrow()
        .as_ref()
        .map(glib::SourceId::as_raw);
    assert!(
        second.is_some() && second != first,
        "activity must replace the pending timer, not add a second one"
    );

    page.set_immersive(false);
    assert!(
        imp.immersive_idle_source.borrow().is_none(),
        "leaving immersive clears the countdown, so a stale timer cannot hide chrome later"
    );
}

#[gtk::test]
fn the_pointer_watcher_listens_on_the_page_not_only_the_image_stage() {
    init_viewer_test();
    let page = viewer();
    let on_page = page
        .observe_controllers()
        .snapshot()
        .iter()
        .any(|controller| {
            controller
                .downcast_ref::<gtk::EventControllerMotion>()
                .is_some()
        });
    assert!(
        on_page,
        "stillness has to be measured across the whole page: moving over the header is \
         activity too, and watching only the stage would fold the chrome the user aims at"
    );
    // The stage keeps exactly its input controllers: this adds no gesture that
    // could compete with the overlay buttons (see stage_input's own assertions).
    let on_overlay = page
        .imp()
        .image_overlay
        .get()
        .observe_controllers()
        .snapshot()
        .iter()
        .any(|controller| {
            controller
                .downcast_ref::<gtk::EventControllerMotion>()
                .is_some()
        });
    assert!(
        !on_overlay,
        "the watcher belongs to the page, not to the image stage"
    );
}
