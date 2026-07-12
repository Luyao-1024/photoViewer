use super::*;
use chrono::{TimeZone, Utc};
use std::path::PathBuf;

/// Window for polling the viewer-open debounce's *recovery*. `open_viewer`
/// arms a one-shot `VIEWER_OPEN_POP_GUARD_MS` (~350 ms) timeout that clears
/// the `viewer_open_pending` re-entry flag and restores source-page input.
/// (Pop is never disabled on open — immediate back/Escape/swipe is intentional
/// user input and must work right away.) These tests pump the main loop until
/// that recovery lands. The deadline must clear 350 ms with generous margin:
/// GitHub Actions runners dispatch the one-shot late enough that a 600 ms
/// window flaked on CI (the recovery landed just past it). 3 s leaves ample
/// headroom and still fails fast if recovery is genuinely broken — the happy
/// path exits the loop as soon as the ~350 ms timeout fires.
const OPEN_GUARD_RECOVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

fn sample_item(id: i64, name: &str) -> crate::core::media::MediaItem {
    let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
    crate::core::media::MediaItem {
        id,
        uri: format!("file:///tmp/{name}"),
        path: PathBuf::from(format!("/tmp/{name}")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/png".into(),
        media_subkind: "standard".into(),
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

fn sample_new_item(name: &str, ts: i64) -> crate::core::media::NewMediaItem {
    let path = PathBuf::from(format!("/tmp/{name}.jpg"));
    crate::core::media::NewMediaItem {
        uri: format!("file:///tmp/{name}.jpg"),
        path: path.clone(),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc.timestamp_opt(ts, 0).unwrap(),
        file_size: 100,
        blake3_hash: format!("hash-{name}"),
    }
}

#[gtk::test]
fn select_all_is_capped_at_two_thousand_not_current_virtual_window() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let items = (0..2_500)
        .map(|idx| sample_new_item(&format!("photo-{idx:03}"), 10_000 - idx))
        .collect::<Vec<_>>();
    crate::core::db::upsert_media_items_batch(&pool, &items).unwrap();

    let repo = crate::core::repository::MediaRepository::new(pool.clone());
    let first_window = repo.items(MediaQuery::LiveAll, 0, 500).unwrap();
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in first_window {
        media_list.append(&glib::BoxedAnyObject::new(item));
    }

    let page = PhotosPage::new(media_list, loader);
    page.set_db_pool(pool);
    page.select_all_in_current_mode();

    assert_eq!(
            page.selected_count_for_tests(),
            2_000,
            "Photos select-all should select the first 2000 live media ids, not only the loaded 500-item window"
        );
}

#[gtk::test]
fn repeated_photo_activation_pushes_only_one_viewer_while_pending() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));

    let nav = adw::NavigationView::new();
    let page = PhotosPage::new(media_list, loader);
    page.set_nav_target(&nav);
    nav.push(&page);

    page.open_viewer(MediaId::from(1));
    page.open_viewer(MediaId::from(1));

    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "back-to-back photo activations must not stack duplicate viewer pages"
    );
    assert!(
        nav.visible_page().and_downcast::<ViewerPage>().is_some(),
        "the single pushed page should be a ViewerPage"
    );
}

#[gtk::test]
fn opening_viewer_temporarily_disables_photos_page_input() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));

    let nav = adw::NavigationView::new();
    let page = PhotosPage::new(media_list, loader);
    page.set_nav_target(&nav);
    nav.push(&page);

    page.open_viewer(MediaId::from(1));

    assert!(
        !page.is_sensitive(),
        "the source Photos page should ignore pointer input while viewer push is guarded"
    );

    let ctx = glib::MainContext::default();
    let deadline = std::time::Instant::now() + OPEN_GUARD_RECOVERY_DEADLINE;
    while std::time::Instant::now() < deadline && !page.is_sensitive() {
        ctx.iteration(true);
    }

    assert!(
        page.is_sensitive(),
        "Photos page input should be restored after the guarded push window"
    );
}

#[gtk::test]
fn opening_viewer_pushes_through_browsing_root_page_wrapper() {
    // Regression: the browsing-stack refactor moved PhotosPage inside a
    // `browsing_root_page` wrapper on the host `AdwNavigationView`, so
    // `nav.visible_page()` is the wrapper rather than the PhotosPage.
    // PhotosPage's open_viewer must not early-return when the visible page is
    // the wrapper; it must check whether PhotosPage itself is the visible
    // browsing child (e.g. via `is_visible()`).
    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.PhotosBrowsingRootViewer")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    crate::ui::grid_css::install();
    let window = crate::ui::MainWindow::new(&app);
    let nav = window.nav_view();

    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("photos-root.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());

    let photos = PhotosPage::new(media_list, loader);
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool);
    window.show_photos_browsing_page(&photos);

    // Production sets browsing_root_page as the visible NavigationPage; the
    // PhotosPage lives inside the inner browsing_stack.
    assert_ne!(
        nav.visible_page().as_ref(),
        Some(photos.upcast_ref()),
        "sanity check: PhotosPage must NOT be the visible NavigationPage once \
         wrapped in browsing_root_page — this is the condition the production \
         viewer-open guard must tolerate"
    );

    // Realize the widget tree so `WidgetExt::is_visible()` reflects the same
    // mapped state as the running app; GTK only marks descendants as visible
    // once the toplevel has been shown.
    window.present();
    while glib::MainContext::default().iteration(false) {}

    photos.open_viewer(MediaId::from(1));

    assert_eq!(
        nav.navigation_stack().n_items(),
        2,
        "clicking a photo on the Photos page must push a ViewerPage even when \
         the host nav stack holds browsing_root_page rather than PhotosPage directly"
    );
    assert!(
        nav.visible_page().and_downcast::<ViewerPage>().is_some(),
        "viewer should be the top page after activating a Photos thumbnail"
    );
}

#[gtk::test]
fn arm_scroll_date_hide_does_not_panic_on_a_fired_source() {
    // Regression: the hide timer stored its one-shot SourceId, and after the
    // timer fired (GLib auto-destroys it) the next scroll re-armed and called
    // `SourceId::remove` on the stale id, which panics:
    //   "Source ID N was not found when attempting to remove it"
    // arm_scroll_date_hide must tolerate a fired/stale stored id.
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("hide-timer.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let page = PhotosPage::new(media_list, loader);

    // Create a one-shot source and pump the main loop until it fires, so we
    // hold a SourceId whose source GLib has already destroyed.
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_cb = fired.clone();
    let stale_id = glib::timeout_add_local_once(std::time::Duration::from_millis(1), move || {
        fired_cb.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let ctx = glib::MainContext::default();
    while !fired.load(std::sync::atomic::Ordering::SeqCst) {
        ctx.iteration(true);
    }
    assert!(
        glib::MainContext::default()
            .find_source_by_id(&stale_id)
            .is_none(),
        "sanity: the one-shot source should be gone after firing"
    );

    // Inject the stale id as if it were the pending hide timer, then re-arm.
    // The old code panicked here; the fixed code skips the stale id.
    *page.imp().scroll_date_hide_timer.borrow_mut() = Some(stale_id);
    page.arm_scroll_date_hide();

    // Clean up the real 700ms timer we just armed so it can't fire later.
    // Bind out of the `if let` so the `RefMut` (and its borrow of `page`)
    // drops at the semicolon, not at the end of the `if let` block.
    let armed = page.imp().scroll_date_hide_timer.borrow_mut().take();
    if let Some(armed) = armed {
        if glib::MainContext::default()
            .find_source_by_id(&armed)
            .is_some()
        {
            armed.remove();
        }
    }
}
