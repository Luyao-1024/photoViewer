use super::super::test_support::*;
use super::super::*;
use super::*;

use std::path::PathBuf;

use std::time::Duration;

#[test]
fn video_stage_click_toggles_above_builtin_controls() {
    assert!(should_toggle_video_from_stage_click(240.0, 600.0));
}

#[test]
fn video_stage_click_leaves_builtin_controls_alone() {
    assert!(!should_toggle_video_from_stage_click(570.0, 600.0));
    assert!(!should_toggle_video_from_stage_click(-1.0, 600.0));
    assert!(!should_toggle_video_from_stage_click(0.0, 40.0));
}

#[test]
fn viewer_preview_uses_medium_thumbnail() {
    assert_eq!(viewer_preview_thumbnail_size(), ThumbnailSize::Medium);
}

#[test]
fn preview_thumbnail_yields_once_original_has_landed() {
    // The original full-resolution texture is authoritative for its token;
    // a late preview thumbnail must not overwrite it. This is the race that
    // left PNG screenshots stuck on the thumbnail: non-JPEG thumbnails decode
    // the full source before downscaling and can land after the original.
    assert!(
        original_has_landed(7, 7),
        "original for the current token must suppress a late thumbnail"
    );
    assert!(
        !original_has_landed(0, 7),
        "thumbnail may paint while the original is still decoding"
    );
    assert!(
        !original_has_landed(5, 7),
        "a previous item's original must not block the current item's thumbnail"
    );
}

#[test]
fn animated_image_adds_half_second_pause_before_looping() {
    let texture = test_texture();
    let frames = vec![
        AnimatedImageFrame {
            texture: texture.clone(),
            delay: Duration::from_millis(80),
        },
        AnimatedImageFrame {
            texture,
            delay: Duration::from_millis(120),
        },
    ];

    assert_eq!(
        animated_image_next_delay(&frames, 0),
        Duration::from_millis(80)
    );
    assert_eq!(
        animated_image_next_delay(&frames, 1),
        Duration::from_millis(120 + ANIMATED_IMAGE_LOOP_PAUSE_MS)
    );
}

#[test]
fn viewer_plays_misnamed_gif_even_when_db_row_is_stale() {
    let mut item = sample_media_item();
    item.path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/media/gif_with_jpg_extension.jpg");
    item.uri = format!("file://{}", item.path.display());
    item.mime_type = "image/jpeg".into();
    item.media_attributes = "{}".into();

    assert!(
        should_play_animated_image(&item),
        "viewer should probe the current file header so unchanged stale DB rows still animate"
    );
}

#[gtk::test]
fn viewer_starts_frame_timer_for_animated_gif() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/media/gif_with_jpg_extension.jpg");

    // GIF frames now decode off the main thread via gio::spawn_blocking.
    // start_animated_image_playback returns true immediately (decode is
    // in-flight). Verify the decode dispatches; the async completion path
    // is covered by the integration flow through show_at.
    assert!(viewer.start_animated_image_playback(&path, 1));

    viewer.stop_animated_image_playback();
}

#[test]
fn animated_image_budget_rejects_excessive_frames_or_pixels() {
    assert!(!animated_image_budget_exceeded(300, 64 * 1024 * 1024));
    assert!(animated_image_budget_exceeded(301, 1));
    assert!(animated_image_budget_exceeded(1, 64 * 1024 * 1024 + 1));
}

#[test]
fn video_stage_reveals_only_for_current_prepared_stream() {
    assert!(
        should_reveal_prepared_video_stage(7, 7, true),
        "current prepared streams should switch from thumbnail preview to video"
    );
    assert!(
        !should_reveal_prepared_video_stage(7, 8, true),
        "stale streams from previous navigation must not reveal the video layer"
    );
    assert!(
        !should_reveal_prepared_video_stage(7, 7, false),
        "unprepared streams should keep the thumbnail preview visible"
    );
}

#[gtk::test]
fn video_audio_preferences_are_applied_to_media_stream() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.mp4");
    std::fs::write(&path, b"fake mp4").unwrap();
    let stream = gtk::MediaFile::for_filename(&path);

    apply_video_audio_preferences_to_stream(&stream, true, 0.42);

    assert!(stream.is_muted(), "video should respect default muted pref");
    assert_eq!(stream.volume(), 0.42);
}

#[gtk::test]
fn stop_video_playback_retires_stream_until_next_idle() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    let stream = gtk::MediaFile::for_filename("/tmp/photo-viewer-test.mp4");
    viewer.imp().video.get().set_media_stream(Some(&stream));
    assert!(
        viewer.imp().video.get().media_stream().is_some(),
        "precondition: a stream is attached"
    );

    viewer.stop_video_playback();

    // Detached from the widget at once, but NOT finalized: the retire slot
    // holds the only remaining reference so GstPlay can finish its terminal
    // state-changed signal against a live object. Releasing it synchronously
    // here is exactly what crashed the GstPlay thread.
    assert!(
        viewer.imp().video.get().media_stream().is_none(),
        "stream must detach from the video widget right away"
    );
    assert!(
        viewer.imp().retired_video_stream.borrow().is_some(),
        "stream must be retained past teardown to avoid the GstPlay-thread UAF"
    );

    // Pumping the default main context fires the idle callback that drops it.
    while glib::MainContext::default().iteration(false) {}
    assert!(
        viewer.imp().retired_video_stream.borrow().is_none(),
        "retired stream is released after the idle cycle"
    );
}

#[gtk::test]
fn show_at_keeps_video_stream_when_startup_scan_re_anchors_same_item() {
    init_viewer_test();
    let dir = tempfile::tempdir().unwrap();
    let video_path = dir.path().join("clip.mp4");
    std::fs::write(&video_path, b"fake mp4").unwrap();

    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let mut video = sample_media_item();
    video.id = 20;
    video.mime_type = "video/mp4".into();
    video.uri = format!("file://{}", video_path.display());
    video.path = video_path;
    media_list.append(&glib::BoxedAnyObject::new(video));

    let viewer =
        ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(20), media_list.clone());

    viewer.show_at(0);
    assert!(
        viewer.imp().video.get().media_stream().is_some(),
        "first show_at for a video should attach a stream"
    );
    assert!(
        viewer.imp().retired_video_stream.borrow().is_none(),
        "precondition: nothing retired on first show"
    );

    // Startup scan inserts a media row before the current one. The viewer
    // re-resolves the render index to 1, but the media id is unchanged, so
    // show_at must reuse the live stream instead of rebuilding it.
    let mut inserted = sample_media_item();
    inserted.id = 10;
    media_list.insert(0, &glib::BoxedAnyObject::new(inserted));
    viewer.show_at(1);

    // If show_video_stage had rebuilt, its first call (stop_video_playback)
    // would have moved the previous stream into retired_video_stream, which
    // is only released on a later idle this test never pumps. An empty slot
    // plus a still-attached stream proves the live stream was reused.
    assert!(
        viewer.imp().video.get().media_stream().is_some(),
        "the live video stream should still be attached"
    );
    assert!(
        viewer.imp().retired_video_stream.borrow().is_none(),
        "same-id re-show must not tear down and rebuild the live GstPlay stream"
    );
}

#[gtk::test]
fn show_at_rebuilds_video_stream_after_optimistic_navigation_to_different_video() {
    init_viewer_test();
    let dir = tempfile::tempdir().unwrap();
    let first_path = dir.path().join("first.mp4");
    let second_path = dir.path().join("second.mp4");
    std::fs::write(&first_path, b"fake first mp4").unwrap();
    std::fs::write(&second_path, b"fake second mp4").unwrap();

    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let mut first = sample_media_item();
    first.id = 20;
    first.mime_type = "video/mp4".into();
    first.uri = format!("file://{}", first_path.display());
    first.path = first_path;
    media_list.append(&glib::BoxedAnyObject::new(first));

    let mut second = sample_media_item();
    second.id = 30;
    second.mime_type = "video/mp4".into();
    second.uri = format!("file://{}", second_path.display());
    second.path = second_path;
    media_list.append(&glib::BoxedAnyObject::new(second));

    let viewer =
        ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(20), media_list.clone());
    viewer.show_at(0);
    let first_stream = viewer
        .imp()
        .video
        .get()
        .media_stream()
        .expect("first video should attach a stream");

    // navigate_by_delta advances current_media_id optimistically before
    // the deferred show_at paints. show_at must still rebuild the video
    // stage for the target video instead of treating that optimistic id as
    // proof that the attached stream already belongs to the target.
    viewer.imp().current_media_id.set(30);
    viewer.show_at(1);

    let second_stream = viewer
        .imp()
        .video
        .get()
        .media_stream()
        .expect("second video should attach a stream");
    assert!(
        first_stream.as_ptr() != second_stream.as_ptr(),
        "switching to a different video must replace the attached stream"
    );
    assert!(
        viewer.imp().retired_video_stream.borrow().is_some(),
        "old video stream should be retired when navigating to a different video"
    );
}
