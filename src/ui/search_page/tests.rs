use super::*;
use chrono::Utc;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn loader_for(pool: DbPool) -> Arc<ThumbnailLoader> {
    let dir = tempfile::tempdir().unwrap().keep();
    Arc::new(ThumbnailLoader::new(pool, dir.join("cache")))
}

fn search_page() -> SearchPage {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("search-test.db")).unwrap();
    SearchPage::new(pool.clone(), loader_for(pool))
}

fn media_item(id: i64) -> MediaItem {
    MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: PathBuf::from(format!("/tmp/{id}.jpg")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".to_string(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(1),
        height: Some(1),
        video_duration_secs: None,
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1,
        blake3_hash: format!("hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}

fn visible_state(page: &SearchPage) -> String {
    page.imp()
        .search_state_stack
        .get()
        .visible_child_name()
        .map(|name| name.to_string())
        .unwrap_or_default()
}

fn no_results_page(page: &SearchPage) -> adw::StatusPage {
    page.imp()
        .no_results_page
        .borrow()
        .as_ref()
        .cloned()
        .expect("search page should own a zero-result status page")
}

/// Drive the default main context long enough for the busy delay to elapse.
fn pump_for(patience: Duration) {
    let deadline = Instant::now() + patience;
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gtk::test]
fn segment_labels_follow_the_active_locale() {
    let _ = gtk::init();
    let page = search_page();
    let imp = page.imp();

    let cases = [
        (&imp.field_all, "search.field.all"),
        (&imp.field_name, "search.field.name"),
        (&imp.field_date, "search.field.date"),
    ];
    for (button, key) in cases {
        let label = button
            .label()
            .map(|value| value.to_string())
            .unwrap_or_default();
        assert!(!label.is_empty(), "the {key} segment must not be blank");
        assert_eq!(label, tr(key), "the {key} segment should read from i18n");
    }

    // Naming the segments must not cost them their native radio grouping.
    imp.field_date.set_active(true);
    assert!(!imp.field_all.is_active());
    assert!(imp.field_date.is_active());
}

#[gtk::test]
fn a_blank_query_shows_the_idle_prompt_instead_of_blank_space() {
    let _ = gtk::init();
    let page = search_page();

    assert_eq!(visible_state(&page), SEARCH_STATE_IDLE);
    page.replace_results(Vec::new(), Vec::new());
    assert_eq!(visible_state(&page), SEARCH_STATE_IDLE);
    assert!(
        page.imp()
            .search_state_stack
            .get()
            .child_by_name(SEARCH_STATE_RESULTS)
            .is_some(),
        "the result sections must stay in the stack so hits can return to them"
    );
}

#[gtk::test]
fn a_miss_names_the_term_and_offers_a_way_out() {
    let _ = gtk::init();
    let page = search_page();
    page.imp().search_entry.get().set_text("  不存在的文件名  ");

    page.replace_results(Vec::new(), Vec::new());

    assert_eq!(visible_state(&page), SEARCH_STATE_NO_RESULTS);
    let title = no_results_page(&page).title().to_string();
    assert!(
        title.contains("不存在的文件名"),
        "the miss page should echo the trimmed term, got {title:?}"
    );

    let clear = no_results_page(&page)
        .child()
        .and_downcast::<gtk::Button>()
        .expect("the miss page should carry a clear-search action");
    clear.emit_clicked();

    assert_eq!(page.imp().search_entry.get().text(), "");
    assert_eq!(
        visible_state(&page),
        SEARCH_STATE_IDLE,
        "clearing the term should leave the idle prompt, not a stale miss page"
    );
}

#[gtk::test]
fn hits_replace_the_miss_page_with_the_result_sections() {
    let _ = gtk::init();
    let page = search_page();
    page.imp().search_entry.get().set_text("cat");

    page.replace_results(Vec::new(), Vec::new());
    assert_eq!(visible_state(&page), SEARCH_STATE_NO_RESULTS);

    page.replace_results(vec![media_item(1)], Vec::new());
    assert_eq!(visible_state(&page), SEARCH_STATE_RESULTS);
    assert!(page.imp().image_results_box.get().is_visible());
    assert!(
        !page.imp().video_results_box.get().is_visible(),
        "an empty section should not hold a header on the page"
    );
}

#[gtk::test]
fn trashing_the_last_hit_lands_on_the_miss_page() {
    let _ = gtk::init();
    let page = search_page();
    page.imp().search_entry.get().set_text("cat");
    page.replace_results(vec![media_item(1)], Vec::new());
    assert_eq!(visible_state(&page), SEARCH_STATE_RESULTS);

    page.remove_media_ids_from_results(&[MediaId::new(1)]);

    assert_eq!(
        visible_state(&page),
        SEARCH_STATE_NO_RESULTS,
        "an emptied result set should read as a miss, not as a blank results page"
    );
}

#[gtk::test]
fn the_busy_row_waits_for_a_query_to_be_slow() {
    let _ = gtk::init();
    let page = search_page();
    let imp = page.imp();
    assert!(
        !imp.search_busy_box.get().is_visible(),
        "the busy row should start hidden"
    );

    page.begin_search_busy(1);
    assert!(
        !imp.search_busy_box.get().is_visible(),
        "arming a query must not flash the spinner before the delay"
    );

    pump_for(Duration::from_millis(SEARCH_BUSY_DELAY_MS + 600));
    assert!(
        imp.search_busy_box.get().is_visible(),
        "a query slower than {SEARCH_BUSY_DELAY_MS} ms should show the spinner"
    );
    assert!(imp.search_spinner.get().is_spinning());

    page.end_search_busy();
    assert!(!imp.search_busy_box.get().is_visible());
    assert!(!imp.search_spinner.get().is_spinning());
}

#[gtk::test]
fn a_settled_query_never_paints_the_busy_row() {
    let _ = gtk::init();
    let page = search_page();
    let imp = page.imp();

    page.begin_search_busy(1);
    // The user keeps typing, then the newest query resolves inside the delay.
    page.begin_search_busy(2);
    page.end_search_busy();

    pump_for(Duration::from_millis(SEARCH_BUSY_DELAY_MS + 600));

    assert!(
        !imp.search_busy_box.get().is_visible(),
        "a timer left over from a settled query must not raise the row"
    );
    assert!(!imp.search_spinner.get().is_spinning());
    assert_eq!(
        imp.busy_generation.get(),
        None,
        "the settled query must retire its own busy token"
    );
}

/// The preview sections used to be the one media list in the app with no way to
/// act on a photo: the grid was built with the context menu switched off and
/// its favorite/add-to-album callbacks were empty closures.
#[gtk::test]
fn the_preview_sections_open_the_same_batch_door_as_albums() {
    let _ = gtk::init();
    let page = search_page();
    page.imp().search_entry.get().set_text("cat");
    page.replace_results(vec![media_item(701)], Vec::new());

    let grid = page
        .imp()
        .image_grid
        .borrow()
        .clone()
        .expect("a hit should build the image section grid");
    assert_eq!(
        grid.context_menu_overlay_for_tests()
            .map(|overlay| overlay.upcast::<gtk::Widget>()),
        Some(page.imp().search_overlay.get().upcast::<gtk::Widget>()),
        "the section menu must render into the page overlay, not disappear"
    );

    grid.set_multi_select_mode(true);
    grid.select_ids(&[MediaId::new(701)]);
    assert_eq!(
        grid.selected_ids(),
        vec![MediaId::new(701)],
        "entering multi-select from a section must actually select the tile"
    );
}

#[gtk::test]
fn the_favorite_menu_items_ask_the_database_instead_of_defaulting() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("search-fav.db")).unwrap();
    let rows = crate::core::db::upsert_media_items_batch(
        &pool,
        &[crate::core::NewMediaItem {
            uri: "file:///tmp/702.jpg".into(),
            path: PathBuf::from("/tmp/702.jpg"),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(1),
            height: Some(1),
            video_duration_secs: None,
            taken_at: Some(Utc::now()),
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: "h702".into(),
        }],
    )
    .unwrap();
    let id = MediaId::new(rows[0].id);
    let page = SearchPage::new(pool.clone(), loader_for(pool.clone()));
    page.replace_results(vec![rows[0].clone()], Vec::new());

    let state = page.favorite_state_for_ids(&[id]);
    assert!(
        state.can_favorite && !state.can_unfavorite,
        "an unfiled photo should offer 收藏 only, got {state:?}"
    );

    crate::core::repository::MediaRepository::new(pool)
        .set_favorite(&[id], true)
        .unwrap();
    let state = page.favorite_state_for_ids(&[id]);
    assert!(
        state.can_unfavorite && !state.can_favorite,
        "the menu must follow the written state, got {state:?}"
    );

    page.apply_favorite_flags(&[id], false);
    let list = page
        .imp()
        .image_list
        .borrow()
        .clone()
        .expect("the image section should own a result list");
    let stored = list
        .item(0)
        .and_downcast::<glib::BoxedAnyObject>()
        .expect("list items are boxed media rows");
    assert!(
        !stored.borrow::<MediaItem>().is_favorite,
        "a written favorite has to reach the tile, not just the database"
    );
}
