use super::*;

/// Unique temp path per test invocation (no env-var mutation, so the
/// parallel test runner cannot race on `XDG_CONFIG_HOME`).
fn tmp_path(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    // pid + counter-ish suffix via name keeps parallel invocations distinct.
    p.push(format!(
        "photoViewer-prefs-test-{}-{}-{}",
        std::process::id(),
        name,
        read_liquid_glass_at_counter(),
    ));
    p
}

// Monotonic counter so each call to tmp_path() within one process yields a
// distinct file even when `name` repeats.
use std::sync::atomic::{AtomicUsize, Ordering};
static COUNTER: AtomicUsize = AtomicUsize::new(0);
fn read_liquid_glass_at_counter() -> usize {
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

#[test]
fn defaults_to_enabled_when_file_missing() {
    let path = tmp_path("missing");
    cleanup(&path);
    assert!(
        read_liquid_glass_at(&path),
        "absent file should fall back to default (true)"
    );
    cleanup(&path);
}

#[test]
fn round_trip_true_and_false() {
    let path = tmp_path("roundtrip");
    cleanup(&path);

    write_liquid_glass_at(&path, false).unwrap();
    assert!(
        !read_liquid_glass_at(&path),
        "after writing false, read should be false"
    );

    write_liquid_glass_at(&path, true).unwrap();
    assert!(
        read_liquid_glass_at(&path),
        "after writing true, read should be true"
    );

    cleanup(&path);
}

#[test]
fn defaults_when_key_absent_but_file_present() {
    let path = tmp_path("keymissing");
    cleanup(&path);
    // A settings file that exists but lacks the liquid_glass key.
    std::fs::write(&path, "{\"something_else\": 42}").unwrap();
    assert!(
        read_liquid_glass_at(&path),
        "present file without the key should fall back to default (true)"
    );
    cleanup(&path);
}

#[test]
fn writing_preserves_other_keys() {
    let path = tmp_path("preserve");
    cleanup(&path);
    // Seed with an unrelated key.
    std::fs::write(&path, "{\"locale_hint\": \"en\"}").unwrap();

    write_liquid_glass_at(&path, false).unwrap();

    let obj = read_object_at(&path);
    assert_eq!(obj.get("locale_hint").and_then(|v| v.as_str()), Some("en"));
    assert_eq!(
        obj.get(LIQUID_GLASS_KEY).and_then(|v| v.as_bool()),
        Some(false)
    );

    cleanup(&path);
}

#[test]
fn malformed_json_falls_back_to_default() {
    let path = tmp_path("malformed");
    cleanup(&path);
    std::fs::write(&path, "{ not valid json").unwrap();
    assert!(
        read_liquid_glass_at(&path),
        "garbage file should fall back to default (true)"
    );
    cleanup(&path);
}

#[test]
fn video_audio_defaults_to_muted_and_full_volume() {
    let path = tmp_path("video-defaults");
    cleanup(&path);

    assert!(
        read_video_default_muted_at(&path),
        "missing video_default_muted should default to true"
    );
    assert_eq!(
        read_video_volume_at(&path),
        1.0,
        "missing video_volume should default to full volume"
    );

    cleanup(&path);
}

#[test]
fn trash_backend_defaults_to_system_and_round_trips() {
    let path = tmp_path("trash-backend");
    cleanup(&path);

    assert_eq!(
        read_trash_backend_at(&path),
        TrashBackend::System,
        "missing trash backend preference should default to the system trash"
    );

    write_trash_backend_at(&path, TrashBackend::App).unwrap();
    assert_eq!(read_trash_backend_at(&path), TrashBackend::App);

    write_trash_backend_at(&path, TrashBackend::System).unwrap();
    assert_eq!(read_trash_backend_at(&path), TrashBackend::System);

    cleanup(&path);
}

#[test]
fn liquid_glass_transparency_defaults_to_opaque() {
    let path = tmp_path("glass-transparency-default");
    cleanup(&path);

    assert_eq!(
        read_liquid_glass_transparency_at(&path),
        0.0,
        "missing liquid_glass_transparency should default to fully opaque"
    );

    cleanup(&path);
}

#[test]
fn liquid_glass_transparency_round_trip_clamps_and_preserves_keys() {
    let path = tmp_path("glass-transparency-roundtrip");
    cleanup(&path);
    std::fs::write(&path, "{\"liquid_glass\": false}").unwrap();

    write_liquid_glass_transparency_at(&path, 0.62).unwrap();
    assert_eq!(read_liquid_glass_transparency_at(&path), 0.62);

    write_liquid_glass_transparency_at(&path, -0.2).unwrap();
    assert_eq!(read_liquid_glass_transparency_at(&path), 0.0);

    write_liquid_glass_transparency_at(&path, 1.6).unwrap();
    assert_eq!(read_liquid_glass_transparency_at(&path), 1.0);

    let obj = read_object_at(&path);
    assert_eq!(
        obj.get(LIQUID_GLASS_KEY).and_then(|v| v.as_bool()),
        Some(false),
        "writing glass transparency should preserve appearance prefs"
    );

    cleanup(&path);
}

#[test]
fn video_audio_preferences_round_trip_and_preserve_existing_keys() {
    let path = tmp_path("video-roundtrip");
    cleanup(&path);
    std::fs::write(&path, "{\"liquid_glass\": false}").unwrap();

    write_video_default_muted_at(&path, false).unwrap();
    write_video_volume_at(&path, 0.42).unwrap();

    assert!(
        !read_video_default_muted_at(&path),
        "written default muted preference should be read back"
    );
    assert_eq!(read_video_volume_at(&path), 0.42);

    let obj = read_object_at(&path);
    assert_eq!(
        obj.get(LIQUID_GLASS_KEY).and_then(|v| v.as_bool()),
        Some(false),
        "writing video prefs should preserve appearance prefs"
    );

    cleanup(&path);
}

#[test]
fn disabling_default_mute_recovers_zero_volume() {
    let path = tmp_path("video-unmute-recovers-volume");
    cleanup(&path);

    write_video_volume_at(&path, 0.0).unwrap();
    write_video_default_muted_at(&path, false).unwrap();

    assert!(
        !read_video_default_muted_at(&path),
        "default mute should be disabled"
    );
    assert_eq!(
        read_video_volume_at(&path),
        DEFAULT_VIDEO_VOLUME,
        "turning default mute off should recover an audible volume from stale zero"
    );

    cleanup(&path);
}

#[test]
fn effective_volume_recovers_existing_unmuted_zero_config() {
    let path = tmp_path("video-existing-unmuted-zero");
    cleanup(&path);

    write_video_default_muted_at(&path, false).unwrap();
    write_video_volume_at(&path, 0.0).unwrap();

    assert_eq!(
        read_video_volume_at(&path),
        0.0,
        "raw persisted volume should still reflect the file"
    );
    assert_eq!(
        read_effective_video_volume_at(&path),
        DEFAULT_VIDEO_VOLUME,
        "existing unmuted configs with stale zero volume should start audible"
    );

    cleanup(&path);
}

#[test]
fn video_volume_is_clamped_when_written() {
    let path = tmp_path("video-volume-clamp");
    cleanup(&path);

    write_video_volume_at(&path, 1.7).unwrap();
    assert_eq!(read_video_volume_at(&path), 1.0);

    write_video_volume_at(&path, -0.2).unwrap();
    assert_eq!(read_video_volume_at(&path), 0.0);

    cleanup(&path);
}

#[test]
fn motion_photo_auto_play_defaults_to_disabled() {
    let path = tmp_path("motion-auto-play-default");
    cleanup(&path);

    assert!(
        !read_auto_play_motion_photo_at(&path),
        "missing auto_play_motion_photo should default to false"
    );

    cleanup(&path);
}

#[test]
fn motion_photo_auto_play_round_trip_and_preserves_keys() {
    let path = tmp_path("motion-auto-play-roundtrip");
    cleanup(&path);
    std::fs::write(&path, "{\"video_default_muted\": false}").unwrap();

    write_auto_play_motion_photo_at(&path, true).unwrap();
    assert!(read_auto_play_motion_photo_at(&path));

    write_auto_play_motion_photo_at(&path, false).unwrap();
    assert!(!read_auto_play_motion_photo_at(&path));

    let obj = read_object_at(&path);
    assert_eq!(
        obj.get(VIDEO_DEFAULT_MUTED_KEY).and_then(|v| v.as_bool()),
        Some(false),
        "writing motion-photo setting should preserve existing video prefs"
    );

    cleanup(&path);
}

#[test]
fn theme_preference_defaults_to_system() {
    let path = tmp_path("theme-default");
    cleanup(&path);

    assert_eq!(
        read_theme_preference_at(&path),
        ThemePreference::System,
        "missing theme should default to following the system"
    );

    cleanup(&path);
}

#[test]
fn theme_preference_round_trips_valid_values() {
    let path = tmp_path("theme-roundtrip");
    cleanup(&path);

    for preference in [
        ThemePreference::System,
        ThemePreference::Light,
        ThemePreference::Dark,
    ] {
        write_theme_preference_at(&path, preference).unwrap();
        assert_eq!(read_theme_preference_at(&path), preference);
    }

    cleanup(&path);
}

#[test]
fn theme_preference_invalid_values_fall_back_to_system() {
    let path = tmp_path("theme-invalid");
    cleanup(&path);
    std::fs::write(&path, "{\"theme\": \"sepia\"}").unwrap();

    assert_eq!(
        read_theme_preference_at(&path),
        ThemePreference::System,
        "unknown theme values should not force a color scheme"
    );

    cleanup(&path);
}

#[test]
fn theme_preference_writing_preserves_other_keys() {
    let path = tmp_path("theme-preserve");
    cleanup(&path);
    std::fs::write(&path, "{\"liquid_glass\": false}").unwrap();

    write_theme_preference_at(&path, ThemePreference::Dark).unwrap();

    let obj = read_object_at(&path);
    assert_eq!(
        obj.get(LIQUID_GLASS_KEY).and_then(|v| v.as_bool()),
        Some(false),
        "writing theme preference should preserve existing appearance prefs"
    );
    assert_eq!(read_theme_preference_at(&path), ThemePreference::Dark);

    cleanup(&path);
}

#[test]
fn scan_path_preferences_default_to_empty_lists() {
    let path = tmp_path("scan-path-defaults");
    cleanup(&path);

    assert!(read_custom_scan_roots_at(&path).is_empty());
    assert!(read_excluded_scan_roots_at(&path).is_empty());

    cleanup(&path);
}

#[test]
fn scan_path_preferences_round_trip_clean_absolute_unique_paths() {
    let path = tmp_path("scan-path-roundtrip");
    cleanup(&path);
    std::fs::write(
        &path,
        "{\"locale_hint\": \"en\", \"custom_scan_roots\": [\"relative\", \"/old\"]}",
    )
    .unwrap();

    write_custom_scan_roots_at(
        &path,
        &[
            std::path::PathBuf::from("/library/Camera"),
            std::path::PathBuf::from("relative"),
            std::path::PathBuf::from("/library/Camera"),
            std::path::PathBuf::from("/library/Exports"),
        ],
    )
    .unwrap();
    write_excluded_scan_roots_at(
        &path,
        &[
            std::path::PathBuf::from("/library/Camera/Private"),
            std::path::PathBuf::from("/library/Camera/Private"),
            std::path::PathBuf::from("../nope"),
        ],
    )
    .unwrap();

    assert_eq!(
        read_custom_scan_roots_at(&path),
        vec![
            std::path::PathBuf::from("/library/Camera"),
            std::path::PathBuf::from("/library/Exports"),
        ]
    );
    assert_eq!(
        read_excluded_scan_roots_at(&path),
        vec![std::path::PathBuf::from("/library/Camera/Private")]
    );

    let obj = read_object_at(&path);
    assert_eq!(obj.get("locale_hint").and_then(|v| v.as_str()), Some("en"));

    cleanup(&path);
}

#[test]
fn scan_path_preferences_ignore_malformed_json_values() {
    let path = tmp_path("scan-path-malformed");
    cleanup(&path);
    std::fs::write(
        &path,
        "{\"custom_scan_roots\": [42, false, \"/ok\"], \"excluded_scan_roots\": \"nope\"}",
    )
    .unwrap();

    assert_eq!(
        read_custom_scan_roots_at(&path),
        vec![std::path::PathBuf::from("/ok")]
    );
    assert!(read_excluded_scan_roots_at(&path).is_empty());

    cleanup(&path);
}
