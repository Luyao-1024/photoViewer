//! Viewer UX runs driven by real pointer targeting.
//!
//! The chrome regression this file exists for was invisible to the rest of the
//! suite because every other viewer "click" test bypasses the input path
//! completely: `emit_by_name("clicked")` asks a button to run its handler no
//! matter what is painted on top of it. A `Gtk.Overlay` child that fills the
//! stage is a perfectly good hit-test target, so a real pointer press can land
//! on an invisible container, the button never runs, and every signal-level
//! test stays green.
//!
//! So each case here does what a user does:
//!
//! 1. build a real `MainWindow`, push a real `ViewerPage`, present it, and let
//!    GTK lay it out;
//! 2. ask GTK *where a pointer at that position actually goes*
//!    (`gtk_widget_pick` — the real hit test) and assert it resolves to the
//!    control the user aimed at;
//! 3. hand the press to the gesture controller that control really owns, which
//!    is the same `GtkGestureClick` `GtkButton` activates through;
//! 4. assert the user-visible result on a library of real, distinct photos —
//!    the media id, the render index, the header rank, the header date.
//!
//! GTK is single-threaded and must be initialized once per process, so the cases
//! run serially from one `#[test]`, the same pattern as `ux_click_flows.rs`.
mod common;

use chrono::{TimeZone, Utc};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use image::{ImageBuffer, Rgb};
use libadwaita as adw;
use photo_viewer::core::identity::MediaId;
use photo_viewer::core::media::{MediaItem, NewMediaItem, MEDIA_SUBKIND_STANDARD};
use photo_viewer::core::thumbnails::ThumbnailLoader;
use photo_viewer::core::MediaQuery;
use photo_viewer::core::{albums, db};
use photo_viewer::ui::{MainWindow, ViewerPage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// GApplication is single-instance per id per process, and every case here
/// builds its own shell, so the ids have to be unique.
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// How many distinct real photos each run gets: enough to walk to the tail and
/// back, which is where the dimmed-arrow contract lives.
const PHOTO_COUNT: usize = 4;

#[test]
fn viewer_ux_runs_by_real_pointer() {
    gtk::init().expect("GTK init failed");
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime for viewer UX runs");
    let _runtime_guard = runtime.enter();

    overlay_nav_pair_walks_a_real_library();
    stage_center_is_never_swallowed_by_chrome();
    zoom_cluster_operates_the_stage_by_pointer();
    header_actions_reach_their_panels_and_mutations();
    chrome_returns_and_stays_clickable_after_immersive_fold();
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// The reported regression: Previous/Next stop responding. Walking the whole
/// library with real presses proves the pair is reachable at every rank, that
/// the photo really changes, and that a dimmed arrow at either end is a
/// visible "no further" rather than a dead control.
fn overlay_nav_pair_walks_a_real_library() {
    let run = ViewerRun::build(PHOTO_COUNT);
    let viewer = &run.viewer;
    let imp = viewer.imp();
    let next_btn = imp.next_btn.get();
    let prev_btn = imp.prev_btn.get();

    // At the head: Next is live and Previous is dimmed. Both arrows are
    // unknown until the first neighbour prefetch lands, so wait for the
    // resolved state rather than assuming it.
    assert!(
        run.wait_until(Duration::from_secs(10), || {
            next_btn.is_sensitive() && !prev_btn.is_sensitive()
        }),
        "the head of the library should resolve to a live Next and a dimmed Previous"
    );
    let head_id = imp.current_media_id.get();
    run.press_when_unreachable(&prev_btn);
    assert_eq!(
        imp.current_media_id.get(),
        head_id,
        "a press on the dimmed Previous arrow must not move the viewer"
    );

    // Walk forward one photo at a time and check every rank the user can see.
    let mut seen_dates = vec![run.date_label_text()];
    let tail = PHOTO_COUNT as u32 - 1;
    for step in 1..=tail {
        run.pointer_click(&next_btn, &format!("Next (to rank {})", step + 1));
        assert!(
            run.wait_until(Duration::from_secs(10), || viewer.current_index() == step),
            "pressing Next should land on rank {}, stuck at {}",
            step + 1,
            viewer.current_index()
        );
        assert_eq!(
            imp.current_media_id.get(),
            run.items[step as usize].id,
            "rank {} should show photo-{}'s media id",
            step + 1,
            step
        );

        // The header rank is the user's "where am I" — it has to follow.
        assert!(
            run.wait_until(Duration::from_secs(10), || {
                run.position_label_text()
                    .starts_with(&format!("{} /", step + 1))
            }),
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
        run.wait_until(Duration::from_secs(10), || !next_btn.is_sensitive()),
        "Next should be dimmed at the tail of the library"
    );
    assert!(
        prev_btn.is_sensitive(),
        "Previous should be live again at the tail"
    );
    let tail_id = imp.current_media_id.get();
    run.press_when_unreachable(&next_btn);
    assert_eq!(
        imp.current_media_id.get(),
        tail_id,
        "a press on the dimmed Next arrow must not move the viewer"
    );

    // And all the way back to the head, with the mirror-image dimming.
    for step in (0..tail).rev() {
        run.pointer_click(&prev_btn, &format!("Previous (to rank {})", step + 1));
        assert!(
            run.wait_until(Duration::from_secs(10), || viewer.current_index() == step),
            "pressing Previous should return to rank {}, stuck at {}",
            step + 1,
            viewer.current_index()
        );
        assert_eq!(
            imp.current_media_id.get(),
            run.items[step as usize].id,
            "walking back to rank {} should show photo-{} again",
            step + 1,
            step
        );
    }
    assert!(
        run.wait_until(Duration::from_secs(10), || !prev_btn.is_sensitive()),
        "Previous should be dimmed again at the head"
    );
}

/// The generalized form of the regression. A chrome cluster that fills the stage
/// does not only break its own buttons: it also becomes the hit-test target for
/// every point on the media underneath it, so a full-stage `Gtk.Revealer`
/// silently takes over the whole image area.
///
/// Both halves are asserted, because either one alone can pass while the viewer
/// is unusable: the stage centre must belong to the media, and every chrome
/// control must belong to itself.
fn stage_center_is_never_swallowed_by_chrome() {
    let run = ViewerRun::build(PHOTO_COUNT);
    let viewer = &run.viewer;
    let imp = viewer.imp();

    let rect = imp
        .image_overlay
        .get()
        .compute_bounds(viewer)
        .expect("the image overlay should have bounds");
    let x = f64::from(rect.x()) + f64::from(rect.width()) / 2.0;
    let y = f64::from(rect.y()) + f64::from(rect.height()) / 2.0;
    let stage_center = describe_target(viewer.pick(x, y, gtk::PickFlags::empty()).as_ref());
    for owner in ["GtkRevealer", "viewer-overlay-nav", "viewer-zoom-controls"] {
        assert!(
            !stage_center.contains(owner),
            "the centre of the stage must not be owned by {owner}, got {stage_center}"
        );
    }

    // Step off rank 1 first: at the head of the library Previous is dimmed on
    // purpose, and a dimmed arrow is not supposed to be pickable. Rank 2 is the
    // first rank where every chrome control is live at once.
    run.pointer_click(
        &imp.next_btn.get(),
        "Next (to a rank with both arrows live)",
    );
    assert!(
        run.wait_until(Duration::from_secs(10), || run.viewer.current_index() == 1),
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
        run.assert_reachable(widget, label);
    }

    // The cluster changes shape as the state changes: zoom-out and reset only
    // exist while the image is enlarged, so they have to be reachable too once
    // they appear.
    run.pointer_click(&imp.zoom_in_btn.get(), "Zoom in");
    assert!(
        run.wait_until(Duration::from_secs(5), || imp
            .zoom_out_btn
            .get()
            .is_visible()),
        "Zoom out should appear once the image is enlarged"
    );
    for (label, widget) in [
        ("Zoom out", &imp.zoom_out_btn.get()),
        ("Reset view", &imp.zoom_reset_btn.get()),
    ] {
        run.assert_reachable(widget, label);
    }
}

/// The top-right cluster, operated by real presses, must really transform the
/// stage. Note the cluster's own visibility rule: rotating is only offered at
/// identity zoom, so the case has to come back down before it can turn.
fn zoom_cluster_operates_the_stage_by_pointer() {
    let run = ViewerRun::build(PHOTO_COUNT);
    let imp = run.viewer.imp();

    assert_eq!(
        imp.zoom_scale.get(),
        1.0,
        "a freshly opened photo starts at identity zoom"
    );

    run.pointer_click(&imp.zoom_in_btn.get(), "Zoom in");
    let zoomed = run.wait_for_zoom(|scale| scale > 1.0);
    assert!(
        zoomed > 1.0,
        "pressing Zoom in should enlarge the stage, scale={zoomed}"
    );
    run.pointer_click(&imp.zoom_out_btn.get(), "Zoom out");
    let shrunk = run.wait_for_zoom(|scale| scale < zoomed);
    assert!(
        shrunk < zoomed,
        "pressing Zoom out should shrink the stage, scale={shrunk}"
    );

    // Back at identity zoom the transform group is offered again.
    run.wait_until(Duration::from_secs(5), || {
        imp.rotate_right_btn.get().is_visible()
    });
    run.pointer_click(&imp.rotate_right_btn.get(), "Rotate right");
    let right = run.wait_for_rotation(|deg| deg != 0);
    assert_ne!(right, 0, "pressing Rotate right should turn the stage");

    run.pointer_click(&imp.rotate_left_btn.get(), "Rotate left");
    let left = run.wait_for_rotation(|deg| deg != right);
    assert_ne!(
        left, right,
        "pressing Rotate left should turn the stage the other way"
    );

    // Enlarge again so Reset is on screen, then put the stage back.
    run.pointer_click(&imp.zoom_in_btn.get(), "Zoom in (before reset)");
    run.wait_for_zoom(|scale| scale > 1.0);
    run.pointer_click(&imp.zoom_reset_btn.get(), "Reset view");
    assert!(
        run.wait_until(Duration::from_secs(5), || {
            imp.zoom_scale.get() == 1.0 && imp.viewer_rotation_degrees.get() == 0
        }),
        "Reset should return the stage to identity zoom and rotation"
    );
}

/// The header actions, reached by real presses, must open their panels and
/// persist their mutations. The editor also takes the navigation lock, and the
/// lock has to hold against a real press rather than only against a signal.
fn header_actions_reach_their_panels_and_mutations() {
    let run = ViewerRun::build(PHOTO_COUNT);
    let viewer = &run.viewer;
    let imp = viewer.imp();
    let media_id = imp.current_media_id.get();

    run.pointer_click(&imp.favorite_btn.get(), "Favorite");
    assert!(
        run.wait_until(Duration::from_secs(10), || {
            db::is_media_favorite(&run.pool, media_id).unwrap_or(false)
        }),
        "pressing Favorite should persist the heart"
    );
    run.pointer_click(&imp.favorite_btn.get(), "Favorite (toggle back)");
    assert!(
        run.wait_until(Duration::from_secs(10), || {
            !db::is_media_favorite(&run.pool, media_id).unwrap_or(false)
        }),
        "pressing Favorite again should clear the heart"
    );

    run.pointer_click(&imp.details_btn.get(), "Details");
    assert!(
        run.wait_until(Duration::from_secs(5), || imp
            .details_split_view
            .get()
            .shows_sidebar()),
        "pressing Details should reveal the details panel"
    );
    run.pointer_click(&imp.details_close_btn.get(), "Close details");
    assert!(
        run.wait_until(Duration::from_secs(5), || !imp
            .details_split_view
            .get()
            .shows_sidebar()),
        "pressing Close should hide the details panel"
    );

    run.pointer_click(&imp.edit_btn.get(), "Edit");
    assert!(
        run.wait_until(Duration::from_secs(10), || {
            imp.editor_split_view.get().shows_sidebar()
                && imp.editor_panel.get().imp().source_image.borrow().is_some()
        }),
        "pressing Edit should open the editor side panel on the real file"
    );

    // The editor locks viewer navigation, and the visible chrome has to match:
    // a press aimed at the pair must not move the photo.
    let locked_id = imp.current_media_id.get();
    run.press_when_unreachable(&imp.next_btn.get());
    run.press_when_unreachable(&imp.prev_btn.get());
    assert_eq!(
        imp.current_media_id.get(),
        locked_id,
        "the navigation pair must be inert while the editor is open"
    );
}

/// Immersive browsing folds the chrome into revealers. Coming back has to
/// restore a *usable* chrome, not merely a visible one: a press taken right
/// after the fold has to reach the button again, which is the same
/// hit-test-and-deliver path that regressed.
fn chrome_returns_and_stays_clickable_after_immersive_fold() {
    let run = ViewerRun::build(PHOTO_COUNT);
    let viewer = &run.viewer;
    let imp = viewer.imp();

    // Drive immersion through the production keyboard router, the way a user
    // reaches it, rather than by poking the viewer's internals.
    //
    // Every assertion is sampled synchronously, before the main loop runs again.
    // That is not fussiness: "any pointer activity re-reveals the chrome" is part
    // of the design, and this environment delivers real motion events at
    // unpredictable moments, so waiting over wall-clock time for "still folded"
    // is a coin flip. What a press did is knowable the instant the router
    // returns, and that is what gets asserted.
    let fold = run.press_key(gtk::gdk::Key::f);
    assert_eq!(
        fold.dispatched, 1,
        "F should be handled exactly once by the production router while the viewer is open"
    );
    assert_eq!(
        fold.after_press, [false; 4],
        "F should fold all four chrome regions — header, filmstrip, navigation pair, zoom cluster"
    );

    // A folded cluster must not take the media with it. The revealer animates, so
    // the fold's own transition is not a stable thing to assert, but "the stage
    // centre still belongs to the stage" is.
    run.pump(Duration::from_millis(400));
    assert!(
        !run.stage_center_is_chrome(),
        "folding the chrome must not let it claim the media underneath"
    );

    // Escape unwinds one layer: immersion first, and the viewer stays pushed.
    // This end state *is* stable — leaving immersion cancels the pending
    // stillness timer and a pointer event is a no-op once immersion is off — so
    // it can be waited on.
    let escape = run.press_key(gtk::gdk::Key::Escape);
    assert_eq!(
        escape.dispatched, 1,
        "Escape should be handled while immersive"
    );
    assert!(
        run.wait_until(Duration::from_secs(5), || {
            run.immersive_region_states() == [true; 4]
        }),
        "Escape should leave immersion and hand all four regions back"
    );
    assert!(
        viewer
            .ancestor(adw::NavigationPage::static_type())
            .is_some(),
        "leaving immersion must not also pop the viewer"
    );
    run.wait_for_chrome_relayout();

    // A resting pointer must not undo the fold. GDK keeps delivering motion
    // events for a pointer that has not gone anywhere — folding the chrome
    // re-targets what sits under it — and counting those as "the user moved"
    // cancelled and re-armed the stillness timer on every event, so the timer
    // could never reach its deadline and the chrome stayed pinned open for as
    // long as the pointer rested on the picture. That is what made immersive
    // browsing read as a flicker instead of a fold.
    //
    // Nothing here pumps the main loop: the watcher anchors on the last accepted
    // movement, and a real pointer event landing between "set the baseline" and
    // "assert" would move that anchor and make this a coin flip under load.
    run.press_key(gtk::gdk::Key::Escape);
    run.pointer_motion(300.0, 300.0);
    let refold = run.key_gesture_unsettled(gtk::gdk::Key::f, 0);
    assert_eq!(
        refold.after_press, [false; 4],
        "F should fold all four chrome regions"
    );
    for _ in 0..12 {
        run.pointer_motion(300.0, 300.0);
    }
    assert_eq!(
        run.immersive_region_states(),
        [false; 4],
        "a burst of motion events at unchanged coordinates must leave the chrome folded: a \
         resting pointer is not activity, and treating it as such re-arms the stillness timer \
         forever so the chrome can never actually fold"
    );
    run.pointer_motion(340.0, 300.0);
    assert_eq!(
        run.immersive_region_states(),
        [true; 4],
        "actually moving the pointer must still bring the chrome straight back"
    );
    // Leave immersion so the hold below starts from the state a user is in
    // before pressing F: chrome present, not immersive.
    run.press_key(gtk::gdk::Key::Escape);
    run.wait_for_chrome_relayout();

    // Holding F down is not holding F once, so start from the non-immersive state
    // the Escape above just established. GDK repeats `key-pressed` for as long as
    // the key is down, and every repeat used to be dispatched as a fresh press.
    // Each repeat both re-revealed the chrome (any activity counts) and toggled
    // immersion again, so a hold walked the state on/off/on at the OS repeat rate:
    // the 220 ms fold never completed and all that was left on screen was the
    // picture twitching. A hold must dispatch exactly one action.
    let hold = run.key_gesture(gtk::gdk::Key::f, 5);
    assert_eq!(
        hold.dispatched, 1,
        "one physical press of F must dispatch exactly one action, but {} of the 6 presses in \
         that hold were dispatched, so the chrome kept flipping and the picture only twitched",
        hold.dispatched
    );
    assert_eq!(
        hold.after_press, [false; 4],
        "the initial press of a held F should fold all four regions once"
    );
    assert_eq!(
        hold.after_repeats, [false; 4],
        "the auto-repeats of a held F must leave the chrome exactly where the initial press put \
         it: still folded, and all four regions in agreement"
    );
    run.press_key(gtk::gdk::Key::Escape);
    run.wait_for_chrome_relayout();

    // The whole point: a press right after the chrome returns still works.
    run.pointer_click(&imp.next_btn.get(), "Next after the chrome returns");
    assert!(
        run.wait_until(Duration::from_secs(10), || viewer.current_index() == 1),
        "a press right after the chrome returns should still navigate"
    );
    assert_eq!(
        imp.current_media_id.get(),
        run.items[1].id,
        "the press after the fold should show the second photo"
    );
    for (label, widget) in [
        ("Previous", &imp.prev_btn.get()),
        ("Zoom in", &imp.zoom_in_btn.get()),
        ("Favorite", &imp.favorite_btn.get()),
    ] {
        run.assert_reachable(widget, label);
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// What one key gesture did, sampled synchronously so the assertions cannot race
/// with unrelated queued input.
struct KeyGesture {
    /// How many of the gesture's presses the router actually dispatched. One
    /// physical press must dispatch at most one action.
    dispatched: usize,
    /// The four immersive regions, sampled the instant the initial press
    /// returned.
    after_press: [bool; 4],
    /// The same regions, sampled after the auto-repeats.
    after_repeats: [bool; 4],
}

struct ViewerRun {
    _app: adw::Application,
    _tmp: tempfile::TempDir,
    pool: db::DbPool,
    items: Vec<MediaItem>,
    window: MainWindow,
    viewer: ViewerPage,
}

impl Drop for ViewerRun {
    fn drop(&mut self) {
        // Each case presents its own window. Leaving them all mapped starves the
        // frame clock and makes later cases depend on how many ran before them,
        // so tear this one down as soon as the case is done with it.
        self.window.destroy();
    }
}

impl ViewerRun {
    fn build(photo_count: usize) -> Self {
        // GIO refuses to trash files on some tmpfs mounts, and a viewer run
        // wants a filesystem that behaves like the user's.
        let fixture_root = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/tmp"));
        let tmp = tempfile::Builder::new()
            .prefix("photo-viewer-viewer-ux-")
            .tempdir_in(fixture_root)
            .expect("create viewer UX fixture on a trash-capable filesystem");
        let pool = db::init_pool(&tmp.path().join("viewer-ux.db")).unwrap();
        let loader = Arc::new(ThumbnailLoader::new(
            pool.clone(),
            tmp.path().join("thumbs"),
        ));

        // Real, distinct photos: each has its own size, its own color, and its
        // own capture day, so a switch is observable in the pixels and in the
        // header.
        let photos_dir = tmp.path().join("photos");
        std::fs::create_dir_all(&photos_dir).unwrap();
        let mut items = Vec::new();
        for idx in 0..photo_count {
            let path = write_photo(&photos_dir, idx);
            let item = sample_item(i64::try_from(idx).unwrap(), path, idx);
            let id = common::db::insert_media_item(&pool, &NewMediaItem::from(&item)).unwrap();
            items.push(db::get_media_item(&pool, id).unwrap());
        }
        albums::refresh(&pool).unwrap();

        let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for item in &items {
            media_list.append(&glib::BoxedAnyObject::new(item.clone()));
        }
        let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
        let db_actor = photo_viewer::core::start_db_actor(pool.clone(), event_sender);

        let seq = RUN_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
        let app = adw::Application::builder()
            .application_id(format!("io.github.luyao_1024.photoviewer.ViewerUx{seq}"))
            .build();
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("test application should register");
        photo_viewer::ui::grid_css::install();

        let window = MainWindow::new(&app);
        window.populate_sidebar();
        window.set_resources(pool.clone(), loader.clone(), media_list.clone());
        window.set_db_actor(db_actor.clone());
        let nav = window.nav_view();

        // The production entry point: a query plus a stable media id. The
        // legacy index-only constructor leaves `media_query` unset, so
        // neighbour prefetch never runs and the end-of-library dimming — part
        // of what these cases assert — is never exercised.
        let viewer =
            ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(items[0].id), media_list);
        viewer.set_edit_target(&nav, pool.clone());
        viewer.set_db_actor(db_actor);
        viewer.set_thumbnail_loader(loader);
        viewer.show_at(0);
        nav.push(&viewer);
        window.present();

        let run = Self {
            _app: app,
            _tmp: tmp,
            pool,
            items,
            window,
            viewer,
        };

        // Every contract here depends on real geometry, so wait for the stage
        // to be laid out before the first hit test.
        let imp = run.viewer.imp();
        assert!(
            run.wait_until(Duration::from_secs(15), || {
                imp.next_btn.get().is_mapped()
                    && imp.next_btn.get().width() > 0
                    && imp.zoom_in_btn.get().width() > 0
            }),
            "the viewer chrome should be laid out within 15s"
        );
        run.wait_for_chrome_relayout();
        run
    }

    // -- pointer ---------------------------------------------------------

    /// Where a pointer at the control's own centre actually lands, resolved by
    /// GTK's real hit test. `None` means nothing picked it — the right answer
    /// for a hidden, folded, or dimmed control.
    fn pick_target(&self, widget: &impl IsA<gtk::Widget>) -> Option<gtk::Widget> {
        let widget = widget.as_ref();
        // A widget that is not on screen has no pointer position: its last
        // allocation is stale, and `compute_bounds` would hand back coordinates
        // that belong to whatever happens to sit there now. "Not mapped" is the
        // honest answer for a folded or torn-down control.
        if !widget.is_mapped() {
            return None;
        }
        let rect = widget
            .compute_bounds(&self.viewer)
            .unwrap_or_else(|| panic!("control has no bounds inside the viewer page"));
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let x = f64::from(rect.x()) + f64::from(rect.width()) / 2.0;
        let y = f64::from(rect.y()) + f64::from(rect.height()) / 2.0;
        self.viewer.pick(x, y, gtk::PickFlags::empty())
    }

    /// The reachability contract: a sensitive control that is on screen must be
    /// the widget GTK hands a pointer at its own centre to. A control that is
    /// visible but shadowed by an invisible container fails here, which is
    /// exactly the class of bug this file was written for.
    fn assert_reachable(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        let widget = widget.as_ref();
        assert!(
            widget.is_sensitive(),
            "{label} should be sensitive while it is on screen"
        );
        let target = self
            .pick_target(widget)
            .unwrap_or_else(|| panic!("a pointer at the centre of {label} picked nothing"));
        assert!(
            target == *widget || widget.is_ancestor(&target) || target.is_ancestor(widget),
            "a pointer at the centre of {label} must reach {label}, but GTK picked {}",
            describe_target(Some(&target))
        );
    }

    /// A real primary-button click: resolve the pointer target first, then hand
    /// the press and release to the gesture controller the control itself owns
    /// — the same `GtkGestureClick` `GtkButton` activates through.
    fn pointer_click(&self, widget: &impl IsA<gtk::Widget>, label: &str) {
        self.assert_reachable(widget, label);
        let Some(button) = button_behind_widget(widget.as_ref()) else {
            panic!("{label} should be a Gtk.Button to press");
        };
        self.press(&button, label);
    }

    /// Deliver one pointer motion event at page coordinates, the way a real
    /// pointer position reaches the page-level stillness watcher.
    fn pointer_motion(&self, x: f64, y: f64) {
        let motion = self
            .viewer
            .observe_controllers()
            .snapshot()
            .into_iter()
            .find_map(|controller| controller.downcast::<gtk::EventControllerMotion>().ok())
            .expect("the viewer page should watch pointer motion for immersive browsing");
        motion.emit_by_name::<()>("motion", &[&x, &y]);
    }

    /// The four immersive regions' revealed state, in template order: header,
    /// filmstrip, navigation pair, zoom cluster. They are written by a single
    /// `set_chrome_revealed` call, so they can never legitimately disagree.
    fn immersive_region_states(&self) -> [bool; 4] {
        let imp = self.viewer.imp();
        [
            imp.header_revealer.get().reveals_child(),
            imp.filmstrip_revealer.get().reveals_child(),
            imp.nav_buttons_revealer.get().reveals_child(),
            imp.zoom_controls_revealer.get().reveals_child(),
        ]
    }

    /// The stage centre — the middle of the picture, where the user aims to
    /// click the image — resolved through GTK's real hit test. Chrome
    /// claiming this point is what makes a photo viewer feel broken.
    fn stage_center_is_chrome(&self) -> bool {
        let overlay = self.viewer.imp().image_overlay.get();
        let Some(rect) = overlay.compute_bounds(&self.viewer) else {
            return false;
        };
        let x = f64::from(rect.x()) + f64::from(rect.width()) / 2.0;
        let y = f64::from(rect.y()) + f64::from(rect.height()) / 2.0;
        let Some(target) = self.viewer.pick(x, y, gtk::PickFlags::empty()) else {
            return false;
        };
        let described = describe_target(Some(&target));
        described.contains("GtkRevealer")
            || described.contains("viewer-overlay-nav")
            || described.contains("viewer-zoom-controls")
    }

    /// A press that is expected to reach nothing: a dimmed arrow, folded
    /// chrome, or a cluster the editor has hidden. Reachability is
    /// deliberately not asserted — the contract is that the press cannot reach
    /// a live handler.
    fn press_when_unreachable(&self, widget: &impl IsA<gtk::Widget>) {
        let Some(target) = self.pick_target(widget) else {
            return;
        };
        if button_behind(&target, widget.as_ref()).is_none() {
            return;
        }
        let Some(button) = button_behind_widget(widget.as_ref()) else {
            return;
        };
        if !button.is_sensitive() {
            return;
        }
        self.press(&button, "unreachable control");
    }

    fn press(&self, button: &gtk::Button, label: &str) {
        let gesture = button
            .observe_controllers()
            .snapshot()
            .into_iter()
            .find_map(|controller| controller.downcast::<gtk::GestureClick>().ok())
            .unwrap_or_else(|| panic!("{label} should own a GtkGestureClick to press"));
        let x = f64::from(button.width()) / 2.0;
        let y = f64::from(button.height()) / 2.0;
        gesture.emit_by_name::<()>("pressed", &[&1i32, &x, &y]);
        gesture.emit_by_name::<()>("released", &[&1i32, &x, &y]);
        self.pump(Duration::from_millis(200));
    }

    /// One physical key press, delivered through the production window router
    /// rather than by calling page-level handlers, so the binding table and the
    /// router's own gating are exercised too.
    fn press_key(&self, key: gtk::gdk::Key) -> KeyGesture {
        self.key_gesture(key, 0)
    }

    /// A whole key gesture: the initial press, `repeats` GDK auto-repeats, then
    /// the release — what a real hold looks like.
    ///
    /// Nothing is pumped inside the gesture on purpose. Dispatching a binding is
    /// synchronous, so the outcome is fully determined by the router's own
    /// gating; pumping would let unrelated queued input land mid-gesture and
    /// turn a deterministic assertion into a race.
    fn key_gesture(&self, key: gtk::gdk::Key, repeats: usize) -> KeyGesture {
        let gesture = self.key_gesture_unsettled(key, repeats);
        self.pump(Duration::from_millis(200));
        gesture
    }

    /// The same gesture, but without letting the main loop run again afterwards.
    ///
    /// Needed wherever a real pointer event arriving in between would change the
    /// answer. The stillness watcher anchors on the last accepted movement, and
    /// this environment delivers real motion events at unpredictable moments, so
    /// a pump between "establish the baseline" and "assert" makes the assertion a
    /// coin flip.
    fn key_gesture_unsettled(&self, key: gtk::gdk::Key, repeats: usize) -> KeyGesture {
        let controller = self.keyboard_router();
        let state = gtk::gdk::ModifierType::empty();
        let mut dispatched = 0usize;
        let mut press = || -> bool {
            let handled: bool = controller.emit_by_name("key-pressed", &[&key, &0_u32, &state]);
            if handled {
                dispatched += 1;
            }
            handled
        };

        press();
        // Sampled here, with the main loop still untouched: a pointer event that
        // arrives later is allowed to re-reveal immersive chrome by design, so
        // the only trustworthy observation of "what this press did" is the one
        // taken the instant the router returned.
        let after_press = self.immersive_region_states();
        for _ in 0..repeats {
            press();
        }
        let after_repeats = self.immersive_region_states();
        controller.emit_by_name::<()>("key-released", &[&key, &0_u32, &state]);
        KeyGesture {
            dispatched,
            after_press,
            after_repeats,
        }
    }

    fn keyboard_router(&self) -> gtk::EventControllerKey {
        self.window
            .observe_controllers()
            .snapshot()
            .into_iter()
            .find_map(|controller| controller.downcast::<gtk::EventControllerKey>().ok())
            .filter(|controller| {
                controller.name().as_deref() == Some("photo-viewer-keyboard-router")
            })
            .expect("MainWindow should install the production keyboard router")
    }

    // -- observable viewer state ----------------------------------------

    fn position_label_text(&self) -> String {
        self.viewer.imp().position_label.get().text().to_string()
    }

    fn date_label_text(&self) -> String {
        self.viewer.imp().date_label.get().text().to_string()
    }

    fn wait_for_zoom(&self, reached: impl Fn(f64) -> bool) -> f64 {
        self.viewer.imp().zoom_scale.get();
        let mut last = self.viewer.imp().zoom_scale.get();
        if reached(last) {
            return last;
        }
        self.wait_until(Duration::from_secs(5), || {
            last = self.viewer.imp().zoom_scale.get();
            reached(last)
        });
        last
    }

    fn wait_for_rotation(&self, reached: impl Fn(i32) -> bool) -> i32 {
        let mut last = self.viewer.imp().viewer_rotation_degrees.get();
        if reached(last) {
            return last;
        }
        self.wait_until(Duration::from_secs(5), || {
            last = self.viewer.imp().viewer_rotation_degrees.get();
            reached(last)
        });
        last
    }

    // -- main loop -------------------------------------------------------

    fn pump(&self, duration: Duration) {
        let ctx = glib::MainContext::default();
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            while ctx.iteration(false) {}
            std::thread::sleep(Duration::from_millis(5));
        }
        while ctx.iteration(false) {}
    }

    fn wait_until(&self, timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let ctx = glib::MainContext::default();
        while Instant::now() < deadline {
            while ctx.iteration(false) {}
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        while ctx.iteration(false) {}
        condition()
    }

    /// The immersive revealers animate open. A hit test taken mid-crossfade
    /// would be measuring a half-faded allocation, so let the chrome settle and
    /// then re-measure.
    fn wait_for_chrome_relayout(&self) {
        let imp = self.viewer.imp();
        assert!(
            self.wait_until(Duration::from_secs(10), || {
                ["next_btn", "zoom_in_btn", "fullscreen_btn", "favorite_btn"]
                    .iter()
                    .all(|name| {
                        let widget: &gtk::Button = match *name {
                            "next_btn" => &imp.next_btn.get(),
                            "zoom_in_btn" => &imp.zoom_in_btn.get(),
                            "fullscreen_btn" => &imp.fullscreen_btn.get(),
                            _ => &imp.favorite_btn.get(),
                        };
                        widget.is_mapped() && widget.width() > 0 && widget.height() > 0
                    })
            }),
            "the viewer chrome should be laid out and clickable"
        );
        self.pump(Duration::from_millis(400));
    }
}

/// If a pointer aimed at `intended` resolved to `intended` or to the image drawn
/// inside it, hand back that button. Anything else means the pointer would not
/// have activated `intended` at all.
fn button_behind(target: &gtk::Widget, intended: &gtk::Widget) -> Option<gtk::Button> {
    if *target != *intended && !intended.is_ancestor(target) {
        return None;
    }
    button_behind_widget(intended)
}

fn button_behind_widget(widget: &gtk::Widget) -> Option<gtk::Button> {
    widget.downcast_ref::<gtk::Button>().cloned()
}

fn describe_target(target: Option<&gtk::Widget>) -> String {
    let Some(widget) = target else {
        return "nothing".to_string();
    };
    let classes = widget
        .css_classes()
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(".");
    let type_name = widget.type_().name().to_string();
    format!("<{type_name} class=[{classes}]>")
}

// ---------------------------------------------------------------------------
// Sample data
// ---------------------------------------------------------------------------

/// A real, decodable JPEG. Each photo gets its own size and color, so a switch
/// changes the actual pixels and not just a label.
fn write_photo(dir: &Path, idx: usize) -> PathBuf {
    let size = 48 + 16 * (idx as u32);
    let base = [
        30u32 + 55 * (idx as u32 % 3),
        80u32 + 45 * (idx as u32 % 2),
        170u32 - 35 * (idx as u32 % 4),
    ];
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(size, size, |x, y| {
        Rgb([
            (base[0] + x % 32) as u8,
            (base[1] + y % 32) as u8,
            (base[2] - (x + y) % 24) as u8,
        ])
    });
    let path = dir.join(format!("photo-{idx}.jpg"));
    img.save(&path).expect("write sample photo");
    path
}

fn sample_item(id: i64, path: PathBuf, idx: usize) -> MediaItem {
    // One distinct capture day per photo, so the header date is a switching
    // signal too. Newest first, the way the library sorts
    // (`COALESCE(taken_at, file_mtime) DESC`), so photo-0 is rank 1 and the
    // last photo is the tail. Kept well away from today so the label is a real
    // date and not the localized 今天/昨天 shortcut.
    let taken_at = Utc
        .with_ymd_and_hms(2026, 6, 20 - u32::try_from(idx).unwrap(), 12, 0, 0)
        .unwrap();
    let folder_path = path
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .to_path_buf();
    MediaItem {
        id,
        uri: format!("file://{}", path.display()),
        path,
        folder_path,
        mime_type: "image/jpeg".into(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(64),
        video_duration_secs: None,
        taken_at: Some(taken_at),
        file_mtime: taken_at,
        file_size: 128,
        blake3_hash: format!("viewer-ux-hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}
