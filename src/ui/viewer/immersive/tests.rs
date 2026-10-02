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
    page.note_immersive_activity();
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
    page.note_immersive_activity();
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
