//! PhotosPage header bar + batch toolbar should carry the liquid-glass
//! material classes introduced in Task 1 (the global glass style system).
//!
//! GTK is single-threaded, so all checks live in one `#[test]` function.
//! See `tests/ui_mode_selector.rs` for the same pattern.

use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::ui::PhotosPage;

fn css_classes_vec<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

#[test]
fn photos_header_uses_glass_toolbar_classes() {
    gtk::init().expect("GTK init failed");

    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.PhotosToolbar")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));

    let page = PhotosPage::new(media_list, loader);
    let imp = page.imp();

    // HeaderBar carries glass-header.
    let header_classes = css_classes_vec(&imp.header_bar.get());
    assert!(
        header_classes.iter().any(|c| c == "glass-header"),
        "header_bar should carry glass-header, got {header_classes:?}",
    );

    // The search button and four batch-action toolbar widgets carry
    // glass-toolbar-button.
    // favorite_btn is now the merged heart trigger (icon-only); its two
    // actions live in a popover built in PhotosPage::new.
    let buttons: [(&str, gtk::Button); 5] = [
        ("search_btn", imp.search_btn.get()),
        ("select_all_btn", imp.select_all_btn.get()),
        ("add_to_album_btn", imp.add_to_album_btn.get()),
        ("favorite_btn", imp.favorite_btn.get()),
        ("delete_to_trash_btn", imp.delete_to_trash_btn.get()),
    ];
    for (name, btn) in buttons.iter() {
        let classes = css_classes_vec(btn);
        assert!(
            classes.iter().any(|c| c == "glass-toolbar-button"),
            "{name} should carry glass-toolbar-button, got {classes:?}",
        );
    }

    assert!(
        imp.search_btn.get().has_css_class("round-search-button"),
        "search_btn should use the dedicated circular search-button class"
    );

    // favorite_btn is a smart toggle: it reuses the viewer's red-heart hook
    // (viewer-favorite-btn + favorite-active) so an all-favorited selection
    // shows the same translucent red heart as the viewer. With no selection it
    // must not yet be active.
    let fav_classes = css_classes_vec(&imp.favorite_btn.get());
    assert!(
        fav_classes.iter().any(|c| c == "viewer-favorite-btn"),
        "favorite_btn should carry viewer-favorite-btn (red-heart hook), got {fav_classes:?}",
    );
    assert!(
        !fav_classes.iter().any(|c| c == "favorite-active"),
        "favorite_btn should not be favorite-active without a favorited selection, got {fav_classes:?}",
    );

    // The merged favorite popover exposes 收藏 / 取消收藏 as glass-menu items,
    // wired in PhotosPage::new and anchored to favorite_btn.
    let fav_item = imp
        .favorite_item_btn
        .borrow()
        .as_ref()
        .expect("favorite popover should have a 收藏 item")
        .clone();
    let unfav_item = imp
        .unfavorite_item_btn
        .borrow()
        .as_ref()
        .expect("favorite popover should have a 取消收藏 item")
        .clone();
    for (name, btn) in [
        ("favorite_item", &fav_item),
        ("unfavorite_item", &unfav_item),
    ] {
        let classes = css_classes_vec(btn);
        assert!(
            classes.iter().any(|c| c == "glass-menu-item"),
            "{name} should carry glass-menu-item, got {classes:?}",
        );
    }

    // delete_to_trash_btn additionally carries glass-toolbar-danger.
    let trash_classes = css_classes_vec(&imp.delete_to_trash_btn.get());
    assert!(
        trash_classes.iter().any(|c| c == "glass-toolbar-button"),
        "delete_to_trash_btn should carry glass-toolbar-button, got {trash_classes:?}",
    );
    assert!(
        trash_classes.iter().any(|c| c == "glass-toolbar-danger"),
        "delete_to_trash_btn should carry glass-toolbar-danger, got {trash_classes:?}",
    );
}
