//! User preferences persisted as JSON under `config_dir()`.
//!
//! Mirrors the JSON-file convention established by the i18n settings
//! (`window.rs::persist_locale` → `i18n.json`): reads/writes a small JSON
//! object, upserting a single key while preserving any others. Lives in a
//! sibling file (`settings.json`) so the glass toggle stays independent of
//! the language config.
//!
//! The hot entry points (`liquid_glass_enabled` / `set_liquid_glass`) resolve
//! the path from `config_dir()`, while the actual read/write logic is split
//! into path-injected helpers (`*_at`) so the unit tests can point at a
//! temp file without mutating process-global env vars (which race under
//! `cargo test`'s parallel runner).
use std::path::Path;

use serde_json::{Map, Value};

use crate::config::config_dir;

const SETTINGS_FILE: &str = "settings.json";
const LIQUID_GLASS_KEY: &str = "liquid_glass";
const VIDEO_DEFAULT_MUTED_KEY: &str = "video_default_muted";
const VIDEO_VOLUME_KEY: &str = "video_volume";

/// Default state of the Liquid Glass effect: **on** (opt-out). Keeps the
/// existing visual identity; users who dislike it turn it off in Settings.
const DEFAULT_LIQUID_GLASS: bool = true;
const DEFAULT_VIDEO_MUTED: bool = true;
const DEFAULT_VIDEO_VOLUME: f64 = 1.0;

fn settings_path() -> std::path::PathBuf {
    config_dir().join(SETTINGS_FILE)
}

/// Read the parsed top-level object from `path`, or an empty object on any
/// missing file / parse error (so a missing settings file falls back to
/// defaults cleanly).
fn read_object_at(path: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str::<Value>(&data)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default(),
        Err(_) => Map::new(),
    }
}

fn read_liquid_glass_at(path: &Path) -> bool {
    let obj = read_object_at(path);
    obj.get(LIQUID_GLASS_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(DEFAULT_LIQUID_GLASS)
}

fn write_liquid_glass_at(path: &Path, enabled: bool) -> Result<(), String> {
    write_bool_at(path, LIQUID_GLASS_KEY, enabled)
}

fn write_bool_at(path: &Path, key: &str, enabled: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut object = read_object_at(path);
    object.insert(key.to_string(), Value::Bool(enabled));
    let json = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_f64_at(path: &Path, key: &str, value: f64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut object = read_object_at(path);
    object.insert(key.to_string(), Value::from(value));
    let json = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

fn read_video_default_muted_at(path: &Path) -> bool {
    let obj = read_object_at(path);
    obj.get(VIDEO_DEFAULT_MUTED_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(DEFAULT_VIDEO_MUTED)
}

fn write_video_default_muted_at(path: &Path, enabled: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut object = read_object_at(path);
    object.insert(VIDEO_DEFAULT_MUTED_KEY.to_string(), Value::Bool(enabled));
    let current_volume = object
        .get(VIDEO_VOLUME_KEY)
        .and_then(|v| v.as_f64())
        .map(clamp_video_volume)
        .unwrap_or(DEFAULT_VIDEO_VOLUME);
    if !enabled && current_volume <= 0.0 {
        object.insert(
            VIDEO_VOLUME_KEY.to_string(),
            Value::from(DEFAULT_VIDEO_VOLUME),
        );
    }
    let json = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

fn read_video_volume_at(path: &Path) -> f64 {
    let obj = read_object_at(path);
    obj.get(VIDEO_VOLUME_KEY)
        .and_then(|v| v.as_f64())
        .map(clamp_video_volume)
        .unwrap_or(DEFAULT_VIDEO_VOLUME)
}

fn read_effective_video_volume_at(path: &Path) -> f64 {
    let volume = read_video_volume_at(path);
    if !read_video_default_muted_at(path) && volume <= 0.0 {
        DEFAULT_VIDEO_VOLUME
    } else {
        volume
    }
}

fn write_video_volume_at(path: &Path, volume: f64) -> Result<(), String> {
    write_f64_at(path, VIDEO_VOLUME_KEY, clamp_video_volume(volume))
}

fn clamp_video_volume(volume: f64) -> f64 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        DEFAULT_VIDEO_VOLUME
    }
}

/// Current Liquid Glass preference, resolved from `settings.json`.
/// Defaults to enabled when the file or key is absent.
pub fn liquid_glass_enabled() -> bool {
    read_liquid_glass_at(&settings_path())
}

/// Persist the Liquid Glass preference to `settings.json`, preserving any
/// other keys already present. Returns an error string on IO/serialize failure.
pub fn set_liquid_glass(enabled: bool) -> Result<(), String> {
    write_liquid_glass_at(&settings_path(), enabled)
}

/// Whether newly opened videos start muted. Defaults to muted on startup.
pub fn video_default_muted() -> bool {
    read_video_default_muted_at(&settings_path())
}

/// Persist whether newly opened videos start muted.
pub fn set_video_default_muted(enabled: bool) -> Result<(), String> {
    write_video_default_muted_at(&settings_path(), enabled)
}

/// Last persisted video volume in the inclusive range `[0.0, 1.0]`.
pub fn video_volume() -> f64 {
    read_effective_video_volume_at(&settings_path())
}

/// Persist the current video volume, clamped to `[0.0, 1.0]`.
pub fn set_video_volume(volume: f64) -> Result<(), String> {
    write_video_volume_at(&settings_path(), volume)
}

#[cfg(test)]
mod tests {
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
}
