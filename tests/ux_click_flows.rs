//! Full-shell UX journeys driven by real pointer and keyboard input.
//!
//! Every scenario here builds a presented `MainWindow` over a real database and
//! real JPEGs on a real filesystem, then drives it through
//! [`common::interaction::Ui`]: hit-test where the pointer actually lands, assert
//! it lands on the control the user aimed at, deliver press and release to the
//! gesture controller that control owns, and finally check the *durable* result —
//! the file on disk, the row in the database, the page the user ends up on.
//!
//! Nothing calls a page-level action handler, and nothing emits `clicked`. A
//! `clicked` emission runs a button's handler regardless of what is painted on
//! top of it, so it cannot see an overlay that swallows the pointer, a stale
//! allocation, or chrome that stayed unreachable after folding — the exact class
//! of failure that used to leave a green suite and a broken viewer.
//!
//! GTK must be initialized once per process and shells cannot be driven
//! concurrently, so the scenarios run serially from the single `#[test]` below,
//! each tearing its window down on drop.
//!
//! The Tokio runtime the single `#[test]` enters is load-bearing, not ceremony:
//! the shell starts thumbnail workers, and those need a reactor. A test binary
//! that builds a `Shell` without entering one panics inside the loader with
//! "there is no reactor running".
//!
//! Journeys own cross-page behaviour. A contract stays isolated only when putting
//! it into a journey would make the journey branch unnaturally or hide the thing
//! being diagnosed.

mod common;

use common::interaction::{
    contains_focus, descendants, find_button_with_css, find_button_with_label, find_descendant,
    find_label_containing, wait_for_button_with_label, wait_for_descendant,
    wait_for_label_containing, Ui,
};
use common::shell::Shell;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use libadwaita::prelude::NavigationPageExt;
use photo_viewer::core::db;
use photo_viewer::core::edit::Rotation;
use photo_viewer::core::i18n::tr;
use photo_viewer::core::identity::MediaId;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::sync::{Fingerprint, NewSyncJob, SyncDirection, SyncStore, UploadScope};
use photo_viewer::ui::virtual_media_grid::VirtualMediaGrid;
use photo_viewer::ui::{
    album_picker, AlbumDetailPage, ModeSelector, SearchPage, TrashPage, ViewerPage,
};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[test]
fn ux_full_shell_user_journeys_and_interaction_contracts() {
    gtk::init().expect("GTK init failed");
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime for UX journeys");
    let _runtime_guard = runtime.enter();

    // Journeys: a whole user goal, from first click to durable effect.
    journey_search_view_edit_and_save_copy();
    journey_select_copy_to_album_then_open_it();
    journey_trash_restore_and_permanently_delete();
    journey_viewer_delete_toast_offers_undo();
    journey_rename_in_details_renames_the_file_on_disk();
    journey_save_overwrite_rewrites_the_file_and_keeps_a_backup();
    journey_editor_close_guard_keeps_or_discards_pending_edits();
    journey_context_menu_select_then_favorite_the_selection();
    journey_empty_trash_destroys_every_photo_for_good();
    journey_editor_rotation_buttons_then_reset_restore_the_source();
    journey_editor_crop_ratio_arrows_drive_a_pending_crop();
    journey_album_multi_select_cancel_keeps_every_album_on_disk();
    journey_album_multi_select_delete_moves_that_album_to_the_trash();
    journey_corrupt_photo_offers_retry_and_reveal();
    journey_album_context_menu_deletes_and_ignores_real_albums();
    journey_viewer_shortcuts_reach_the_same_handlers_as_the_buttons();
    journey_filmstrip_shows_every_photo_and_centres_the_current_one();

    // The library's own lifecycle: what the scanner finds on disk, and what it
    // gives up when a file disappears. Both drive the production scan, because
    // the scan *is* the behaviour under test.
    runtime.block_on(journey_photos_imported_into_the_folder_are_scanned_and_shown());
    runtime.block_on(journey_photos_deleted_outside_the_app_are_reconciled());

    // Interaction contracts for variants a journey would have to contort to reach.
    mode_selector_click_switches_photos_view();
    tile_double_click_opens_exactly_one_viewer();
    search_result_double_click_opens_exactly_one_viewer_while_pending();
    keyboard_shortcuts_drive_full_shell_navigation();
    photos_batch_toolbar_clicks_select_favorite_and_album();
    viewer_details_and_zoom_clicks_drive_visible_operations();
    sidebar_clicks_drive_top_level_navigation();
    album_sidebar_multi_select_deletes_real_albums();
    album_picker_clicks_album_row_and_copy_move();
    album_detail_context_menu_moves_to_album();
    synced_image_badge_survives_the_jump_from_day_grid_to_viewer();
}

// ---------------------------------------------------------------------------
// Journeys
// ---------------------------------------------------------------------------

/// Open a photo whose bytes cannot be decoded, and prove the viewer's dead end
/// is a real, reachable escape hatch rather than a blank stage.
///
/// The file is genuinely corrupt — it scans, it gets a row, it gets a tile a
/// pointer lands on — so the error surface is reached by the same path a user
/// takes, not by setting the surface visible. Neither button on that surface
/// had any scenario behind it: `ui_viewer_toolbar.rs` only asserts the chrome's
/// CSS classes and never once opens a viewer.
///
/// Retry is pressed, because its whole effect is inside the app. "Show in File
/// Manager" is only reached, never pressed: it hands the folder to the desktop
/// through `gtk::show_uri_full`, so a press would spawn a real file manager
/// that outlives the test, or fail on a machine with no default handler, and
/// which one happens says nothing about this code.
fn journey_corrupt_photo_offers_retry_and_reveal() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let broken_item = shell.seed_broken_photo("broken");
    let broken = broken_item.path.clone();
    let broken_album_dir = broken_item
        .path
        .parent()
        .expect("the corrupt fixture lives in a directory")
        .to_path_buf();
    let grid = shell.visible_photos_grid();

    // The corrupt photo is a real, hittable tile: the failure is in the bytes,
    // not in the row. `Shell::tile_for` is deliberately not used — it insists
    // on a painted thumbnail, which is the one thing this file can never have.
    let mut found = None;
    let reached = ui.wait_until(Duration::from_secs(10), || {
        found = grid.tile_for_media(MediaId::from(broken_item.id));
        found.is_some()
    });
    assert!(
        reached,
        "a corrupt photo should still get a tile the user can click"
    );
    let tile = found.expect("the tile was captured above");
    ui.click(&tile, "the corrupt photo");

    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the corrupt photo");
    assert!(
        ui.wait_until(Duration::from_secs(15), || viewer
            .imp()
            .media_error_box
            .get()
            .is_visible()),
        "a photo that cannot be decoded should raise the viewer's media-error surface"
    );

    // The wording names the file, because the real causes are a moved, renamed
    // or unreadable file and the name is what a user can act on.
    let title = viewer.imp().media_error_title.get().text().to_string();
    assert_eq!(
        title,
        tr("viewer.image_error.title"),
        "the error surface should use the catalogue's image-error title"
    );
    let subtitle = viewer.imp().media_error_subtitle.get().text().to_string();
    assert!(
        subtitle.contains(&tr("viewer.image_error.subtitle").replace("{name}", "broken.jpg")),
        "the error surface should name the file the user has to find, got {subtitle:?}"
    );

    // Reveal runs first, while the error surface is still up: the retry below
    // is what loses it, so anything that needs the surface has to come before.

    // Reveal is the other way out, and it is pressed for real. What it hands
    // over is captured by the project's `*_for_tests` seam, which *replaces* the
    // platform call rather than merely watching it: `gtk::show_uri_full` would
    // launch the developer's file manager at a directory this fixture is about to
    // delete, leaving a window that outlives the test and complains that the
    // folder is missing. The desktop has no business inside a test run.
    //
    // What is asserted is the part this application actually decides: which
    // folder, named against a fixed fixture directory.
    let revealed: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let sink = revealed.clone();
    viewer.set_reveal_folder_observer_for_tests(move |folder| {
        *sink.borrow_mut() = Some(folder.to_string());
    });

    let reveal = viewer.imp().media_error_reveal_btn.get();
    assert_eq!(
        reveal.label().unwrap().as_str(),
        tr("viewer.error.reveal"),
        "the second way out should be labelled from the catalogue"
    );
    ui.click(&reveal, "Show in File Manager");

    let folder = ui.wait_until(Duration::from_secs(5), || revealed.borrow().is_some());
    assert!(
        folder,
        "Show in File Manager should hand a folder to the desktop"
    );
    let handed_over = revealed.borrow().clone().expect("captured above");
    // The fixture is installed in a fixed directory, so "the folder the photo is
    // in" is a name the test can state rather than a path it has to guess at.
    let handed_path = PathBuf::from(
        handed_over
            .strip_prefix("file://")
            .expect("GIO hands out file:// URIs"),
    );
    assert_eq!(
        handed_path,
        broken_item
            .path
            .parent()
            .expect("the fixture has a parent directory"),
        "Reveal should hand over the folder that actually contains the photo"
    );
    assert_eq!(
        handed_path.file_name().and_then(|n| n.to_str()),
        Some("broken-album"),
        "that folder is the fixed `broken-album` fixture directory, not a generated one"
    );
    assert!(
        broken_album_dir.is_dir(),
        "the revealed folder is a real directory"
    );

    // Retry re-runs the load. The file is still corrupt, so the honest outcome
    // is the same error again — the point is that the button is live and the
    // viewer does not wedge behind it. Asserting the error surface is still up
    // would be vacuous, since it never went away; what proves the press did
    // something is the load token advancing, because that is `show_at` re-
    // entering for the current index.
    let token_before_retry = viewer.imp().current_token.get();
    let retry = viewer.imp().media_error_retry_btn.get();
    assert_eq!(retry.label().unwrap().as_str(), tr("viewer.error.retry"));
    ui.click(&retry, "Retry");
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer.imp().current_token.get()
            != token_before_retry),
        "Retry should start a fresh load, but the load token never moved off {token_before_retry}"
    );

    // The honest outcome of a retry on a still-corrupt file is the *same error
    // again* — and that is exactly what has to happen. It used not to:
    // `thumbnails::decode` answers a failed decode with a generated "unavailable"
    // placeholder rather than an error, so `picture.paintable()` became
    // `Some(placeholder)` and `show_original_decode_error` — which reads *any*
    // paintable as "the picture is showing" — declined to raise the surface. The
    // user was left on a grey box with neither Retry nor Reveal on it, and no
    // number of presses could get them back. An image that cannot be decoded now
    // behaves like a video that cannot be played.
    //
    // The stand-in is now flagged (`LoadedThumb::unavailable`), so the stage
    // raises the error surface instead of painting it.
    let error_back = ui.wait_until(Duration::from_secs(10), || {
        let token = viewer.imp().current_token.get();
        token != token_before_retry
            && viewer.imp().media_error_box.get().is_visible()
            && !viewer.imp().picture.get().is_visible()
    });
    assert!(
        error_back,
        "Retry on a still-undecodable photo should bring the error surface back, not leave a \
         grey stand-in: error box visible={} picture visible={} token {} -> {}",
        viewer.imp().media_error_box.get().is_visible(),
        viewer.imp().picture.get().is_visible(),
        token_before_retry,
        viewer.imp().current_token.get()
    );

    // And the surface is genuinely live again, not merely redrawn: its own two
    // buttons are back on it and can be pressed. `assert_reachable` panics with
    // the hit-test result if either has not come back.
    ui.assert_reachable(&retry, "Retry after the error surface came back");
    ui.assert_reachable(
        &reveal,
        "Show in File Manager after the error surface came back",
    );

    // And the user can still leave: the error surface is a dead end for the
    // picture, not for the viewer.
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::Escape,
            gtk::gdk::ModifierType::empty()
        ),
        "Escape should still pop the viewer after a decode failure"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || nav
            .visible_page()
            .and_downcast::<ViewerPage>()
            .is_none()),
        "the viewer should pop so the user gets back to their library"
    );
    assert!(
        broken.is_file(),
        "nothing about a failed decode should touch the file on disk"
    );
}

/// Copy photographs into the library the way a user does — files appear in a
/// watched folder — and check the application really finds, indexes, and shows
/// every one of them.
///
/// The scan is the product code under test here, so nothing here hand-builds a
/// row: the files are written to disk, the production scanner runs, and every
/// claim is made against what it produced. Each imported photo is a different
/// shape, because a scanner that only ever gets 4:3 files is not evidence about
/// the rest — a width and height that disagree with the bytes is exactly the
/// class of bug that a hand-written fixture hides.
///
/// Scope: this covers what the filesystem and the database do. It deliberately
/// stops short of "the running grid gains a tile", which needs the window's
/// domain-event consumer — see `docs/testing.md` for why the shell does not wire
/// that up today.
async fn journey_photos_imported_into_the_folder_are_scanned_and_shown() {
    let shell = Shell::with_photos(2);

    let before = db::list_all_media(&shell.pool).unwrap();
    assert_eq!(
        before.len(),
        2,
        "the library starts with the two photos the shell was built with"
    );

    // Four new photographs in the watched folder, spanning the extremes: a
    // 1:1 square, a 9:16 phone shot, a 21:9 panorama, and a 3:2 frame.
    let imports = [
        ("imported-square.jpg", "ridge-square", 40, (420u32, 420u32)),
        ("imported-tall.jpg", "night-916", 41, (360, 640)),
        ("imported-pano.jpg", "pano-219", 42, (840, 360)),
        ("imported-wide.jpg", "mesa-32", 43, (540, 360)),
    ];
    for (name, fixture, day, _) in imports {
        let path = shell.import_fixture(name, fixture, day);
        assert!(path.is_file(), "{name} should be on disk before the scan");
    }

    shell.rescan().await;

    // The scan is only "successful" if every row agrees with the bytes it
    // describes, so the declared shape is checked against the real file.
    let mut imported_rows = Vec::new();
    for (name, _, _, (width, height)) in imports {
        let path = shell.photos_dir().join(name);
        let row = db::list_all_media(&shell.pool)
            .unwrap()
            .into_iter()
            .find(|item| item.path == path)
            .unwrap_or_else(|| panic!("the scan should have indexed {name} at {}", path.display()));
        assert_eq!(
            (row.width, row.height),
            (Some(width), Some(height)),
            "the row for {name} must describe the file that is actually there"
        );
        assert!(
            row.file_size == std::fs::metadata(&path).unwrap().len() as u64,
            "the row for {name} must record the real byte count"
        );
        imported_rows.push(row);
    }

    assert_eq!(
        db::list_all_media(&shell.pool).unwrap().len(),
        2 + imports.len(),
        "the library should hold the two originals plus every import"
    );

    // "Indexed" is not "loadable". Each import has to come back out of the
    // production thumbnail workers as an actual picture, which is the part of
    // "the app loaded my photos" that a database row cannot stand in for — a
    // scanner that happily indexed a file nothing can decode would pass every
    // assertion above and show the user a grid of grey cells.
    for row in &imported_rows {
        assert!(
            shell.loads_thumbnail_for(row).await,
            "the thumbnail workers should render {} — it is on disk, indexed, and \
             still not loadable",
            row.display_name()
        );
    }

    // The originals are still loadable too: a rescan must not have disturbed the
    // photographs that were already in the library.
    for row in db::list_all_media(&shell.pool).unwrap() {
        if imported_rows.iter().any(|new| new.id == row.id) {
            continue;
        }
        assert!(
            shell.loads_thumbnail_for(&row).await,
            "the rescan should have left {} renderable",
            row.display_name()
        );
    }
}

/// Delete photographs from outside the application — the way a sync client, a
/// file manager, or a cleanup script would — and check the library notices.
///
/// This is the mirror of the import journey, and it is the one half that used to
/// be untested. Nothing in the suite removed a file behind the app's back, so the
/// reconcile that a user hits when they tidy up a folder by hand was entirely
/// unguarded: a library that keeps offering a photo whose file is gone is a user
/// clicking through to a broken viewer with no way to tell why.
///
/// Scope matches the import journey: the reconcile and the storage, not the
/// running grid's tile list.
async fn journey_photos_deleted_outside_the_app_are_reconciled() {
    let shell = Shell::with_photos(4);
    let ui = &shell.ui;
    let grid = shell.visible_photos_grid();

    let before = db::list_all_media(&shell.pool).unwrap();
    assert_eq!(before.len(), 4, "the library starts with four photos");

    // Remove two of them from the filesystem, directly, with the application
    // unaware. The first is the photo the user is looking at, so the journey
    // also covers the viewer losing its subject.
    let doomed: Vec<MediaItem> = before.iter().take(2).cloned().collect();
    let surviving: Vec<MediaItem> = before.iter().skip(2).cloned().collect();
    for item in &doomed {
        std::fs::remove_file(&item.path)
            .unwrap_or_else(|e| panic!("remove {}: {e}", item.path.display()));
        assert!(!item.path.exists(), "the file is really gone from disk");
    }

    shell.rescan().await;

    // The rows must be gone, not merely hidden.
    let after = db::list_all_media(&shell.pool).unwrap();
    for item in &doomed {
        assert!(
            after.iter().all(|row| row.id != item.id),
            "the row for {} should have been pruned; the file no longer exists",
            item.path.display()
        );
        assert!(
            db::get_media_item(&shell.pool, item.id).is_err(),
            "the pruned row should be gone from the database, not just unlisted"
        );
    }
    assert_eq!(
        after.len(),
        surviving.len(),
        "every photo still on disk should keep its row"
    );
    for item in &surviving {
        assert!(
            after.iter().any(|row| row.id == item.id),
            "{} is still on disk and must survive the reconcile",
            item.path.display()
        );
    }

    // What survives still works, which is the part a prune that over-reached
    // would break.
    for row in &surviving {
        assert!(
            shell.loads_thumbnail_for(row).await,
            "{} is still on disk and must still be renderable",
            row.display_name()
        );
    }
    let kept = shell.tile_for(&grid, MediaId::from(surviving[0].id), "the surviving photo");
    ui.click(&kept, "a photo that is still on disk");
    let viewer = expect_page::<ViewerPage>(ui, &shell.window.nav_view(), "opening a survivor");
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "a photo that survived the reconcile should still open normally"
    );
}

/// How far the current film's thumbnail may sit from the middle of the filmstrip.
///
/// The strip centres through a CSS transform rather than by scrolling, so there
/// is no integer pixel to land on exactly; a couple of pixels is rounding in the
/// transform and the button's own width, not a centring failure.
const FILMSTRIP_CENTER_TOLERANCE_PX: f64 = 3.0;

/// Where the current photo's filmstrip thumbnail actually sits, measured the way
/// a user sees it.
///
/// This deliberately measures `compute_bounds` in the window's coordinate space
/// rather than the scroll adjustment: the strip sizes its content to the
/// viewport and centres by transform, so `upper == page_size` and the
/// adjustment never moves at all. Reading the adjustment would report a
/// perfectly centred filmstrip as "no scroll happened" — or the reverse.
/// `compute_bounds` carries the transform, so it reports what is on screen.
///
/// Returns `(current_center_x, strip_center_x, thumb_width)`, or `None` while the
/// filmstrip has not been built yet.
fn filmstrip_centering(
    viewer: &ViewerPage,
    root: &impl IsA<gtk::Widget>,
) -> Option<(f64, f64, f64)> {
    let imp = viewer.imp();
    let strip = imp.thumb_scrolled.get();
    let strip_bounds = strip.compute_bounds(root)?;
    let strip_center = f64::from(strip_bounds.x()) + f64::from(strip_bounds.width()) / 2.0;
    let offset = imp
        .current_index
        .get()
        .checked_sub(imp.thumb_window_start.get())? as usize;
    let items = imp.thumb_items.borrow();
    let current = items.get(offset)?;
    let bounds = current.compute_bounds(root)?;
    let center = f64::from(bounds.x()) + f64::from(bounds.width()) / 2.0;
    Some((center, strip_center, f64::from(bounds.width())))
}

/// Assert the filmstrip is on screen and showing the current photo, and that the
/// current photo's thumbnail is in the middle of it.
fn assert_filmstrip_centers_current(ui: &Ui, shell: &Shell, viewer: &ViewerPage, where_: &str) {
    // Let the strip finish settling before measuring it. The result itself is
    // not the assertion — the checks below report *how* it is wrong, which is
    // more use than a bare timeout.
    let _settled = ui.wait_until(Duration::from_secs(10), || {
        filmstrip_centering(viewer, &shell.window)
            .is_some_and(|(c, s, w)| w > 0.0 && (c - s).abs() <= FILMSTRIP_CENTER_TOLERANCE_PX)
    });
    let imp = viewer.imp();
    let strip = imp.thumb_scrolled.get();
    assert!(
        strip.is_visible() && strip.is_mapped() && strip.height() > 0,
        "{where_}: the filmstrip should be on screen, got visible={} mapped={} {}x{}",
        strip.is_visible(),
        strip.is_mapped(),
        strip.width(),
        strip.height()
    );
    let (current_center, strip_center, width) = filmstrip_centering(viewer, &shell.window)
        .unwrap_or_else(|| {
            panic!(
                "{where_}: the filmstrip should hold a thumbnail for the current photo (index {}, \
                 window starts at {}, {} built)",
                imp.current_index.get(),
                imp.thumb_window_start.get(),
                imp.thumb_items.borrow().len()
            )
        });
    assert!(
        width > 0.0,
        "{where_}: the current film's thumbnail should have a real width, got {width}"
    );
    let delta = current_center - strip_center;
    assert!(
        delta.abs() <= FILMSTRIP_CENTER_TOLERANCE_PX,
        "{where_}: the current photo (index {}) should sit in the middle of the filmstrip, \
         but it is {delta:.1}px off centre (thumb centre {current_center:.1}, strip centre \
         {strip_center:.1})",
        imp.current_index.get()
    );
}

/// The filmstrip is how a user knows where they are in a long photo run, and the
/// one thing it must always do is put the photo you are looking at in the middle
/// — that is what makes "keep going" and "go back" legible at a glance.
///
/// Nothing covered this. `ui_viewer_toolbar.rs` asserts the chrome's CSS classes
/// and `ux_viewer_pointer_flows.rs` asserts chrome comes *back* after an
/// immersive fold, but no scenario ever looked at the strip itself, so a
/// filmstrip that never rendered, or that pinned the current photo to the edge
/// after a jump, would have left every suite green.
///
/// The journey walks the whole run the way a user does — the Next button, the
/// keyboard, and a jump to the end — and checks the invariant after every step.
/// The fixture has twelve photos, more than the strip's eleven-item window, so
/// this crosses the window boundary and exercises the lazy extension as well as
/// the initial build. Twelve distinct scenes matter here too: a strip of
/// identical placeholder tiles cannot be checked by eye, and several of the
/// guards in `update_thumb_scroll_position` bail out quietly, which is exactly
/// the kind of failure a visual assertion is supposed to catch.
fn journey_filmstrip_shows_every_photo_and_centres_the_current_one() {
    let shell = Shell::with_photos(12);
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let total = shell.items.len();
    assert!(
        total > 11,
        "the fixture needs more photos than the filmstrip's 11-item window, got {total}"
    );

    let tile = shell.tile_for(&grid, MediaId::from(shell.items[0].id), "the Photos grid");
    ui.click(&tile, "the first photo");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the first photo");
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading before the filmstrip is judged"
    );

    // Opened on the first photo of the run: the strip should be built and the
    // current tile centred even at the very start, where centring is the easiest
    // thing to get wrong because there is nothing to scroll toward.
    assert_filmstrip_centers_current(ui, &shell, &viewer, "on opening the first photo");
    assert!(
        viewer.imp().thumb_items.borrow().len() > 1,
        "the filmstrip should hold more than one thumbnail, got {}",
        viewer.imp().thumb_items.borrow().len()
    );

    // Step forward with the Next button, as a user browsing a run would.
    for step in 1..4u32 {
        ui.click(&viewer.imp().next_btn.get(), "Next");
        assert!(
            ui.wait_until(Duration::from_secs(6), || viewer.current_index() == step),
            "Next should advance to index {step}, still at {}",
            viewer.current_index()
        );
        assert_filmstrip_centers_current(ui, &shell, &viewer, &format!("after Next to {step}"));
    }

    // The same invariant through the keyboard, which reaches the viewer by a
    // different route and must not leave the strip behind.
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::Right,
            gtk::gdk::ModifierType::empty()
        ),
        "Right should be handled by the viewer keyboard router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(6), || viewer.current_index() == 4),
        "Right should advance to index 4, still at {}",
        viewer.current_index()
    );
    assert_filmstrip_centers_current(ui, &shell, &viewer, "after the Right arrow key");

    // Jump to the end of the run. This is the interesting one: the current photo
    // is the last one, so the window has to extend past its original range and
    // the strip has nowhere to scroll *toward* on the right. A strip that
    // centres by scrolling rather than by transform would jam against the end
    // here and leave the current photo stranded at the right edge.
    let last = u32::try_from(total - 1).expect("fixture is small");
    viewer.show_at(last);
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer.current_index() == last),
        "jumping to the end should land on index {last}, still at {}",
        viewer.current_index()
    );
    assert_filmstrip_centers_current(ui, &shell, &viewer, "on the last photo of the run");

    // And back to the first, which is the mirror of the same problem: the window
    // has to retreat to the start with nothing to scroll toward on the left.
    viewer.show_at(0);
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer.current_index() == 0),
        "jumping back should land on index 0, still at {}",
        viewer.current_index()
    );
    assert_filmstrip_centers_current(ui, &shell, &viewer, "back on the first photo");

    // Resize the strip's viewport. This is the case that is easy to miss: the
    // strip centres from the viewport's `page_size`, so a viewport that gets
    // narrower leaves the old transform in place unless something recomputes it.
    // The invariant has to hold at every size, not only the one the fixture
    // happens to open at.
    //
    // The width comes off the filmstrip rather than off the window. A window
    // size is a hint the window manager applies, and a headless Xvfb has no
    // window manager: the running window never took the new size, sat at
    // 1430 px, and the failure read as though the strip had stopped re-centring.
    // `set_size_request` is not the answer on its own either — it is a floor GTK
    // enforces, so it grows a small widget and leaves a large one alone — which
    // is why the strip also stops expanding and takes its requested width. That
    // is the size the product actually centres from, so what is under test is
    // unchanged: the viewport width changes and the strip has to follow.
    let strip = viewer.imp().thumb_scrolled.get();
    for (label, width) in [("narrowed", 1_180i32), ("restored", i32::MAX)] {
        if width == i32::MAX {
            strip.set_hexpand(true);
            strip.set_size_request(-1, -1);
        } else {
            strip.set_hexpand(false);
            strip.set_size_request(width, -1);
        }
        assert!(
            ui.wait_until(Duration::from_secs(8), || {
                width == i32::MAX || strip.width() == width
            }),
            "the filmstrip viewport should be {label} to {width}, got {}",
            strip.width()
        );
        ui.pump(Duration::from_millis(600));
        assert_filmstrip_centers_current(
            ui,
            &shell,
            &viewer,
            &format!("with the viewport {label}"),
        );
    }

    // The strip must be showing real pictures, not blank cells. Geometry alone
    // cannot tell the two apart: a `GtkPicture` with no paintable is still laid
    // out, still has a width, and still centres perfectly — which is exactly how
    // a completely grey filmstrip passed this journey the first time it ran.
    // Every built thumbnail therefore has to carry an actual paintable.
    let imp = viewer.imp();
    let items = imp.thumb_items.borrow();
    let offset = (imp.current_index.get() - imp.thumb_window_start.get()) as usize;
    let pictures: Vec<Option<gtk::Picture>> = items
        .iter()
        .map(|item| {
            item.first_child()
                .and_then(|child| child.downcast::<gtk::Picture>().ok())
        })
        .collect();
    let painted = pictures
        .iter()
        .filter(|pic| pic.as_ref().is_some_and(|p| p.paintable().is_some()))
        .count();
    assert_eq!(
        painted,
        pictures.len(),
        "every filmstrip thumbnail should show a decoded picture, got {painted}/{} — a blank \
         cell is laid out and centres just as well as a filled one",
        pictures.len()
    );
    assert!(
        items
            .iter()
            .all(|item| item.is_mapped() && item.width() > 0),
        "every filmstrip thumbnail should be laid out, got widths {:?}",
        items.iter().map(|i| i.width()).collect::<Vec<_>>()
    );

    // The current photo is the one the strip draws larger. Asserting that as
    // "it has the greatest width" was true only while every thumbnail had the
    // same shape: a 21:9 photo lays out far wider than a 1:1 one, so on a mixed
    // set the current 1:1 thumbnail is not the widest and the assertion started
    // failing on a correct strip. What the CSS actually does is apply
    // `scale(1.24)` to the current thumbnail, so the thing to check is that its
    // *drawn* size exceeds the size it was laid out at, while every other
    // thumbnail is drawn at exactly its laid-out size.
    let drawn_vs_allocated: Vec<(f64, i32)> = items
        .iter()
        .map(|item| {
            let drawn = item
                .compute_bounds(&shell.window)
                .map(|b| f64::from(b.width()))
                .unwrap_or(0.0);
            (drawn, item.allocation().width())
        })
        .collect();
    let (current_drawn, current_alloc) = drawn_vs_allocated[offset];
    assert!(
        current_drawn > f64::from(current_alloc) + 0.5,
        "the current photo's thumbnail should be drawn larger than it is laid out, \
         got drawn {current_drawn} against allocated {current_alloc} (scale 1.24 expected)"
    );
    for (n, (drawn, alloc)) in drawn_vs_allocated.iter().enumerate() {
        if n == offset {
            continue;
        }
        assert!(
            (drawn - f64::from(*alloc)).abs() < 0.5,
            "only the current thumbnail is enlarged; #{n} is drawn at {drawn} against \
             allocated {alloc}"
        );
    }
}

/// Take an album's sidebar row, right-click it, and press the named entry in the
/// menu that appears — the whole route a user takes to reach a destructive album
/// action.
fn press_album_context_action(shell: &Shell, album_name: &str, action_label: &str) {
    let ui = &shell.ui;
    let row_label = wait_for_label_containing(
        &shell.window.imp().album_list.get(),
        album_name,
        Duration::from_secs(5),
    )
    .unwrap_or_else(|| panic!("the sidebar should list {album_name:?}"));
    ui.right_click(&row_label, &format!("{album_name:?} album row"));
    let entry = wait_for_button_with_label(&shell.window, action_label, Duration::from_secs(4))
        .unwrap_or_else(|| panic!("the album menu should offer {action_label:?}"));
    ui.click(&entry, action_label);
}

/// Ignore and Delete sit one menu apart and promise opposite things, which is
/// exactly why one journey drives both: Ignore must leave every file on disk and
/// only take the album out of the library, while Delete must move the files to
/// the Trash. A regression that routed one through the other would destroy a
/// user's folders, and neither action had a scenario — `ui_context_menu.rs`
/// rebuilds the menu to check its CSS classes and never triggers a callback, so
/// "the item has the right style" was the only thing ever asserted.
fn journey_album_context_menu_deletes_and_ignores_real_albums() {
    // --- Ignore: files stay, library forgets them ---
    let shell = Shell::new();
    let ui = &shell.ui;
    let window = &shell.window;
    let ignored = shell.seed_extra_album();
    window.populate_album_rows();
    let ignored_photo = ignored.join("three.jpg");
    let kept_dirs: Vec<PathBuf> = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .filter(|album| !album.is_virtual)
        .map(|album| album.folder_path.clone())
        .filter(|dir| dir != &ignored)
        .collect();
    let kept_photos: Vec<PathBuf> = kept_dirs
        .iter()
        .flat_map(|dir| {
            shell
                .items
                .iter()
                .map(|i| i.path.clone())
                .filter(move |p| p.starts_with(dir))
                .collect::<Vec<_>>()
        })
        .collect();
    let ignored_name = ignored.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        ignored_photo.is_file(),
        "the album to ignore should hold a real photo at {}",
        ignored_photo.display()
    );

    press_album_context_action(&shell, &ignored_name, &tr("album.context.ignore"));
    respond_to_alert(ui, window, &tr("album.ignore.confirm_action"));

    assert!(
        ui.wait_until(Duration::from_secs(15), || !window
            .imp()
            .album_targets
            .borrow()
            .iter()
            .any(|album| album.folder_path == ignored)),
        "ignoring an album should take it out of the sidebar"
    );
    assert!(
        ignored_photo.is_file(),
        "Ignore must never touch the user's files: {} is gone",
        ignored_photo.display()
    );
    assert!(
        ignored.is_dir(),
        "Ignore must leave the album's folder exactly where it was"
    );
    assert!(
        !db::list_all_media(&shell.pool)
            .unwrap()
            .iter()
            .any(|item| item.path == ignored_photo),
        "Ignore should drop the album's rows from the library, not trash them"
    );
    for photo in &kept_photos {
        assert!(
            photo.is_file(),
            "ignoring one album must not disturb another: {} is gone",
            photo.display()
        );
    }

    // --- Delete: files go to the Trash ---
    let deleted = shell.seed_extra_album();
    window.populate_album_rows();
    let deleted_photo = deleted.join("three.jpg");
    let deleted_name = deleted.file_name().unwrap().to_string_lossy().into_owned();

    press_album_context_action(&shell, &deleted_name, &tr("album.context.delete"));
    respond_to_alert(ui, window, &tr("album.delete.confirm_action"));

    assert!(
        ui.wait_until(Duration::from_secs(15), || !deleted_photo.exists()),
        "deleting an album should move its photo out of the library, looked for at {}",
        deleted_photo.display()
    );
    assert!(
        ui.wait_until(Duration::from_secs(8), || db::list_trashed_media(
            &shell.pool
        )
        .is_ok_and(|items| items.iter().any(|item| item.path == deleted_photo))),
        "Delete should send the album's photos to the Trash, where they are recoverable"
    );
    assert!(
        ui.wait_until(Duration::from_secs(8), || !window
            .imp()
            .album_targets
            .borrow()
            .iter()
            .any(|album| album.folder_path == deleted)),
        "a deleted album should leave the sidebar"
    );
    for photo in &kept_photos {
        assert!(
            photo.is_file(),
            "deleting one album must not touch another: {} is gone",
            photo.display()
        );
    }
    assert!(
        db::list_all_media(&shell.pool)
            .unwrap()
            .iter()
            .all(|item| item.trashed_at.is_none()),
        "only the deleted album's photo should carry a trash flag"
    );
}

/// Prove the Viewer shortcuts reach the same handlers as the buttons beside them.
///
/// The toolbar contract already proves the *buttons* move zoom, rotation, details
/// and favourite state, and the keyboard contract proves navigation, Search and
/// the text-input guard. Between them, the viewer's own actions had a gap in the
/// middle: the actions a user reaches with one keystroke were never pressed, so a
/// binding that silently stopped dispatching — a renamed action, a scope that no
/// longer resolves, a handler that stopped being connected — would have left both
/// suites green.
fn journey_viewer_shortcuts_reach_the_same_handlers_as_the_buttons() {
    let shell = Shell::with_photos(3);
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let target = shell.items[0].clone();
    let original_bytes = std::fs::read(&target.path).unwrap();

    let tile = shell.tile_for(&grid, MediaId::from(target.id), "the Photos grid");
    ui.click(&tile, "the photo to drive by keyboard");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the photo");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading before the shortcuts are pressed"
    );
    let start_index = viewer.current_index();

    // R and Shift+R are the rotation bindings, and they are pending edits: the
    // file on disk must not move.
    assert_eq!(
        viewer.imp().viewer_rotation_degrees.get(),
        0,
        "the viewer should open unrotated"
    );
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::r,
            gtk::gdk::ModifierType::empty()
        ),
        "R should be handled by the viewer keyboard router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer
            .imp()
            .viewer_rotation_degrees
            .get()
            == 90),
        "R should rotate the viewer a quarter turn, got {}",
        viewer.imp().viewer_rotation_degrees.get()
    );
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::R,
            gtk::gdk::ModifierType::SHIFT_MASK
        ),
        "Shift+R should be handled by the viewer keyboard router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer
            .imp()
            .viewer_rotation_degrees
            .get()
            == 0),
        "Shift+R should rotate the other way and land back at zero, got {}",
        viewer.imp().viewer_rotation_degrees.get()
    );

    // The zoom bindings share the buttons' state, so pressing them is checked
    // against the same scale the toolbar contract moves.
    let zoom_before = viewer.imp().zoom_scale.get();
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::plus,
            gtk::gdk::ModifierType::empty()
        ),
        "+ should be handled by the viewer keyboard router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer.imp().zoom_scale.get()
            > zoom_before),
        "+ should zoom in past the {} it started at, got {}",
        zoom_before,
        viewer.imp().zoom_scale.get()
    );
    let zoomed = viewer.imp().zoom_scale.get();
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::_0,
            gtk::gdk::ModifierType::empty()
        ),
        "0 should be handled by the viewer keyboard router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || {
            let now = viewer.imp().zoom_scale.get();
            now < zoomed && (now - 1.0).abs() < f64::EPSILON
        }),
        "0 should return the viewer to 1:1, got {}",
        viewer.imp().zoom_scale.get()
    );

    // Arrow navigation walks the same list the Next button walks.
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::Right,
            gtk::gdk::ModifierType::empty()
        ),
        "Right should be handled by the viewer keyboard router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer.current_index()
            != start_index),
        "Right should advance to another photo, still at index {start_index}"
    );
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::Left,
            gtk::gdk::ModifierType::empty()
        ),
        "Left should be handled by the viewer keyboard router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer.current_index()
            == start_index),
        "Left should walk back to the photo we started on"
    );

    // E is the same action as the Edit button, and it must land in the same place.
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::e,
            gtk::gdk::ModifierType::empty()
        ),
        "E should be handled by the viewer keyboard router"
    );
    let editor = viewer.imp().editor_panel.get();
    assert!(
        ui.wait_until(Duration::from_secs(10), || viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()
            && editor.imp().source_image.borrow().is_some()),
        "E should open the editor exactly as the Edit button does"
    );

    // Leaving through the editor's own gate, then leaving the viewer, must still
    // work — the keyboard is a way in, not a way to get stranded.
    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::Escape,
            gtk::gdk::ModifierType::empty()
        ),
        "Escape should be handled while the editor is open"
    );
    assert!(
        ui.wait_until(Duration::from_secs(8), || !viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()),
        "Escape should close a clean editor without asking about unsaved work"
    );
    assert_eq!(
        std::fs::read(&target.path).unwrap(),
        original_bytes,
        "keyboard-driven rotation and the editor are pending edits; the source must be unchanged"
    );
}

/// Search for a photo by name, open it, read its details, favorite it, brighten
/// it, and save a copy — then prove the copy is a real file that entered the
/// library and that the original was not touched.
fn journey_search_view_edit_and_save_copy() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let target = &shell.items[0];
    let original_bytes = std::fs::read(&target.path).unwrap();
    let initial_count = db::list_all_media(&shell.pool).unwrap().len();

    // Open Search from the Photos toolbar, by pointer.
    ui.click(
        &shell.photos.imp().search_btn.get(),
        "Search toolbar button",
    );
    let search = expect_page::<SearchPage>(ui, &nav, "clicking Search should open Search");

    // Type the query one character at a time. `set_text` does not reach the
    // search pipeline at all: on GTK 4.22 it leaves a connected `search-changed`
    // listener silent, so only `Ui::type_search` makes a result appear.
    let entry = search.imp().search_entry.get();
    assert!(
        ui.wait_until(Duration::from_secs(3), || contains_focus(
            entry.upcast_ref()
        )),
        "opening Search by pointer should move focus into the query field"
    );
    // The query is the file's stem: what the user sees as the photo's name.
    // Typing the full name with its extension is not how the field matches.
    let stem = target
        .path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    ui.type_search(&entry, &stem);
    let result = wait_for_flowbox_result(ui, &search, &stem);
    ui.click(&result, "the search result tile");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the result should push Viewer");

    // The right photo, not merely *a* photo.
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer
            .imp()
            .current_media_id
            .get()
            == target.id),
        "the viewer should show the photo the user searched for"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the JPEG should finish loading and enable Edit"
    );

    // Details open and close by pointer.
    ui.click(&viewer.imp().details_btn.get(), "Details");
    assert!(
        viewer.imp().details_split_view.get().shows_sidebar(),
        "clicking Details should reveal the details panel"
    );
    ui.click(&viewer.imp().details_close_btn.get(), "Close details");
    assert!(
        !viewer.imp().details_split_view.get().shows_sidebar(),
        "clicking Close should hide the details panel again"
    );

    // Favorite, and check it reached the database rather than only the widget.
    ui.click(&viewer.imp().favorite_btn.get(), "Favorite");
    assert!(
        ui.wait_until(Duration::from_secs(5), || db::is_media_favorite(
            &shell.pool,
            target.id
        )
        .unwrap_or(false)),
        "favoriting from the Viewer should persist to the database"
    );

    // Edit, brighten, Save Copy.
    ui.click(&viewer.imp().edit_btn.get(), "Edit");
    let editor = viewer.imp().editor_panel.get();
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()
            && editor.imp().source_image.borrow().is_some()),
        "Edit should open the side panel on the real file"
    );
    editor.imp().brightness_scale.get().set_value(18.0);
    assert!(
        ui.wait_until(Duration::from_secs(5), || !editor
            .imp()
            .render_running
            .get()
            && !editor.imp().render_pending.get()),
        "the edited preview should settle before Save Copy"
    );
    ui.click(&editor.imp().save_copy_btn.get(), "Save Copy");

    let saved = ui.wait_until(Duration::from_secs(10), || {
        db::list_all_media(&shell.pool).is_ok_and(|items| {
            items.len() == initial_count + 1
                && items.iter().any(|item| {
                    item.path.is_file()
                        && item
                            .path
                            .file_stem()
                            .is_some_and(|stem| stem.to_string_lossy().contains("_edited_"))
                })
        })
    });
    assert!(
        saved,
        "Save Copy should publish a real edited file and add it to the library"
    );
    assert!(
        ui.wait_until(Duration::from_secs(4), || !viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()),
        "a successful save should return the user to the Viewer"
    );

    // The promise of "copy": the original is byte-for-byte untouched.
    assert_eq!(
        std::fs::read(&target.path).unwrap(),
        original_bytes,
        "Save Copy must not modify the original file"
    );
    let copies: Vec<_> = db::list_all_media(&shell.pool)
        .unwrap()
        .into_iter()
        .filter(|item| item.id != target.id)
        .collect();
    assert!(
        !copies.is_empty() && copies[0].path != target.path,
        "the copy should be a different file from the original"
    );
    assert_ne!(
        std::fs::read(&copies[0].path).unwrap(),
        original_bytes,
        "the saved copy should carry the brightness change, not the original pixels"
    );
}

/// Select the collection the way a user does — right-click a photo, choose
/// multi-select, then click the other tiles — copy it into an album through the
/// picker, and reach the copies again from the sidebar.
fn journey_select_copy_to_album_then_open_it() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let original_count = db::list_all_media(&shell.pool).unwrap().len();
    let grid = shell.visible_photos_grid();

    // Reach the batch bar the way a user does: a photo's context menu opens
    // multi-select, and only then does the bar reveal Select All.
    select_every_photo(&shell, &grid);
    assert!(
        shell
            .photos
            .imp()
            .add_to_album_revealer
            .get()
            .reveals_child(),
        "a selection should reveal the batch Add to Album action"
    );

    ui.click(&shell.photos.imp().add_to_album_btn.get(), "Add to Album");
    let picker = album_picker_dialog(ui, &shell.window);
    let picker_ui = Ui::for_widget(&picker);
    let tile = wait_for_picker_tile(&picker_ui, &picker);
    picker_ui.click(&tile, "the album tile in the picker");
    assert!(
        find_descendant::<photo_viewer::ui::SquareTile>(&picker)
            .is_some_and(|t| t.has_css_class("media-selected")),
        "the chosen album cover should show the grid's selection state"
    );

    let copy =
        find_button_with_label(&picker, &tr("album_picker.copy")).expect("the picker offers Copy");
    picker_ui.click(&copy, "Copy");
    assert!(
        ui.wait_until(Duration::from_secs(10), || db::list_all_media(&shell.pool)
            .is_ok_and(|items| items.len() > original_count)),
        "copying through the picker should persist real copied files"
    );
    assert!(
        ui.wait_until(Duration::from_secs(6), || !picker.is_mapped()),
        "copying should close the picker (visible={} mapped={})",
        picker.is_visible(),
        picker.is_mapped()
    );

    // Reopen the album from the sidebar by clicking the row that reads its name,
    // then open one copied photo.
    shell.window.populate_album_rows();
    let album_name = real_album_name_in_sidebar(&shell);
    let row_label = wait_for_label_containing(
        &shell.window.imp().album_list.get(),
        &album_name,
        Duration::from_secs(5),
    )
    .unwrap_or_else(|| panic!("the sidebar should show an album row reading {album_name:?}"));
    ui.click(&row_label, &format!("the {album_name:?} sidebar row"));
    assert!(
        ui.wait_until(Duration::from_secs(5), || shell
            .window
            .browsing_stack()
            .visible_child_name()
            .as_deref()
            == Some("album")),
        "clicking an album row should open that album"
    );
    let detail = shell
        .window
        .browsing_stack()
        .visible_child()
        .and_downcast::<AlbumDetailPage>()
        .expect("the album detail page should be visible");
    let detail_grid = find_descendant::<VirtualMediaGrid>(&detail)
        .expect("the album detail page should own its grid");
    let copied = newest_media(&shell.pool);
    let tile = shell.tile_for(&detail_grid, MediaId::from(copied.id), "the album grid");
    ui.click(&tile, "a copied photo in the album");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening a copied album photo");
    assert_eq!(
        viewer.imp().current_media_id.get(),
        copied.id,
        "the viewer should show the copied photo that was clicked"
    );
    assert!(
        copied.path.is_file(),
        "the copied photo must exist on disk for the user to look at it"
    );
}

/// Trash two selected photos through the confirmation dialog, verify the files
/// really left the library folder, then restore one and permanently delete the
/// other — checking the filesystem each time, not only the database.
fn journey_trash_restore_and_permanently_delete() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let paths: Vec<PathBuf> = shell.items.iter().map(|item| item.path.clone()).collect();
    let ids: Vec<i64> = shell.items.iter().map(|item| item.id).collect();

    let grid = shell.visible_photos_grid();
    select_every_photo(&shell, &grid);
    ui.click(
        &shell.photos.imp().delete_to_trash_btn.get(),
        "Move to Trash",
    );

    respond_to_alert(ui, &shell.window, &tr("dialog.trash"));
    assert!(
        ui.wait_until(Duration::from_secs(8), || {
            db::list_trashed_media(&shell.pool).is_ok_and(|items| {
                items.len() == 2 && ids.iter().all(|id| items.iter().any(|i| i.id == *id))
            })
        }),
        "confirming should move both selected photos into the trash"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || paths
            .iter()
            .all(|path| !path.exists())),
        "moving to the trash should take the files out of the library folder"
    );

    let trash = shell.open_trash();
    let grid = trash
        .imp()
        .grid
        .borrow()
        .as_ref()
        .cloned()
        .expect("Trash grid");
    assert!(
        ui.wait_until(Duration::from_secs(5), || grid.logical_media_count() == 2),
        "Trash should list the two trashed photos"
    );

    // Select one, then Cancel: the bar hides, the page stays.
    let first = newest_trashed(&shell.pool);
    click_trashed_photo(&shell, &grid, &first, true);
    wait_action_bar(ui, &trash, true);
    ui.click(&trash.imp().cancel_btn.get(), "Cancel selection");
    wait_action_bar(ui, &trash, false);

    // Restore one: it goes back on disk, leaves the Trash list, and the page stays.
    click_trashed_photo(&shell, &grid, &first, true);
    wait_action_bar(ui, &trash, true);
    ui.click(&trash.imp().restore_btn.get(), "Restore");
    assert!(
        ui.wait_until(Duration::from_secs(8), || {
            first.path.exists()
                && db::get_media_item(&shell.pool, first.id)
                    .is_ok_and(|item| item.trashed_at.is_none())
        }),
        "Restore should put the file back on disk and clear its trash mark"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || grid.logical_media_count() == 1),
        "Trash should reload to the one remaining photo"
    );
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "the Trash page must stay open after restoring one photo"
    );

    // Delete the last one permanently: the row leaves the database for good.
    let remaining = oldest_trashed(&shell.pool);
    // The grid reloads after Restore, so the tile is re-resolved rather than
    // reusing the handle from before the reload.
    click_trashed_photo(&shell, &grid, &remaining, true);
    wait_action_bar(ui, &trash, true);
    ui.click(&trash.imp().delete_btn.get(), "Delete Permanently");
    assert!(
        ui.wait_until(Duration::from_secs(8), || db::list_trashed_media(
            &shell.pool
        )
        .is_ok_and(|items| items.is_empty())),
        "Delete Permanently should empty the trashed rows"
    );
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "the Trash page must stay open after deleting the last photo"
    );

    // Idempotent cleanup: whatever state the assertions above caught, leave no
    // fixture files in the host trash.
    for path in &paths {
        let _ =
            photo_viewer::core::trash::delete_permanently(&format!("file://{}", path.display()));
    }
}

/// Delete from the Viewer and use the toast's Undo — which has to really put the
/// file back, not just re-flag the row.
fn journey_viewer_delete_toast_offers_undo() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let target = shell.items[0].clone();

    let tile = shell.tile_for(&grid, MediaId::from(target.id), "the Photos grid");
    ui.click(&tile, &format!("{} in Photos", target.display_name()));
    let viewer = expect_page::<ViewerPage>(ui, &nav, "clicking a photo should open the Viewer");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading before it can be deleted"
    );
    assert!(target.path.exists(), "the fixture photo should exist");

    ui.click(&viewer.imp().delete_btn.get(), "Delete");
    respond_to_alert(ui, &shell.window, &tr("dialog.trash"));
    assert!(
        ui.wait_until(Duration::from_secs(8), || db::list_trashed_media(
            &shell.pool
        )
        .is_ok_and(|items| items.iter().any(|item| item.id == target.id))),
        "confirming should move the photo into the trash"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || !target.path.exists()),
        "the file should leave the library folder"
    );

    let overlay = viewer.imp().toast_overlay.get().clone();
    // Wait for the toast's Undo button to be *mapped*, and click it from the
    // widget found by that same wait.
    //
    // Holding a button across the earlier database waits used to work only
    // because nothing else in the shell was competing for the main loop. With
    // the domain-event consumer wired in (which is what the application does),
    // a trash change also schedules an album refresh, and the 6-second toast
    // could be gone — or already animating out — by the time the click was
    // aimed, leaving a stale reference that hit-tests to nothing. Re-locating
    // the control immediately before pressing it is also the more honest test:
    // the claim is that the toast offers a *reachable* way back
    // (`toasts::success_with_action`: "cannot reach before the toast disappears
    // is not an undo"), so the wait is part of the assertion, not a workaround.
    let undo_label = tr("viewer.toast.undo");
    let mut undo_found: Option<gtk::Button> = None;
    let reached = ui.wait_until(Duration::from_secs(4), || {
        undo_found = find_toast(&overlay)
            .and_then(|toast| find_button_with_label(&toast, &undo_label))
            .filter(|button| button.is_mapped());
        undo_found.is_some()
    });
    let undo = undo_found.unwrap_or_else(|| {
        panic!(
            "the delete toast should offer a reachable way back; overlay children mapped: {:?}",
            descendants(&overlay)
                .iter()
                .map(|w: &gtk::Widget| (w.type_().name().to_string(), w.is_mapped()))
                .collect::<Vec<_>>()
        )
    });
    assert!(
        reached,
        "the delete toast's Undo should be on screen, not dismissed before it could be pressed"
    );
    let toast_ui = Ui::for_widget(&undo);
    toast_ui.click(&undo, "Undo");

    assert!(
        ui.wait_until(Duration::from_secs(8), || db::get_media_item(
            &shell.pool,
            target.id
        )
        .is_ok_and(|item| item.trashed_at.is_none())
            && target.path.exists()),
        "Undo should put the file back on disk and clear its trash mark"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || viewer
            .imp()
            .current_media_id
            .get()
            == target.id
            && shell.visible_photos_grid().logical_media_count()
                == shell.items.len() as u32),
        "Undo should show the restored photo again and put its tile back in the grid"
    );
}

/// Rename a photo from the Viewer's details panel and verify the file on disk
/// actually changed name, the old path is gone, and the grid now shows the new
/// one. This is the durable promise: a user's photo is now called something else.
fn journey_rename_in_details_renames_the_file_on_disk() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let target = shell.items[0].clone();
    let old_path = target.path.clone();

    let tile = shell.tile_for(&grid, MediaId::from(target.id), "the Photos grid");
    ui.click(&tile, "the photo to rename");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the photo");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading"
    );

    ui.click(&viewer.imp().details_btn.get(), "Details");
    assert!(
        viewer.imp().details_split_view.get().shows_sidebar(),
        "Details should open"
    );

    // The rename row is an `AdwActionRow`. A press on its title has to travel
    // through the enclosing `GtkListBox` before the row activates — which is how
    // the user reaches the inline rename entry.
    let name_row = viewer.imp().name_row.get();
    ui.click(&name_row, "the file-name row");
    let entry = viewer.imp().name_entry.get();
    assert!(
        ui.wait_until(Duration::from_secs(3), || {
            gtk::prelude::WidgetExt::is_visible(&entry)
        }),
        "clicking the file-name row should reveal the inline rename entry"
    );
    assert_eq!(
        entry.text().as_str(),
        old_path.file_stem().unwrap().to_string_lossy(),
        "the entry should be pre-filled with the stem only, not the whole file name"
    );

    ui.type_entry(&entry, "holiday photo");
    ui.activate_entry(&entry);

    let renamed = old_path.parent().unwrap().join("holiday photo.jpg");
    assert!(
        ui.wait_until(Duration::from_secs(8), || renamed.is_file()
            && !old_path.exists()),
        "renaming should rename the real file, extension preserved, and leave the old path"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || db::get_media_item(
            &shell.pool,
            target.id
        )
        .is_ok_and(
            |item| item.display_name() == "holiday photo.jpg" && item.path == renamed
        )),
        "the library row should point at the renamed file"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || shell
            .visible_photos_grid()
            .tile_for_media(MediaId::from(target.id))
            .is_some_and(|tile| tile.cache_key().is_some())),
        "the grid should repaint the tile under its new name rather than lose it"
    );
    assert!(
        renamed.is_file() && renamed.metadata().unwrap().len() > 0,
        "the renamed file should still carry the photo's bytes"
    );
}

/// Edit and Save Overwrite, confirming the destructive dialog, then verify the
/// original really was rewritten and really was backed up — the two things a user
/// cares about when they agree to overwrite.
fn journey_save_overwrite_rewrites_the_file_and_keeps_a_backup() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let target = shell.items[0].clone();
    let original_bytes = std::fs::read(&target.path).unwrap();
    let count_before = db::list_all_media(&shell.pool).unwrap().len();

    let tile = shell.tile_for(&grid, MediaId::from(target.id), "the Photos grid");
    ui.click(&tile, "the photo to overwrite");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the photo");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading and enable Edit"
    );

    ui.click(&viewer.imp().edit_btn.get(), "Edit");
    let editor = viewer.imp().editor_panel.get();
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()
            && editor.imp().source_image.borrow().is_some()),
        "Edit should load the source image"
    );
    editor.imp().contrast_scale.get().set_value(-30.0);
    assert!(
        ui.wait_until(Duration::from_secs(5), || !editor
            .imp()
            .render_running
            .get()
            && !editor.imp().render_pending.get()),
        "the preview should settle before saving"
    );

    ui.click(&editor.imp().save_overwrite_btn.get(), "Save Overwrite");
    // The app asks first, and the destructive answer is a real button.
    let confirm = wait_for_alert(ui, &shell.window, &tr("dialog.overwrite"))
        .expect("overwriting should ask for confirmation");
    let confirm_ui = Ui::for_widget(&confirm);
    let overwrite = find_button_with_label(&confirm, &tr("dialog.overwrite"))
        .expect("the confirmation should offer the overwrite response");
    assert!(
        confirm_ui.pointer_at_center_of(&overwrite).is_some(),
        "the overwrite button should be visible before it is clicked"
    );
    confirm_ui.click(&overwrite, "Overwrite");

    let saved = ui.wait_until(Duration::from_secs(12), || {
        std::fs::read(&target.path)
            .map(|bytes| bytes != original_bytes)
            .unwrap_or(false)
    });
    assert!(
        saved,
        "confirming the overwrite should rewrite the original file's bytes"
    );

    let backup = {
        let mut name = target
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        name.push_str(".bak");
        target.path.parent().unwrap().join(name)
    };
    assert!(
        backup.is_file(),
        "an overwrite should leave the .bak the dialog promised, looked for at {}",
        backup.display()
    );
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        original_bytes,
        "the backup should hold the pre-edit bytes"
    );
    assert_eq!(
        db::list_all_media(&shell.pool).unwrap().len(),
        count_before,
        "overwriting must not add a library row"
    );
    let item = db::get_media_item(&shell.pool, target.id).unwrap();
    assert_eq!(
        item.path, target.path,
        "the row should still point at the original"
    );
    // The library row follows the bytes asynchronously: the encoder replaces the
    // file first, then the editor hands the recomputed metadata to the database
    // actor, and only then does the row carry the new content hash. Reading the
    // row once — which is what an earlier version of this assertion did — races
    // that commit and reports the *old* row as a product failure roughly half the
    // time, because the scanner deliberately leaves the column empty
    // (`local.rs`: "The column is left empty") while the editor fills it with a
    // real blake3 of the rewritten file. So this waits for the durable effect
    // instead of sampling it, and still fails loudly if the hash never lands.
    let rehashed = ui.wait_until(Duration::from_secs(15), || {
        db::get_media_item(&shell.pool, target.id)
            .is_ok_and(|row| !row.blake3_hash.is_empty() && row.blake3_hash != target.blake3_hash)
    });
    let item = db::get_media_item(&shell.pool, target.id).unwrap();
    assert!(
        rehashed,
        "the row's recorded content hash should follow the new bytes, \
         got {:?} (pre-edit was {:?})",
        item.blake3_hash, target.blake3_hash
    );
}

/// Edit a photo, ask to leave, and be asked first — then prove both answers do
/// what the user reads on the buttons: "keep editing" really preserves the pending
/// edit, "discard" really closes the editor and really does not touch the file.
fn journey_editor_close_guard_keeps_or_discards_pending_edits() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let target = shell.items[0].clone();
    let original_bytes = std::fs::read(&target.path).unwrap();

    let tile = shell.tile_for(&grid, MediaId::from(target.id), "the Photos grid");
    ui.click(&tile, "the photo to edit");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the photo");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading and enable Edit"
    );
    ui.click(&viewer.imp().edit_btn.get(), "Edit");
    let editor = viewer.imp().editor_panel.get();
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()
            && editor.imp().source_image.borrow().is_some()),
        "Edit should load the source image"
    );

    // Make a real, uncommitted change.
    editor.imp().brightness_scale.get().set_value(35.0);
    assert!(
        ui.wait_until(Duration::from_secs(5), || editor
            .imp()
            .state
            .borrow()
            .has_pending_edits()),
        "moving the brightness slider should leave a pending edit"
    );

    // Ask to leave. The app has to answer before anything is lost.
    ui.click(&editor.imp().editor_close_btn.get(), "Close editor");
    assert!(
        ui.wait_until(Duration::from_secs(4), || viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()),
        "closing with a pending edit must not discard it silently"
    );
    let guard = wait_for_alert(ui, &shell.window, &tr("editor.unsaved.keep"))
        .expect("the editor should ask before throwing away an edit");
    let guard_ui = Ui::for_widget(&guard);

    // "Keep editing": the panel stays and the edit survives.
    let keep = find_button_with_label(&guard, &tr("editor.unsaved.keep")).expect("Keep editing");
    guard_ui.click(&keep, "Keep editing");
    assert!(
        ui.wait_until(Duration::from_secs(4), || viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()),
        "keeping the edits should leave the editor open"
    );
    assert!(
        editor.imp().state.borrow().brightness != 0,
        "the brightness change should still be there after keeping it"
    );
    assert!(
        editor.imp().close_guard.borrow().is_none(),
        "an answered guard dialog should be released so the user can be asked again"
    );

    // Ask again and discard: the editor closes, and the file is byte-for-byte
    // untouched, because discarding is not saving.
    ui.click(&editor.imp().editor_close_btn.get(), "Close editor again");
    let guard = wait_for_alert(ui, &shell.window, &tr("editor.unsaved.discard"))
        .expect("the second exit request should ask again");
    let guard_ui = Ui::for_widget(&guard);
    let discard = find_button_with_label(&guard, &tr("editor.unsaved.discard")).expect("Discard");
    guard_ui.click(&discard, "Discard changes");

    assert!(
        ui.wait_until(Duration::from_secs(5), || !viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()),
        "discarding should close the editor and leave the user at the photo"
    );
    assert_eq!(
        std::fs::read(&target.path).unwrap(),
        original_bytes,
        "discarding an edit must not write to the original file"
    );
    assert!(
        viewer
            .ancestor(adw::NavigationPage::static_type())
            .is_some()
            || nav.visible_page().and_downcast::<ViewerPage>().is_some(),
        "discarding should leave the user on the photo they were looking at"
    );
}

/// Select with the context menu the way a user does — right-click a photo, choose
/// multi-select, click the second tile — then favorite the selection from the
/// batch bar and check the database.
fn journey_context_menu_select_then_favorite_the_selection() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let grid = shell.visible_photos_grid();
    let (first, second) = (&shell.items[0], &shell.items[1]);

    enter_multi_select(&shell, &grid, first);

    // A plain click on another tile now toggles it into the selection.
    let tile2 = shell.tile_for(&grid, MediaId::from(second.id), "the Photos grid");
    ui.click(
        &tile2,
        &format!("{} to add it to the selection", second.display_name()),
    );
    assert!(
        ui.wait_until(Duration::from_secs(4), || {
            let ids = grid.selected_ids();
            ids.len() == 2
                && ids.contains(&MediaId::from(first.id))
                && ids.contains(&MediaId::from(second.id))
        }),
        "clicking a second tile in multi-select should add it to the selection"
    );

    // Favorite both, from the batch bar.
    ui.click(&shell.photos.imp().favorite_btn.get(), "batch Favorite");
    assert!(
        ui.wait_until(Duration::from_secs(6), || db::is_media_favorite(
            &shell.pool,
            first.id
        )
        .unwrap_or(false)
            && db::is_media_favorite(&shell.pool, second.id).unwrap_or(false)),
        "the batch favorite should persist to both selected photos"
    );
    assert!(
        ui.wait_until(Duration::from_secs(4), || db::get_media_item(
            &shell.pool,
            first.id
        )
        .is_ok_and(|item| item.is_favorite)),
        "the library row itself should report the favorite"
    );
}

/// Empty the whole Trash in one action, and verify the photos are gone for good:
/// the files leave the trash directory as well as the database, and the page the
/// user is standing on stays open instead of being popped by its own success.
fn journey_empty_trash_destroys_every_photo_for_good() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let paths: Vec<PathBuf> = shell.items.iter().map(|i| i.path.clone()).collect();

    // Put both photos in the Trash first, through the batch bar.
    let grid = shell.visible_photos_grid();
    select_every_photo(&shell, &grid);
    ui.click(
        &shell.photos.imp().delete_to_trash_btn.get(),
        "Move to Trash",
    );
    respond_to_alert(ui, &shell.window, &tr("dialog.trash"));
    assert!(
        ui.wait_until(Duration::from_secs(8), || db::list_trashed_media(
            &shell.pool
        )
        .is_ok_and(|items| items.len() == 2)),
        "both photos should be in the Trash before it is emptied"
    );

    let trash = shell.open_trash();
    let trash_grid = trash
        .imp()
        .grid
        .borrow()
        .as_ref()
        .cloned()
        .expect("Trash grid");
    assert!(
        ui.wait_until(Duration::from_secs(5), || trash_grid.logical_media_count()
            == 2),
        "Trash should list both photos"
    );

    ui.click(&trash.imp().empty_btn.get(), "Empty Trash");
    respond_to_alert(ui, &shell.window, &tr("dialog.empty"));

    assert!(
        ui.wait_until(Duration::from_secs(10), || db::list_trashed_media(
            &shell.pool
        )
        .is_ok_and(|items| items.is_empty())),
        "confirming should empty the trashed rows"
    );
    assert!(
        paths.iter().all(|path| !path.exists()),
        "emptying the Trash must leave none of the files anywhere in the library"
    );
    // The files are gone for good, so nothing to clean up in the host trash.
    assert!(
        ui.wait_until(Duration::from_secs(5), || trash_grid.logical_media_count()
            == 0),
        "the Trash grid should reload to empty"
    );
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "the Trash page must stay open after Empty Trash"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || !trash
            .imp()
            .action_bar
            .get()
            .is_revealed()),
        "with nothing left, the batch bar should not be offering actions"
    );
}

/// Open a photo in the editor and hand the caller the panel, already loaded.
///
/// Every editor journey starts the same way a user does — grid, tile, viewer,
/// Edit — and that prefix is long enough that repeating it hides what each
/// journey is actually about. It also asserts the two things a scenario cannot
/// work without: the source image is decoded, and the panel has settled so a
/// click lands on a live button rather than one that is still spinning up.
///
/// `target` is the row to open rather than a slot index, because which photo a
/// journey needs is a property of its assertions: crop geometry is only
/// meaningful on a non-square source, and deriving that from the library's sort
/// order made the dependency invisible.
fn open_editor(
    shell: &Shell,
    target: &MediaItem,
    label: &str,
) -> (ViewerPage, photo_viewer::ui::EditorPanel) {
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();

    let tile = shell.tile_for(&grid, MediaId::from(target.id), "the Photos grid");
    ui.click(&tile, label);
    let viewer = expect_page::<ViewerPage>(ui, &nav, "opening the photo");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading and enable Edit"
    );

    ui.click(&viewer.imp().edit_btn.get(), "Edit");
    let editor = viewer.imp().editor_panel.get();
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .editor_split_view
            .get()
            .shows_sidebar()
            && editor.imp().source_image.borrow().is_some()),
        "Edit should load the source image into the editor panel"
    );
    (viewer, editor)
}

/// Wait until the editor's single-flight preview is neither running nor queued,
/// so the next click is judged against a settled panel.
fn settle_editor_preview(ui: &Ui, editor: &photo_viewer::ui::EditorPanel) {
    assert!(
        ui.wait_until(Duration::from_secs(10), || !editor
            .imp()
            .render_running
            .get()
            && !editor.imp().render_pending.get()),
        "the editor preview should finish rendering before the next assertion"
    );
}

/// Rotate a photo with the three rotation buttons, undo the rotation with the
/// header reset, and prove the two things that make this more than button
/// bookkeeping: the pending edit really is non-destructive (the source file is
/// byte-for-byte unchanged), and reset really returns the panel to the state it
/// opened in — including re-disarming itself, so a second press cannot fire a
/// reset that has nothing to do.
fn journey_editor_rotation_buttons_then_reset_restore_the_source() {
    let shell = Shell::new();
    let ui = &shell.ui;
    // Named rather than `items[0]`: rotation is asserted on the pending `Rotation`
    // delta and on disk bytes, never on pixel dimensions, so any fixture will do —
    // but naming it keeps the choice from silently re-deriving itself from the
    // library's sort order every time the fixture set changes.
    let target = shell.item_from_fixture("ridge-square");
    let (_viewer, editor) = open_editor(&shell, &target, "the photo to rotate");
    let original_bytes = std::fs::read(&target.path).unwrap();

    // Reset is armed by `has_pending_edits`, so with a clean session it must not
    // be pressable at all — a test that could reset an untouched panel would
    // pass no matter what the button did.
    assert_eq!(
        editor.imp().state.borrow().rotation,
        Rotation::None,
        "a freshly opened editor should have no pending rotation"
    );
    assert!(
        !editor.imp().reset_btn.get().is_sensitive(),
        "Reset must stay insensitive until there is a pending edit to undo"
    );

    // 90° clockwise, by pointer on the real button.
    ui.click(&editor.imp().rotate_90_cw.get(), "Rotate 90° clockwise");
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().state.borrow().rotation,
        Rotation::R90,
        "Rotate 90° CW should leave a 90° pending rotation"
    );
    assert!(
        editor.imp().reset_btn.get().is_sensitive(),
        "a pending rotation should arm Reset"
    );

    // Another 90° CW composes to 180° rather than replacing the first press.
    ui.click(
        &editor.imp().rotate_90_cw.get(),
        "Rotate 90° clockwise again",
    );
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().state.borrow().rotation,
        Rotation::R180,
        "rotating twice clockwise should compose to 180°"
    );

    // A half turn back returns to square one: the rotation is a delta on one
    // state, not a latch that only ever turns one way.
    ui.click(&editor.imp().rotate_180.get(), "Rotate 180°");
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().state.borrow().rotation,
        Rotation::None,
        "a 180° turn on top of 180° should land back at no rotation"
    );

    // Counter-clockwise is its own path, not a mirrored repeat of CW.
    ui.click(
        &editor.imp().rotate_90_ccw.get(),
        "Rotate 90° anticlockwise",
    );
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().state.borrow().rotation,
        Rotation::R270,
        "Rotate 90° CCW should leave a 270° pending rotation"
    );
    assert!(
        editor.imp().editor_dirty_label.get().is_visible(),
        "a pending edit should show the unsaved-changes label"
    );

    // Reset clears the pending edit without closing the editor.
    ui.click(&editor.imp().reset_btn.get(), "Reset");
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().state.borrow().rotation,
        Rotation::None,
        "Reset should clear the pending rotation"
    );
    assert!(
        !editor.imp().reset_btn.get().is_sensitive(),
        "Reset should disarm itself once there is nothing left to undo"
    );
    assert!(
        !editor.imp().editor_dirty_label.get().is_visible(),
        "with no pending edits the unsaved-changes label should go away"
    );
    assert!(
        editor.imp().source_image.borrow().is_some(),
        "Reset must not close the editor or drop the loaded source"
    );

    // The durable half of the contract: nothing above touched a byte on disk.
    assert_eq!(
        std::fs::read(&target.path).unwrap(),
        original_bytes,
        "rotating and resetting are pending edits; the source file must be byte-for-byte unchanged"
    );
    let item = db::get_media_item(&shell.pool, target.id).unwrap();
    assert_eq!(
        item.blake3_hash, target.blake3_hash,
        "a pending edit must not rewrite the library row's content hash"
    );
}

/// Enter crop mode, step the ratio selector with its two narrow arrows, and
/// prove the arrows really drive a *pending* crop.
///
/// The ratio selector is deliberately absent until crop mode is on
/// (`crop_ratio_box` is `visible: false` in the template and
/// `update_crop_controls` is what reveals it), so this journey is also the
/// evidence that the arrows are unreachable by design before the mode starts —
/// a group that leaked its controls would let a user crop without ever entering
/// crop mode. The arrows are also narrow vertical controls inside the editor's
/// scrolled side panel, which is exactly the sort of control a stale allocation
/// or a folded group makes unpressable, so they are reached by pointer and
/// scrolled to rather than set directly.
fn journey_editor_crop_ratio_arrows_drive_a_pending_crop() {
    // A 4:3 source, named explicitly. This journey's whole point is that stepping
    // the ring from "source" to 1:1 *changes the rectangle* — which is only true
    // when the source is not already square. Aiming at `items[0]` made that
    // dependency implicit and, once the fixtures became a multi-ratio set whose
    // first row is a 1:1, the "squaring should take width away" assertion failed
    // on a 420x420 source where stepping to 1:1 is correctly a no-op.
    let shell = Shell::with_photos(3);
    let ui = &shell.ui;
    let target = shell.item_from_fixture("coast-43");
    let (_viewer, editor) = open_editor(&shell, &target, "the photo to crop");
    let original_bytes = std::fs::read(&target.path).unwrap();
    let (source_w, source_h) = editor.imp().source_dimensions.get();
    assert!(
        source_w > 0 && source_h > 0,
        "the editor should know the source dimensions before cropping"
    );
    assert_eq!(
        (source_w, source_h),
        (480, 360),
        "this journey's geometry expectations are written for the 4:3 coast fixture, \
         so opening a differently-proportioned photo means they no longer hold"
    );

    // Before crop mode the ratio arrows are not on screen at all.
    assert!(
        !editor.imp().crop_ratio_box.get().is_visible(),
        "the ratio selector must stay hidden until crop mode is entered"
    );
    ui.assert_not_reachable(
        &editor.imp().crop_ratio_next_btn.get(),
        "Next crop ratio before crop mode",
    );
    assert!(
        editor.imp().state.borrow().crop.is_none(),
        "a freshly opened editor should have no pending crop"
    );

    // Start Crop is the only route into the mode, and entering it is what stages
    // the first rectangle.
    ui.click(&editor.imp().start_crop_btn.get(), "Start Crop");
    assert!(
        ui.wait_until(Duration::from_secs(5), || editor
            .imp()
            .crop_mode_active
            .get()),
        "Start Crop should enter crop mode"
    );
    assert!(
        ui.wait_until(Duration::from_secs(2), || editor
            .imp()
            .crop_ratio_box
            .get()
            .is_visible()),
        "entering crop mode should reveal the ratio selector"
    );
    let staged = editor
        .imp()
        .state
        .borrow()
        .crop
        .expect("entering crop mode should stage a crop rectangle");
    assert_eq!(
        staged,
        (0, 0, source_w, source_h),
        "the first ratio is the untouched source, so the rectangle should be the whole image"
    );
    let source_ratio_label = editor.imp().crop_ratio_label.get().text().to_string();

    // Scroll the arrows into reach, then step forward. The source is 4:3, so the
    // first step lands on 1:1 and genuinely narrows the rectangle: that geometry
    // change is the proof the press did something a label alone could not.
    ui.scroll_to_reveal(&editor.imp().crop_ratio_next_btn.get(), "Next crop ratio");
    ui.click(&editor.imp().crop_ratio_next_btn.get(), "Next crop ratio");
    settle_editor_preview(ui, &editor);
    let after_one = editor.imp().crop_ratio_label.get().text().to_string();
    assert_ne!(
        after_one, source_ratio_label,
        "the next arrow should move the ratio off {source_ratio_label:?}"
    );
    assert_eq!(
        after_one,
        tr("editor.crop.ratio.square"),
        "one step forward from the source ratio should offer 1:1"
    );
    let square = editor
        .imp()
        .state
        .borrow()
        .crop
        .expect("a 1:1 ratio should stage a rectangle");
    assert_eq!(
        square.2, square.3,
        "the staged rectangle should be square, got {square:?}"
    );
    assert!(
        square.2 < source_w,
        "squaring a {source_w}x{source_h} source should take width away, got {square:?}"
    );
    assert_eq!(
        square.3, source_h,
        "squaring should keep the full height and trim the sides, got {square:?}"
    );

    // A second step reaches 4:3, which for a 4:3 source is the whole frame
    // again — the rectangle grows back out to the full image, still inside the
    // strip, and still holding the aspect the selector promised.
    ui.click(
        &editor.imp().crop_ratio_next_btn.get(),
        "Next crop ratio a second time",
    );
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().crop_ratio_label.get().text().to_string(),
        tr("editor.crop.ratio.4_3"),
        "two steps forward from the source ratio should offer 4:3"
    );
    let four_three = editor
        .imp()
        .state
        .borrow()
        .crop
        .expect("a 4:3 ratio should stage a rectangle");
    assert!(
        four_three.2 * 3 == four_three.3 * 4,
        "the staged rectangle should hold the 4:3 aspect the selector promised, got {four_three:?}"
    );
    assert_eq!(
        four_three,
        (0, 0, source_w, source_h),
        "a 4:3 ratio on a 4:3 source should be the whole frame, got {four_three:?}"
    );

    // The previous arrow walks the same ring backwards, and returning to the
    // source ratio hands back the whole image rather than leaving the narrower
    // 4:3 rectangle staged with nothing on screen to explain it.
    ui.click(
        &editor.imp().crop_ratio_prev_btn.get(),
        "Previous crop ratio",
    );
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().crop_ratio_label.get().text().to_string(),
        after_one,
        "the previous arrow should step back to the 1:1 ratio"
    );
    ui.click(
        &editor.imp().crop_ratio_prev_btn.get(),
        "Previous crop ratio again",
    );
    settle_editor_preview(ui, &editor);
    assert_eq!(
        editor.imp().crop_ratio_label.get().text().to_string(),
        source_ratio_label,
        "stepping back twice should return the selector to the source ratio"
    );
    assert_eq!(
        editor.imp().state.borrow().crop,
        Some((0, 0, source_w, source_h)),
        "returning to the source ratio should restore the whole-image rectangle"
    );

    // Reset clears pending crop along with every other edit, per the editor's
    // single `has_pending_edits` contract.
    ui.click(&editor.imp().reset_btn.get(), "Reset");
    settle_editor_preview(ui, &editor);
    assert!(
        editor.imp().state.borrow().crop.is_none(),
        "Reset should clear the pending crop too"
    );
    assert!(
        !editor.imp().crop_mode_active.get(),
        "Reset should leave crop mode"
    );
    assert!(
        ui.wait_until(Duration::from_secs(2), || !editor
            .imp()
            .crop_ratio_box
            .get()
            .is_visible()),
        "leaving crop mode should take the ratio selector away again"
    );

    assert_eq!(
        std::fs::read(&target.path).unwrap(),
        original_bytes,
        "staging and clearing a crop is a pending edit; the source must be unchanged"
    );
}

/// Enter album multi-select the way a user does — right-click a real album row,
/// choose Multi select, then tick real album rows by clicking them — and return
/// the ticked albums together with the folder paths they stand for.
///
/// A test that set `album_selection_bar` visible or pushed paths into the
/// selection itself would never notice that the context menu is unreachable, so
/// the whole route in is driven by pointer.
fn tick_real_albums_in_multi_select(shell: &Shell) -> Vec<PathBuf> {
    let ui = &shell.ui;
    let window = &shell.window;
    let album_name = real_album_name_in_sidebar(shell);
    let row_label = wait_for_label_containing(
        &window.imp().album_list.get(),
        &album_name,
        Duration::from_secs(5),
    )
    .expect("the sidebar should list a real album");
    ui.right_click(&row_label, &format!("{album_name:?} album row"));
    let multi = wait_for_button_with_label(
        window,
        &tr("album.context.multi_select"),
        Duration::from_secs(4),
    )
    .expect("the album row menu should offer multi-select");
    ui.click(&multi, "Multi select albums");

    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .imp()
            .album_selection_bar
            .get()
            .is_revealed()),
        "album multi-select should reveal the batch bar"
    );

    for name in sidebar_real_album_names(shell) {
        let label = find_label_containing(&window.imp().album_list.get(), &name)
            .unwrap_or_else(|| panic!("the sidebar should still list {name:?}"));
        ui.click(&label, &format!("{name:?} album row"));
    }
    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .selected_album_delete_count()
            >= 1),
        "clicking real album rows should add them to the album selection"
    );
    assert!(
        window.imp().album_selection_delete_btn.get().is_sensitive(),
        "selecting a real album should arm the delete action"
    );

    window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .filter(|album| !album.is_virtual)
        .map(|album| album.folder_path.clone())
        .collect()
}

/// Cancel out of album multi-select with real albums already ticked, and prove
/// the two things a user would be afraid of: nothing is deleted, and the mode
/// really is over rather than left armed behind a hidden bar.
///
/// `album_selection_cancel_btn` had no scenario at all — its sibling Delete was
/// exercised only as far as "the button is now sensitive", so the button a user
/// reaches for when they change their mind was the one control in this bar with
/// no evidence behind it.
fn journey_album_multi_select_cancel_keeps_every_album_on_disk() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let window = &shell.window;
    shell.seed_extra_album();
    window.populate_album_rows();

    let album_dirs = tick_real_albums_in_multi_select(&shell);
    assert!(
        !album_dirs.is_empty() && album_dirs.iter().all(|dir| dir.is_dir()),
        "the fixture should give at least one real album directory on disk, got {album_dirs:?}"
    );

    ui.click(
        &window.imp().album_selection_cancel_btn.get(),
        "Cancel album selection",
    );

    assert!(
        ui.wait_until(Duration::from_secs(4), || !window
            .imp()
            .album_selection_bar
            .get()
            .is_revealed()),
        "Cancel should take the album batch bar away"
    );
    assert_eq!(
        window.selected_album_delete_count(),
        0,
        "Cancel should clear the pending album selection rather than keep it armed"
    );
    assert!(
        !window.imp().album_selection_delete_btn.get().is_sensitive(),
        "leaving multi-select should disarm Delete, so a later stray press cannot fire it"
    );
    for dir in &album_dirs {
        assert!(
            dir.is_dir(),
            "Cancel must not delete anything: {} is gone",
            dir.display()
        );
    }
    assert!(
        !sidebar_real_album_names(&shell).is_empty(),
        "the sidebar should still list its real albums after cancelling"
    );
}

/// Delete one album through the batch bar and the confirmation dialog, and end
/// on the durable result the user would check: the photos in that album are
/// gone from the library and recoverable in the Trash, the album itself leaves
/// the sidebar because it has no live photos left, and every other album is
/// untouched.
///
/// The existing sidebar contract stopped at "the delete button is now
/// sensitive", so the destructive response — the part that actually destroys a
/// user's folder arrangement — had no evidence behind it.
fn journey_album_multi_select_delete_moves_that_album_to_the_trash() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let window = &shell.window;
    let extra = shell.seed_extra_album();
    window.populate_album_rows();

    // The album's own photo, and a photo belonging to an album that must survive.
    let doomed = extra.join("three.jpg");
    let survivor_dirs: Vec<PathBuf> = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .filter(|album| !album.is_virtual)
        .map(|album| album.folder_path.clone())
        .filter(|dir| dir != &extra)
        .collect();
    let survivor_photos: Vec<PathBuf> = survivor_dirs
        .iter()
        .flat_map(|dir| {
            shell
                .items
                .iter()
                .map(|i| i.path.clone())
                .filter(move |p| p.starts_with(dir))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        doomed.is_file(),
        "the extra album should hold a real photo at {}",
        doomed.display()
    );
    assert!(
        !survivor_photos.is_empty(),
        "the fixture should keep at least one other album's photo so the journey can prove \
         the deletion is selective"
    );
    let survivor_bytes: Vec<Vec<u8>> = survivor_photos
        .iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect();

    // Tick only the extra album, so the deletion has to be selective to pass.
    let album_name = extra.file_name().unwrap().to_string_lossy().into_owned();
    let row_label = wait_for_label_containing(
        &window.imp().album_list.get(),
        &album_name,
        Duration::from_secs(5),
    )
    .unwrap_or_else(|| panic!("the sidebar should list {album_name:?}"));
    ui.right_click(&row_label, &format!("{album_name:?} album row"));
    let multi = wait_for_button_with_label(
        window,
        &tr("album.context.multi_select"),
        Duration::from_secs(4),
    )
    .expect("the album row menu should offer multi-select");
    ui.click(&multi, "Multi select albums");
    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .imp()
            .album_selection_bar
            .get()
            .is_revealed()),
        "album multi-select should reveal the batch bar"
    );
    ui.click(&row_label, &format!("{album_name:?} album row"));
    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .selected_album_delete_count()
            == 1),
        "ticking one album should put exactly one album in the selection"
    );

    ui.click(
        &window.imp().album_selection_delete_btn.get(),
        "Delete selected albums",
    );
    respond_to_alert(ui, window, &tr("album.delete.confirm_action"));

    // Durable result 1: the album's photo left the library on disk, recoverable
    // in the Trash rather than destroyed.
    assert!(
        ui.wait_until(Duration::from_secs(15), || !doomed.exists()),
        "confirming should move the album's photo out of the library, looked for at {}",
        doomed.display()
    );
    assert!(
        ui.wait_until(Duration::from_secs(8), || db::list_trashed_media(
            &shell.pool
        )
        .is_ok_and(|items| items.iter().any(|item| item.path == doomed))),
        "the album's photo should be in the Trash, not destroyed"
    );

    // Durable result 2: the album leaves the sidebar, because it has no live
    // photos left to represent it.
    assert!(
        ui.wait_until(Duration::from_secs(8), || !window
            .imp()
            .album_targets
            .borrow()
            .iter()
            .any(|album| album.folder_path == extra)),
        "a deleted album should be gone from the sidebar's album list"
    );

    // Durable result 3: the other albums are untouched, byte for byte.
    for (path, bytes) in survivor_photos.iter().zip(&survivor_bytes) {
        assert_eq!(
            &std::fs::read(path).unwrap(),
            bytes,
            "deleting one album must not touch the photos in another: {}",
            path.display()
        );
    }
    assert!(
        db::list_all_media(&shell.pool)
            .unwrap()
            .iter()
            .all(|item| item.trashed_at.is_none()),
        "only the ticked album's photos should be trashed"
    );
    assert!(
        ui.wait_until(Duration::from_secs(5), || !window
            .imp()
            .album_selection_bar
            .get()
            .is_revealed()),
        "finishing the deletion should leave album multi-select"
    );
    assert_eq!(
        window.selected_album_delete_count(),
        0,
        "the pending album selection should be cleared once the deletion is done"
    );
}

/// The cloud badge is a promise about sync state that has to survive the jump
/// from a grid tile to the Viewer header, and change when the photo does.
/// The cloud-badge images of one class currently under `grid`.
fn sync_badges(grid: &VirtualMediaGrid, class: &str) -> Vec<gtk::Image> {
    descendants::<gtk::Image>(grid)
        .into_iter()
        .filter(|image| image.has_css_class(class))
        .collect()
}

fn synced_image_badge_survives_the_jump_from_day_grid_to_viewer() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let grid = shell.visible_photos_grid();
    let badges = |root: &VirtualMediaGrid, class: &str| sync_badges(root, class);

    assert!(
        ui.wait_until(Duration::from_secs(5), || !badges(
            &grid,
            "thumb-sync-badge"
        )
        .is_empty()),
        "Day tiles should carry a cloud badge widget"
    );

    // Mark the newest photo synced — rank 1, so the header's Next is live and the
    // journey can walk to an unsynced neighbour rather than into a dimmed arrow.
    let synced = &shell.items[0];
    let store = SyncStore::new(shell.pool.clone());
    let job = store
        .create_job(&NewSyncJob {
            endpoint: "https://dav.example.test/root/".into(),
            username: "alice".into(),
            credential_ref: "ux-sync-badge".into(),
            local_root: synced.folder_path.clone(),
            remote_root: "PhotoViewer".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::All,
            upload_albums: Vec::new(),
        })
        .unwrap();
    let fingerprint = Fingerprint {
        size: synced.file_size,
        blake3: "verified-sync-content".into(),
    };
    let conn = shell.pool.get().unwrap();
    conn.execute(
        "UPDATE media_items SET blake3_hash = '' WHERE id = ?1",
        [synced.id],
    )
    .unwrap();
    let mtime_ns: i64 = conn
        .query_row(
            "SELECT file_mtime_ns FROM media_items WHERE id = ?1",
            [synced.id],
            |row| row.get(0),
        )
        .unwrap();
    let entry = store
        .upsert_observation(
            job.id,
            synced.display_name(),
            Some(&fingerprint),
            Some(mtime_ns),
            Some(&fingerprint),
            Some("etag"),
            false,
            "pending",
        )
        .unwrap();
    store
        .commit_baseline(entry, &fingerprint, Some("etag"))
        .unwrap();

    grid.refresh_sync_badges();
    assert!(
        ui.wait_until(Duration::from_secs(6), || {
            let shown = badges(&grid, "thumb-sync-badge")
                .into_iter()
                .filter(|image| image.is_visible())
                .filter_map(|image| image.resource())
                .collect::<Vec<_>>();
            shown.iter().any(|p| p.ends_with("gnome-cloud-white.png"))
                && shown
                    .iter()
                    .any(|p| p.ends_with("gnome-cloud-off-white.png"))
        }),
        "Day view should distinguish the synced photo from the unsynced ones"
    );

    let tile = shell.tile_for(&grid, MediaId::from(synced.id), "the Day grid");
    ui.click(&tile, "the synced photo");
    let viewer =
        expect_page::<ViewerPage>(ui, &shell.window.nav_view(), "opening the synced photo");
    let header_badge = viewer.imp().sync_badge.get();
    assert!(
        ui.wait_until(Duration::from_secs(6), || header_badge.is_visible()
            && header_badge.resource().is_some_and(
                |p| p.contains("gnome-cloud-") && !p.contains("cloud-off")
            )),
        "the synced photo should keep its cloud mark in the Viewer header"
    );

    // Navigate to an unsynced neighbour by real click and watch the mark change.
    ui.click(&viewer.imp().next_btn.get(), "Next");
    assert!(
        ui.wait_until(Duration::from_secs(8), || {
            let id = viewer.imp().current_media_id.get();
            id != synced.id
                && header_badge
                    .resource()
                    .is_some_and(|p| p.contains("cloud-off"))
        }),
        "moving to an unsynced photo should switch the header mark to cloud-off"
    );
}

// ---------------------------------------------------------------------------
// Interaction contracts
// ---------------------------------------------------------------------------

/// The year/month/day capsule is the app's canonical segmented control, and its
/// cells are bare `Gtk.Box` widgets with a gesture — the kind of control a signal
/// test can fake. Aim at each cell's own label and check the view underneath
/// really swaps.
fn mode_selector_click_switches_photos_view() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let selector = shell
        .photos
        .imp()
        .mode_selector
        .get()
        .clone()
        .downcast::<ModeSelector>()
        .expect("PhotosPage should contain a ModeSelector");
    let stack = shell.photos.imp().view_stack.get();

    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("day"),
        "Photos starts on Day when media exists"
    );

    for (label_key, expected) in [
        ("photo.mode.year", "year"),
        ("photo.mode.month", "month"),
        ("photo.mode.day", "day"),
    ] {
        let cell_label = find_label_containing(&selector, &tr(label_key))
            .unwrap_or_else(|| panic!("the capsule should show a {} cell", tr(label_key)));
        ui.click(&cell_label, &tr(label_key));
        assert_eq!(
            stack.visible_child_name().as_deref(),
            Some(expected),
            "clicking the {} cell should switch the Photos view",
            tr(label_key)
        );
        // The control is a radio group: exactly one cell carries the checked state.
        assert_eq!(
            selector.imp().active_index.get(),
            ["year", "month", "day"]
                .iter()
                .position(|name| *name == expected)
                .unwrap() as u32,
            "the capsule's internal state should agree with the view it drives"
        );
    }
}

/// A double click is two presses with the loop not running in between, which is
/// when the "push exactly one Viewer" guard actually matters.
fn tile_double_click_opens_exactly_one_viewer() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let tile = shell.tile_for(&grid, MediaId::from(shell.items[0].id), "the Photos grid");

    ui.double_click(&tile, "the same photo twice");
    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "a double click on a photo should push exactly one Viewer page"
    );
    assert!(
        nav.visible_page().and_downcast::<ViewerPage>().is_some(),
        "the Viewer should be what the user landed on"
    );
}

fn search_result_double_click_opens_exactly_one_viewer_while_pending() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();

    ui.click(
        &shell.photos.imp().search_btn.get(),
        "Search toolbar button",
    );
    let search = expect_page::<SearchPage>(ui, &nav, "Ctrl+F should open Search");
    ui.type_search(&search.imp().search_entry.get(), "photo-0");
    let result = wait_for_flowbox_result(ui, &search, "photo-0");

    ui.double_click(&result, "the search result twice");
    assert_eq!(
        nav.navigation_stack().n_items(),
        3,
        "a double click on a search result should push exactly one Viewer page"
    );
    assert!(
        nav.visible_page().and_downcast::<ViewerPage>().is_some(),
        "the Viewer should be visible"
    );
}

/// The production capture-phase router, through a realized window: the binding
/// table and its gating are what a user's fingers actually reach.
fn keyboard_shortcuts_drive_full_shell_navigation() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();

    assert!(
        ui.press_key(
            &shell.window,
            gtk::gdk::Key::a,
            gtk::gdk::ModifierType::CONTROL_MASK
        ),
        "Ctrl+A should be handled by the router"
    );
    assert!(
        ui.wait_until(Duration::from_secs(8), || grid.is_all_displayed_selected()),
        "Ctrl+A should select the rendered Photos tiles"
    );
    assert!(
        shell.photos.imp().favorite_revealer.get().reveals_child(),
        "keyboard selection should expose the same batch actions as pointer selection"
    );

    assert!(ui.press_key(
        &shell.window,
        gtk::gdk::Key::Escape,
        gtk::gdk::ModifierType::empty()
    ));
    assert!(
        ui.wait_until(Duration::from_secs(4), || !shell
            .photos
            .imp()
            .favorite_revealer
            .get()
            .reveals_child()),
        "Escape should clear the selection"
    );
    assert_eq!(
        nav.navigation_stack().n_items(),
        1,
        "clearing a selection with Escape must keep the browsing root visible"
    );

    assert!(ui.press_key(
        &shell.window,
        gtk::gdk::Key::f,
        gtk::gdk::ModifierType::CONTROL_MASK
    ));
    let search = expect_page::<SearchPage>(ui, &nav, "Ctrl+F should open Search");
    assert!(
        ui.wait_until(Duration::from_secs(3), || contains_focus(
            search.imp().search_entry.get().upcast_ref()
        )),
        "opening Search by keyboard should move focus into the query field"
    );
    assert!(
        !ui.press_key(
            &shell.window,
            gtk::gdk::Key::f,
            gtk::gdk::ModifierType::CONTROL_MASK
        ),
        "Ctrl+F must not be intercepted while the Search entry owns text input"
    );
    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "Ctrl+F from the Search entry must not push a duplicate page"
    );
    assert!(nav.pop(), "returning from Search should pop one page");
    assert!(
        ui.wait_until(Duration::from_secs(3), || nav
            .visible_page()
            .and_downcast::<SearchPage>()
            .is_none()),
        "popping Search should restore the browsing root"
    );

    // Open a photo so the Viewer bindings are the ones in scope.
    let tile = shell.tile_for(&grid, MediaId::from(shell.items[0].id), "the Photos grid");
    ui.click(&tile, "a photo");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "a photo click should open the Viewer");
    let media_id = viewer.imp().current_media_id.get();
    assert_ne!(media_id, 0, "the Viewer should resolve an active media id");

    assert!(ui.press_key(
        &shell.window,
        gtk::gdk::Key::i,
        gtk::gdk::ModifierType::empty()
    ));
    assert!(
        viewer.imp().details_split_view.get().shows_sidebar(),
        "I should reveal the details panel"
    );
    assert!(ui.press_key(
        &shell.window,
        gtk::gdk::Key::i,
        gtk::gdk::ModifierType::empty()
    ));
    assert!(
        !viewer.imp().details_split_view.get().shows_sidebar(),
        "I again should close it"
    );
    assert!(
        ui.wait_until(Duration::from_secs(4), || viewer.can_pop()),
        "the Viewer should re-enable Back/Escape after the close transition"
    );

    assert!(ui.press_key(
        &shell.window,
        gtk::gdk::Key::h,
        gtk::gdk::ModifierType::empty()
    ));
    assert!(
        ui.wait_until(Duration::from_secs(5), || db::is_media_favorite(
            &shell.pool,
            media_id
        )
        .unwrap_or(false)),
        "H should persist the favorite, not just animate the button"
    );

    // Holding the key is one action, not one per auto-repeat.
    let hold = ui.key_gesture(
        &shell.window,
        gtk::gdk::Key::h,
        gtk::gdk::ModifierType::empty(),
        4,
    );
    assert_eq!(
        hold.dispatched, 1,
        "a held H must dispatch exactly one action, but {} of 5 presses went through",
        hold.dispatched
    );

    assert!(ui.press_key(
        &shell.window,
        gtk::gdk::Key::Escape,
        gtk::gdk::ModifierType::empty()
    ));
    assert!(
        ui.wait_until(Duration::from_secs(4), || nav.navigation_stack().n_items()
            == 1
            && nav.visible_page().and_downcast::<ViewerPage>().is_none()),
        "Escape should return from the Viewer to the browsing root"
    );
}

/// The batch toolbar is a set of reveals driven by selection; every button on it
/// has to answer a pointer at its own centre and then really change the library.
fn photos_batch_toolbar_clicks_select_favorite_and_album() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let grid = shell.visible_photos_grid();
    let first = MediaId::from(shell.items[0].id);

    let first_item = shell.items[0].clone();
    enter_multi_select(&shell, &grid, &first_item);
    assert!(
        grid.selected_ids() == vec![first],
        "the right-clicked photo should be the whole selection"
    );
    assert!(
        shell
            .photos
            .imp()
            .add_to_album_revealer
            .get()
            .reveals_child()
            && shell.photos.imp().favorite_revealer.get().reveals_child()
            && shell
                .photos
                .imp()
                .delete_to_trash_revealer
                .get()
                .reveals_child(),
        "a selection should reveal every batch action"
    );

    ui.click(&shell.photos.imp().select_all_btn.get(), "Select All");
    assert!(
        ui.wait_until(Duration::from_secs(8), || grid.is_all_displayed_selected()),
        "clicking Select All should select every rendered tile"
    );
    ui.click(&shell.photos.imp().select_all_btn.get(), "Select All again");
    assert!(
        ui.wait_until(Duration::from_secs(4), || !shell
            .photos
            .imp()
            .favorite_revealer
            .get()
            .reveals_child()),
        "the toggled Select All should clear the selection and hide the batch bar"
    );

    // Re-select one photo and favorite it through the bar.
    let grid = shell.visible_photos_grid();
    enter_multi_select(&shell, &grid, &first_item);
    ui.click(&shell.photos.imp().favorite_btn.get(), "batch Favorite");
    assert!(
        ui.wait_until(Duration::from_secs(6), || db::is_media_favorite(
            &shell.pool,
            shell.items[0].id
        )
        .unwrap_or(false)),
        "the batch favorite button should persist the favorite"
    );

    let grid = shell.visible_photos_grid();
    enter_multi_select(&shell, &grid, &first_item);
    ui.click(&shell.photos.imp().add_to_album_btn.get(), "Add to Album");
    let picker = album_picker_dialog(ui, &shell.window);
    assert!(
        find_descendant::<gtk::FlowBox>(&picker).is_some()
            || !descendants::<photo_viewer::ui::SquareTile>(&picker).is_empty(),
        "the picker should list real albums"
    );
    assert_eq!(
        shell.window.nav_view().navigation_stack().n_items(),
        1,
        "opening the picker must not push a navigation page"
    );
}

/// The Viewer's details rename entry and the zoom cluster's visibility rules.
/// Pointer reachability of this chrome is covered by
/// `ux_viewer_pointer_flows.rs`; what stays here is the state machine the buttons
/// drive, reached by opening the photo through a real click.
fn viewer_details_and_zoom_clicks_drive_visible_operations() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let nav = shell.window.nav_view();
    let grid = shell.visible_photos_grid();
    let target = shell.items[0].clone();

    let tile = shell.tile_for(&grid, MediaId::from(target.id), "the Photos grid");
    ui.click(&tile, "a photo");
    let viewer = expect_page::<ViewerPage>(ui, &nav, "a photo click should open the Viewer");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer
            .imp()
            .edit_btn
            .get()
            .is_sensitive()),
        "the photo should finish loading"
    );

    let initial_zoom = viewer.imp().zoom_scale.get();
    ui.click(&viewer.imp().zoom_in_btn.get(), "Zoom in");
    assert!(
        viewer.imp().zoom_scale.get() > initial_zoom,
        "zooming in should enlarge the stage"
    );
    assert!(
        !viewer.imp().rotate_left_btn.get().is_visible()
            && !viewer.imp().rotate_right_btn.get().is_visible(),
        "rotating is only offered at identity zoom, so the pair must hide once enlarged"
    );
    ui.click(&viewer.imp().zoom_reset_btn.get(), "Reset view");
    assert_eq!(
        viewer.imp().zoom_scale.get(),
        initial_zoom,
        "reset should restore the initial zoom"
    );
    ui.click(&viewer.imp().rotate_right_btn.get(), "Rotate right");
    assert_eq!(
        viewer.imp().viewer_rotation_degrees.get(),
        90,
        "rotate right should turn the image clockwise"
    );
    ui.click(&viewer.imp().rotate_left_btn.get(), "Rotate left");
    assert_eq!(
        viewer.imp().viewer_rotation_degrees.get(),
        0,
        "rotate left should bring it back"
    );

    // The header rank and the date follow the photo the user is on.
    let head_date = viewer.imp().date_label.get().text().to_string();
    ui.click(&viewer.imp().next_btn.get(), "Next");
    assert!(
        ui.wait_until(Duration::from_secs(8), || viewer.current_index() == 1),
        "Next should move to the second photo"
    );
    assert_ne!(
        viewer.imp().date_label.get().text().to_string(),
        head_date,
        "each photo has its own capture day, so the header date must change"
    );
}

/// The top-level sidebar: a header row that collapses its own list, and rows that
/// swap the browsing page.
fn sidebar_clicks_drive_top_level_navigation() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let window = &shell.window;

    let sidebar = window.imp().sidebar_list.get();
    assert_eq!(
        sidebar.observe_children().n_items(),
        2,
        "the top sidebar should hold Photos and the Albums header"
    );
    let header = sidebar.row_at_index(1).expect("the Albums header row");
    assert!(
        window.imp().album_scroll.get().is_visible(),
        "albums should start expanded"
    );
    ui.click(&header, "the Albums header row");
    assert!(
        ui.wait_until(Duration::from_secs(3), || !window
            .imp()
            .album_scroll
            .get()
            .is_visible()),
        "clicking the Albums header should collapse the album list"
    );
    ui.click(&header, "the Albums header row again");
    assert!(
        ui.wait_until(Duration::from_secs(3), || window
            .imp()
            .album_scroll
            .get()
            .is_visible()),
        "clicking it again should expand the album list"
    );

    let trash_row = window
        .imp()
        .trash_list
        .get()
        .row_at_index(0)
        .expect("Trash row");
    ui.click(&trash_row, "the Trash sidebar row");
    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .nav_view()
            .visible_page()
            .and_downcast::<TrashPage>()
            .is_some()),
        "clicking the Trash row should show the Trash page"
    );

    let photos_row = sidebar.row_at_index(0).expect("Photos row");
    ui.click(&photos_row, "the Photos sidebar row");
    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .browsing_stack()
            .visible_child_name()
            .as_deref()
            == Some("photos")),
        "clicking the Photos row should return to Photos"
    );

    ui.click(&window.imp().settings_button.get(), "the Settings button");
    assert!(
        window.nav_view().has_css_class("settings-background-blur"),
        "opening settings should put its chrome over the content nav"
    );
}

/// Multi-select albums from the row context menu, and check the delete action
/// arms only once real albums are selected.
fn album_sidebar_multi_select_deletes_real_albums() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let window = &shell.window;
    shell.seed_extra_album();
    window.populate_album_rows();

    // Enter album selection by right-clicking an album row and choosing the menu.
    let album_name = real_album_name_in_sidebar(&shell);
    let row_label = wait_for_label_containing(
        &window.imp().album_list.get(),
        &album_name,
        Duration::from_secs(5),
    )
    .expect("the sidebar should list a real album");
    ui.right_click(&row_label, &format!("{album_name:?} album row"));
    let multi = wait_for_button_with_label(
        window,
        &tr("album.context.multi_select"),
        Duration::from_secs(4),
    )
    .expect("the album row menu should offer multi-select");
    ui.click(&multi, "Multi select albums");

    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .imp()
            .album_selection_bar
            .get()
            .is_revealed()),
        "album multi-select should reveal the batch bar"
    );
    assert!(
        !window.imp().album_selection_delete_btn.get().is_sensitive(),
        "delete should stay disabled until a real album is ticked"
    );

    // Tick the real albums by clicking their rows.
    for name in sidebar_real_album_names(&shell) {
        let label = find_label_containing(&window.imp().album_list.get(), &name)
            .unwrap_or_else(|| panic!("the sidebar should still list {name:?}"));
        ui.click(&label, &format!("{name:?} album row"));
    }
    assert!(
        ui.wait_until(Duration::from_secs(4), || window
            .selected_album_delete_count()
            >= 1),
        "clicking real album rows should add them to the album selection"
    );
    assert!(
        window.imp().album_selection_delete_btn.get().is_sensitive(),
        "selecting a real album should arm the delete action"
    );
}

fn album_picker_clicks_album_row_and_copy_move() {
    let shell = Shell::new();
    let ui = &shell.ui;
    let grid = shell.visible_photos_grid();
    let original_count = db::list_all_media(&shell.pool).unwrap().len();

    let first_item = shell.items[0].clone();
    enter_multi_select(&shell, &grid, &first_item);
    ui.click(&shell.photos.imp().add_to_album_btn.get(), "Add to Album");

    let picker = album_picker_dialog(ui, &shell.window);
    let picker_ui = Ui::for_widget(&picker);
    let album_tile = wait_for_picker_tile(&picker_ui, &picker);
    picker_ui.click(&album_tile, "an album in the picker");
    let copy = find_button_with_label(&picker, &tr("album_picker.copy"))
        .expect("the picker should offer Copy");
    picker_ui.click(&copy, "Copy");
    assert!(
        ui.wait_until(Duration::from_secs(10), || db::list_all_media(&shell.pool)
            .is_ok_and(|items| items.len() > original_count)),
        "clicking Copy should create a real copied file and row"
    );
    assert!(
        ui.wait_until(Duration::from_secs(6), || !picker.is_mapped()),
        "the picker should close after Copy"
    );

    // Move the other photo into a second album.
    let move_target = shell.seed_extra_album();
    let second = &shell.items[1];
    album_picker::AlbumPickerDialog::present(
        &shell.window.nav_view(),
        shell.pool.clone(),
        shell.db_actor.clone(),
        shell.loader.clone(),
        vec![second.id],
    );
    let picker = album_picker_dialog(ui, &shell.window);
    let picker_ui = Ui::for_widget(&picker);
    let tile = wait_for_picker_tile_at(&picker_ui, &picker, &move_target).unwrap_or_else(|| {
        panic!(
            "the picker should list the album at {}",
            move_target.display()
        )
    });
    picker_ui.click(&tile, "the move destination");
    let move_btn = find_button_with_label(&picker, &tr("album_picker.move"))
        .expect("the picker should offer Move");
    picker_ui.click(&move_btn, "Move");
    assert!(
        ui.wait_until(Duration::from_secs(12), || {
            let item = db::get_media_item(&shell.pool, second.id).unwrap();
            item.folder_path == move_target && item.path.is_file()
        }),
        "Move should really relocate the file into the chosen album folder"
    );
    assert!(
        !shell.items[1].path.exists(),
        "the original path should be gone after a move"
    );
}

/// Right-click a photo inside an album and move it to another album. The context
/// menu is the only way to reach this, so the whole path — secondary press, menu
/// item, picker, move — is real input.
fn album_detail_context_menu_moves_to_album() {
    for source_is_virtual in [true, false] {
        let shell = Shell::new();
        let ui = &shell.ui;
        let target = shell.seed_extra_album();
        shell.window.populate_album_rows();

        // Open a source album by clicking its sidebar row.
        let name = sidebar_album_names(&shell)
            .into_iter()
            .find(|name| {
                if source_is_virtual {
                    name.is_empty() || !name.starts_with("second-album")
                } else {
                    *name != target.file_name().unwrap().to_string_lossy() && !name.is_empty()
                }
            })
            .unwrap_or_else(|| panic!("the fixture should offer a source album"));
        let row_label = wait_for_label_containing(
            &shell.window.imp().album_list.get(),
            &name,
            Duration::from_secs(5),
        )
        .unwrap_or_else(|| panic!("the sidebar should list the {name:?} album"));
        ui.click(&row_label, &format!("{name:?} album row"));
        assert!(
            ui.wait_until(Duration::from_secs(5), || shell
                .window
                .browsing_stack()
                .visible_child_name()
                .as_deref()
                == Some("album")),
            "clicking an album row should open that album"
        );
        let detail = shell
            .window
            .browsing_stack()
            .visible_child()
            .and_downcast::<AlbumDetailPage>()
            .expect("the album detail page");
        let grid = find_descendant::<VirtualMediaGrid>(&detail).expect("the album grid");

        let source = if source_is_virtual {
            shell.items[0].clone()
        } else {
            shell.items[1].clone()
        };
        let tile = shell.tile_for(&grid, MediaId::from(source.id), "the album grid");
        ui.right_click(&tile, &format!("{} in the album", source.display_name()));
        let move_item = wait_for_button_with_label(
            &shell.window,
            &tr("photos.batch.move_to_album"),
            Duration::from_secs(4),
        )
        .expect("the photo's context menu should offer Move to Album");
        ui.click(&move_item, "Move to Album");

        let picker = album_picker_dialog(ui, &shell.window);
        let picker_ui = Ui::for_widget(&picker);
        let tile = wait_for_picker_tile_at(&picker_ui, &picker, &target)
            .unwrap_or_else(|| panic!("the picker should list {}", target.display()));
        picker_ui.click(&tile, "the destination album");
        let move_btn = find_button_with_label(&picker, &tr("album_picker.move"))
            .expect("the picker should offer Move");
        picker_ui.click(&move_btn, "Move");
        assert!(
            ui.wait_until(Duration::from_secs(12), || db::get_media_item(
                &shell.pool,
                source.id
            )
            .is_ok_and(|item| item.folder_path == target && item.path.is_file())),
            "the menu-driven move should file the photo under the chosen album"
        );
    }
}

// ---------------------------------------------------------------------------
// Shared scenario helpers
// ---------------------------------------------------------------------------

/// Enter multi-select the only way a user can from a clean grid: right-click a
/// photo and choose the menu's multi-select entry.
///
/// This is not ceremony. The Photos batch bar — Select All, Favorite, Add to
/// Album, Move to Trash — lives in revealers that `set_reveal_child(has_any)`
/// opens only once something is already selected, so a click on any of them is
/// unreachable until this runs. A test that emitted `clicked` on those buttons
/// would pass against chrome the user cannot see.
fn enter_multi_select(shell: &Shell, grid: &VirtualMediaGrid, photo: &MediaItem) {
    let ui = &shell.ui;
    let tile = shell.tile_for(grid, MediaId::from(photo.id), "the Photos grid");
    ui.right_click(
        &tile,
        &format!("{} with the right button", photo.display_name()),
    );
    let item = wait_for_button_with_label(
        &shell.window,
        &tr("photos.batch.multi_select"),
        Duration::from_secs(5),
    )
    .expect("right-clicking a photo should open a menu offering multi-select");
    ui.click(&item, "Multi select");
    assert!(
        ui.wait_until(Duration::from_secs(5), || grid.is_multi_select_mode()
            && grid.selected_ids().contains(&MediaId::from(photo.id))),
        "choosing multi-select should enter the mode with the right-clicked photo selected"
    );
}

/// Select the whole collection through the chrome a user actually has: enter
/// multi-select from a photo's menu, then click the batch bar's Select All.
fn select_every_photo(shell: &Shell, grid: &VirtualMediaGrid) {
    let first = shell.items[0].clone();
    enter_multi_select(shell, grid, &first);
    shell
        .ui
        .click(&shell.photos.imp().select_all_btn.get(), "Select All");
    assert!(
        shell
            .ui
            .wait_until(Duration::from_secs(8), || grid.is_all_displayed_selected()),
        "clicking Select All should select every rendered photo"
    );
}

/// The page a scenario expects to have navigated to, waited for and typed.
fn expect_page<T>(ui: &Ui, nav: &adw::NavigationView, message: &str) -> T
where
    T: IsA<gtk::Widget> + IsA<adw::NavigationPage> + glib::object::ObjectType,
{
    assert!(
        ui.wait_until(Duration::from_secs(8), || nav
            .visible_page()
            .and_downcast::<T>()
            .is_some()),
        "{message} (within 8s)"
    );
    nav.visible_page()
        .and_downcast::<T>()
        .expect("the expected page is visible")
}

/// Type into the Search field and wait for a result tile for `needle`. Returns
/// the `GtkFlowBoxChild` — the thing a user would click.
fn wait_for_flowbox_result(ui: &Ui, page: &SearchPage, needle: &str) -> gtk::FlowBoxChild {
    let scan = |ui: &Ui| -> Option<gtk::FlowBoxChild> {
        for flow in descendants::<gtk::FlowBox>(page) {
            // A hidden section still holds its children; only the visible one is
            // something the user can aim at.
            if !flow.is_visible() {
                continue;
            }
            for i in 0..flow.observe_children().n_items() {
                let child = match flow.child_at_index(i as i32) {
                    Some(child) => child,
                    None => continue,
                };
                if ui.pointer_at_center_of(&child).is_none() {
                    continue;
                }
                if find_label_containing(&child, needle).is_some()
                    || !descendants::<photo_viewer::ui::SquareTile>(&child).is_empty()
                {
                    return Some(child);
                }
            }
        }
        None
    };

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let Some(child) = scan(ui) else {
            assert!(
                Instant::now() < deadline,
                "a query for {needle:?} should render a reachable result tile within 20s"
            );
            ui.pump(Duration::from_millis(100));
            continue;
        };
        // The grid rebuilds once when the decoded thumbnails land, which recycles
        // the item widgets it had put up first. A reference taken before that
        // points at a widget the user is no longer looking at, so require the same
        // tile to survive a settle window before aiming at it.
        let mut settled = true;
        let until = Instant::now() + Duration::from_millis(600);
        while Instant::now() < until {
            ui.pump(Duration::from_millis(60));
            if scan(ui) != Some(child.clone()) {
                settled = false;
                break;
            }
        }
        if settled {
            return child;
        }
        assert!(
            Instant::now() < deadline,
            "the result tile for {needle:?} kept being rebuilt for 20s"
        );
    }
}

/// The `adw::Dialog` the album picker presents onto the window.
fn album_picker_dialog(ui: &Ui, window: &photo_viewer::ui::MainWindow) -> adw::Dialog {
    let dialog = wait_for_descendant::<adw::Dialog>(window, Duration::from_secs(5))
        .expect("the picker should present a dialog");
    assert!(
        ui.wait_until(Duration::from_secs(4), || dialog.is_visible()),
        "the picker dialog should become visible"
    );
    dialog
}

fn wait_for_picker_tile(ui: &Ui, picker: &adw::Dialog) -> gtk::Button {
    assert!(
        ui.wait_until(Duration::from_secs(6), || find_button_with_css(
            picker,
            "album-picker-tile",
        )
        .is_some()),
        "the picker should list at least one album tile"
    );
    find_button_with_css(picker, "album-picker-tile").expect("a picker album tile")
}

fn wait_for_picker_tile_at(ui: &Ui, picker: &adw::Dialog, path: &Path) -> Option<gtk::Button> {
    let expected = path.display().to_string();
    let reached = ui.wait_until(Duration::from_secs(6), || {
        descendants::<gtk::Button>(picker).iter().any(|b| {
            b.has_css_class("album-picker-tile") && b.tooltip_text().as_deref() == Some(&expected)
        })
    });
    if !reached {
        return None;
    }
    descendants::<gtk::Button>(picker).into_iter().find(|b| {
        b.has_css_class("album-picker-tile") && b.tooltip_text().as_deref() == Some(&expected)
    })
}

/// Wait for a delete/overwrite confirmation to appear and answer it by clicking
/// the response button the user reads, inside the dialog's own surface.
fn respond_to_alert(ui: &Ui, window: &photo_viewer::ui::MainWindow, response_label: &str) {
    let dialog = wait_for_alert(ui, window, response_label)
        .unwrap_or_else(|| panic!("a confirmation offering {response_label:?} should appear"));
    let dialog_ui = Ui::for_widget(&dialog);
    let button = find_button_with_label(&dialog, response_label)
        .expect("the dialog should carry the named response button");
    dialog_ui.click(&button, response_label);
}

fn wait_for_alert(
    ui: &Ui,
    window: &photo_viewer::ui::MainWindow,
    response_label: &str,
) -> Option<adw::AlertDialog> {
    let mut found: Option<adw::AlertDialog> = None;
    let reached = ui.wait_until(Duration::from_secs(6), || {
        found = descendants::<adw::AlertDialog>(window)
            .into_iter()
            .find(|dialog| find_button_with_label(dialog, response_label).is_some());
        found.is_some()
    });
    if reached {
        found
    } else {
        None
    }
}

/// The `AdwToast` libadwaita puts on the overlay. `AdwToastWidget` is not
/// exported, so it is found by type name among the overlay's children.
fn find_toast(overlay: &adw::ToastOverlay) -> Option<gtk::Widget> {
    let mut child = overlay.first_child();
    while let Some(node) = child {
        if node.type_().name().contains("Toast") {
            return Some(node);
        }
        child = node.next_sibling();
    }
    None
}

/// Click the Trash tile that shows `photo`, and wait for the grid's selection to
/// reach `selected`.
///
/// The tile is resolved from the photo's identity every time: the Trash grid
/// reloads after Restore and Cancel, and the grid rebinds its item widgets rather
/// than handing out fresh ones, so a widget held across a reload can point at a
/// different photo — or at none.
fn click_trashed_photo(shell: &Shell, grid: &VirtualMediaGrid, photo: &MediaItem, selected: bool) {
    let ui = &shell.ui;
    let id = MediaId::from(photo.id);
    let tile = shell.tile_for(grid, id, "the Trash grid");
    ui.click(&tile, &format!("{} in Trash", photo.display_name()));
    assert!(
        ui.wait_until(Duration::from_secs(5), || grid
            .selected_ids()
            .contains(&id)
            == selected),
        "clicking the Trash photo {} should leave it selected={selected}; selection is {:?} and the grid reports multi-select={}",
        photo.display_name(),
        grid.selected_ids(),
        grid.is_multi_select_mode()
    );
}

/// Wait for the Trash page's batch bar to reach the revealed state a user would
/// see after selecting or clearing a photo.
fn wait_action_bar(ui: &Ui, trash: &TrashPage, revealed: bool) {
    let bar = trash.imp().action_bar.get();
    assert!(
        ui.wait_until(Duration::from_secs(5), || bar.is_revealed() == revealed),
        "the Trash action bar should be revealed={revealed} (selection count: {})",
        trash
            .imp()
            .grid
            .borrow()
            .as_ref()
            .map(|g| g.selected_ids().len())
            .unwrap_or(usize::MAX)
    );
}

/// The name of a real (folder-backed) album currently in the sidebar.
fn real_album_name_in_sidebar(shell: &Shell) -> String {
    sidebar_album_names(shell)
        .into_iter()
        .find(|name| !name.is_empty())
        .expect("the library should give at least one folder album")
}

/// Only the folder-backed albums, which are the ones the delete action accepts.
fn sidebar_real_album_names(shell: &Shell) -> Vec<String> {
    shell
        .window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .filter(|album| !album.is_virtual)
        // The sidebar row reads `display_name()` — the folder's basename — while
        // `name` is the raw path for a folder album, so aim at what the user reads.
        .map(|album| album.display_name())
        .collect()
}

fn sidebar_album_names(shell: &Shell) -> Vec<String> {
    shell
        .window
        .imp()
        .album_targets
        .borrow()
        .iter()
        // The sidebar row reads `display_name()` — the folder's basename — while
        // `name` is the raw path for a folder album, so aim at what the user reads.
        .map(|album| album.display_name())
        .collect()
}

fn newest_media(pool: &db::DbPool) -> MediaItem {
    let mut items = db::list_all_media(pool).unwrap();
    items.sort_by(|a, b| b.taken_at.cmp(&a.taken_at).then_with(|| b.id.cmp(&a.id)));
    items
        .into_iter()
        .find(|item| item.path.is_file())
        .expect("a library photo")
}

fn newest_trashed(pool: &db::DbPool) -> MediaItem {
    let mut items = db::list_trashed_media(pool).unwrap();
    items.sort_by_key(|item| std::cmp::Reverse(item.id));
    items.into_iter().next().expect("a trashed photo")
}

fn oldest_trashed(pool: &db::DbPool) -> MediaItem {
    let mut items = db::list_trashed_media(pool).unwrap();
    items.sort_by_key(|a| a.id);
    items.into_iter().next().expect("a trashed photo")
}
