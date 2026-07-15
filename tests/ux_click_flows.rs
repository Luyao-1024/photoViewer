//! UX-level click flow coverage.
//!
//! These tests exercise the same GTK signal paths a user hits: clicking the
//! Photos mode selector cells and activating a rendered thumbnail tile. They
//! intentionally avoid calling `PhotosPage` internals such as `open_viewer`.
mod common;

use chrono::{TimeZone, Utc};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::albums::Album;
use photo_viewer::core::identity::MediaId;
use photo_viewer::core::media::{MediaItem, NewMediaItem, MEDIA_SUBKIND_STANDARD};
use photo_viewer::core::thumbnails::ThumbnailLoader;
use photo_viewer::core::{albums, db};
use photo_viewer::ui::virtual_media_grid::VirtualMediaGrid;
use photo_viewer::ui::{
    album_picker, AlbumBrowserPage, AlbumDetailPage, MainWindow, ModeSelector, PhotosPage,
    SearchPage, TrashPage, ViewerPage,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
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

#[allow(dead_code)] // reusable fixture; fields are read by later sub-flows
struct AppShell {
    _app: adw::Application,
    _tmp: tempfile::TempDir,
    pool: db::DbPool,
    loader: Arc<ThumbnailLoader>,
    media_list: gtk::gio::ListStore,
    db_actor: photo_viewer::core::db_actor::DbActorHandle,
    items: Vec<MediaItem>,
    window: MainWindow,
    photos: PhotosPage,
}

/// Monotonic counter so every full-shell fixture registers a unique
/// `adw::Application` id (GApplication is single-instance per id per process).
static FULL_SHELL_SEQ: AtomicU64 = AtomicU64::new(0);

#[test]
fn ux_click_flow_suite_including_album_sidebar_multi_select_deletes_real_albums() {
    gtk::init().expect("GTK init failed");
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime for UX click flows");
    let _runtime_guard = runtime.enter();

    mode_selector_click_switches_photos_view();
    thumbnail_activation_opens_one_viewer();
    search_result_activation_opens_one_viewer_while_pending();
    photos_batch_toolbar_clicks_select_favorite_and_album();
    viewer_chrome_clicks_drive_visible_operations();
    sidebar_clicks_drive_top_level_navigation();
    album_sidebar_multi_select_deletes_real_albums();
    album_picker_clicks_album_row_and_copy_move();
    album_pages_clicks_open_album_and_viewer();
    album_browser_reorder_persists_full_album_order();
    trash_page_clicks_selection_cancel_restore_and_delete();
    full_app_shell_renders_photos_and_opens_trash_via_sidebar();
}

fn search_result_activation_opens_one_viewer_while_pending() {
    let fixture = build_photos_page_with_nav();
    let page = SearchPage::new(fixture.pool.clone(), fixture.loader.clone());
    page.set_nav_target(&fixture.nav);
    fixture.nav.push(&page);

    page.imp().search_entry.get().set_text("one");
    assert!(
        wait_until(Duration::from_secs(2), || first_flowbox_child(
            page.upcast_ref()
        )
        .is_some()),
        "SearchPage should render a result tile for the fixture query"
    );

    let first_tile = first_flowbox_child(page.upcast_ref()).expect("search result tile exists");
    let flow = first_tile
        .parent()
        .and_then(|w| w.downcast::<gtk::FlowBox>().ok())
        .expect("search result tile should belong to a FlowBox");
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);
    assert!(
        !page.is_sensitive(),
        "SearchPage should ignore pointer input while viewer push is guarded"
    );
    flow.emit_by_name::<()>("child-activated", &[&first_tile]);

    assert_eq!(
        fixture.nav.navigation_stack().n_items(),
        3,
        "rapid repeated Search result activation should push only one viewer page"
    );
    assert!(
        fixture
            .nav
            .visible_page()
            .and_downcast::<ViewerPage>()
            .is_some(),
        "search result activation should open the viewer page"
    );
}

fn mode_selector_click_switches_photos_view() {
    let fixture = build_photos_page_with_nav();
    let selector = find_descendant::<ModeSelector>(fixture.page.upcast_ref())
        .expect("PhotosPage should contain a ModeSelector");
    let stack = find_descendant::<gtk::Stack>(fixture.page.upcast_ref())
        .expect("PhotosPage should contain a GtkStack");

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
    let grid = visible_photos_grid(&fixture.page);

    let first_slot = grid
        .first_media_slot()
        .expect("Photos should expose a media slot");
    activate_virtual_grid_slot(&grid, first_slot);
    activate_virtual_grid_slot(&grid, first_slot);

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
    let grid = visible_photos_grid(&fixture.page);
    let first_id = MediaId::from(fixture.items[0].id);

    grid.select_ids(&[first_id]);

    assert!(
        fixture
            .page
            .imp()
            .add_to_album_revealer
            .get()
            .reveals_child(),
        "selecting a tile should reveal the batch add-to-album action"
    );
    assert!(
        fixture.page.imp().favorite_revealer.get().reveals_child(),
        "selecting a tile should reveal the batch favorite action"
    );
    assert!(
        fixture
            .page
            .imp()
            .delete_to_trash_revealer
            .get()
            .reveals_child(),
        "selecting a tile should reveal the batch trash action"
    );

    click_button(&fixture.page.imp().select_all_btn.get());
    assert!(
        grid.is_all_displayed_selected(),
        "clicking Select All selects every rendered tile in the current mode"
    );
    click_button(&fixture.page.imp().select_all_btn.get());
    assert!(
        !fixture.page.imp().favorite_revealer.get().reveals_child(),
        "clicking the toggled Select All button clears selection and hides batch actions"
    );

    grid.select_ids(&[first_id]);
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
    assert!(
        wait_until(Duration::from_secs(2), || !fixture
            .page
            .imp()
            .favorite_revealer
            .get()
            .reveals_child()),
        "favorite action should clear the previous selection before the next batch action"
    );

    grid.select_ids(&[first_id]);
    assert!(
        wait_until(Duration::from_secs(2), || fixture
            .page
            .imp()
            .add_to_album_btn
            .get()
            .is_visible()),
        "selecting a tile after favorite should expose the batch add-to-album action"
    );
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
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(fixture.pool.clone(), event_sender);
    viewer.set_edit_target(&fixture.nav, fixture.pool.clone());
    viewer.set_db_actor(db_actor);
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
    viewer
        .imp()
        .name_row
        .get()
        .emit_by_name::<()>("activated", &[]);
    assert!(
        gtk::prelude::WidgetExt::is_visible(&viewer.imp().name_entry.get()),
        "clicking the details name row should reveal the inline rename entry"
    );
    assert_eq!(
        viewer.imp().name_entry.get().text().as_str(),
        "one",
        "inline rename should edit only the stem by default"
    );
    viewer.imp().name_entry.get().set_text("renamed.png");
    viewer
        .imp()
        .name_entry
        .get()
        .emit_by_name::<()>("activate", &[]);
    let renamed_path = fixture._tmp.path().join("photos").join("renamed.jpg");
    assert!(
        wait_until(Duration::from_secs(2), || renamed_path.exists()
            && db::get_media_item(&fixture.pool, fixture.items[0].id)
                .map(|item| item.display_name() == "renamed.jpg")
                .unwrap_or(false)),
        "inline rename should preserve the original extension and update the DB item"
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
    assert!(
        !viewer.imp().rotate_left_btn.get().is_visible()
            && !viewer.imp().rotate_right_btn.get().is_visible(),
        "viewer rotate buttons should hide once the image is enlarged"
    );
    click_button(&viewer.imp().zoom_reset_btn.get());
    assert_eq!(
        viewer.imp().zoom_scale.get(),
        initial_zoom,
        "clicking zoom reset should restore the initial zoom"
    );
    click_button(&viewer.imp().rotate_right_btn.get());
    assert_eq!(
        viewer.imp().viewer_rotation_degrees.get(),
        90,
        "clicking rotate-right should rotate the current viewer image clockwise"
    );
    click_button(&viewer.imp().zoom_in_btn.get());
    assert!(
        viewer.imp().zoom_scale.get() > initial_zoom,
        "clicking zoom-in should still enlarge after a viewer-only rotation"
    );
    click_button(&viewer.imp().rotate_left_btn.get());
    assert_eq!(
        viewer.imp().viewer_rotation_degrees.get(),
        0,
        "clicking rotate-left should rotate the current viewer image counter-clockwise"
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
        .application_id("io.github.luyao_1024.photoviewer.UxClickFlows")
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
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    window.set_db_actor(photo_viewer::core::start_db_actor(
        fixture.pool.clone(),
        event_sender,
    ));
    albums::refresh(&fixture.pool).unwrap();
    window.populate_album_rows();
    let nav = window.nav_view();
    let root_page = PhotosPage::new(fixture.media_list.clone(), fixture.loader.clone());
    root_page.set_nav_target(&nav);
    root_page.set_db_pool(fixture.pool.clone());
    window.show_photos_browsing_page(&root_page);
    window.connect_sidebar(&nav);

    let sidebar = window.imp().sidebar_list.get();
    let trash_list = window.imp().trash_list.get();
    assert_eq!(
        sidebar.observe_children().n_items(),
        2,
        "top sidebar list should contain Photos and Albums header",
    );
    assert_eq!(
        trash_list.observe_children().n_items(),
        1,
        "trash list should contain one stable Trash row",
    );
    let header = sidebar.row_at_index(1).expect("Albums header row exists");
    assert!(visible_flag(window.imp().album_scroll.get().upcast_ref()));
    release_click_on_widget(header.upcast_ref());
    assert!(
        !visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "clicking the Albums header should collapse the album scroll region"
    );
    release_click_on_widget(header.upcast_ref());
    assert!(
        visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "clicking the Albums header again should expand the album scroll region"
    );

    let trash_row = trash_list.row_at_index(0).expect("Trash row exists");
    trash_list.select_row(Some(&trash_row));
    while glib::MainContext::default().iteration(false) {}
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "selecting the Trash sidebar row should show TrashPage"
    );

    let photos_row = sidebar.row_at_index(0).expect("Photos row exists");
    sidebar.select_row(Some(&photos_row));
    assert!(
        window.browsing_stack().visible_child_name().as_deref() == Some("photos"),
        "selecting the Photos sidebar row should return to PhotosPage"
    );

    click_button(&window.imp().settings_button.get());
    assert!(
        nav.has_css_class("settings-background-blur"),
        "clicking the settings button should present settings chrome over the content nav"
    );
}

fn album_sidebar_multi_select_deletes_real_albums() {
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.AlbumMultiSelect")
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
    seed_extra_album(&fixture);
    albums::refresh(&fixture.pool).unwrap();
    window.populate_album_rows();
    let nav = window.nav_view();
    window.connect_sidebar(&nav);

    window.enter_album_selection_mode();
    assert_eq!(
        window.imp().album_list.get().selection_mode(),
        gtk::SelectionMode::Multiple,
        "album selection mode should switch the album list to multiple selection",
    );
    assert!(
        window.imp().album_selection_bar.get().is_revealed(),
        "album selection mode should reveal the batch action bar",
    );
    assert!(
        !window.imp().album_selection_delete_btn.get().is_sensitive(),
        "delete selected should stay disabled until real albums are selected",
    );

    let real_rows: Vec<gtk::ListBoxRow> = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .enumerate()
        .filter(|(_, album)| !album.is_virtual)
        .take(2)
        .filter_map(|(idx, _)| window.imp().album_list.get().row_at_index(idx as i32))
        .collect();
    assert_eq!(
        real_rows.len(),
        2,
        "fixture should include two real album rows"
    );
    for row in &real_rows {
        window.imp().album_list.get().select_row(Some(row));
    }
    assert_eq!(window.selected_album_delete_count(), 2);
    assert!(
        window.imp().album_selection_delete_btn.get().is_sensitive(),
        "selecting real albums should enable the delete selected action",
    );
}

fn album_picker_clicks_album_row_and_copy_move() {
    let fixture = build_photos_page_with_nav();
    let window = gtk::Window::new();
    window.set_child(Some(&fixture.nav));
    let original_count = db::list_all_media(&fixture.pool).unwrap().len();
    let (events, _receiver) = photo_viewer::core::events::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(fixture.pool.clone(), events);

    photo_viewer::ui::AlbumPickerDialog::present(
        &fixture.nav,
        fixture.pool.clone(),
        db_actor.clone(),
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
        db_actor,
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
    drop(window);
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

    let detail_grid = find_descendant::<VirtualMediaGrid>(detail.upcast_ref())
        .expect("Album detail should use VirtualMediaGrid");
    activate_virtual_grid_slot(&detail_grid, 0);
    assert!(
        !detail.is_sensitive(),
        "AlbumDetailPage should ignore pointer input while viewer push is guarded"
    );
    activate_virtual_grid_slot(&detail_grid, 0);
    assert!(
        fixture
            .nav
            .visible_page()
            .and_downcast::<ViewerPage>()
            .is_some(),
        "activating an AlbumDetail tile should open the viewer"
    );
    assert_eq!(
        fixture.nav.navigation_stack().n_items(),
        3,
        "rapid repeated AlbumDetail tile activation should push only one viewer page"
    );
}

fn album_browser_reorder_persists_full_album_order() {
    let fixture = build_photos_page_with_nav();
    let browser = AlbumBrowserPage::new(
        fixture.pool.clone(),
        fixture.loader.clone(),
        Rc::new(|_| {}),
    );
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    browser.set_db_actor(photo_viewer::core::start_db_actor(
        fixture.pool.clone(),
        event_sender,
    ));

    let before: Vec<String> = albums::list_with_favorites(&fixture.pool)
        .unwrap()
        .into_iter()
        .map(|album| album.folder_path.to_string_lossy().into_owned())
        .collect();
    assert!(
        before.len() >= 4,
        "fixture should include virtual albums plus at least one folder album"
    );

    let source = before[3].clone();
    let target = before[0].clone();
    browser.reorder_album(&source, &target, false);

    let after: Vec<String> = albums::list_with_favorites(&fixture.pool)
        .unwrap()
        .into_iter()
        .map(|album| album.folder_path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        after[0], source,
        "dragging an album browser card above the first card should persist it first"
    );
    assert_eq!(
        browser.album_folder_paths()[0],
        source,
        "AlbumBrowserPage should refresh into the persisted order"
    );
}

fn full_app_shell_renders_photos_and_opens_trash_via_sidebar() {
    let shell = build_full_app_shell();
    let nav = shell.window.nav_view();
    assert!(
        nav.visible_page().is_some(),
        "full app shell should have a visible browsing root"
    );
    assert_eq!(
        shell
            .window
            .imp()
            .trash_list
            .get()
            .observe_children()
            .n_items(),
        1,
        "sidebar should contain one Trash row"
    );
    let trash = open_trash_via_sidebar(&shell.window);
    assert!(
        nav.visible_page().and_downcast::<TrashPage>().is_some(),
        "selecting the Trash sidebar row should show the TrashPage"
    );
    let _ = trash;
}

fn trash_page_clicks_selection_cancel_restore_and_delete() {
    let fixture = build_photos_page_with_nav();
    common::db::mark_trashed(&fixture.pool, fixture.items[0].id).unwrap();
    common::db::mark_trashed(&fixture.pool, fixture.items[1].id).unwrap();
    let shared = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let trash =
        TrashPage::with_media_list(fixture.pool.clone(), fixture.loader.clone(), shared.clone());
    let grid = trash.imp().grid.borrow().as_ref().cloned().unwrap();
    assert!(
        wait_until(Duration::from_secs(2), || grid.logical_media_count() == 2),
        "TrashPage should render trashed media"
    );

    assert!(
        wait_until(Duration::from_secs(2), || grid
            .first_ready_media_slot()
            .is_some()),
        "Trash should load an interactive media slot"
    );
    let first_slot = grid
        .first_ready_media_slot()
        .expect("Trash should retain a ready media slot");
    activate_virtual_grid_slot(&grid, first_slot);
    assert!(
        trash.imp().action_bar.get().is_revealed(),
        "selecting a Trash tile should reveal the action bar"
    );
    click_button(&trash.imp().cancel_btn.get());
    assert!(
        !trash.imp().action_bar.get().is_revealed(),
        "clicking Trash cancel should clear selection and hide actions"
    );

    activate_virtual_grid_slot(&grid, first_slot);
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
        wait_until(Duration::from_secs(2), || grid.logical_media_count() > 0),
        "TrashPage should reload remaining rows after Restore"
    );

    assert!(
        wait_until(Duration::from_secs(2), || grid
            .first_ready_media_slot()
            .is_some()),
        "Trash should load the remaining media slot"
    );
    let remaining_slot = grid
        .first_ready_media_slot()
        .expect("Trash should retain a ready media slot");
    activate_virtual_grid_slot(&grid, remaining_slot);
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

fn build_full_app_shell() -> AppShell {
    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("shell.db")).unwrap();
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
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(pool.clone(), event_sender);

    let seq = FULL_SHELL_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
    let app = adw::Application::builder()
        .application_id(format!("io.github.luyao_1024.photoviewer.UxFullShell{seq}"))
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    photo_viewer::ui::grid_css::install();

    let window = MainWindow::new(&app);
    window.populate_sidebar();
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());
    window.set_db_actor(db_actor.clone());
    window.populate_album_rows();

    let nav = window.nav_view();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    photos.set_db_actor(db_actor.clone());
    window.show_photos_browsing_page(&photos);
    window.connect_sidebar(&nav);

    AppShell {
        _app: app,
        _tmp: tmp,
        pool,
        loader,
        media_list,
        db_actor,
        items,
        window,
        photos,
    }
}

/// Open the Trash page the way a user does: select the Trash sidebar row.
/// Returns the `TrashPage` that `show_trash_page` built from the window's
/// real pool/loader/media_list/db_actor.
fn open_trash_via_sidebar(window: &MainWindow) -> TrashPage {
    let trash_list = window.imp().trash_list.get();
    let trash_row = trash_list.row_at_index(0).expect("Trash row exists");
    trash_list.select_row(Some(&trash_row));
    let nav = window.nav_view();
    assert!(
        wait_until(Duration::from_secs(2), || {
            nav.visible_page().and_downcast::<TrashPage>().is_some()
        }),
        "selecting the Trash sidebar row should show TrashPage"
    );
    nav.visible_page()
        .and_downcast::<TrashPage>()
        .expect("TrashPage is visible")
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
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(pool.clone(), event_sender);
    page.set_nav_target(&nav);
    page.set_db_pool(pool.clone());
    page.set_db_actor(db_actor);
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
        let id = common::db::insert_media_item(pool, &NewMediaItem::from(&item)).unwrap();
        items.push(db::get_media_item(pool, id).unwrap());
    }
    items
}

fn seed_extra_album(fixture: &PhotosFixture) {
    let album_dir = fixture._tmp.path().join("second-album");
    std::fs::create_dir_all(&album_dir).unwrap();
    let album_path = album_dir.join("three.jpg");
    std::fs::write(&album_path, b"ux-flow-second-album").unwrap();
    let item = sample_item(200, album_path);
    common::db::insert_media_item(&fixture.pool, &NewMediaItem::from(&item)).unwrap();
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

fn visible_photos_grid(page: &PhotosPage) -> VirtualMediaGrid {
    let stack = find_descendant::<gtk::Stack>(page.upcast_ref())
        .expect("PhotosPage should contain a GtkStack");
    stack
        .visible_child()
        .and_downcast::<VirtualMediaGrid>()
        .expect("the active Photos mode should use VirtualMediaGrid")
}

fn activate_virtual_grid_slot(grid: &VirtualMediaGrid, position: u32) {
    let view = find_descendant::<gtk::GridView>(grid.upcast_ref())
        .expect("VirtualMediaGrid should contain its GtkGridView");
    view.emit_by_name::<()>("activate", &[&position]);
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
