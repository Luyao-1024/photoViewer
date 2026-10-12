//! Navigation and transition lifecycle shared by browsing viewer entry points.
//!
//! Pages retain their query, visibility checks, dependency injection and domain
//! callbacks. This helper wires navigation and protects the source page while
//! the push settles, using weak references in the navigation callback.

use std::cell::Cell;

use gtk4::{glib, prelude::*, subclass::prelude::ObjectSubclassIsExt};
use libadwaita as adw;

use super::viewer_page::{NavDelta, ViewerPage, NAV_POP, VIEWER_OPEN_POP_GUARD_MS};

pub(super) fn push_viewer(
    nav: &adw::NavigationView,
    viewer: &ViewerPage,
    pending: &Cell<bool>,
    clear_pending: impl FnOnce() + 'static,
) {
    let viewer_weak = viewer.downgrade();
    let nav_weak = nav.downgrade();
    viewer.connect_navigation(move |delta: NavDelta| {
        if delta == NAV_POP {
            if let Some(n) = nav_weak.upgrade() {
                n.pop();
            }
            return;
        }
        if let Some(v) = viewer_weak.upgrade() {
            let cur = v.current_index();
            let next = (cur as i32 + delta).max(0) as u32;
            if let Some(list) = v.imp().media_list.borrow().as_ref() {
                if next < list.n_items() {
                    v.show_at(next);
                }
            }
        }
    });

    pending.set(true);
    let source_page = nav.visible_page();
    if let Some(page) = source_page.as_ref() {
        page.set_sensitive(false);
    }
    glib::timeout_add_local_once(
        std::time::Duration::from_millis(VIEWER_OPEN_POP_GUARD_MS),
        move || {
            clear_pending();
            if let Some(page) = source_page {
                page.set_sensitive(true);
            }
        },
    );

    nav.push(viewer);
}
