//! Album destinations share the Photos thumbnail and glass action vocabulary.

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{AdwApplicationWindowExt, AdwDialogExt};
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

    // P2-6: the picker used to open on an empty grid and then either fill it or,
    // when the read failed, print "还没有相册" underneath. Loading / empty / failed
    // are three facts and only one of them is an empty state.
    let stack = find_widget(&dialog, |widget| widget.is::<gtk::Stack>())
        .expect("the picker keeps loading, empty, albums and error as stack pages")
        .downcast::<gtk::Stack>()
        .unwrap();
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("loading"),
        "the dialog must open on its loading page, not on a grid that merely happens to be empty"
    );
    pump_until(|| stack.visible_child_name().as_deref() == Some("empty"));
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("empty"),
        "an album-less library is the empty page"
    );
    // Every stack page stays in the widget tree, so read the page that is showing.
    let visible = stack
        .visible_child()
        .expect("the picker stack always shows one page");
    let empty_page = find_widget(&visible, |widget| widget.is::<adw::StatusPage>())
        .expect("the empty page is a status page")
        .downcast::<adw::StatusPage>()
        .unwrap();
    assert_eq!(
        empty_page.title().as_str(),
        photo_viewer::core::i18n::tr("album_picker.no_albums_yet.title").as_str(),
        "the empty state must say which fact it states, not just show a description line"
    );
    dialog
        .downcast_ref::<adw::Dialog>()
        .expect("the first picker is an Adw.Dialog")
        .close();

    let broken_dir = tempfile::tempdir().unwrap();
    let broken_pool = db::init_pool(&broken_dir.path().join("broken.db")).unwrap();
    broken_pool
        .get()
        .unwrap()
        .execute_batch("DROP TABLE albums")
        .unwrap();
    let broken_loader = Arc::new(ThumbnailLoader::new(
        broken_pool.clone(),
        broken_dir.path().join("thumbs"),
    ));
    let (broken_events, _broken_receiver) = events::DomainEventSender::new();
    let broken_actor = db_actor::start_db_actor(broken_pool.clone(), broken_events);
    AlbumPickerDialog::present(&nav, broken_pool, broken_actor, broken_loader, vec![1]);

    let second = find_dialog_excluding(&window, &dialog);
    let second_ref: &gtk::Widget = &second;
    let error_stack = find_widget(&second, |widget| widget.is::<gtk::Stack>())
        .expect("the second dialog also starts on a stack")
        .downcast::<gtk::Stack>()
        .unwrap();
    pump_until(|| error_stack.visible_child_name().as_deref() == Some("error"));
    assert_eq!(
        error_stack.visible_child_name().as_deref(),
        Some("error"),
        "a failed read must land on the error page rather than the empty one"
    );
    let error_visible = error_stack
        .visible_child()
        .expect("the second picker stack always shows one page");
    let error_page = find_widget(&error_visible, |widget| widget.is::<adw::StatusPage>())
        .expect("the error page is a status page")
        .downcast::<adw::StatusPage>()
        .unwrap();
    let description = error_page.description().unwrap_or_default();
    assert!(
        description.contains("albums"),
        "the real database reason has to be visible, got {description:?}"
    );
    assert!(
        find_widget(second_ref, |widget| {
            widget.downcast_ref::<gtk::Button>().is_some_and(|button| {
                button.label().as_deref()
                    == Some(photo_viewer::core::i18n::tr("common.retry").as_str())
            })
        })
        .is_some(),
        "an error page without a way out leaves the user staring at it"
    );
    second_ref
        .downcast_ref::<adw::Dialog>()
        .expect("the second picker is an Adw.Dialog")
        .close();
}

/// Pump the default main context until `settled`, with a deadline so a missing
/// worker answer fails the test instead of hanging it.
fn pump_until(settled: impl Fn() -> bool) {
    let context = gtk::glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && !settled() {
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        settled(),
        "the album picker never settled within the deadline"
    );
}

fn find_dialog_excluding(window: &adw::ApplicationWindow, previous: &gtk::Widget) -> gtk::Widget {
    let mut found = None;
    let mut stack = vec![window.clone().upcast::<gtk::Widget>()];
    while let Some(widget) = stack.pop() {
        if widget.is::<adw::Dialog>() && &widget != previous {
            found = Some(widget.clone());
        }
        let mut child = widget.first_child();
        while let Some(next) = child {
            stack.push(next.clone());
            child = next.next_sibling();
        }
    }
    found.expect("the second picker dialog should be present")
}
