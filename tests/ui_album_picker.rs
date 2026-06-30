//! AlbumPickerDialog Copy/Move buttons use the glass vocabulary introduced
//! in Phase 1 + Task 1. They must NOT carry the old `pill` / `suggested-action`
//! / `destructive-action` libadwaita defaults.
//!
//! GTK is single-threaded; all checks live in one `#[test]` function.
//!
//! The Copy/Move buttons live on the level-2 chooser page, which is built
//! by the (otherwise private) `push_action_page` helper. We call it
//! directly to avoid depending on `glib::spawn_future_local` (which needs
//! a running main loop and would deadlock a synchronous test).

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use photo_viewer::ui::{album_picker, grid_css};

fn probe_classes<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

fn find_widget<F: Fn(&gtk::Widget) -> bool>(root: &gtk::Widget, pred: F) -> Option<gtk::Widget> {
    let mut stack: Vec<gtk::Widget> = vec![root.clone()];
    while let Some(w) = stack.pop() {
        if pred(&w) {
            return Some(w);
        }
        let mut next = w.first_child();
        while let Some(c) = next {
            stack.push(c.clone());
            next = c.next_sibling();
        }
    }
    None
}

#[test]
fn album_picker_buttons_use_glass() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.AlbumPickerGlass")
        .build();
    app.register(None::<&gtk::gio::Cancellable>).unwrap();
    grid_css::install();

    let inner = adw::NavigationView::new();
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();

    // Call the level-2 chooser builder directly. This is normally invoked
    // when a row in the level-1 album list is activated; we skip that flow
    // because `present()` schedules it via `glib::spawn_future_local`.
    album_picker::push_action_page(
        &inner,
        pool,
        vec![1],
        std::path::PathBuf::from("/tmp/test-album"),
        &inner,
    );

    // Locate Copy and Move buttons by label (zh-CN or en).
    let root = inner.upcast::<gtk::Widget>();
    let copy_btn: gtk::Button = find_widget(&root, |w| {
        w.downcast_ref::<gtk::Button>()
            .map(|b| {
                let lbl = b.label().unwrap_or_default().to_string();
                lbl == "Copy" || lbl == "复制"
            })
            .unwrap_or(false)
    })
    .expect("Copy button should exist after push_action_page")
    .downcast::<gtk::Button>()
    .unwrap();
    let move_btn: gtk::Button = find_widget(&root, |w| {
        w.downcast_ref::<gtk::Button>()
            .map(|b| {
                let lbl = b.label().unwrap_or_default().to_string();
                lbl == "Move" || lbl == "移动"
            })
            .unwrap_or(false)
    })
    .expect("Move button should exist after push_action_page")
    .downcast::<gtk::Button>()
    .unwrap();

    let copy_classes = probe_classes(&copy_btn);
    let mov_classes = probe_classes(&move_btn);

    assert!(
        copy_classes.iter().any(|c| c == "glass-toolbar-button"),
        "Copy button should carry glass-toolbar-button, got {copy_classes:?}"
    );
    assert!(
        copy_classes.iter().any(|c| c == "glass-toolbar-suggested"),
        "Copy button should carry glass-toolbar-suggested, got {copy_classes:?}"
    );
    assert!(
        !copy_classes.iter().any(|c| c == "pill"),
        "Copy button must NOT carry pill, got {copy_classes:?}"
    );
    assert!(
        !copy_classes.iter().any(|c| c == "suggested-action"),
        "Copy button must NOT carry suggested-action, got {copy_classes:?}"
    );
    assert!(
        mov_classes.iter().any(|c| c == "glass-toolbar-button"),
        "Move button should carry glass-toolbar-button, got {mov_classes:?}"
    );
    assert!(
        mov_classes.iter().any(|c| c == "glass-toolbar-danger"),
        "Move button should carry glass-toolbar-danger, got {mov_classes:?}"
    );
    assert!(
        !mov_classes.iter().any(|c| c == "pill"),
        "Move button must NOT carry pill, got {mov_classes:?}"
    );
    assert!(
        !mov_classes.iter().any(|c| c == "destructive-action"),
        "Move button must NOT carry destructive-action, got {mov_classes:?}"
    );
}
