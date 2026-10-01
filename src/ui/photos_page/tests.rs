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
fn photos_overview_requires_an_extra_pull_at_the_grid_top() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("photos");
    std::fs::create_dir(&local).unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("overview.db")).unwrap();
    let mut video = sample_new_item("clip", 8_000);
    video.mime_type = "video/mp4".into();
    video.video_duration_secs = Some(12.0);
    crate::core::db::upsert_media_items_batch(
        &pool,
        &[
            sample_new_item("photo-one", 10_000),
            sample_new_item("photo-two", 9_000),
            video,
        ],
    )
    .unwrap();
    let store = crate::core::sync::SyncStore::new(pool.clone());
    let job = store
        .create_job(&crate::core::sync::NewSyncJob {
            endpoint: "https://dav.example.test/root/".into(),
            username: "alice".into(),
            credential_ref: "overview-credential".into(),
            local_root: local,
            remote_root: "PhotoViewer".into(),
            direction: crate::core::sync::SyncDirection::Bidirectional,
            upload_scope: crate::core::sync::UploadScope::SelectedAlbums,
            upload_albums: Vec::new(),
        })
        .unwrap();
    store.set_job_paused(job.id, true).unwrap();

    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "visible.png")));
    let page = PhotosPage::new(media_list, loader);
    page.set_db_pool(pool);

    assert!(
        !page.imp().overview_revealer.get().reveals_child(),
        "loading Photos at the first image should not reveal the library overview"
    );
    page.handle_overview_scroll_intent(GroupBy::Day, -1.0);
    assert!(
        page.imp().overview_revealer.get().reveals_child(),
        "an additional upward scroll at the top should reveal the overview"
    );

    let context = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && page.imp().overview_count_label.get().label() == tr("photos.overview.loading").as_str()
    {
        context.iteration(true);
    }

    assert_eq!(
        page.imp().overview_count_label.get().label(),
        trf(
            "photos.overview.counts",
            &[("photos", "2"), ("videos", "1")]
        )
    );
    if crate::core::prefs::webdav_sync_enabled() {
        assert_eq!(
            page.imp().overview_sync_label.get().label(),
            sync_overview_text(
                SyncOverview {
                    status: SyncOverviewStatus::Paused,
                    job_count: 1,
                    synced_items: 0,
                    conflict_images: 0,
                },
                None,
            )
        );
        assert!(page.imp().overview_sync_row.get().is_visible());
    } else {
        assert!(page.imp().overview_sync_label.get().label().is_empty());
        assert!(!page.imp().overview_sync_row.get().is_visible());
    }

    page.handle_overview_scroll_intent(GroupBy::Day, 1.0);
    assert!(
        !page.imp().overview_revealer.get().reveals_child(),
        "scrolling down into the media grid should hide the overview"
    );
}

#[gtk::test]
fn running_sync_uses_a_rotating_indicator_and_stops_for_static_states() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("sync-spinner.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let page = PhotosPage::new(gtk::gio::ListStore::new::<glib::BoxedAnyObject>(), loader);

    page.apply_overview_snapshot(PhotosOverviewSnapshot {
        photos: 2,
        videos: 1,
        sync: SyncOverview {
            status: SyncOverviewStatus::Running,
            job_count: 1,
            synced_items: 0,
            conflict_images: 0,
        },
        sync_progress: None,
    });

    let imp = page.imp();
    assert!(
        imp.overview_sync_running.get(),
        "the Running sync status should animate the slow spinner"
    );
    assert!(
        imp.overview_sync_spinner.get().is_visible() && !imp.overview_sync_icon.get().is_visible(),
        "the visible spinner should replace the static icon while synchronization is running"
    );

    page.apply_overview_sync_icon(SyncOverviewStatus::Completed);
    assert!(
        !imp.overview_sync_running.get(),
        "the spinner should stop once synchronization is no longer running"
    );
    assert!(
        !imp.overview_sync_spinner.get().is_visible() && imp.overview_sync_icon.get().is_visible(),
        "completed synchronization should restore the static status icon"
    );
}

#[gtk::test]
fn globally_disabled_sync_has_no_home_overview_hint() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("sync-disabled.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let page = PhotosPage::new(gtk::gio::ListStore::new::<glib::BoxedAnyObject>(), loader);

    page.apply_overview_snapshot(PhotosOverviewSnapshot {
        photos: 2,
        videos: 1,
        sync: SyncOverview {
            status: SyncOverviewStatus::Disabled,
            job_count: 1,
            synced_items: 8,
            conflict_images: 1,
        },
        sync_progress: None,
    });

    assert!(!page.imp().overview_sync_row.get().is_visible());
    assert!(page.imp().overview_sync_label.get().label().is_empty());
    assert!(!page.imp().overview_sync_running.get());
}

/// The three reasons an empty Photos grid can be empty have to stay separate on
/// screen: indexing, a failed scan, and a genuinely empty library each need a
/// different next step. `ScanPhase` is the only signal that distinguishes them,
/// so the stack routing is asserted directly.
#[gtk::test]
fn placeholders_separate_indexing_scan_failure_and_an_empty_library() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("scan-phase.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let page = PhotosPage::new(media_list.clone(), loader);
    page.set_db_pool(pool);
    let stack = page.imp().view_stack.get();

    // The default is the grid, so an unmeasured stack never falls back to a
    // "scanning" lie when the scan already finished before the page opened.
    page.set_scan_phase(false, None);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some(PLACEHOLDER_EMPTY),
        "an empty library with no scan error must offer setup help"
    );

    page.set_scan_phase(true, None);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some(PLACEHOLDER_SCANNING),
        "the first scan must read as indexing, not as an empty library"
    );

    page.set_scan_phase(
        false,
        Some("cannot read /pictures: permission denied".into()),
    );
    let name = stack.visible_child_name().map(|name| name.to_string());
    assert_eq!(
        name.as_deref(),
        Some(PLACEHOLDER_SCAN_ERROR),
        "a failed scan must not stay silent as an empty library"
    );
    let description = {
        let binding = page.imp().placeholders.borrow();
        let placeholders = binding.as_ref().expect("placeholders are built in new()");
        placeholders.scan_error.description().unwrap_or_default()
    };
    assert!(
        description.contains("permission denied"),
        "the scan failure page must surface the real reason, got: {description}"
    );

    // A blank reason is not a failure worth reporting.
    page.set_scan_phase(false, Some("   ".into()));
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some(PLACEHOLDER_EMPTY),
        "an empty error string must fall back to the empty-library state"
    );

    // Tiles always win over a placeholder, including mid-scan.
    page.set_scan_phase(true, None);
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "arriving.png")));
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some("day"),
        "the first thumbnail is better feedback than a spinner"
    );
}

#[test]
fn scan_failure_text_names_the_reason_when_there_is_one() {
    let generic = empty_states::scan_error_text(None);
    let specific = empty_states::scan_error_text(Some("disk is full"));
    assert!(
        !generic.contains("disk is full"),
        "the generic text must not carry a reason: {generic}"
    );
    assert!(
        specific.contains("disk is full"),
        "the reason must reach the user, got: {specific}"
    );
    assert!(
        !specific.contains('{'),
        "the reason must be substituted, not left as a template: {specific}"
    );
    // Whitespace-only reasons read as a broken sentence.
    assert_eq!(
        empty_states::scan_error_text(Some("   ")),
        generic,
        "a blank reason must fall back to the generic text"
    );
}

#[test]
fn completed_sync_overview_includes_items_and_optional_image_conflicts() {
    let overview = SyncOverview {
        status: SyncOverviewStatus::Completed,
        job_count: 1,
        synced_items: 103,
        conflict_images: 0,
    };
    assert!(sync_overview_text(overview, None).contains("103"));
    let with_conflicts = SyncOverview {
        conflict_images: 2,
        ..overview
    };
    let text = sync_overview_text(with_conflicts, None);
    assert!(text.contains("103"));
    assert!(text.contains('2'));
    assert_ne!(sync_overview_text(overview, None), text);
}

#[test]
fn running_sync_overview_shows_live_upload_and_download_counts() {
    let running = SyncOverview {
        status: SyncOverviewStatus::Running,
        job_count: 1,
        synced_items: 0,
        conflict_images: 0,
    };
    assert_eq!(
        sync_overview_text(running, None),
        tr("photos.overview.sync.running"),
        "before the run reports progress the generic running label applies"
    );
    assert_eq!(
        sync_overview_text(
            running,
            Some(SyncLiveProgress {
                phase: SyncLivePhase::Preparing,
                downloaded: 0,
                download_total: 0,
                uploaded: 0,
                upload_total: 0,
                transfer_active: false,
                current_bytes: 0,
                current_total: 0,
            })
        ),
        tr("photos.overview.sync.running")
    );

    let downloading = SyncLiveProgress {
        phase: SyncLivePhase::Downloading,
        downloaded: 12,
        download_total: 340,
        uploaded: 0,
        upload_total: 0,
        transfer_active: false,
        current_bytes: 0,
        current_total: 0,
    };
    assert_eq!(
        sync_overview_text(running, Some(downloading)),
        trf(
            "photos.overview.sync.running_download",
            &[("done", "12"), ("total", "340")]
        )
    );

    let streaming_download = SyncLiveProgress {
        transfer_active: true,
        current_bytes: 13 * 1024 * 1024,
        current_total: 46 * 1024 * 1024,
        ..downloading
    };
    assert_eq!(
        sync_overview_text(running, Some(streaming_download)),
        sync_overview_text(running, Some(downloading)),
        "byte-level transfer progress stays out of the label text"
    );

    let uploading = SyncLiveProgress {
        phase: SyncLivePhase::Uploading,
        downloaded: 340,
        download_total: 340,
        uploaded: 5,
        upload_total: 9,
        transfer_active: false,
        current_bytes: 0,
        current_total: 0,
    };
    assert_eq!(
        sync_overview_text(running, Some(uploading)),
        trf(
            "photos.overview.sync.running_upload",
            &[("done", "5"), ("total", "9")]
        )
    );

    let streaming_upload = SyncLiveProgress {
        transfer_active: true,
        current_bytes: 700,
        current_total: 2048,
        ..uploading
    };
    assert_eq!(
        sync_overview_text(running, Some(streaming_upload)),
        sync_overview_text(running, Some(uploading)),
        "byte-level transfer progress stays out of the label text"
    );
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
fn virtual_grid_creates_and_switches_all_three_photos_modes() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("gridview-modes.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));

    let page = PhotosPage::new(media_list, loader);

    {
        let grids = page.imp().grids.borrow();
        assert_eq!(grids.len(), 3);
        assert_eq!(
            grids.iter().map(VirtualMediaGrid::mode).collect::<Vec<_>>(),
            vec![GroupBy::Year, GroupBy::Month, GroupBy::Day],
            "Photos must always construct virtual grids for every mode"
        );
    }

    assert_eq!(
        page.current_grid().map(|grid| grid.mode()),
        Some(GroupBy::Day)
    );

    let stack = page.imp().view_stack.get();
    stack.set_visible_child_name("month");
    page.sync_active_grid_rebuilds();
    assert_eq!(
        page.current_grid().map(|grid| grid.mode()),
        Some(GroupBy::Month)
    );

    stack.set_visible_child_name("year");
    page.sync_active_grid_rebuilds();
    assert_eq!(
        page.current_grid().map(|grid| grid.mode()),
        Some(GroupBy::Year)
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

/// Left-click opens the viewer, so batch actions used to be reachable only from
/// the right-click menu — invisible on a desktop and unreachable on a
/// touchscreen. The header must therefore carry a persistent entry that hands
/// over to the exit button while multi-select is on, and restores itself on exit
/// so the same gesture works twice in a row.
#[gtk::test]
fn multi_select_entry_button_hands_over_to_exit_and_back() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("select-entry.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    let page = PhotosPage::new(media_list, loader);

    let imp = page.imp();
    assert!(
        imp.select_mode_revealer.get().reveals_child(),
        "the entry button is the discoverable half of the pair, so it starts visible"
    );
    assert!(
        !imp.exit_multi_select_revealer.get().reveals_child(),
        "the exit button must not appear before multi-select is on"
    );

    imp.select_mode_btn.get().emit_clicked();
    assert!(
        !imp.select_mode_revealer.get().reveals_child(),
        "the entry button hides itself once multi-select is active"
    );
    assert!(
        imp.exit_multi_select_revealer.get().reveals_child(),
        "entering multi-select from the header must reveal the way back out"
    );
    assert!(
        !imp.select_all_revealer.get().reveals_child(),
        "entering multi-select with no selection must not fake a batch toolbar"
    );

    // Exiting returns to browsing: the entry reappears so the flow repeats.
    page.clear_selection();
    assert!(
        !imp.exit_multi_select_revealer.get().reveals_child(),
        "the exit button must collapse once multi-select is off"
    );
    imp.select_mode_btn.get().emit_clicked();
    assert!(
        imp.exit_multi_select_revealer.get().reveals_child(),
        "the entry must work again after exiting multi-select"
    );
}

/// The `[start]` header group gained a persistent button, so the narrow-window
/// crowding the plan flagged has to stay bounded: at 800x600 both start icons
/// must be allocated real width rather than squeezed to nothing.
#[gtk::test]
fn narrow_window_keeps_both_start_header_buttons_allocated() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("narrow-header.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    let page = PhotosPage::new(media_list, loader);

    let window = gtk::Window::builder()
        .default_width(800)
        .default_height(600)
        .child(&page)
        .build();
    window.present();

    let context = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let search = page.imp().search_btn.get().width();
        let select = page.imp().select_mode_btn.get().width();
        if (search >= 24 && select >= 24) || deadline.elapsed() > std::time::Duration::from_secs(3)
        {
            break;
        }
        context.iteration(true);
    }

    let search = page.imp().search_btn.get().width();
    let select = page.imp().select_mode_btn.get().width();
    assert!(
        search >= 24 && select >= 24,
        "both start header buttons must keep a tappable allocation at 800x600, \
         got search={search} select={select}"
    );
}

#[gtk::test]
fn the_header_counter_names_the_selection_the_batch_buttons_act_on() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("counter.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for id in 1..=3 {
        media_list.append(&glib::BoxedAnyObject::new(sample_item(
            id,
            &format!("{id}.png"),
        )));
    }

    let page = PhotosPage::new(media_list, loader);
    let imp = page.imp();
    assert!(
        !imp.selection_count_revealer.get().reveals_child(),
        "without a selection the counter slot must stay out of the header"
    );

    let grid = page.current_grid().expect("the photos page owns a grid");
    grid.select_ids(&[MediaId::from(1), MediaId::from(2)]);

    assert!(imp.selection_count_revealer.get().reveals_child());
    assert_eq!(
        imp.selection_count_label.get().label().as_str(),
        crate::core::i18n::trf("photos.selection.count", &[("n", "2")]),
        "the counter must state the selection size the batch buttons will act on"
    );

    grid.clear_selection();

    assert!(
        !imp.selection_count_revealer.get().reveals_child(),
        "the counter must leave with the selection"
    );
}

/// The counter joins the `[end]` group only while something is selected, which
/// is exactly when that group is fullest. At 800x600 the batch icons must still
/// get a tappable allocation rather than being squeezed out by the number.
#[gtk::test]
fn the_selection_counter_does_not_squeeze_the_batch_actions_out() {
    let _ = gtk::init();
    let tmp = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&tmp.path().join("counter-narrow.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, tmp.path().join("thumbs")));
    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for id in 1..=3 {
        media_list.append(&glib::BoxedAnyObject::new(sample_item(
            id,
            &format!("{id}.png"),
        )));
    }
    let page = PhotosPage::new(media_list, loader);

    let window = gtk::Window::builder()
        .default_width(800)
        .default_height(600)
        .child(&page)
        .build();
    window.present();

    page.current_grid()
        .expect("the photos page owns a grid")
        .select_ids(&[MediaId::from(1), MediaId::from(2), MediaId::from(3)]);
    assert!(page.imp().selection_count_revealer.get().reveals_child());

    let context = glib::MainContext::default();
    for _ in 0..200 {
        while context.pending() {
            context.iteration(false);
        }
        if page.imp().add_to_album_btn.get().width() > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let counter = page.imp().selection_count_label.get().width();
    let add = page.imp().add_to_album_btn.get().width();
    let trash = page.imp().delete_to_trash_btn.get().width();
    assert!(
        counter > 0 && add >= 24 && trash >= 24,
        "the counter and the batch icons must coexist at 800x600, \
         got counter={counter} add={add} trash={trash}"
    );
}
