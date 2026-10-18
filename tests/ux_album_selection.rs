//! Album selection must identify the folders the user's presses actually select.
mod common;

use common::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use std::collections::BTreeSet;
use std::time::Duration;

#[test]
fn album_selection_tracks_the_pressed_folder_identities() {
    // GLib reads $HOME and the XDG directories once per process, on the
    // first GTK call, so the sandbox has to be in place before `gtk::init()`.
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().expect("GTK init failed");
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime for album UX");
    let _guard = runtime.enter();
    hovering_albums_does_not_select_or_navigate();
    reselecting_the_open_album_returns_from_the_viewer();
    additive_selection_and_deselection();
    duplicate_names_survive_scroll_and_refresh(&runtime);
}

fn hovering_albums_does_not_select_or_navigate() {
    let shell = Shell::new();
    let extra = shell.seed_extra_album();
    let ui = &shell.ui;
    let window = &shell.window;
    let row = common::sidebar::album_row(ui, window, &extra);
    let (x, y) = ui.pointer_at_center_of(&row).expect("a visible album row");
    ui.pointer_motion(x, y, "Hover an album without clicking");
    ui.pump(Duration::from_millis(900));
    assert!(
        window.imp().active_album.borrow().is_none(),
        "hovering from Photos must not open an album"
    );
    assert!(
        common::sidebar::model_selected_album_paths(window).is_empty(),
        "hovering must not change persistent album selection"
    );
    common::sidebar::open_album(&shell, &extra);
    let root = shell.photos_dir();
    let row = common::sidebar::album_row(ui, window, &root);
    let (x, y) = ui
        .pointer_at_center_of(&row)
        .expect("another visible album row");
    ui.pointer_motion(x, y, "Hover another album without clicking");
    ui.pump(Duration::from_millis(900));
    assert_eq!(
        window.imp().active_album.borrow().as_ref(),
        Some(&extra),
        "hovering another album must not change the open album"
    );
    assert_eq!(
        common::sidebar::model_selected_album_paths(window),
        BTreeSet::from([extra])
    );
    common::sidebar::open_album(&shell, &root);
    assert_eq!(window.imp().active_album.borrow().as_ref(), Some(&root));
}

fn reselecting_the_open_album_returns_from_the_viewer() {
    use common::interaction::descendants;
    use photo_viewer::core::identity::MediaId;
    use photo_viewer::ui::{virtual_media_grid::VirtualMediaGrid, AlbumDetailPage, ViewerPage};
    let shell = Shell::new();
    let ui = &shell.ui;
    let album = shell.photos_dir();
    common::sidebar::open_album(&shell, &album);
    let detail = shell
        .window
        .browsing_stack()
        .visible_child()
        .and_downcast::<AlbumDetailPage>()
        .unwrap();
    let grid = descendants::<VirtualMediaGrid>(&detail)
        .into_iter()
        .next()
        .unwrap();
    let tile = shell.tile_for(
        &grid,
        MediaId::from(shell.items[0].id),
        "the selected album",
    );
    ui.click(&tile, "Open a photo from the selected album");
    assert!(ui.wait_until(Duration::from_secs(5), || shell
        .window
        .nav_view()
        .visible_page()
        .is_some_and(|page| page.is::<ViewerPage>())));
    let row = common::sidebar::album_row(ui, &shell.window, &album);
    ui.click_native(&row, "Return through the already selected album row");
    assert!(
        ui.wait_until(Duration::from_secs(5), || shell
            .window
            .nav_view()
            .visible_page()
            .is_some_and(|page| !page.is::<ViewerPage>())),
        "clicking the selected album must return from the viewer to its album"
    );
    assert_eq!(
        shell.window.imp().active_album.borrow().as_ref(),
        Some(&album)
    );
}

fn additive_selection_and_deselection() {
    let shell = Shell::new();
    let extra = shell.seed_extra_album();
    let ui = &shell.ui;
    let window = &shell.window;
    let photos = shell.photos_dir();
    common::sidebar::open_album(&shell, &photos);
    common::sidebar::enter_album_multi_select(&shell, &photos);
    common::sidebar::toggle_album(ui, window, &extra);
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &BTreeSet::from([photos.clone(), extra.clone()]),
        "both named folders selected",
    );
    window.populate_album_rows();
    ui.pump(Duration::from_millis(200));
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &BTreeSet::from([photos.clone(), extra.clone()]),
        "unchanged refresh preserves both folders",
    );
    common::sidebar::toggle_album(ui, window, &photos);
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &BTreeSet::from([extra]),
        "only second-album remains after cancelling photos",
    );
}

fn duplicate_names_survive_scroll_and_refresh(runtime: &tokio::runtime::Runtime) {
    let shell = Shell::new();
    let ui = &shell.ui;
    let window = &shell.window;
    let root = shell.photos_dir();
    let left = root.join("zz-left/photos");
    let right = root.join("zz-right/photos");
    let source = shell.items[0].path.clone();
    for (index, folder) in std::iter::once(left.clone())
        .chain(std::iter::once(right.clone()))
        .chain((0..32).map(|index| root.join(format!("scroll-{index:02}"))))
        .enumerate()
    {
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::copy(&source, folder.join("fixture.jpg")).unwrap();
        // Folder ordering is by filesystem mtime. Keep these albums older
        // than the shell's June fixtures so the duplicate names start offscreen.
        std::fs::File::options()
            .write(true)
            .open(folder.join("fixture.jpg"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(
                std::time::SystemTime::UNIX_EPOCH
                    + Duration::from_secs(1_700_000_000 + index as u64 * 60),
            ))
            .unwrap();
    }
    runtime.block_on(shell.rescan());
    window.populate_album_rows();
    assert!(
        ui.wait_until(Duration::from_secs(5), || window
            .imp()
            .album_targets
            .borrow()
            .iter()
            .filter(|album| !album.is_virtual)
            .count()
            == 35),
        "the scanner must produce every fixture album"
    );
    common::sidebar::enter_album_multi_select(&shell, &root);
    let scroll = window.imp().album_scroll.get().vadjustment();
    let initial_scroll = scroll.value();
    common::sidebar::toggle_album(ui, window, &left);
    assert!(
        scroll.value() > initial_scroll + 1.0,
        "the offscreen album must require scrolling through the virtual list"
    );
    common::sidebar::toggle_album(ui, window, &right);
    let expected = BTreeSet::from([root.clone(), left.clone(), right.clone()]);
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &expected,
        "three different folders all displayed as photos",
    );

    // Changing an album count and adding an album forces a model refresh with
    // rebinds. This is filesystem input followed by the production scanner,
    // rather than changing the selection or seeding the result under test.
    std::fs::copy(&source, left.join("new-photo.jpg")).unwrap();
    let inserted = root.join("new-album");
    std::fs::create_dir_all(&inserted).unwrap();
    std::fs::copy(&source, inserted.join("fixture.jpg")).unwrap();
    runtime.block_on(shell.rescan());
    window.populate_album_rows();
    ui.pump(Duration::from_millis(200));
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &expected,
        "model replacement retains the selected folder identities",
    );
    common::sidebar::toggle_album(ui, window, &left);
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &BTreeSet::from([root.clone(), right.clone()]),
        "deselecting one duplicate name leaves the other two folders selected",
    );
    let virtual_path = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .find(|album| album.is_virtual)
        .expect("the sidebar must contain a virtual album")
        .folder_path
        .clone();
    common::sidebar::toggle_album(ui, window, &virtual_path);
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &BTreeSet::from([root, right]),
        "virtual albums cannot enter the pending deletion set",
    );
    let files_before: Vec<_> = photo_viewer::core::db::list_all_media(&shell.pool)
        .unwrap()
        .into_iter()
        .map(|item| {
            let bytes = std::fs::read(&item.path).unwrap();
            (item, bytes)
        })
        .collect();
    ui.click(
        &window.imp().album_selection_cancel_btn.get(),
        "Cancel album selection",
    );
    common::sidebar::assert_selected_albums(
        ui,
        window,
        &BTreeSet::new(),
        "Cancel clears all folder identities",
    );
    assert!(ui.wait_until(Duration::from_secs(4), || !window
        .imp()
        .album_selection_mode
        .get()
        && !window.imp().album_selection_bar.get().is_revealed()));
    for (item, bytes) in files_before {
        assert_eq!(
            std::fs::read(&item.path).unwrap(),
            bytes,
            "cancel must leave every indexed photo unchanged"
        );
        let row = photo_viewer::core::db::get_media_item_by_uri(&shell.pool, &item.uri)
            .unwrap()
            .unwrap();
        assert!(
            row.trashed_at.is_none(),
            "cancel must leave every media row live"
        );
    }
}
