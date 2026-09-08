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
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::config::config_dir;

const SETTINGS_FILE: &str = "settings.json";
const THEME_KEY: &str = "theme";
const LIQUID_GLASS_KEY: &str = "liquid_glass";
const LIQUID_GLASS_TRANSPARENCY_KEY: &str = "liquid_glass_transparency";
const VIDEO_DEFAULT_MUTED_KEY: &str = "video_default_muted";
const VIDEO_VOLUME_KEY: &str = "video_volume";
const AUTO_PLAY_MOTION_PHOTO_KEY: &str = "auto_play_motion_photo";
const CUSTOM_SCAN_ROOTS_KEY: &str = "custom_scan_roots";
const EXCLUDED_SCAN_ROOTS_KEY: &str = "excluded_scan_roots";
const TRASH_BACKEND_KEY: &str = "trash_backend";

/// Default state of the Liquid Glass effect: **on** (opt-out). Keeps the
/// existing visual identity; users who dislike it turn it off in Settings.
const DEFAULT_LIQUID_GLASS: bool = true;
const DEFAULT_LIQUID_GLASS_TRANSPARENCY: f64 = 0.0;
const DEFAULT_VIDEO_MUTED: bool = true;
const DEFAULT_VIDEO_VOLUME: f64 = 1.0;
const DEFAULT_AUTO_PLAY_MOTION_PHOTO: bool = false;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}

impl ThemePreference {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrashBackend {
    System,
    App,
}

impl TrashBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::App => "app",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "app" => Self::App,
            _ => Self::System,
        }
    }
}

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

fn read_theme_preference_at(path: &Path) -> ThemePreference {
    let obj = read_object_at(path);
    obj.get(THEME_KEY)
        .and_then(|v| v.as_str())
        .map(ThemePreference::from_str)
        .unwrap_or(ThemePreference::System)
}

fn write_theme_preference_at(path: &Path, preference: ThemePreference) -> Result<(), String> {
    write_string_at(path, THEME_KEY, preference.as_str())
}

fn read_liquid_glass_transparency_at(path: &Path) -> f64 {
    let obj = read_object_at(path);
    obj.get(LIQUID_GLASS_TRANSPARENCY_KEY)
        .and_then(|v| v.as_f64())
        .map(clamp_liquid_glass_transparency)
        .unwrap_or(DEFAULT_LIQUID_GLASS_TRANSPARENCY)
}

fn write_liquid_glass_transparency_at(path: &Path, transparency: f64) -> Result<(), String> {
    write_f64_at(
        path,
        LIQUID_GLASS_TRANSPARENCY_KEY,
        clamp_liquid_glass_transparency(transparency),
    )
}

fn write_bool_at(path: &Path, key: &str, enabled: bool) -> Result<(), String> {
    let mut object = read_object_at(path);
    object.insert(key.to_string(), Value::Bool(enabled));
    write_object_at(path, object)
}

fn write_f64_at(path: &Path, key: &str, value: f64) -> Result<(), String> {
    let mut object = read_object_at(path);
    object.insert(key.to_string(), Value::from(value));
    write_object_at(path, object)
}

fn write_string_at(path: &Path, key: &str, value: &str) -> Result<(), String> {
    let mut object = read_object_at(path);
    object.insert(key.to_string(), Value::String(value.to_string()));
    write_object_at(path, object)
}

fn read_path_list_at(path: &Path, key: &str) -> Vec<PathBuf> {
    let obj = read_object_at(path);
    let Some(values) = obj.get(key).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    sanitize_path_list(values.iter().filter_map(|v| v.as_str()).map(PathBuf::from))
}

fn write_path_list_at(path: &Path, key: &str, paths: &[PathBuf]) -> Result<(), String> {
    let mut object = read_object_at(path);
    let values = sanitize_path_list(paths.iter().cloned())
        .into_iter()
        .map(|path| Value::String(path.to_string_lossy().into_owned()))
        .collect();
    object.insert(key.to_string(), Value::Array(values));
    write_object_at(path, object)
}

/// Publish the complete settings object with a sibling rename. A crash can
/// therefore leave either the old JSON or the new JSON, never a truncated
/// file. If an unreadable settings file exists, retain one timestamped copy
/// before replacing it with a valid object so recovery remains possible.
fn write_object_at(path: &Path, object: Map<String, Value>) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    if let Ok(data) = std::fs::read(path) {
        let valid_object = serde_json::from_slice::<Value>(&data)
            .ok()
            .is_some_and(|value| value.is_object());
        if !valid_object {
            let suffix = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            let mut corrupt_name = path.as_os_str().to_owned();
            corrupt_name.push(format!(".corrupt-{suffix}"));
            let corrupt = PathBuf::from(corrupt_name);
            std::fs::copy(path, corrupt).map_err(|e| e.to_string())?;
        }
    }

    let json = serde_json::to_vec_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    let mut staged = tempfile::Builder::new()
        .prefix(".photo-viewer-settings-")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    staged.write_all(&json).map_err(|e| e.to_string())?;
    staged.as_file().sync_all().map_err(|e| e.to_string())?;
    staged.persist(path).map_err(|e| e.error.to_string())?;
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())
}

fn sanitize_path_list<I>(paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut result = Vec::new();
    for path in paths {
        if !path.is_absolute() {
            continue;
        }
        if !result.iter().any(|existing| existing == &path) {
            result.push(path);
        }
    }
    result
}

fn read_video_default_muted_at(path: &Path) -> bool {
    let obj = read_object_at(path);
    obj.get(VIDEO_DEFAULT_MUTED_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(DEFAULT_VIDEO_MUTED)
}

fn write_video_default_muted_at(path: &Path, enabled: bool) -> Result<(), String> {
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
    write_object_at(path, object)
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

fn read_auto_play_motion_photo_at(path: &Path) -> bool {
    let obj = read_object_at(path);
    obj.get(AUTO_PLAY_MOTION_PHOTO_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(DEFAULT_AUTO_PLAY_MOTION_PHOTO)
}

fn write_auto_play_motion_photo_at(path: &Path, enabled: bool) -> Result<(), String> {
    write_bool_at(path, AUTO_PLAY_MOTION_PHOTO_KEY, enabled)
}

fn read_custom_scan_roots_at(path: &Path) -> Vec<PathBuf> {
    read_path_list_at(path, CUSTOM_SCAN_ROOTS_KEY)
}

fn write_custom_scan_roots_at(path: &Path, roots: &[PathBuf]) -> Result<(), String> {
    write_path_list_at(path, CUSTOM_SCAN_ROOTS_KEY, roots)
}

fn read_excluded_scan_roots_at(path: &Path) -> Vec<PathBuf> {
    read_path_list_at(path, EXCLUDED_SCAN_ROOTS_KEY)
}

fn write_excluded_scan_roots_at(path: &Path, roots: &[PathBuf]) -> Result<(), String> {
    write_path_list_at(path, EXCLUDED_SCAN_ROOTS_KEY, roots)
}

fn read_trash_backend_at(path: &Path) -> TrashBackend {
    let obj = read_object_at(path);
    obj.get(TRASH_BACKEND_KEY)
        .and_then(|v| v.as_str())
        .map(TrashBackend::from_str)
        .unwrap_or(TrashBackend::System)
}

fn write_trash_backend_at(path: &Path, backend: TrashBackend) -> Result<(), String> {
    write_string_at(path, TRASH_BACKEND_KEY, backend.as_str())
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

fn clamp_liquid_glass_transparency(transparency: f64) -> f64 {
    if transparency.is_finite() {
        transparency.clamp(0.0, 1.0)
    } else {
        DEFAULT_LIQUID_GLASS_TRANSPARENCY
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

/// App theme preference. Defaults to following the system color scheme.
pub fn theme_preference() -> ThemePreference {
    read_theme_preference_at(&settings_path())
}

/// Persist the app theme preference.
pub fn set_theme_preference(preference: ThemePreference) -> Result<(), String> {
    write_theme_preference_at(&settings_path(), preference)
}

/// Shared transparency for every glass material. `0.0` is opaque, `1.0` transparent.
pub fn liquid_glass_transparency() -> f64 {
    read_liquid_glass_transparency_at(&settings_path())
}

/// Persist the shared glass material transparency.
pub fn set_liquid_glass_transparency(transparency: f64) -> Result<(), String> {
    write_liquid_glass_transparency_at(&settings_path(), transparency)
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

/// Whether motion photos should auto-play their embedded video in the viewer.
pub fn auto_play_motion_photo() -> bool {
    read_auto_play_motion_photo_at(&settings_path())
}

/// Persist the motion-photo auto-play preference.
pub fn set_auto_play_motion_photo(enabled: bool) -> Result<(), String> {
    write_auto_play_motion_photo_at(&settings_path(), enabled)
}

/// Extra directories to scan in addition to the default Pictures/Videos roots.
pub fn custom_scan_roots() -> Vec<PathBuf> {
    read_custom_scan_roots_at(&settings_path())
}

/// Persist extra scan directories.
pub fn set_custom_scan_roots(roots: &[PathBuf]) -> Result<(), String> {
    write_custom_scan_roots_at(&settings_path(), roots)
}

/// Directories excluded from scanning. Exclusion never deletes files.
pub fn excluded_scan_roots() -> Vec<PathBuf> {
    read_excluded_scan_roots_at(&settings_path())
}

/// Persist scan exclusion directories.
pub fn set_excluded_scan_roots(roots: &[PathBuf]) -> Result<(), String> {
    write_excluded_scan_roots_at(&settings_path(), roots)
}

/// Trash storage backend. Defaults to the system trash.
pub fn trash_backend() -> TrashBackend {
    read_trash_backend_at(&settings_path())
}

/// Persist the trash storage backend.
pub fn set_trash_backend(backend: TrashBackend) -> Result<(), String> {
    write_trash_backend_at(&settings_path(), backend)
}

#[cfg(test)]
mod tests;
