use super::*;

fn sample_item() -> MediaItem {
    MediaItem {
        id: 1,
        uri: "file:///tmp/IMG_001.jpg".into(),
        path: PathBuf::from("/tmp/IMG_001.jpg"),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".into(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: Some(1920),
        height: Some(1080),
        video_duration_secs: None,
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 123_456,
        blake3_hash: "abc123".into(),
        is_favorite: false,
        trashed_at: None,
    }
}

#[test]
fn display_name_from_path() {
    let item = sample_item();
    assert_eq!(item.display_name(), "IMG_001.jpg");
}

#[test]
fn trashed_flag() {
    let mut item = sample_item();
    assert!(item.trashed_at.is_none());
    item.trashed_at = Some(Utc::now());
    assert!(item.trashed_at.is_some());
}

#[test]
fn media_attribute_helpers_read_top_level_json_flags() {
    let mut item = sample_item();
    item.media_attributes = r#"{"animated":true,"hdr":true}"#.into();

    assert!(item.is_animated());
    assert!(item.is_hdr());
}

#[test]
fn media_attribute_helpers_ignore_missing_or_false_flags() {
    let mut item = sample_item();
    assert!(!item.is_animated());
    assert!(!item.is_hdr());

    item.media_attributes = r#"{"animated":false,"hdr":false}"#.into();
    assert!(!item.is_animated());
    assert!(!item.is_hdr());
}

#[test]
fn logical_media_type_flags_materialize_overlapping_categories() {
    let flags = media_type_flags(
        MEDIA_SUBKIND_MOTION_PHOTO,
        r#"{"animated":true,"hdr":true}"#,
    );

    assert_eq!(
        flags,
        MEDIA_TYPE_MOTION_PHOTO | MEDIA_TYPE_ANIMATED | MEDIA_TYPE_HDR
    );
}
