//! Viewer UX runs driven by real pointer targeting.
//!
//! The chrome regression this file exists for was invisible to the rest of the
//! suite because every other viewer "click" test bypassed the input path
//! completely: `emit_by_name("clicked")` asks a button to run its handler no
//! matter what is painted on top of it. A `Gtk.Overlay` child that fills the
//! stage is a perfectly good hit-test target, so a real pointer press can land on
//! an invisible container, the button never runs, and every signal-level test
//! stays green.
//!
//! Each case therefore builds a real `MainWindow`, pushes a real `ViewerPage`
//! through the production `ViewerPage::new_for_query` entry point over a library
//! of real distinct JPEGs, and drives it through the shared
//! [`common::interaction::Ui`] harness: hit-test where the pointer lands, assert
//! it lands on the control the user aimed at, deliver press and release to the
//! gesture that control owns, then assert the user-visible result — the media id,
//! the render index, the header rank, the header date.
//!
//! GTK is single-threaded and must be initialized once per process, so the cases
//! run serially from one `#[test]`, the same pattern as `ux_click_flows.rs`.
mod common;

use common::interaction::Ui;
use common::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::identity::MediaId;
use photo_viewer::core::{db, MediaQuery};
use photo_viewer::ui::ViewerPage;
use std::time::Duration;

/// How many distinct real photos each run gets: enough to walk to the tail and
/// back, which is where the dimmed-arrow contract lives.
const PHOTO_COUNT: usize = 4;

#[test]
fn viewer_ux_runs_by_real_pointer() {
    gtk::init().expect("GTK init failed");
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime for viewer UX runs");
    let _runtime_guard = runtime.enter();

    let run = ViewerRun::build();
    overlay_nav_pair_walks_a_real_library(&run);
    run.finish();

    let run = ViewerRun::build();
    stage_center_is_never_swallowed_by_chrome(&run);
    run.finish();

    let run = ViewerRun::build();
    zoom_cluster_operates_the_stage_by_pointer(&run);
    run.finish();

    let run = ViewerRun::build();
    header_actions_reach_their_panels_and_mutations(&run);
    run.finish();

    let run = ViewerRun::build();
    chrome_returns_and_stays_clickable_after_immersive_fold(&run);
    run.finish();
}

/// A shell plus a `ViewerPage` pushed through the production query entry point.
struct ViewerRun {
    shell: Shell,
    viewer: ViewerPage,
}

impl ViewerRun {
    fn build() -> Self {
        let shell = Shell::with_photos(PHOTO_COUNT);
        let nav = shell.window.nav_view();

        // The production entry point: a query plus a stable media id. The legacy
        // index-only constructor leaves `media_query` unset, so neighbour prefetch
        // never runs and the end-of-library dimming — part of what these cases
        // assert — is never exercised.
        let viewer = ViewerPage::new_for_query(
            MediaQuery::LiveAll,
            MediaId::from(shell.items[0].id),
            shell.media_list.clone(),
        );
        viewer.set_edit_target(&nav, shell.pool.clone());
        viewer.set_db_actor(shell.db_actor.clone());
        viewer.set_thumbnail_loader(shell.loader.clone());
        viewer.show_at(0);
        nav.push(&viewer);

        let run = Self { shell, viewer };
        // Every contract here depends on real geometry, so wait for the chrome to
        // be laid out before the first hit test.
        run.wait_for_chrome_relayout();
        run
    }

    fn ui(&self) -> &Ui {
        &self.shell.ui
    }

    /// Tear the window down before the next run: leaving several shells mapped
    /// starves the frame clock, so later cases would depend on how many ran
    /// before them.
    fn finish(self) {
        self.ui().pump(Duration::from_millis(100));
        drop(self);
    }

    /// The immersive revealers animate open. A hit test taken mid-crossfade would
    /// measure a half-faded allocation, so let the chrome settle and re-measure.
    fn wait_for_chrome_relayout(&self) {
        let imp = self.viewer.imp();
        assert!(
            self.ui()
                .wait_until(Duration::from_secs(15), || imp.next_btn.get().is_mapped()
                    && imp.next_btn.get().width() > 0
                    && imp.zoom_in_btn.get().width() > 0
                    && imp.favorite_btn.get().height() > 0),
            "the viewer chrome should be laid out and clickable within 15s"
        );
        self.ui().pump(Duration::from_millis(400));
    }

    /// The four immersive regions' revealed state, in template order: header,
    /// filmstrip, navigation pair, zoom cluster. They are written by a single
    /// `set_chrome_revealed` call, so they can never legitimately disagree.
    fn chrome_regions(&self) -> [bool; 4] {
        let imp = self.viewer.imp();
        [
            imp.header_revealer.get().reveals_child(),
            imp.filmstrip_revealer.get().reveals_child(),
            imp.nav_buttons_revealer.get().reveals_child(),
            imp.zoom_controls_revealer.get().reveals_child(),
        ]
    }

    fn position_label_text(&self) -> String {
        self.viewer.imp().position_label.get().text().to_string()
    }

    fn date_label_text(&self) -> String {
        self.viewer.imp().date_label.get().text().to_string()
    }

    fn wait_for_zoom(&self, reached: impl Fn(f64) -> bool) -> f64 {
        let imp = self.viewer.imp();
        let mut last = imp.zoom_scale.get();
        if reached(last) {
            return last;
        }
        self.ui().wait_until(Duration::from_secs(5), || {
            last = imp.zoom_scale.get();
            reached(last)
        });
        last
    }

    fn wait_for_rotation(&self, reached: impl Fn(i32) -> bool) -> i32 {
        let imp = self.viewer.imp();
        let mut last = imp.viewer_rotation_degrees.get();
        if reached(last) {
            return last;
        }
        self.ui().wait_until(Duration::from_secs(5), || {
            last = imp.viewer_rotation_degrees.get();
            reached(last)
        });
        last
    }

    /// The stage centre — the middle of the picture, where the user aims to click
    /// the image — resolved through GTK's real hit test. Chrome claiming this point
    /// is what makes a photo viewer feel broken.
    fn stage_center_is_chrome(&self) -> bool {
        let overlay = self.viewer.imp().image_overlay.get();
        let Some((x, y)) = self.ui().pointer_at_center_of(&overlay) else {
            return false;
        };
        let Some(target) = self.ui().pick(x, y) else {
            return false;
        };
        let described = Ui::describe(Some(&target));
        described.contains("GtkRevealer")
            || described.contains("viewer-overlay-nav")
            || described.contains("viewer-zoom-controls")
    }

    /// One physical F/Escape press through the production router, with the chrome
    /// state sampled synchronously.
    ///
    /// Nothing is pumped around the press on purpose: "any pointer activity
    /// re-reveals the chrome" is part of the design, and this environment delivers
    /// real motion events at unpredictable moments, so waiting over wall-clock time
    /// for "still folded" is a coin flip. What the press did is knowable the instant
    /// the router returns, and the harness's unsettled gesture runs no main loop
    /// inside the press, so this stays deterministic.
    fn press_key_and_sample(&self, key: gtk::gdk::Key, repeats: usize) -> (usize, [bool; 4]) {
        let gesture = self.ui().key_gesture_unsettled(
            &self.shell.window,
            key,
            gtk::gdk::ModifierType::empty(),
            repeats,
        );
        (gesture.dispatched, self.chrome_regions())
    }
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// The reported regression: Previous/Next stop responding. Walking the whole
/// library with real presses proves the pair is reachable at every rank, that the
/// photo really changes, and that a dimmed arrow at either end is a visible "no
/// further" rather than a dead control.
fn overlay_nav_pair_walks_a_real_library(run: &ViewerRun) {
    let viewer = &run.viewer;
    let imp = viewer.imp();
    let ui = run.ui();
    let next_btn = imp.next_btn.get();
    let prev_btn = imp.prev_btn.get();

    // At the head: Next is live and Previous is dimmed. Both arrows are unknown
    // until the first neighbour prefetch lands, so wait for the resolved state
    // rather than assuming it.
    assert!(
        ui.wait_until(Duration::from_secs(10), || next_btn.is_sensitive()
            && !prev_btn.is_sensitive()),
        "the head of the library should resolve to a live Next and a dimmed Previous"
    );
    let head_id = imp.current_media_id.get();
    ui.try_click_even_if_inert(&prev_btn, "the dimmed Previous arrow")
        .expect("a press aimed at a dimmed arrow should be deliverable or unreachable");
    assert_eq!(
        imp.current_media_id.get(),
        head_id,
        "a press on the dimmed Previous arrow must not move the viewer"
    );

    // Walk forward one photo at a time and check every rank the user can see.
    let mut seen_dates = vec![run.date_label_text()];
    let tail = PHOTO_COUNT as u32 - 1;
    for step in 1..=tail {
        ui.click(&next_btn, &format!("Next (to rank {})", step + 1));
        assert!(
            ui.wait_until(Duration::from_secs(10), || viewer.current_index() == step),
            "pressing Next should land on rank {}, stuck at {}",
            step + 1,
            viewer.current_index()
        );
        assert_eq!(
            imp.current_media_id.get(),
            run.shell.items[step as usize].id,
            "rank {} should show photo-{}'s media id",
            step + 1,
            step
        );

        // The header rank is the user's "where am I" — it has to follow.
        assert!(
            ui.wait_until(Duration::from_secs(10), || run
                .position_label_text()
                .starts_with(&format!("{} /", step + 1))),
            "the header rank should read {} / {}, got {:?}",
            step + 1,
            PHOTO_COUNT,
            run.position_label_text()
        );

        // So does the capture date, which is a different day per photo.
        let date = run.date_label_text();
        assert!(
            !date.is_empty(),
            "the header date should be populated at rank {}",
            step + 1
        );
        assert!(
            !seen_dates.contains(&date),
            "each photo has its own capture day, so the header date must change at rank {} (still {date:?})",
            step + 1
        );
        seen_dates.push(date);
    }

    // At the tail: Next is dimmed and inert, Previous is live again.
    assert!(
        ui.wait_until(Duration::from_secs(10), || !next_btn.is_sensitive()),
        "Next should be dimmed at the tail of the library"
    );
    assert!(
        prev_btn.is_sensitive(),
        "Previous should be live again at the tail"
    );
    let tail_id = imp.current_media_id.get();
    ui.try_click_even_if_inert(&next_btn, "the dimmed Next arrow")
        .ok();
    assert_eq!(
        imp.current_media_id.get(),
        tail_id,
        "a press on the dimmed Next arrow must not move the viewer"
    );

    // And all the way back to the head, with the mirror-image dimming.
    for step in (0..tail).rev() {
        ui.click(&prev_btn, &format!("Previous (to rank {})", step + 1));
        assert!(
            ui.wait_until(Duration::from_secs(10), || viewer.current_index() == step),
            "pressing Previous should return to rank {}, stuck at {}",
            step + 1,
            viewer.current_index()
        );
        assert_eq!(
            imp.current_media_id.get(),
            run.shell.items[step as usize].id,
            "walking back to rank {} should show photo-{} again",
            step + 1,
            step
        );
    }
    assert!(
        ui.wait_until(Duration::from_secs(10), || !prev_btn.is_sensitive()),
        "Previous should be dimmed again at the head"
    );
}

/// The generalized form of the regression. A chrome cluster that fills the stage
/// does not only break its own buttons: it also becomes the hit-test target for
/// every point on the media underneath it, so a full-stage `Gtk.Revealer` silently
/// takes over the whole image area.
///
/// Both halves are asserted, because either one alone can pass while the viewer is
/// unusable: the stage centre must belong to the media, and every chrome control
/// must belong to itself.
fn stage_center_is_never_swallowed_by_chrome(run: &ViewerRun) {
    let viewer = &run.viewer;
    let imp = viewer.imp();
    let ui = run.ui();

    let rect = imp
        .image_overlay
        .get()
        .compute_bounds(ui.root())
        .expect("the image overlay should have bounds");
    let x = f64::from(rect.x()) + f64::from(rect.width()) / 2.0;
    let y = f64::from(rect.y()) + f64::from(rect.height()) / 2.0;
    let stage_center = Ui::describe(ui.pick(x, y).as_ref());
    for owner in ["GtkRevealer", "viewer-overlay-nav", "viewer-zoom-controls"] {
        assert!(
            !stage_center.contains(owner),
            "the centre of the stage must not be owned by {owner}, got {stage_center}"
        );
    }

    // Step off rank 1 first: at the head of the library Previous is dimmed on
    // purpose, and a dimmed arrow is not supposed to be pickable. Rank 2 is the
    // first rank where every chrome control is live at once.
    ui.click(
        &imp.next_btn.get(),
        "Next (to a rank with both arrows live)",
    );
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer.current_index() == 1),
        "the run should be on rank 2 before sweeping the chrome"
    );
    run.wait_for_chrome_relayout();

    // Every chrome control answers to a pointer at its own centre.
    for (label, widget) in [
        ("Previous", &imp.prev_btn.get()),
        ("Next", &imp.next_btn.get()),
        ("Zoom in", &imp.zoom_in_btn.get()),
        ("Fullscreen preview", &imp.fullscreen_btn.get()),
        ("Favorite", &imp.favorite_btn.get()),
        ("Edit", &imp.edit_btn.get()),
        ("Delete", &imp.delete_btn.get()),
        ("Details", &imp.details_btn.get()),
    ] {
        ui.assert_reachable(widget, label);
    }

    // The cluster changes shape as the state changes: zoom-out and reset only
    // exist while the image is enlarged, so they have to be reachable too once
    // they appear.
    ui.click(&imp.zoom_in_btn.get(), "Zoom in");
    assert!(
        ui.wait_until(Duration::from_secs(5), || imp
            .zoom_out_btn
            .get()
            .is_visible()),
        "Zoom out should appear once the image is enlarged"
    );
    for (label, widget) in [
        ("Zoom out", &imp.zoom_out_btn.get()),
        ("Reset view", &imp.zoom_reset_btn.get()),
    ] {
        ui.assert_reachable(widget, label);
    }
}

/// The top-right cluster, operated by real presses, must really transform the
/// stage. Note the cluster's own visibility rule: rotating is only offered at
/// identity zoom, so the case has to come back down before it can turn.
fn zoom_cluster_operates_the_stage_by_pointer(run: &ViewerRun) {
    let imp = run.viewer.imp();
    let ui = run.ui();

    assert_eq!(
        imp.zoom_scale.get(),
        1.0,
        "a freshly opened photo starts at identity zoom"
    );

    ui.click(&imp.zoom_in_btn.get(), "Zoom in");
    let zoomed = run.wait_for_zoom(|scale| scale > 1.0);
    assert!(
        zoomed > 1.0,
        "pressing Zoom in should enlarge the stage, scale={zoomed}"
    );
    ui.click(&imp.zoom_out_btn.get(), "Zoom out");
    let shrunk = run.wait_for_zoom(|scale| scale < zoomed);
    assert!(
        shrunk < zoomed,
        "pressing Zoom out should shrink the stage, scale={shrunk}"
    );

    // Back at identity zoom the transform group is offered again.
    ui.wait_until(Duration::from_secs(5), || {
        imp.rotate_right_btn.get().is_visible()
    });
    ui.click(&imp.rotate_right_btn.get(), "Rotate right");
    let right = run.wait_for_rotation(|deg| deg != 0);
    assert_ne!(right, 0, "pressing Rotate right should turn the stage");

    ui.click(&imp.rotate_left_btn.get(), "Rotate left");
    let left = run.wait_for_rotation(|deg| deg != right);
    assert_ne!(
        left, right,
        "pressing Rotate left should turn the stage the other way"
    );

    // Enlarge again so Reset is on screen, then put the stage back.
    ui.click(&imp.zoom_in_btn.get(), "Zoom in (before reset)");
    run.wait_for_zoom(|scale| scale > 1.0);
    ui.click(&imp.zoom_reset_btn.get(), "Reset view");
    assert!(
        ui.wait_until(Duration::from_secs(5), || imp.zoom_scale.get() == 1.0
            && imp.viewer_rotation_degrees.get() == 0),
        "Reset should return the stage to identity zoom and rotation"
    );
}

/// The header actions, reached by real presses, must open their panels and persist
/// their mutations. The editor also takes the navigation lock, and the lock has to
/// hold against a real press rather than only against a signal.
fn header_actions_reach_their_panels_and_mutations(run: &ViewerRun) {
    let viewer = &run.viewer;
    let imp = viewer.imp();
    let ui = run.ui();
    let media_id = imp.current_media_id.get();

    ui.click(&imp.favorite_btn.get(), "Favorite");
    assert!(
        ui.wait_until(Duration::from_secs(10), || db::is_media_favorite(
            &run.shell.pool,
            media_id
        )
        .unwrap_or(false)),
        "pressing Favorite should persist the heart"
    );
    ui.click(&imp.favorite_btn.get(), "Favorite (toggle back)");
    assert!(
        ui.wait_until(Duration::from_secs(10), || !db::is_media_favorite(
            &run.shell.pool,
            media_id
        )
        .unwrap_or(false)),
        "pressing Favorite again should clear the heart"
    );

    ui.click(&imp.details_btn.get(), "Details");
    assert!(
        ui.wait_until(Duration::from_secs(5), || imp
            .details_split_view
            .get()
            .shows_sidebar()),
        "pressing Details should reveal the details panel"
    );
    ui.click(&imp.details_close_btn.get(), "Close details");
    assert!(
        ui.wait_until(Duration::from_secs(5), || !imp
            .details_split_view
            .get()
            .shows_sidebar()),
        "pressing Close should hide the details panel"
    );

    ui.click(&imp.edit_btn.get(), "Edit");
    assert!(
        ui.wait_until(Duration::from_secs(10), || imp
            .editor_split_view
            .get()
            .shows_sidebar()
            && imp.editor_panel.get().imp().source_image.borrow().is_some()),
        "pressing Edit should open the editor side panel on the real file"
    );

    // The editor locks viewer navigation, and the visible chrome has to match: a
    // press aimed at the pair must not move the photo.
    let locked_id = imp.current_media_id.get();
    ui.try_click_even_if_inert(&imp.next_btn.get(), "Next while the editor is open")
        .ok();
    ui.try_click_even_if_inert(&imp.prev_btn.get(), "Previous while the editor is open")
        .ok();
    assert_eq!(
        imp.current_media_id.get(),
        locked_id,
        "the navigation pair must be inert while the editor is open"
    );
}

/// Immersive browsing folds the chrome into revealers. Coming back has to restore a
/// *usable* chrome, not merely a visible one: a press taken right after the fold has
/// to reach the button again, which is the same hit-test-and-deliver path that
/// regressed.
fn chrome_returns_and_stays_clickable_after_immersive_fold(run: &ViewerRun) {
    let viewer = &run.viewer;
    let imp = viewer.imp();
    let ui = run.ui();

    // Drive immersion through the production keyboard router, the way a user
    // reaches it, rather than by poking the viewer's internals.
    let (dispatched, after_press) = run.press_key_and_sample(gtk::gdk::Key::f, 0);
    assert_eq!(
        dispatched, 1,
        "F should be handled exactly once by the production router while the viewer is open"
    );
    assert_eq!(
        after_press, [false; 4],
        "F should fold all four chrome regions — header, filmstrip, navigation pair, zoom cluster"
    );

    // A folded cluster must not take the media with it. The revealer animates, so
    // the fold's own transition is not a stable thing to assert, but "the stage
    // centre still belongs to the stage" is.
    ui.pump(Duration::from_millis(400));
    assert!(
        !run.stage_center_is_chrome(),
        "folding the chrome must not let it claim the media underneath"
    );

    // Escape unwinds one layer: immersion first, and the viewer stays pushed. This
    // end state *is* stable — leaving immersion cancels the pending stillness timer
    // and a pointer event is a no-op once immersion is off — so it can be waited on.
    let (escape_dispatched, _) = run.press_key_and_sample(gtk::gdk::Key::Escape, 0);
    assert_eq!(
        escape_dispatched, 1,
        "Escape should be handled while immersive"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || run.chrome_regions() == [true; 4]),
        "Escape should leave immersion and hand all four regions back"
    );
    assert!(
        viewer
            .ancestor(adw::NavigationPage::static_type())
            .is_some(),
        "leaving immersion must not also pop the viewer"
    );
    run.wait_for_chrome_relayout();

    // A resting pointer must not undo the fold. GDK keeps delivering motion events
    // for a pointer that has not gone anywhere — folding the chrome re-targets what
    // sits under it — and counting those as "the user moved" cancelled and re-armed
    // the stillness timer on every event, so the timer could never reach its
    // deadline and the chrome stayed pinned open for as long as the pointer rested
    // on the picture. That is what made immersive browsing read as a flicker
    // instead of a fold.
    //
    // Nothing here pumps the main loop: the watcher anchors on the last accepted
    // movement, and a real pointer event landing between "set the baseline" and
    // "assert" would move that anchor and make this a coin flip under load.
    run.press_key_and_sample(gtk::gdk::Key::Escape, 0);
    ui.pointer_motion(viewer, 300.0, 300.0);
    let (_, refold) = run.press_key_and_sample(gtk::gdk::Key::f, 0);
    assert_eq!(refold, [false; 4], "F should fold all four chrome regions");
    for _ in 0..12 {
        ui.pointer_motion(viewer, 300.0, 300.0);
    }
    assert_eq!(
        run.chrome_regions(),
        [false; 4],
        "a burst of motion events at unchanged coordinates must leave the chrome folded: a \
         resting pointer is not activity, and treating it as such re-arms the stillness timer \
         forever so the chrome can never actually fold"
    );
    ui.pointer_motion(viewer, 340.0, 300.0);
    assert_eq!(
        run.chrome_regions(),
        [true; 4],
        "actually moving the pointer must still bring the chrome straight back"
    );
    // Leave immersion so the hold below starts from the state a user is in before
    // pressing F: chrome present, not immersive.
    run.press_key_and_sample(gtk::gdk::Key::Escape, 0);
    run.wait_for_chrome_relayout();

    // Holding F down is not holding F once. GDK repeats `key-pressed` for as long as
    // the key is down, and every repeat used to be dispatched as a fresh press. Each
    // repeat both re-revealed the chrome (any activity counts) and toggled immersion
    // again, so a hold walked the state on/off/on at the OS repeat rate: the 220 ms
    // fold never completed and all that was left on screen was the picture twitching.
    // A hold must dispatch exactly one action.
    let (dispatched, after_hold) = run.press_key_and_sample(gtk::gdk::Key::f, 5);
    assert_eq!(
        dispatched, 1,
        "one physical press of F must dispatch exactly one action, but {} of the 6 presses in \
         that hold were dispatched, so the chrome kept flipping and the picture only twitched",
        dispatched
    );
    assert_eq!(
        after_hold, [false; 4],
        "a held F should leave the chrome folded, with all four regions in agreement"
    );
    run.press_key_and_sample(gtk::gdk::Key::Escape, 0);
    run.wait_for_chrome_relayout();

    // The whole point: a press right after the chrome returns still works.
    ui.click(&imp.next_btn.get(), "Next after the chrome returns");
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer.current_index() == 1),
        "a press right after the chrome returns should still navigate"
    );
    assert_eq!(
        imp.current_media_id.get(),
        run.shell.items[1].id,
        "the press after the fold should show the second photo"
    );
    for (label, widget) in [
        ("Previous", &imp.prev_btn.get()),
        ("Zoom in", &imp.zoom_in_btn.get()),
        ("Favorite", &imp.favorite_btn.get()),
    ] {
        ui.assert_reachable(widget, label);
    }
}
