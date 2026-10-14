use super::*;
use std::path::Path;

#[test]
fn navigation_and_empty_segments_are_detected() {
    assert!(has_navigation_segment("a/../b"));
    assert!(has_navigation_segment("./a"));
    assert!(has_navigation_segment(".."));
    assert!(!has_navigation_segment("a/b.jpg"));
    // A dotted filename is not a navigation segment; only whole segments are.
    assert!(!has_navigation_segment("photos/..hidden/photo.jpg"));

    assert!(has_empty_segment("a//b"));
    assert!(has_empty_segment("a/"));
    assert!(!has_empty_segment("a/b.jpg"));
}

#[test]
fn only_this_applications_reserved_prefix_counts_as_internal() {
    assert!(is_internal_sync_name(Path::new(
        "/root/.photoviewer-sync-1.part"
    )));
    assert!(is_internal_sync_name(Path::new(".photoviewer-sync-thumb")));
    assert!(!is_internal_sync_name(Path::new("/root/photo.jpg")));
    // A directory whose name merely contains the prefix is not a temp file.
    assert!(!is_internal_sync_name(Path::new("/root/photos")));
}
