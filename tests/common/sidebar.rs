//! Pointer operations on virtual sidebar rows, identified by folder path.
use super::interaction::{wait_for_button_with_label, Ui};
use super::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use photo_viewer::core::i18n::tr;
use photo_viewer::ui::MainWindow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn selected_album_paths(window: &MainWindow) -> BTreeSet<PathBuf> {
    window
        .imp()
        .selected_album_paths
        .borrow()
        .iter()
        .cloned()
        .collect()
}

/// Read the GTK selection against the current model, independently of the
/// pending-deletion cache. A refresh must not leave these two views disagreeing.
pub fn model_selected_album_paths(window: &MainWindow) -> BTreeSet<PathBuf> {
    let model = window.imp().album_model.borrow();
    let model = model.as_ref().expect("the sidebar album model");
    let selection = window.imp().album_selection.borrow();
    let selection = selection.as_ref().expect("the sidebar selection model");
    (0..model.n_items())
        .filter(|&index| selection.is_selected(index))
        .map(|index| {
            let item = model
                .item(index)
                .unwrap()
                .downcast::<gtk::glib::BoxedAnyObject>()
                .unwrap();
            let path = item
                .borrow::<photo_viewer::core::albums::Album>()
                .folder_path
                .clone();
            path
        })
        .collect()
}

pub fn assert_selected_albums(
    ui: &Ui,
    window: &MainWindow,
    expected: &BTreeSet<PathBuf>,
    context: &str,
) {
    assert!(
        ui.wait_until(Duration::from_secs(4), || selected_album_paths(window)
            == *expected
            && model_selected_album_paths(window) == *expected),
        "{context}: expected folders {expected:?}, pending deletion {:?}, GTK selection {:?}",
        selected_album_paths(window),
        model_selected_album_paths(window)
    );
    assert_eq!(
        window.selected_album_delete_count(),
        expected.len(),
        "{context}: deletion count must agree with identities"
    );
}

/// Search the scroll viewport afresh after every scroll. A previously found
/// widget can be rebound during scrolling; never chase that old widget handle.
pub fn album_row(ui: &Ui, window: &MainWindow, path: &Path) -> gtk::Widget {
    assert!(
        window
            .imp()
            .album_targets
            .borrow()
            .iter()
            .any(|album| album.folder_path == path),
        "the sidebar must contain the requested album {path:?}"
    );
    let adjustment = window.imp().album_scroll.get().vadjustment();
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut next_offset = adjustment.lower();
    loop {
        if let Some(row) = window.sidebar_album_row_for_tests(path) {
            if let Some((x, y)) = ui.pointer_at_center_of(&row) {
                if ui
                    .pick(x, y)
                    .is_some_and(|picked| picked == row || picked.is_ancestor(&row))
                {
                    // Let model binds/layout settle, then resolve the business
                    // identity again before returning a pointer target.
                    ui.pump(Duration::from_millis(60));
                    if window.sidebar_album_row_for_tests(path).as_ref() == Some(&row) {
                        if let Some((x, y)) = ui.pointer_at_center_of(&row) {
                            if ui
                                .pick(x, y)
                                .is_some_and(|picked| picked == row || picked.is_ancestor(&row))
                            {
                                return row;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "scrolling never exposed the row currently bound to {path:?}"
        );
        let bottom = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value(next_offset.min(bottom));
        ui.pump(Duration::from_millis(60));
        next_offset = if next_offset >= bottom {
            adjustment.lower()
        } else {
            (next_offset + adjustment.page_size().max(1.0) / 2.0).min(bottom)
        };
    }
}

pub fn open_album(shell: &Shell, path: &Path) {
    let row = album_row(&shell.ui, &shell.window, path);
    shell.ui.click_native(&row, &format!("open album {path:?}"));
    assert!(
        shell.ui.wait_until(Duration::from_secs(5), || {
            shell.window.imp().active_album.borrow().as_deref() == Some(path)
                && shell
                    .window
                    .browsing_stack()
                    .visible_child_name()
                    .as_deref()
                    == Some("album")
        }),
        "the press must open the specified album {path:?}"
    );
}

pub fn enter_album_multi_select(shell: &Shell, path: &Path) {
    let window = &shell.window;
    let row = album_row(&shell.ui, window, path);
    shell.ui.right_click(&row, &format!("album {path:?} menu"));
    let multi = wait_for_button_with_label(
        window,
        &tr("album.context.multi_select"),
        Duration::from_secs(4),
    )
    .expect("the album row menu should offer multi-select");
    shell.ui.click(&multi, "Multi select albums");
    assert!(shell.ui.wait_until(Duration::from_secs(4), || window
        .imp()
        .album_selection_mode
        .get()
        && window.imp().album_selection_bar.get().is_revealed()));
    let real = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .find(|album| album.folder_path == path)
        .is_some_and(|album| !album.is_virtual);
    let expected = if real {
        BTreeSet::from([path.to_path_buf()])
    } else {
        BTreeSet::new()
    };
    assert_selected_albums(
        &shell.ui,
        window,
        &expected,
        "entering the specified album's multi-select menu",
    );
}

/// Toggle the named folder through a real Ctrl+press, then prove the exact set
/// changed by that identity alone. Never retry a press after a wrong selection.
pub fn toggle_album(ui: &Ui, window: &MainWindow, path: &Path) {
    assert!(
        window.imp().album_selection_mode.get(),
        "album toggling requires batch mode"
    );
    let real = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .find(|album| album.folder_path == path)
        .is_some_and(|album| !album.is_virtual);
    let mut expected = selected_album_paths(window);
    if real && !expected.remove(path) {
        expected.insert(path.to_path_buf());
    }
    let row = album_row(ui, window, path);
    assert_eq!(
        window.sidebar_album_row_for_tests(path).as_ref(),
        Some(&row),
        "the pointer target must still be bound to {path:?}"
    );
    ui.ctrl_click_native(&row, &format!("toggle album {path:?}"));
    assert_selected_albums(ui, window, &expected, &format!("Ctrl+click on {path:?}"));
}

pub fn select_all_real_albums(ui: &Ui, window: &MainWindow) -> BTreeSet<PathBuf> {
    let expected: BTreeSet<_> = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .filter(|album| !album.is_virtual)
        .map(|album| album.folder_path.clone())
        .collect();
    assert!(
        !expected.is_empty(),
        "the fixture must provide real folder albums"
    );
    for path in &expected {
        if !selected_album_paths(window).contains(path) {
            toggle_album(ui, window, path);
        }
    }
    assert_selected_albums(ui, window, &expected, "selecting every real folder album");
    expected
}
