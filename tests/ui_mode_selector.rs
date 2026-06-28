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
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::media::MediaItem;
use photo_viewer::ui::{ModeSelector, PhotosPage};

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

    // --- Test 4: PhotosPage builds via its template; ModeSelector TemplateChild
    // resolves and the template applies halign=center / valign=end.
    //
    // This exercises the real `PhotosPage::new` path (loading
    // `data/ui/photos-page.ui`, resolving the `mode_selector` TemplateChild,
    // and wiring it to the inner ViewStack). It replaces an earlier version
    // that rebuilt a ViewStack by hand and attached a fresh ModeSelector to
    // it — that path did not load the template and did not exercise the
    // TemplateChild wiring.
    use std::sync::Arc;

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(MediaItem {
        id: 1,
        uri: "file:///tmp/one.jpg".into(),
        path: "/tmp/one.jpg".into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 100,
        blake3_hash: "hash".into(),
        is_favorite: false,
        trashed_at: None,
    }));
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    // Actual signature in src/core/thumbnails.rs is `(pool, cache_dir)`.
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));

    let page = PhotosPage::new(media_list, loader);
    let sel4 = page.imp().mode_selector.get();
    // TemplateChild resolved (the `.get()` above would have panicked if not).
    assert_eq!(sel4.halign(), gtk::Align::Center, "template halign=center");
    assert_eq!(sel4.valign(), gtk::Align::End, "template valign=end");
    assert_eq!(sel4.active_index(), 2, "PhotosPage should default to Day");
    assert_eq!(
        page.imp().view_stack.get().visible_child_name().as_deref(),
        Some("day"),
        "PhotosPage should show the Day grid by default when media exists"
    );

    // --- Test 5: an initially empty PhotosPage must leave the empty state when
    // startup/background scanning appends the first media item. This is the
    // startup path after the app only loads a small first DB page: an empty DB
    // snapshot should not permanently pin the ViewStack to the no-photos child.
    let empty_then_filled: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let tmp_empty = tempfile::tempdir().unwrap();
    let pool_empty = photo_viewer::core::db::init_pool(&tmp_empty.path().join("test.db")).unwrap();
    let loader_empty = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool_empty.clone(),
        tmp_empty.path().join("thumbs"),
    ));
    let empty_page = PhotosPage::new(empty_then_filled.clone(), loader_empty);
    assert_eq!(
        empty_page
            .imp()
            .view_stack
            .get()
            .visible_child_name()
            .as_deref(),
        None,
        "initially empty page should show the untitled empty-state child"
    );
    empty_then_filled.append(&glib::BoxedAnyObject::new(MediaItem {
        id: 2,
        uri: "file:///tmp/two.jpg".into(),
        path: "/tmp/two.jpg".into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 100,
        blake3_hash: "hash2".into(),
        is_favorite: false,
        trashed_at: None,
    }));
    assert_eq!(
        empty_page
            .imp()
            .view_stack
            .get()
            .visible_child_name()
            .as_deref(),
        Some("day"),
        "PhotosPage should switch from empty state to Day when media arrives"
    );

    // --- Structural checks (validation plan §集成测试 items 1, 2, 5) ---
    //
    // 1. view_stack's parent is GtkOverlay, not GtkBox — proves the
    //    .blp template's Gtk.Overlay wrapper was actually loaded
    //    (and not silently downgraded to the prior Gtk.Box layout).
    let stack = page.imp().view_stack.get();
    let stack_parent = stack
        .parent()
        .expect("view_stack should have a parent in the loaded template");
    assert!(
        stack_parent.is::<gtk::Overlay>(),
        "view_stack should be a child of GtkOverlay (not GtkBox) — the .blp must load the overlay wrapper"
    );

    // 2. ModeSelector is a sibling of view_stack under the overlay —
    //    confirms it is an overlay child (floating) rather than a
    //    sibling at the page level (which would re-introduce the
    //    very problem this redesign exists to solve: the selector
    //    claiming grid space at the bottom).
    let overlay = stack_parent
        .downcast::<gtk::Overlay>()
        .expect("parent already asserted to be GtkOverlay");
    // Walk overlay children and confirm both view_stack and the
    // mode_selector are reachable as siblings under it.
    let stack_widget = stack.upcast::<gtk::Widget>();
    let sel_widget = sel4.clone().upcast::<gtk::Widget>();
    let overlay_children: Vec<gtk::Widget> = {
        let mut kids = Vec::new();
        let mut next = overlay.first_child();
        while let Some(c) = next {
            kids.push(c.clone());
            next = c.next_sibling();
        }
        kids
    };
    assert!(
        overlay_children.contains(&stack_widget),
        "overlay should contain view_stack as a child"
    );
    assert!(
        overlay_children.contains(&sel_widget),
        "overlay should contain mode_selector as a sibling of view_stack"
    );
    assert_eq!(
        overlay_children.len(),
        2,
        "overlay should have exactly 2 children (view_stack + mode_selector)"
    );

    // 3. ModeSelector does not claim grid space — no vexpand / hexpand.
    //    The floating-overlay property is what keeps it from competing
    //    with the grid for layout.
    assert!(
        !sel4.vexpands(),
        "ModeSelector must not vexpand (it floats over the grid, not in it)"
    );
    assert!(
        !sel4.hexpands(),
        "ModeSelector must not hexpand (it floats over the grid, not in it)"
    );

    // 4. The internal rows/cells fill the selector content width. If they are
    //    centered at their natural width, the translucent panel can extend
    //    past the three equal hit targets, which makes the Day cell look like
    //    it has excess empty space on the right.
    let row = sel4
        .first_child()
        .and_then(|c| c.downcast::<gtk::Box>().ok())
        .expect("ModeSelector first child should be the label row");
    assert_eq!(row.halign(), gtk::Align::Fill, "label row fills panel");
    assert!(row.hexpands(), "label row expands inside panel");

    let mut cells = Vec::new();
    let mut next = row.first_child();
    while let Some(c) = next {
        let sibling = c.next_sibling();
        cells.push(c.downcast::<gtk::Box>().expect("mode cell should be a Box"));
        next = sibling;
    }
    assert_eq!(cells.len(), 3, "expected 3 label cells");
    for (idx, cell) in cells.iter().enumerate() {
        assert_eq!(
            cell.halign(),
            gtk::Align::Fill,
            "label cell {idx} fills its equal-width slot"
        );
        assert!(cell.hexpands(), "label cell {idx} expands within row");
        let label = cell
            .first_child()
            .and_then(|c| c.downcast::<gtk::Label>().ok())
            .expect("label cell should contain a label");
        assert_eq!(
            label.halign(),
            gtk::Align::Center,
            "label {idx} remains visually centered"
        );
        assert!(
            label.hexpands(),
            "label {idx} expands to center in its slot"
        );
    }

    // --- Test 5: ModeSelector container carries the shared glass-raised
    // material class. The CSS provides the material via the `.glass-raised`
    // rule (Task 1), and the selector template should compose it on top of
    // `mode-selector`. Without this, the floating selector keeps its own
    // duplicated material in `box.mode-selector` and drifts from the
    // menu/popover glass language.
    use photo_viewer::ui::grid_css;
    grid_css::install();

    let sel_glass = ModeSelector::new();
    let classes: Vec<String> = sel_glass
        .css_classes()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(
        classes.iter().any(|c| c == "glass-raised"),
        "ModeSelector should carry glass-raised, got {classes:?}",
    );

    // --- Test 6: shared liquid-glass material classes resolve cleanly ---

    let label = gtk::Label::new(Some("probe"));
    for class in [
        "glass-base",
        "glass-raised",
        "glass-toolbar-button",
        "glass-toolbar-danger",
        "glass-menu",
        "glass-menu-list",
        "glass-menu-item",
        "glass-menu-item-danger",
        "glass-menu-item-suggested",
        "glass-sidebar",
        "glass-sidebar-row",
        "glass-sidebar-label",
        "glass-header",
        "viewer-stage",
        "viewer-image-frame",
        "viewer-details-panel",
        "glass-thumb-card",
    ] {
        label.add_css_class(class);
    }
    // Trigger style resolution; would error if any class crashes the provider.
    let ctx = label.style_context();
    ctx.save();
    ctx.restore();
}
