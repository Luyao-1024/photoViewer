//! Album destinations share the Photos thumbnail and glass action vocabulary.

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::AdwApplicationWindowExt;
use photo_viewer::core::{db, db_actor, events, thumbnails::ThumbnailLoader};
use photo_viewer::ui::{album_picker::AlbumPickerDialog, grid_css};
use std::sync::Arc;

fn find_widget<F: Fn(&gtk::Widget) -> bool>(root: &gtk::Widget, pred: F) -> Option<gtk::Widget> {
    let mut stack = vec![root.clone()];
    while let Some(widget) = stack.pop() {
        if pred(&widget) {
            return Some(widget);
        }
        let mut child = widget.first_child();
        while let Some(next) = child {
            stack.push(next.clone());
            child = next.next_sibling();
        }
    }
    None
}

#[test]
fn album_picker_is_one_cover_grid_dialog_with_glass_actions() {
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.AlbumPickerGlass")
        .build();
    app.register(None::<&gtk::gio::Cancellable>).unwrap();
    grid_css::install();

    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let (events, _receiver) = events::DomainEventSender::new();
    let actor = db_actor::start_db_actor(pool.clone(), events);
    let nav = adw::NavigationView::new();
    let window = adw::ApplicationWindow::builder().application(&app).build();
    window.set_content(Some(&nav));
    window.present();

    AlbumPickerDialog::present(&nav, pool, actor, loader, vec![1]);
    let root = window.upcast_ref::<gtk::Widget>();
    let dialog =
        find_widget(root, |widget| widget.is::<adw::Dialog>()).expect("picker should be a dialog");
    assert_eq!(nav.navigation_stack().n_items(), 0);
    assert!(find_widget(&dialog, |widget| widget.is::<gtk::FlowBox>()).is_some());
    assert!(find_widget(&dialog, |widget| widget.is::<adw::NavigationView>()).is_none());
    let close = find_widget(&dialog, |widget| {
        widget.downcast_ref::<gtk::Button>().is_some_and(|button| {
            button.has_css_class("round-search-button")
                && button.icon_name().as_deref() == Some("window-close-symbolic")
        })
    })
    .expect("picker close control should reuse the circular icon style");
    assert!(close.has_css_class("glass-toolbar-button"));

    for (label, role) in [
        ("album_picker.copy", "glass-toolbar-suggested"),
        ("album_picker.move", "glass-toolbar-danger"),
    ] {
        let button = find_widget(&dialog, |widget| {
            widget.downcast_ref::<gtk::Button>().is_some_and(|button| {
                button.label().as_deref() == Some(photo_viewer::core::i18n::tr(label).as_str())
            })
        })
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
        assert!(button.has_css_class("glass-toolbar-button"));
        assert!(button.has_css_class(role));
        assert!(!button.is_sensitive());
    }
}
