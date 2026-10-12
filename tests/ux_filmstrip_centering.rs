//! Check the frames painted during real navigation, including the first frame
//! after a switch. Waiting for the strip to settle hides a highlight-then-scroll
//! regression even when its eventual position is correct.
mod common;

use common::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use photo_viewer::ui::ViewerPage;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const CENTER_TOLERANCE: f32 = 2.5;
const PHOTO_COUNT: u32 = 44;

#[test]
fn current_thumbnail_is_centered_on_every_navigation_frame() {
    // GLib reads $HOME and the XDG directories once per process, on the
    // first GTK call, so the sandbox has to be in place before `gtk::init()`.
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().expect("GTK init");
    let runtime = tokio::runtime::Runtime::new().expect("viewer runtime");
    let _guard = runtime.enter();
    let shell = Shell::with_repeated_photos(PHOTO_COUNT as usize);
    let ui = &shell.ui;
    let grid = shell.visible_photos_grid();
    let tile = shell.tile_for(&grid, shell.items[0].id.into(), "Photos grid");
    ui.click(&tile, "first photo");
    assert!(ui.wait_until(Duration::from_secs(10), || shell
        .window
        .nav_view()
        .visible_page()
        .is_some_and(|page| page.is::<ViewerPage>())));
    let viewer = shell
        .window
        .nav_view()
        .visible_page()
        .unwrap()
        .downcast::<ViewerPage>()
        .unwrap();
    assert!(ui.wait_until(Duration::from_secs(10), || {
        let imp = viewer.imp();
        !imp.thumb_scroll_scheduled.get()
            && imp.thumb_items.borrow().len() >= 11
            && imp
                .thumb_items
                .borrow()
                .iter()
                .all(|button| button.width() > 0)
            && imp.edit_btn.get().is_sensitive()
    }));
    ui.pump(Duration::from_millis(500));

    let frames = Rc::new(RefCell::new(Vec::new()));
    let observed = frames.clone();
    let weak_viewer = viewer.downgrade();
    let weak_window = shell.window.downgrade();
    let clock = viewer.frame_clock().expect("mapped viewer frame clock");
    let observer = clock.connect_after_paint(move |_| {
        let (Some(viewer), Some(window)) = (weak_viewer.upgrade(), weak_window.upgrade()) else {
            return;
        };
        let imp = viewer.imp();
        let items = imp.thumb_items.borrow();
        let current = imp.current_index.get();
        let offset = current.saturating_sub(imp.thumb_window_start.get()) as usize;
        let highlighted: Vec<_> = items
            .iter()
            .enumerate()
            .filter(|(_, button)| button.has_css_class("viewer-thumb-current"))
            .map(|(index, _)| index)
            .collect();
        let delta = items.get(offset).and_then(|button| {
            let bounds = button.compute_bounds(&window)?;
            let viewport = imp.thumb_scrolled.get().compute_bounds(&window)?;
            Some(bounds.x() + bounds.width() / 2.0 - viewport.x() - viewport.width() / 2.0)
        });
        // The selected picture is larger, but its transparent side space must
        // still separate it from either neighbouring picture during animation.
        let side_gap = items.get(offset).and_then(|button| {
            let selected = button.child()?.compute_bounds(&window)?;
            let mut minimum = f32::INFINITY;
            if let Some(previous) = offset.checked_sub(1).and_then(|index| items.get(index)) {
                let bounds = previous.child()?.compute_bounds(&window)?;
                minimum = minimum.min(selected.x() - bounds.x() - bounds.width());
            }
            if let Some(next) = items.get(offset + 1) {
                let bounds = next.child()?.compute_bounds(&window)?;
                minimum = minimum.min(bounds.x() - selected.x() - selected.width());
            }
            Some(minimum)
        });
        observed
            .borrow_mut()
            .push((current, offset, highlighted, delta, side_gap));
    });

    // Mixed aspect ratios, both ends of the list, and growth past the initial
    // eleven-button window and the forty-button cap in both directions.
    // Every switch goes through the actual input path.
    for target in 1..PHOTO_COUNT {
        ui.click(&viewer.imp().next_btn.get(), "Next");
        assert!(
            ui.wait_until(Duration::from_secs(6), || viewer.current_index() == target),
            "navigation must reach {target}, current index is {}",
            viewer.current_index()
        );
        ui.pump(Duration::from_millis(250));
    }
    assert!(
        viewer.imp().thumb_window_start.get() > 0,
        "the journey must slide past the forty-button window cap"
    );
    for target in (0..PHOTO_COUNT - 1).rev() {
        assert!(ui.press_key(
            &shell.window,
            gtk::gdk::Key::Left,
            gtk::gdk::ModifierType::empty()
        ));
        assert!(
            ui.wait_until(Duration::from_secs(6), || viewer.current_index() == target),
            "navigation must reach {target}, current index is {}",
            viewer.current_index()
        );
        ui.pump(Duration::from_millis(250));
    }
    let target = viewer.imp().thumb_items.borrow()[3].clone();
    ui.click(&target, "third following thumbnail");
    assert!(ui.wait_until(Duration::from_secs(6), || viewer.current_index() == 3));
    ui.pump(Duration::from_millis(300));
    clock.disconnect(observer);
    assert!(ui.press_key(
        &shell.window,
        gtk::gdk::Key::Escape,
        gtk::gdk::ModifierType::empty()
    ));
    assert!(ui.wait_until(Duration::from_secs(5), || viewer
        .imp()
        .thumb_layout_handler
        .borrow()
        .is_none()));

    let frames = frames.borrow();
    assert!(frames.len() > 20, "must observe actual navigation frames");
    let failures: Vec<_> = frames
        .iter()
        .filter(|(_, offset, highlighted, delta, side_gap)| {
            highlighted.as_slice() != [*offset]
                || !delta.is_some_and(|value| value.abs() <= CENTER_TOLERANCE)
                || !side_gap.is_some_and(|value| value >= 2.0)
        })
        .collect();
    assert!(
        failures.is_empty(),
        "every painted navigation frame must keep the current thumbnail centered with clear side space; \
         {} of {} frames failed, first failures: {:?}",
        failures.len(),
        frames.len(),
        &failures[..failures.len().min(8)]
    );
}
