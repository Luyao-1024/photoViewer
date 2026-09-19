use super::*;

#[test]
fn reserved_and_unicode_characters_round_trip() {
    for name in [
        "percent%20.jpg",
        "hash#photo.jpg",
        "question?.jpg",
        "space photo.jpg",
        "中文 相册.jpg",
    ] {
        let path = PathBuf::from("/tmp").join(name);
        let uri = from_path(&path);
        assert_eq!(to_path(&uri).unwrap(), path);
    }
}

#[test]
fn raw_paths_remain_supported_at_internal_compatibility_boundary() {
    assert_eq!(
        path_or_file_uri("/tmp/hash#photo.jpg").unwrap(),
        PathBuf::from("/tmp/hash#photo.jpg")
    );
}

#[test]
fn non_file_uri_is_rejected() {
    assert!(to_path("https://example.com/photo.jpg").is_err());
}
