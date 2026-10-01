use super::super::test_support::*;
use super::super::*;
use super::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[gtk::test]
fn viewer_keyboard_action_navigates_and_closes() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_cb = events.clone();
    viewer.connect_navigation(move |delta| {
        events_for_cb.borrow_mut().push(delta);
    });

    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerNext),
        crate::ui::keyboard::KeyboardResult::Handled
    );
    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerPrevious),
        crate::ui::keyboard::KeyboardResult::Handled
    );
    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::CancelOrClose),
        crate::ui::keyboard::KeyboardResult::Handled
    );

    assert_eq!(events.borrow().as_slice(), &[1, -1, NAV_POP]);
}

#[test]
fn next_index_after_deleted_item_stays_in_bounds() {
    assert_eq!(next_index_after_deleted_item(0, 2), Some(0));
    assert_eq!(next_index_after_deleted_item(1, 2), Some(1));
    assert_eq!(next_index_after_deleted_item(2, 2), Some(1));
    assert_eq!(next_index_after_deleted_item(0, 0), None);
}

#[gtk::test]
fn find_media_index_by_id_uses_item_identity() {
    let _ = gtk::init();
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let mut first = sample_media_item();
    first.id = 10;
    let mut second = sample_media_item();
    second.id = 20;
    list.append(&glib::BoxedAnyObject::new(first));
    list.append(&glib::BoxedAnyObject::new(second));

    assert_eq!(find_media_index_by_id(&list, 20), Some(1));
    assert_eq!(find_media_index_by_id(&list, 30), None);
}

#[gtk::test]
fn current_media_item_stays_anchored_when_startup_scan_inserts_before_it() {
    init_viewer_test();
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let mut opened = sample_media_item();
    opened.id = 20;
    opened.uri = "file:///tmp/opened.jpg".into();
    opened.path = PathBuf::from("/tmp/opened.jpg");
    list.append(&glib::BoxedAnyObject::new(opened));

    let viewer = ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(20), list.clone());

    let mut inserted = sample_media_item();
    inserted.id = 10;
    inserted.uri = "file:///tmp/inserted.jpg".into();
    inserted.path = PathBuf::from("/tmp/inserted.jpg");
    list.insert(0, &glib::BoxedAnyObject::new(inserted));

    let current = viewer
        .current_media_item()
        .expect("viewer should still resolve the opened item");
    assert_eq!(current.id, 20);
    assert_eq!(viewer.current_index(), 1);
}

/// Pump the main context until `done` holds, so async neighbour prefetch
/// replies can land inside a `#[gtk::test]`.
fn pump_until(timeout: std::time::Duration, done: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let context = glib::MainContext::default();
    while std::time::Instant::now() < deadline {
        while context.iteration(false) {}
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    while context.iteration(false) {}
    done()
}

#[gtk::test]
fn a_video_at_the_query_start_dims_the_previous_arrow_without_a_first_navigation() {
    init_viewer_test();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("video-bounds.db")).unwrap();
    let media_dir = tmp.path().join("media");
    std::fs::create_dir_all(&media_dir).unwrap();
    for (id, day) in [(1i64, 23u32), (2i64, 24u32)] {
        let taken =
            chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 6, day, 12, 0, 0).unwrap();
        let path = media_dir.join(format!("{id}.mp4"));
        std::fs::write(&path, b"fake mp4").unwrap();
        crate::core::db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file://{}", path.display()),
                path: path.clone(),
                folder_path: media_dir.clone(),
                mime_type: "video/mp4".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(100),
                height: Some(100),
                video_duration_secs: Some(3.0),
                taken_at: Some(taken),
                file_mtime: taken,
                file_size: 100,
                blake3_hash: format!("hash-{id}"),
            },
        )
        .unwrap();
    }
    let items = crate::core::repository::MediaRepository::new(pool.clone())
        .items(MediaQuery::LiveAll, 0, 10)
        .unwrap();
    assert_eq!(items.len(), 2, "fixture precondition: two ordered videos");
    assert!(items.iter().all(|item| item.is_video()));

    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in &items {
        list.append(&glib::BoxedAnyObject::new(item.clone()));
    }
    let viewer = ViewerPage::new_for_query(
        MediaQuery::LiveAll,
        MediaId::from(items[0].id),
        list.clone(),
    );
    viewer.imp().pool.replace(Some(pool.clone()));
    viewer.set_thumbnail_loader(std::sync::Arc::new(
        crate::core::thumbnails::ThumbnailLoader::new(pool.clone(), tmp.path().join("thumbs")),
    ));

    let prev = viewer.imp().prev_btn.get();
    let next = viewer.imp().next_btn.get();
    viewer.show_at(0);
    assert!(
        pump_until(std::time::Duration::from_secs(3), || !prev.is_sensitive()),
        "opening a video at the query start must dim the previous arrow via prefetch, with no navigation first"
    );
    assert!(
        next.is_sensitive(),
        "a neighbour exists ahead, so the next arrow must stay enabled"
    );

    // Re-anchoring the same video (the startup-scan re-insert case) returns
    // early from show_at and must still re-resolve the bounds through the
    // shared prefetch path.
    viewer.show_at(0);
    assert!(
        pump_until(std::time::Duration::from_secs(3), || !prev.is_sensitive()),
        "the same-video re-anchor branch must also re-resolve the nav bounds"
    );
    assert!(next.is_sensitive());

    // Test-only stream cleanup, same contract as the stage tests: keep the
    // invalid source's native worker from racing finalization after return.
    if let Some(stream) = viewer.imp().video.get().media_stream() {
        viewer.stop_video_playback();
        std::mem::forget(stream);
    }
}
