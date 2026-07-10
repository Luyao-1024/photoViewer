use super::super::test_support::*;
use super::super::*;
use super::*;
use std::cell::RefCell;
use std::rc::Rc;

#[gtk::test]
fn keyboard_scope_is_viewer_when_viewer_page_is_visible() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardScopeViewer")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();

    let viewer = crate::ui::ViewerPage::new(keyboard_media_list(), 0);
    nav.push(&viewer);

    assert_eq!(
        window.keyboard_scope_for_tests(),
        crate::ui::keyboard::KeyboardScope::Viewer
    );
}

#[gtk::test]
fn keyboard_scope_is_browsing_when_photos_page_is_visible() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardScopeBrowsing")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let (_tmp, loader) = keyboard_thumbnail_loader();
    let photos = PhotosPage::new(keyboard_media_list(), loader);
    nav.push(&photos);

    assert_eq!(
        window.keyboard_scope_for_tests(),
        crate::ui::keyboard::KeyboardScope::Browsing
    );
}

#[gtk::test]
fn ctrl_f_opens_search_from_photos_page() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardSearch")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let media_list = keyboard_media_list();
    let (tmp, loader) = keyboard_thumbnail_loader();
    let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-search.db")).unwrap();
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());
    let photos = PhotosPage::new(media_list, loader);
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool);
    nav.push(&photos);

    let handled = emit_key_for_tests(
        &window,
        gtk::gdk::Key::f,
        gtk::gdk::ModifierType::CONTROL_MASK,
    );

    assert!(handled, "Ctrl+F should be handled from PhotosPage");
    assert!(
        nav.visible_page().and_downcast::<SearchPage>().is_some(),
        "Ctrl+F should push SearchPage from PhotosPage"
    );
}

#[gtk::test]
fn ctrl_a_selects_visible_photos_grid_items() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardBrowseSelectAll")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let media_list = keyboard_media_list();
    let (_tmp, loader) = keyboard_thumbnail_loader();
    let photos = PhotosPage::new(media_list, loader);
    nav.push(&photos);

    let handled = emit_key_for_tests(
        &window,
        gtk::gdk::Key::a,
        gtk::gdk::ModifierType::CONTROL_MASK,
    );

    assert!(
        handled,
        "Ctrl+A should be handled by the visible PhotosPage"
    );
    assert_eq!(
        photos.selected_count_for_tests(),
        1,
        "Ctrl+A should select the currently rendered item"
    );
}

#[gtk::test]
fn escape_clears_photos_grid_selection_before_navigation_back() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardBrowseEscapeSelection")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let media_list = keyboard_media_list();
    let (_tmp, loader) = keyboard_thumbnail_loader();
    let photos = PhotosPage::new(media_list, loader);
    nav.push(&photos);

    assert!(emit_key_for_tests(
        &window,
        gtk::gdk::Key::a,
        gtk::gdk::ModifierType::CONTROL_MASK,
    ));
    assert_eq!(photos.selected_count_for_tests(), 1);

    let handled = emit_key_for_tests(
        &window,
        gtk::gdk::Key::Escape,
        gtk::gdk::ModifierType::empty(),
    );

    assert!(
        handled,
        "Escape should be consumed by PhotosPage while selection is active"
    );
    assert_eq!(
        photos.selected_count_for_tests(),
        0,
        "Escape should clear selected photos"
    );
    assert!(
        nav.visible_page().and_downcast::<PhotosPage>().is_some(),
        "Escape should not navigate away while it is clearing selection"
    );
}

#[gtk::test]
fn ctrl_f_opens_search_from_trash_page() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardSearchTrash")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let media_list = keyboard_media_list();
    let (tmp, loader) = keyboard_thumbnail_loader();
    let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-search-trash.db")).unwrap();
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());
    let trash = TrashPage::with_media_list(pool, loader, media_list);
    nav.push(&trash);

    let handled = emit_key_for_tests(
        &window,
        gtk::gdk::Key::f,
        gtk::gdk::ModifierType::CONTROL_MASK,
    );

    assert!(handled, "Ctrl+F should be handled from TrashPage");
    assert!(
        nav.visible_page().and_downcast::<SearchPage>().is_some(),
        "Ctrl+F should push SearchPage from non-Photos pages when resources are available"
    );
}

#[gtk::test]
fn settings_modal_blocks_global_search_and_closes_on_escape() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardSettingsModal")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let media_list = keyboard_media_list();
    let (tmp, loader) = keyboard_thumbnail_loader();
    let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-settings.db")).unwrap();
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());
    let photos = PhotosPage::new(media_list, loader);
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool);
    nav.push(&photos);

    let opened = emit_key_for_tests(
        &window,
        gtk::gdk::Key::comma,
        gtk::gdk::ModifierType::CONTROL_MASK,
    );
    assert!(opened, "Ctrl+, should open Settings");
    assert!(window.imp().settings_dialog.borrow().is_some());
    assert_eq!(
        window.keyboard_scope_for_tests(),
        crate::ui::keyboard::KeyboardScope::Modal
    );

    let leaked = emit_key_for_tests(
        &window,
        gtk::gdk::Key::f,
        gtk::gdk::ModifierType::CONTROL_MASK,
    );
    assert!(!leaked, "Ctrl+F must not leak through Settings modal");
    assert!(
        nav.visible_page().and_downcast::<SearchPage>().is_none(),
        "SearchPage should not open behind Settings"
    );
    let settings_reopen = emit_key_for_tests(
        &window,
        gtk::gdk::Key::comma,
        gtk::gdk::ModifierType::CONTROL_MASK,
    );
    assert!(
        !settings_reopen,
        "Ctrl+, must not re-enter Settings while modal is open"
    );
    assert!(window.imp().settings_dialog.borrow().is_some());

    let closed = emit_key_for_tests(
        &window,
        gtk::gdk::Key::Escape,
        gtk::gdk::ModifierType::empty(),
    );
    assert!(closed, "Escape should close Settings modal");
    assert!(window.imp().settings_dialog.borrow().is_none());
}

#[gtk::test]
fn glass_menu_modal_escape_does_not_pop_visible_page() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardGlassMenuModal")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let root = adw::NavigationPage::builder()
        .title("Root")
        .child(&gtk::Label::new(Some("root")))
        .build();
    let pushed = adw::NavigationPage::builder()
        .title("Pushed")
        .child(&gtk::Label::new(Some("pushed")))
        .build();
    nav.push(&root);
    nav.push(&pushed);

    let layer = gtk::Fixed::builder()
        .can_focus(true)
        .css_classes(["glass-context-menu-layer"])
        .build();
    let button = gtk::Button::with_label("Menu item");
    layer.put(&button, 0.0, 0.0);
    window.imp().root_overlay.get().add_overlay(&layer);
    window.present();
    button.grab_focus();
    while glib::MainContext::default().iteration(false) {}
    assert_eq!(
        window.keyboard_scope_for_tests(),
        crate::ui::keyboard::KeyboardScope::Modal
    );

    let handled = emit_key_for_tests(
        &window,
        gtk::gdk::Key::Escape,
        gtk::gdk::ModifierType::empty(),
    );

    assert!(
        !handled,
        "Window router should let glass menu Escape reach the menu-local handler"
    );
    assert_eq!(
        nav.visible_page().map(|page| page.title().to_string()),
        Some("Pushed".to_string()),
        "Modal Escape must not pop the page underneath"
    );
    window.imp().root_overlay.get().remove_overlay(&layer);
    window.close();
    while glib::MainContext::default().iteration(false) {}
}

#[gtk::test]
fn ctrl_f_reuses_visible_search_page() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardSearchReuse")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();
    let media_list = keyboard_media_list();
    let (tmp, loader) = keyboard_thumbnail_loader();
    let pool = crate::core::db::init_pool(&tmp.path().join("keyboard-search-reuse.db")).unwrap();
    window.set_resources(pool.clone(), loader.clone(), media_list);
    let search = SearchPage::new(pool, loader);
    search.set_nav_target(&nav);
    nav.push(&search);
    let page_before = nav.visible_page().expect("search visible");

    let handled = emit_key_for_tests(
        &window,
        gtk::gdk::Key::f,
        gtk::gdk::ModifierType::CONTROL_MASK,
    );

    assert!(handled, "Ctrl+F should focus/reuse visible SearchPage");
    let page_after = nav.visible_page().expect("search still visible");
    assert!(
        page_before == page_after,
        "Ctrl+F should not push a duplicate SearchPage"
    );
}

#[gtk::test]
fn viewer_right_key_navigates_when_focus_is_on_header_button() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.KeyboardViewerFocus")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();

    let viewer = crate::ui::ViewerPage::new(keyboard_media_list(), 0);
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_cb = events.clone();
    viewer.connect_navigation(move |delta| {
        events_for_cb.borrow_mut().push(delta);
    });
    nav.push(&viewer);

    viewer.imp().details_btn.get().grab_focus();
    let handled = emit_key_for_tests(
        &window,
        gtk::gdk::Key::Right,
        gtk::gdk::ModifierType::empty(),
    );

    assert!(handled, "viewer Right shortcut should stop propagation");
    assert_eq!(events.borrow().as_slice(), &[1]);
}

#[gtk::test]
fn opening_album_pops_viewer_pushed_above_browsing_root() {
    // Regression: clicking an album in the sidebar while the viewer was up
    // used to leave the viewer covering the new album page because the
    // album/media-type sidebar handlers did not pop the nav stack the way
    // Photos/Trash did. `open_album` now pops to the browsing root before
    // swapping the album underneath.
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.AlbumOpenPopsViewer")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = MainWindow::new(&app);
    let nav = window.nav_view();

    let (_tmp, loader) = keyboard_thumbnail_loader();
    let media_list = keyboard_media_list();
    let pool = crate::core::db::init_pool(&_tmp.path().join("album-open-pops.db")).unwrap();
    window.set_resources(pool, loader, media_list);

    // Simulate the post-photo-activation state: browsing root + viewer on top.
    let viewer = crate::ui::ViewerPage::new(keyboard_media_list(), 0);
    nav.push(&viewer);
    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "viewer should sit on top of the browsing root before the album click"
    );

    let album = crate::core::albums::Album {
        folder_path: std::path::PathBuf::from("/tmp/AlbumOpenPopsViewer"),
        name: "AlbumOpenPopsViewer".into(),
        cover_uri: None,
        photo_count: 0,
        last_modified: chrono::Utc::now(),
        is_virtual: false,
    };
    window.open_album(&nav, album);

    assert_eq!(
        nav.navigation_stack().n_items(),
        1,
        "open_album must pop the viewer before swapping the album underneath"
    );
    assert!(
        nav.visible_page()
            .and_downcast::<crate::ui::ViewerPage>()
            .is_none(),
        "viewer must no longer be the visible page after switching albums"
    );
}
