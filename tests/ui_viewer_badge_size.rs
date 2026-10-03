//! The viewer header pairs the cloud badge (`sync_badge`) with the favorite
//! button (`favorite_btn`), and those two are the only glyphs in that row, so
//! they have to render at the same size or the row reads as a mismatched pair.
//!
//! `sync_badge` is a bare `GtkImage`, so with no size of its own it takes GTK's
//! default icon size (16 px) while a button's icon follows whatever the header
//! resolves — and that is not always 16. That gap is what made the badge render
//! a third smaller than the heart beside it. The page therefore copies the
//! resolved toolbar icon size onto the badge once the header is mapped, and
//! this test pins that behaviour against two headers: the default one, and one
//! that resolves 20 px, which is the reported case.
//!
//! The artwork half of the same contract — hairline stroke, a silhouette that
//! fills its box, identical geometry across states and themes — is asserted
//! next to the badge itself in `src/ui/cloud_badge.rs`.
//!
//! GTK is single-threaded and its windows interfere with each other inside one
//! process, so this file holds a single `#[test]`, following the pattern in
//! `tests/ui_viewer_toolbar.rs`.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::ui::ViewerPage;
use std::time::{Duration, Instant};

/// Drain the main context without blocking on an empty queue.
fn pump_for(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
    while context.iteration(false) {}
}

fn natural_size(image: &gtk::Image) -> i32 {
    image.measure(gtk::Orientation::Horizontal, -1).1
}

/// The icon sizes the header actually resolves: the badge's (its own size, and
/// what that renders at) next to the favorite icon's.
fn resolved_header_icon_sizes(extra_css: Option<&str>) -> (i32, i32) {
    if let Some(css) = extra_css {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(css);
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("a display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let viewer = ViewerPage::new(media_list, 0);
    let imp = viewer.imp();
    let badge = imp.sync_badge.get();
    let toolbar_icon: gtk::Image = imp
        .favorite_btn
        .get()
        .child()
        .expect("favorite_btn holds an icon")
        .downcast::<gtk::Image>()
        .expect("favorite_btn's icon is a Gtk.Image");

    // A header only resolves its icon size once it is mapped, so this has to
    // present a real window rather than just build the page.
    let window = gtk::Window::new();
    window.set_default_size(920, 200);
    window.set_titlebar(Some(&imp.header_bar.get()));
    window.present();
    pump_for(Duration::from_millis(400));

    // The badge is asserted on its own size, not on `natural_size`: the page
    // only hands it a bitmap once a cloud state arrives, and a state cannot be
    // pushed from outside the crate. The copied size is what determines the
    // rendered box once the bitmap is there.
    let sizes = (badge.pixel_size(), natural_size(&toolbar_icon));
    window.destroy();
    pump_for(Duration::from_millis(50));
    sizes
}

#[test]
fn viewer_header_badge_matches_the_toolbar_icon_beside_it() {
    gtk::init().expect("GTK init failed");
    photo_viewer::ensure_resources_registered();

    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.ViewerBadgeSize")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    photo_viewer::ui::grid_css::install();

    // Whatever this header resolves, the two glyphs have to agree.
    let (badge, toolbar_icon) = resolved_header_icon_sizes(None);
    assert!(
        toolbar_icon > 0,
        "the header should resolve a real icon size, got {toolbar_icon}",
    );
    assert_eq!(
        badge, toolbar_icon,
        "the cloud badge takes {badge} px while the favorite icon beside it resolves to \
         {toolbar_icon} px; those two are the whole content of this header row",
    );

    // The reported case: a header whose toolbar icons resolve larger than
    // GTK's 16 px default. A badge that only follows the default lands a third
    // smaller than its neighbour here.
    let (badge, toolbar_icon) = resolved_header_icon_sizes(Some(
        ".viewer-chrome .glass-toolbar-button > image { -gtk-icon-size: 20px; }",
    ));
    assert_eq!(
        toolbar_icon, 20,
        "the simulated header should resolve 20 px, otherwise this proves nothing",
    );
    assert_eq!(
        badge, 20,
        "the badge must take the header's icon size, not GTK's 16 px default",
    );

    // The size is copied from the toolbar at runtime, so nothing in the
    // template or the CSS may pin one: a declared size would either be ignored
    // or silently override what the copy just resolved.
    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let viewer = ViewerPage::new(media_list, 0);
    assert_eq!(
        viewer.imp().sync_badge.pixel_size(),
        -1,
        "sync_badge starts unpinned; the size is copied from the toolbar once the header is \
         mapped, and a template pixel-size would fight that",
    );

    let css = photo_viewer::ui::grid_css::css_for_tests();
    let badge_block = css
        .split(".viewer-sync-badge")
        .nth(1)
        .map(|rest| rest.split('}').next().unwrap_or_default())
        .unwrap_or_default();
    for property in ["pixel-size", "icon-size", "min-width", "min-height"] {
        assert!(
            !badge_block.contains(property),
            ".viewer-sync-badge must not set {property}: {badge_block:?}",
        );
    }
    assert!(
        viewer
            .imp()
            .sync_badge
            .css_classes()
            .iter()
            .any(|c| c == "viewer-sync-badge"),
        "sanity: the badge keeps the class those rules are written against",
    );
}
