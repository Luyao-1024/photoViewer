use super::super::test_support::*;

use super::*;

use std::collections::HashSet;
use std::path::Path;

#[test]
fn album_delete_pruning_removes_deleted_folder_rows_except_remaining_live_uris() {
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let deleted = media_item(1, "/tmp/Camera", "deleted.jpg");
    let still_live = media_item(2, "/tmp/Camera", "still-live.jpg");
    let other = media_item(3, "/tmp/Other", "keep.jpg");
    list.append(&glib::BoxedAnyObject::new(deleted.clone()));
    list.append(&glib::BoxedAnyObject::new(still_live.clone()));
    list.append(&glib::BoxedAnyObject::new(other.clone()));

    remove_deleted_album_media_from_media_list(
        &list,
        &[Path::new("/tmp/Camera").to_path_buf()],
        &HashSet::from([still_live.uri.clone()]),
        &HashSet::new(),
    );

    assert_eq!(media_list_uris(&list), vec![still_live.uri, other.uri]);
}

#[test]
fn album_delete_pruning_preserves_unknown_remaining_live_folder_rows() {
    let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let unknown = media_item(1, "/tmp/Camera", "unknown.jpg");
    let deleted = media_item(2, "/tmp/Trips", "deleted.jpg");
    let other = media_item(3, "/tmp/Other", "keep.jpg");
    list.append(&glib::BoxedAnyObject::new(unknown.clone()));
    list.append(&glib::BoxedAnyObject::new(deleted.clone()));
    list.append(&glib::BoxedAnyObject::new(other.clone()));

    remove_deleted_album_media_from_media_list(
        &list,
        &[
            Path::new("/tmp/Camera").to_path_buf(),
            Path::new("/tmp/Trips").to_path_buf(),
        ],
        &HashSet::new(),
        &HashSet::from([Path::new("/tmp/Camera").to_path_buf()]),
    );

    assert_eq!(media_list_uris(&list), vec![unknown.uri, other.uri]);
}

#[test]
fn album_initial_load_limit_caps_large_albums_to_render_window() {
    assert_eq!(album_initial_load_limit(2), 2);
    assert_eq!(
        album_initial_load_limit(100_000),
        crate::core::runtime_config::DEFAULT_MAX_RENDERED_GRID_ITEMS as u32
    );
    assert_eq!(album_initial_load_limit(-1), 0);
}

#[test]
fn album_backfill_limit_caps_large_albums_to_ui_window() {
    let initial = album_initial_load_limit(100_000);
    let limit = album_backfill_fetch_limit(initial, 100_000);

    assert!(
        initial + limit <= crate::core::runtime_config::DEFAULT_UI_MEDIA_LIST_CAP as u32,
        "album backfill must not materialize the full album into the GTK ListStore"
    );
    assert!(
        limit < 100_000 - initial,
        "large album backfill should fetch only a bounded continuation window"
    );
}

#[test]
fn album_switch_trace_points_cover_selection_to_backfill() {
    let production_source = format!(
        "{}\n{}",
        production_source("src/ui/window/navigation.rs"),
        production_source("src/ui/window.rs")
    );
    for trace_name in [
        "album:select_row",
        "album:open_idle",
        "album:already_visible_check",
        "album:bind_page",
        "album:backfill_schedule",
    ] {
        assert!(
            production_source.contains(trace_name),
            "missing album switch trace point {trace_name}"
        );
    }
}

#[test]
fn high_frequency_album_progress_logs_stay_debug() {
    let production_source = format!(
        "{}\n{}",
        production_source("src/ui/window/navigation.rs"),
        production_source("src/ui/window.rs")
    );

    for message in [
        "album_switch: initial_page_loaded",
        "album_backfill: skipped_at_cap",
        "album_backfill: fetched",
        "album_backfill: appended",
    ] {
        let message_index = production_source
            .find(message)
            .unwrap_or_else(|| panic!("missing log message {message}"));
        let before = &production_source[..message_index];
        let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
            .iter()
            .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, candidate)| candidate)
            .expect("log message should be inside a tracing macro");
        assert_eq!(
            actual_macro, "tracing::debug!(",
            "{message} should stay out of default logs"
        );
    }
}
