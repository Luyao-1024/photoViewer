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
use photo_viewer::core::i18n::tr;
use photo_viewer::core::identity::MediaId;
use photo_viewer::core::media::MediaItem;
use photo_viewer::core::sync::{Fingerprint, NewSyncJob, SyncDirection, SyncStore, UploadScope};
use photo_viewer::ui::virtual_media_grid::VirtualMediaGrid;
use photo_viewer::ui::{
    album_picker, AlbumDetailPage, ModeSelector, SearchPage, TrashPage, ViewerPage,
};
use std::path::{Path, PathBuf};
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
    let toast = wait_for_descendant::<gtk::Widget>(&overlay, Duration::from_secs(4))
        .and_then(|_| find_toast(&overlay))
        .expect("the delete should report itself with a toast");
    let undo = find_button_with_label(&toast, &tr("viewer.toast.undo"))
        .expect("the delete toast should offer a way back");
    let toast_ui = Ui::for_widget(&toast);
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
    assert_ne!(
        item.blake3_hash, target.blake3_hash,
        "the row's recorded content hash should follow the new bytes"
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
