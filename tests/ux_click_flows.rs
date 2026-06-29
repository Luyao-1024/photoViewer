//! UX-level click flow coverage.
//!
//! These tests exercise the same GTK signal paths a user hits: clicking the
//! Photos mode selector cells and activating a rendered thumbnail tile. They
//! intentionally avoid calling `PhotosPage` internals such as `open_viewer`.

use chrono::{TimeZone, Utc};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::albums::Album;
use photo_viewer::core::media::{MediaItem, NewMediaItem, MEDIA_SUBKIND_STANDARD};
use photo_viewer::core::thumbnails::ThumbnailLoader;
use photo_viewer::core::{albums, db};
use photo_viewer::ui::{
    album_picker, AlbumBrowserPage, AlbumDetailPage, MainWindow, MediaGrid, ModeSelector,
    PhotosPage, TrashPage, ViewerPage,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct PhotosFixture {
    _tmp: tempfile::TempDir,
    pool: db::DbPool,
    loader: Arc<ThumbnailLoader>,
    media_list: gtk::gio::ListStore,
    page: PhotosPage,
    nav: adw::NavigationView,
    items: Vec<MediaItem>,
}

#[test]
fn ux_click_flow_suite() {
    gtk::init().expect("GTK init failed");
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime for UX click flows");
    let _runtime_guard = runtime.enter();

    mode_selector_click_switches_photos_view();
    thumbnail_activation_opens_one_viewer();
    photos_batch_toolbar_clicks_select_favorite_and_album();
    viewer_chrome_clicks_drive_visible_operations();
    sidebar_clicks_drive_top_level_navigation();
    album_picker_clicks_album_row_and_copy_move();
    album_pages_clicks_open_album_and_viewer();
    trash_page_clicks_selection_cancel_restore_and_delete();
}

fn mode_selector_click_switches_photos_view() {
    let fixture = build_photos_page_with_nav();
    let selector = find_descendant::<ModeSelector>(fixture.page.upcast_ref())
        .expect("PhotosPage should contain a ModeSelector");
    let stack = find_descendant::<adw::ViewStack>(fixture.page.upcast_ref())
        .expect("PhotosPage should contain a ViewStack");

    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("day"),
        "PhotosPage starts on Day when media exists"
    );

    click_mode_selector_cell(&selector, 0);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("year"),
        "clicking Year switches the bound Photos view"
    );

    click_mode_selector_cell(&selector, 1);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("month"),
        "clicking Month switches the bound Photos view"
    );

    click_mode_selector_cell(&selector, 2);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("day"),
        "clicking Day returns to the dense photo grid"
    );
}

fn thumbnail_activation_opens_one_viewer() {
    let fixture = build_photos_page_with_nav();
    let first_tile =
        first_flowbox_child(fixture.page.upcast_ref()).expect("rendered thumbnail exists");
    let flow = first_tile
        .parent()
        .and_then(|w| w.downcast::<gtk::FlowBox>().ok())
        .expect("thumbnail child should belong to a FlowBox");

    flow.emit_by_name::<()>("child-activated", &[&first_tile]);
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);

    assert_eq!(
        fixture.nav.navigation_stack().n_items(),
        2,
        "rapid repeated tile activation should push only one viewer page"
    );
    assert!(
        fixture
            .nav
            .visible_page()
            .and_downcast::<ViewerPage>()
            .is_some(),
        "thumbnail activation should open the viewer page"
    );
}

fn photos_batch_toolbar_clicks_select_favorite_and_album() {
    let fixture = build_photos_page_with_nav();
    let stack = find_descendant::<adw::ViewStack>(fixture.page.upcast_ref())
        .expect("PhotosPage should contain a ViewStack");
    let grid = stack
        .visible_child()
        .and_downcast::<MediaGrid>()
        .expect("Day grid should be visible");
    let first_tile =
        first_flowbox_child(grid.upcast_ref()).expect("rendered thumbnail exists in Day grid");
    let flow = first_tile
        .parent()
        .and_then(|w| w.downcast::<gtk::FlowBox>().ok())
        .expect("thumbnail child should belong to a FlowBox");

    grid.set_multi_select_mode(true);
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);

    assert!(
        fixture.page.imp().add_to_album_btn.get().is_visible(),
        "selecting a tile should expose the batch add-to-album action"
    );
    assert!(
        fixture.page.imp().favorite_btn.get().is_visible(),
        "selecting a tile should expose the batch favorite action"
    );
    assert!(
        fixture.page.imp().delete_to_trash_btn.get().is_visible(),
        "selecting a tile should expose the batch trash action"
    );

    click_button(&fixture.page.imp().select_all_btn.get());
    assert!(
        grid.is_all_displayed_selected(),
        "clicking Select All selects every rendered tile in the current mode"
    );
    click_button(&fixture.page.imp().select_all_btn.get());
    assert!(
        !fixture.page.imp().favorite_btn.get().is_visible(),
        "clicking the toggled Select All button clears selection and hides batch actions"
    );

    grid.set_multi_select_mode(true);
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);
    click_button(&fixture.page.imp().favorite_btn.get());
    let favorite_id = fixture.items[0].id;
    assert!(
        wait_until(Duration::from_secs(2), || db::is_media_favorite(
            &fixture.pool,
            favorite_id
        )
        .unwrap_or(false)),
        "clicking the batch favorite button should persist favorite state"
    );

    grid.set_multi_select_mode(true);
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);
    click_button(&fixture.page.imp().add_to_album_btn.get());
    assert_eq!(
        fixture.nav.navigation_stack().n_items(),
        2,
        "clicking Add to Album should push the album picker page"
    );
}

fn viewer_chrome_clicks_drive_visible_operations() {
    photo_viewer::ui::grid_css::install();
    let fixture = build_photos_page_with_nav();
    let viewer = ViewerPage::new(fixture.media_list.clone(), 0);
    viewer.set_edit_target(&fixture.nav, fixture.pool.clone());
    viewer.set_thumbnail_loader(fixture.loader.clone());
    viewer.show_at(0);

    let nav_events = Rc::new(RefCell::new(Vec::new()));
    let nav_events_for_cb = nav_events.clone();
    viewer.connect_navigation(move |delta| {
        nav_events_for_cb.borrow_mut().push(delta);
    });

    click_button(&viewer.imp().next_btn.get());
    click_button(&viewer.imp().prev_btn.get());
    assert_eq!(
        nav_events.borrow().as_slice(),
        &[1, -1],
        "viewer prev/next button clicks should emit navigation deltas"
    );

    click_button(&viewer.imp().details_btn.get());
    assert!(
        viewer.imp().details_split_view.get().shows_sidebar(),
        "clicking details should reveal the details sidebar"
    );
    click_button(&viewer.imp().details_close_btn.get());
    assert!(
        !viewer.imp().details_split_view.get().shows_sidebar(),
        "clicking the details close button should hide the details sidebar"
    );

    let initial_zoom = viewer.imp().zoom_scale.get();
    click_button(&viewer.imp().zoom_in_btn.get());
    assert!(
        viewer.imp().zoom_scale.get() > initial_zoom,
        "clicking zoom-in should increase viewer zoom"
    );
    click_button(&viewer.imp().zoom_reset_btn.get());
    assert_eq!(
        viewer.imp().zoom_scale.get(),
        initial_zoom,
        "clicking zoom reset should restore the initial zoom"
    );

    click_button(&viewer.imp().favorite_btn.get());
    let favorite_id = fixture.items[0].id;
    assert!(
        wait_until(Duration::from_secs(2), || db::is_media_favorite(
            &fixture.pool,
            favorite_id
        )
        .unwrap_or(false)),
        "clicking the viewer favorite button should persist favorite state"
    );
}

fn sidebar_clicks_drive_top_level_navigation() {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.UxClickFlows")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let fixture = build_photos_page_with_nav();
    let window = MainWindow::new(&app);
    window.populate_sidebar();
    window.set_resources(
        fixture.pool.clone(),
        fixture.loader.clone(),
        fixture.media_list.clone(),
    );
    albums::refresh(&fixture.pool).unwrap();
    window.populate_album_rows();
    let nav = window.nav_view();
    let root_page = PhotosPage::new(fixture.media_list.clone(), fixture.loader.clone());
    root_page.set_nav_target(&nav);
    root_page.set_db_pool(fixture.pool.clone());
    nav.push(&root_page);
    window.connect_sidebar(&nav);

    let sidebar = window.imp().sidebar_list.get();
    let header = sidebar.row_at_index(1).expect("Albums header row exists");
    let first_album_row = window.imp().album_rows.borrow()[0].clone();
    assert!(visible_flag(first_album_row.upcast_ref()));
    release_click_on_widget(header.upcast_ref());
    assert!(
        !visible_flag(first_album_row.upcast_ref()),
        "clicking the Albums header should collapse album rows"
    );
    release_click_on_widget(header.upcast_ref());
    assert!(
        visible_flag(first_album_row.upcast_ref()),
        "clicking the Albums header again should expand album rows"
    );

    let n_items = sidebar.observe_children().n_items() as i32;
    let trash_row = sidebar.row_at_index(n_items - 1).expect("Trash row exists");
    sidebar.select_row(Some(&trash_row));
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "selecting the Trash sidebar row should show TrashPage"
    );

    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    sidebar.select_row(Some(&photos_row));
    assert!(
        nav.visible_page().and_downcast::<PhotosPage>().is_some(),
        "selecting the Photos sidebar row should return to PhotosPage"
    );

    click_button(&window.imp().settings_button.get());
    assert!(
        nav.has_css_class("settings-background-blur"),
        "clicking the settings button should present settings chrome over the content nav"
    );
}

fn album_picker_clicks_album_row_and_copy_move() {
    let fixture = build_photos_page_with_nav();
    let original_count = db::list_all_media(&fixture.pool).unwrap().len();

    photo_viewer::ui::AlbumPickerDialog::present(
        &fixture.nav,
        fixture.pool.clone(),
        vec![fixture.items[0].id],
    );
    let wrapper = fixture
        .nav
        .visible_page()
        .expect("AlbumPicker should push a wrapper page");
    let inner = find_descendant::<adw::NavigationView>(wrapper.upcast_ref())
        .expect("AlbumPicker wrapper should contain an inner NavigationView");
    let list_box = find_descendant::<gtk::ListBox>(wrapper.upcast_ref())
        .expect("AlbumPicker should contain an album ListBox");
    assert!(
        wait_until(Duration::from_secs(2), || list_box
            .observe_children()
            .n_items()
            > 0),
        "AlbumPicker should populate album rows"
    );
    let first_album_row = list_box
        .row_at_index(0)
        .expect("AlbumPicker should render at least one album row");
    first_album_row.emit_by_name::<()>("activate", &[]);
    assert_eq!(
        inner.navigation_stack().n_items(),
        2,
        "activating an album row should push the Copy/Move action page"
    );

    let copy_btn = find_button_with_css(wrapper.upcast_ref(), "glass-toolbar-suggested")
        .expect("Copy button should be present on the AlbumPicker action page");
    click_button(&copy_btn);
    assert!(
        wait_until(Duration::from_secs(2), || db::list_all_media(&fixture.pool)
            .map(|items| items.len() > original_count)
            .unwrap_or(false)),
        "clicking Copy should create a copied media row"
    );
    assert!(
        wait_until(Duration::from_secs(2), || inner
            .navigation_stack()
            .n_items()
            == 1),
        "AlbumPicker should return to the album list after Copy"
    );

    let move_target = fixture._tmp.path().join("move-target");
    std::fs::create_dir_all(&move_target).unwrap();
    album_picker::push_action_page(
        &inner,
        fixture.pool.clone(),
        vec![fixture.items[1].id],
        move_target.clone(),
        &fixture.nav,
    );
    let move_btn = find_button_with_css(wrapper.upcast_ref(), "glass-toolbar-danger")
        .expect("Move button should be present on the AlbumPicker action page");
    click_button(&move_btn);
    assert!(
        wait_until(Duration::from_secs(2), || db::get_media_item(
            &fixture.pool,
            fixture.items[1].id
        )
        .map(|item| item.folder_path == move_target)
        .unwrap_or(false)),
        "clicking Move should update the media item's album folder"
    );
}

fn album_pages_clicks_open_album_and_viewer() {
    let fixture = build_photos_page_with_nav();
    let opened_albums = Rc::new(RefCell::new(Vec::<Album>::new()));
    let opened_albums_for_cb = opened_albums.clone();
    let browser = AlbumBrowserPage::new(
        fixture.pool.clone(),
        fixture.loader.clone(),
        Rc::new(move |album| {
            opened_albums_for_cb.borrow_mut().push(album);
        }),
    );
    let browser_tile =
        first_flowbox_child(browser.upcast_ref()).expect("Album browser should render albums");
    let browser_card = browser_tile
        .first_child()
        .expect("Album browser FlowBoxChild should contain a clickable card");
    release_click_on_widget(&browser_card);
    assert_eq!(
        opened_albums.borrow().len(),
        1,
        "clicking an AlbumBrowser card should invoke the open-album callback"
    );

    let album = albums::list(&fixture.pool)
        .unwrap()
        .into_iter()
        .find(|album| !album.is_virtual)
        .expect("fixture should create a real folder album");
    let album_items = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in fixture
        .items
        .iter()
        .filter(|item| item.folder_path == album.folder_path)
    {
        album_items.append(&glib::BoxedAnyObject::new(item.clone()));
    }
    let detail = AlbumDetailPage::new(
        album,
        album_items,
        fixture.media_list.clone(),
        fixture.pool.clone(),
        fixture.loader.clone(),
    );
    detail.set_nav_target(&fixture.nav);
    fixture.nav.push(&detail);

    let detail_tile =
        first_flowbox_child(detail.upcast_ref()).expect("Album detail should render media tiles");
    let detail_flow = detail_tile
        .parent()
        .and_then(|w| w.downcast::<gtk::FlowBox>().ok())
        .expect("Album detail tile should belong to a FlowBox");
    detail_flow.emit_by_name::<()>("child-activated", &[&detail_tile]);
    assert!(
        fixture
            .nav
            .visible_page()
            .and_downcast::<ViewerPage>()
            .is_some(),
        "activating an AlbumDetail tile should open the viewer"
    );
}

fn trash_page_clicks_selection_cancel_restore_and_delete() {
    let fixture = build_photos_page_with_nav();
    db::mark_trashed(&fixture.pool, fixture.items[0].id).unwrap();
    db::mark_trashed(&fixture.pool, fixture.items[1].id).unwrap();
    let shared = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let trash =
        TrashPage::with_media_list(fixture.pool.clone(), fixture.loader.clone(), shared.clone());
    let flow = trash.imp().flow_box.get();
    assert!(
        wait_until(Duration::from_secs(2), || flow.observe_children().n_items()
            == 2),
        "TrashPage should render trashed media"
    );

    let first = flow
        .first_child()
        .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
        .expect("TrashPage should render a selectable tile");
    flow.select_child(&first);
    assert!(
        trash.imp().action_bar.get().is_revealed(),
        "selecting a Trash tile should reveal the action bar"
    );
    click_button(&trash.imp().cancel_btn.get());
    assert!(
        !trash.imp().action_bar.get().is_revealed(),
        "clicking Trash cancel should clear selection and hide actions"
    );

    flow.select_child(&first);
    click_button(&trash.imp().restore_btn.get());
    assert!(
        wait_until(Duration::from_secs(2), || !trash
            .imp()
            .action_bar
            .get()
            .is_revealed()),
        "clicking Restore should clear the current Trash selection"
    );
    assert!(
        wait_until(Duration::from_secs(2), || flow.observe_children().n_items()
            > 0),
        "TrashPage should reload remaining rows after Restore"
    );

    let remaining = flow
        .first_child()
        .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
        .expect("TrashPage should still expose a tile for delete-path coverage");
    flow.select_child(&remaining);
    click_button(&trash.imp().delete_btn.get());
    assert!(
        wait_until(Duration::from_secs(2), || db::list_trashed_media(
            &fixture.pool
        )
        .map(|items| items.len() < 2)
        .unwrap_or(false)),
        "clicking Delete Permanently should remove selected trash rows from DB"
    );
}

fn build_photos_page_with_nav() -> PhotosFixture {
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let items = seed_media(&pool, tmp.path());
    albums::refresh(&pool).unwrap();

    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in &items {
        media_list.append(&glib::BoxedAnyObject::new(item.clone()));
    }

    let nav = adw::NavigationView::new();
    let page = PhotosPage::new(media_list.clone(), loader.clone());
    page.set_nav_target(&nav);
    page.set_db_pool(pool.clone());
    nav.push(&page);

    PhotosFixture {
        _tmp: tmp,
        pool,
        loader,
        media_list,
        page,
        nav,
        items,
    }
}

fn seed_media(pool: &db::DbPool, root: &std::path::Path) -> Vec<MediaItem> {
    let media_dir = root.join("photos");
    std::fs::create_dir_all(&media_dir).unwrap();
    let mut items = Vec::new();
    for name in ["one.jpg", "two.jpg"] {
        let path = media_dir.join(name);
        std::fs::write(&path, b"ux-flow-test-image").unwrap();
        let item = sample_item(0, path);
        let id = db::insert_media_item(pool, &NewMediaItem::from(&item)).unwrap();
        items.push(db::get_media_item(pool, id).unwrap());
    }
    items
}

fn click_mode_selector_cell(selector: &ModeSelector, index: usize) {
    let row = selector
        .first_child()
        .and_then(|child| child.downcast::<gtk::Box>().ok())
        .expect("ModeSelector first child should be the label row");
    let cell = nth_child(&row, index)
        .and_then(|child| child.downcast::<gtk::Box>().ok())
        .expect("ModeSelector should have a clickable cell at index");
    let gesture = cell
        .observe_controllers()
        .snapshot()
        .into_iter()
        .find_map(|controller| controller.downcast::<gtk::GestureClick>().ok())
        .expect("mode cell should own a GestureClick");

    gesture.emit_by_name::<()>("pressed", &[&1i32, &0.0f64, &0.0f64]);
}

fn first_flowbox_child(root: &gtk::Widget) -> Option<gtk::FlowBoxChild> {
    let flow = find_descendant::<gtk::FlowBox>(root)?;
    flow.first_child()
        .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
}

fn find_button_with_css(root: &gtk::Widget, css_class: &str) -> Option<gtk::Button> {
    if let Some(button) = root.downcast_ref::<gtk::Button>() {
        if button.css_classes().iter().any(|class| class == css_class) {
            return Some(button.clone());
        }
    }

    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(button) = find_button_with_css(&widget, css_class) {
            return Some(button);
        }
        child = widget.next_sibling();
    }

    None
}

fn click_button(button: &gtk::Button) {
    button.emit_by_name::<()>("clicked", &[]);
}

fn release_click_on_widget(widget: &gtk::Widget) {
    let gesture = widget
        .observe_controllers()
        .snapshot()
        .into_iter()
        .find_map(|controller| controller.downcast::<gtk::GestureClick>().ok())
        .expect("widget should own a GestureClick");
    gesture.emit_by_name::<()>("released", &[&1i32, &0.0f64, &0.0f64]);
}

fn visible_flag(w: &gtk::Widget) -> bool {
    w.property::<bool>("visible")
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    let ctx = glib::MainContext::default();
    while Instant::now() < deadline {
        while ctx.iteration(false) {}
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    while ctx.iteration(false) {}
    condition()
}

fn find_descendant<T>(root: &gtk::Widget) -> Option<T>
where
    T: glib::object::IsA<gtk::Widget> + glib::object::ObjectType,
{
    if let Some(found) = root.downcast_ref::<T>() {
        return Some(found.clone());
    }

    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = find_descendant::<T>(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }

    None
}

fn nth_child(parent: &impl IsA<gtk::Widget>, index: usize) -> Option<gtk::Widget> {
    let mut current = parent.as_ref().first_child();
    for _ in 0..index {
        current = current?.next_sibling();
    }
    current
}

fn sample_item(id: i64, path: PathBuf) -> MediaItem {
    let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
    let folder_path = path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/tmp"))
        .to_path_buf();
    MediaItem {
        id,
        uri: format!("file://{}", path.display()),
        path,
        folder_path,
        mime_type: "image/jpeg".into(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(dt),
        file_mtime: dt,
        file_size: 100,
        blake3_hash: format!("hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}
