//! Integration tests for the ModeSelector widget + its wiring into
//! PhotosPage. GTK is single-threaded: every `gtk::init()` call binds GTK
//! to the calling thread, and subsequent widget operations must run on
//! that same thread. Plain `#[test]` runs each test in a fresh thread,
//! which would panic on the second test. We therefore run all four
//! checks inside a single `#[test]` function on a single thread.
//!
//! See `tests/smoke.rs` for the single-test `gtk::init()` pattern and
//! `src/ui/mode_selector.rs::tests` for the `#[gtk::test]` macro pattern
//! (which serializes tests on the GTK main thread but requires
//! `gtk4-macros`, not currently a dev-dependency).

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use photo_viewer::ui::ModeSelector;

#[test]
fn mode_selector_integration_suite() {
    gtk::init().expect("GTK init failed");

    // --- Test 1: standalone construction with default index 0 ---
    let sel = ModeSelector::new();
    assert_eq!(sel.active_index(), 0);
    // The widget tree should be the box + row + dot_row.
    assert!(sel.first_child().is_some(), "row child present");

    // --- Test 2: set_stack seeds active_index from current visible child ---
    let stack = adw::ViewStack::new();
    stack.add_titled(&gtk::Label::new(Some("A")), Some("year"), "年");
    stack.add_titled(&gtk::Label::new(Some("B")), Some("month"), "月");
    stack.add_titled(&gtk::Label::new(Some("C")), Some("day"), "日");
    stack.set_visible_child_name("month");

    let sel2 = ModeSelector::new();
    sel2.set_stack(&stack);
    assert_eq!(sel2.active_index(), 1);

    // --- Test 3: clicking a label cell updates the bound stack ---
    let sel3 = ModeSelector::new();
    let stack3 = adw::ViewStack::new();
    stack3.add_titled(&gtk::Label::new(Some("A")), Some("year"), "年");
    stack3.add_titled(&gtk::Label::new(Some("B")), Some("month"), "月");
    stack3.add_titled(&gtk::Label::new(Some("C")), Some("day"), "日");
    sel3.set_stack(&stack3);

    // Find the third label cell and emit a click.
    let row = sel3
        .first_child()
        .and_then(|c| c.downcast::<gtk::Box>().ok())
        .unwrap();
    let cells: Vec<gtk::Box> = (0..3)
        .scan(row.first_child(), |cur, _| {
            let c = cur.clone()?;
            *cur = c.next_sibling();
            c.downcast::<gtk::Box>().ok()
        })
        .collect();
    let gesture = cells[2]
        .observe_controllers()
        .snapshot()
        .into_iter()
        .find_map(|c| c.downcast::<gtk::GestureClick>().ok())
        .expect("third cell should have a GtkGestureClick");
    gesture.emit_by_name::<()>("pressed", &[&0i32, &0.0f64, &0.0f64]);
    assert_eq!(stack3.visible_child_name().as_deref(), Some("day"));

    // --- Test 4: PhotosPage-like wiring builds with ModeSelector ---
    // Builds the full PhotosPage-equivalent tree (without a real
    // ThumbnailLoader and without DB) just to confirm the template
    // compiles and the ModeSelector inside is wired through the
    // ViewStack.
    use photo_viewer::core::section_model::GroupBy;
    use photo_viewer::ui::media_grid::MediaGrid;
    use std::rc::Rc;
    use std::sync::Arc;

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    // We don't have a real loader in the test crate; pass a
    // freshly-constructed one against an empty media list. The grid
    // never requests a thumbnail in this test, so the loader's
    // internal channel is unused.
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    // Actual signature in src/core/thumbnails.rs is `(pool, cache_dir)`.
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let on_activate: Rc<dyn Fn(u32)> = Rc::new(|_| {});
    let grid = MediaGrid::new(
        media_list.clone(),
        GroupBy::Year,
        loader.clone(),
        on_activate,
    );
    let stack4 = adw::ViewStack::new();
    stack4.add_titled(&grid, Some("year"), "年");
    stack4.add_titled(
        &MediaGrid::new(
            media_list.clone(),
            GroupBy::Month,
            loader.clone(),
            Rc::new(|_| {}),
        ),
        Some("month"),
        "月",
    );
    stack4.add_titled(
        &MediaGrid::new(media_list, GroupBy::Day, loader, Rc::new(|_| {})),
        Some("day"),
        "日",
    );
    let sel4 = ModeSelector::new();
    sel4.set_stack(&stack4);
    assert_eq!(sel4.active_index(), 0);
}
