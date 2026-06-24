//! ViewerPage toolbar buttons + image stage should carry the liquid-glass
//! material classes introduced in Task 1, and the favorite-active visual
//! should be owned by the global CSS provider (Task 7).
//!
//! GTK is single-threaded, so all checks live in one `#[test]` function.
//! See `tests/ui_mode_selector.rs` and `tests/ui_photos_toolbar.rs` for
//! the same pattern.

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::ui::ViewerPage;

fn css_classes_vec<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

#[test]
fn viewer_toolbar_uses_glass_classes() {
    gtk::init().expect("GTK init failed");

    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.ViewerToolbar")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    // Install the global CSS provider so any class-resolving sanity check
    // finds the rules; the assertions only inspect the .css_classes() list
    // (which is set by the .blp template), not the computed style.
    photo_viewer::ui::grid_css::install();

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    let page = ViewerPage::new(media_list, 0);
    let imp = page.imp();

    // HeaderBar carries glass-header. (The previous `viewer-header` class
    // was removed in Fix #4 — it had no corresponding CSS rule and was
    // effectively dead code.)
    let header_classes = css_classes_vec(&imp.header_bar.get());
    assert!(
        header_classes.iter().any(|c| c == "glass-header"),
        "header_bar should carry glass-header, got {header_classes:?}",
    );
    assert!(
        !header_classes.iter().any(|c| c == "viewer-header"),
        "header_bar should no longer carry viewer-header (dead class removed), got {header_classes:?}",
    );

    // All five toolbar buttons carry glass-toolbar-button.
    let button_classes: [(&str, Vec<String>); 5] = [
        ("details_btn", css_classes_vec(&imp.details_btn.get())),
        ("delete_btn", css_classes_vec(&imp.delete_btn.get())),
        ("favorite_btn", css_classes_vec(&imp.favorite_btn.get())),
        (
            "add_to_album_btn",
            css_classes_vec(&imp.add_to_album_btn.get()),
        ),
        ("edit_btn", css_classes_vec(&imp.edit_btn.get())),
    ];
    for (name, classes) in button_classes.iter() {
        assert!(
            classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}",
        );
    }

    // delete_btn also carries glass-toolbar-danger.
    let del_classes = css_classes_vec(&imp.delete_btn.get());
    assert!(
        del_classes.iter().any(|c| c == "glass-toolbar-danger"),
        "delete_btn should carry glass-toolbar-danger, got {del_classes:?}",
    );

    // favorite_btn also carries viewer-favorite-btn.
    let fav_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        fav_classes.iter().any(|c| c == "viewer-favorite-btn"),
        "favorite_btn should carry viewer-favorite-btn, got {fav_classes:?}",
    );

    // Task 8: details_close_btn carries glass-toolbar-button.
    let close_classes = css_classes_vec(&imp.details_close_btn.get());
    assert!(
        close_classes.iter().any(|c| c == "glass-toolbar-button"),
        "details_close_btn should carry glass-toolbar-button, got {close_classes:?}",
    );

    // Task 8: details_panel (the OverlaySplitView sidebar slot) carries
    // viewer-details-panel + glass-base, and no longer carries the opaque
    // `background` class. `details_panel` is not a template_child, so we
    // reach the widget via the split view's `sidebar()` accessor.
    let split_view = imp.details_split_view.get();
    let details_panel = split_view
        .sidebar()
        .expect("OverlaySplitView should have a sidebar widget");
    let panel_classes = css_classes_vec(&details_panel);
    assert!(
        panel_classes.iter().any(|c| c == "viewer-details-panel"),
        "details_panel should carry viewer-details-panel, got {panel_classes:?}",
    );
    assert!(
        panel_classes.iter().any(|c| c == "glass-base"),
        "details_panel should carry glass-base, got {panel_classes:?}",
    );
    assert!(
        !panel_classes.iter().any(|c| c == "background"),
        "details_panel should no longer carry opaque `background`, got {panel_classes:?}",
    );

    // Task: favorite-active class toggle on the viewer favorite button.
    // The class is owned by the global CSS provider (Task 7) and must be
    // addable / removable at runtime without disturbing the base classes.
    let initial_fav_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        !initial_fav_classes.iter().any(|c| c == "favorite-active"),
        "favorite_btn should not initially carry favorite-active, got {initial_fav_classes:?}",
    );

    imp.favorite_btn.get().add_css_class("favorite-active");
    let after_add_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        after_add_classes.iter().any(|c| c == "favorite-active"),
        "favorite_btn should carry favorite-active after add_css_class, got {after_add_classes:?}",
    );
    assert!(
        after_add_classes.iter().any(|c| c == "viewer-favorite-btn"),
        "favorite_btn should still carry viewer-favorite-btn after add_css_class, got {after_add_classes:?}",
    );
    assert!(
        after_add_classes.iter().any(|c| c == "glass-toolbar-button"),
        "favorite_btn should still carry glass-toolbar-button after add_css_class, got {after_add_classes:?}",
    );

    imp.favorite_btn.get().remove_css_class("favorite-active");
    let after_remove_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        !after_remove_classes.iter().any(|c| c == "favorite-active"),
        "favorite_btn should not carry favorite-active after remove_css_class, got {after_remove_classes:?}",
    );
}
