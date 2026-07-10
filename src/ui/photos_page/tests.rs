use super::*;
use chrono::{TimeZone, Utc};
use std::path::PathBuf;

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
fn opening_viewer_temporarily_disables_initial_navigation_pop() {
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

    let viewer = nav
        .visible_page()
        .and_downcast::<ViewerPage>()
        .expect("photo activation should open viewer");
    assert!(
        !viewer.can_pop(),
        "viewer should ignore immediate second-click/back events while the open debounce is active"
    );

    let ctx = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < deadline && !viewer.can_pop() {
        ctx.iteration(true);
    }

    assert!(
        viewer.can_pop(),
        "viewer should allow normal navigation again after the open debounce"
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < deadline && !page.is_sensitive() {
        ctx.iteration(true);
    }

    assert!(
        page.is_sensitive(),
        "Photos page input should be restored after the guarded push window"
    );
}
