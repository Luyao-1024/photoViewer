//! Integration tests for `photo_viewer::config::pictures_dir` locale-aware
//! resolution. These tests are environment-driven and must NOT run in parallel
//! with each other: each test calls `std::env::set_var` / `remove_var` to
//! control `HOME`, `XDG_CONFIG_HOME` (via HOME), `LANG`, `LC_ALL`, and a
//! synthetic `user-dirs.dirs`. Run with `--test-threads=1` to avoid races.
use photo_viewer::config::{cache_dir, config_dir, data_dir, pictures_dir};
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::tempdir;

/// Process-wide lock that serializes env-mutating tests in this binary.
/// `std::env::set_var`/`remove_var` mutate process-global state, so tests
/// that touch HOME/LANG/LC_ALL/XDG_CONFIG_HOME cannot run concurrently
/// without leaking state into each other.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Isolated env: each test sets HOME to a fresh tempdir, clears
/// LANG/LC_ALL/XDG_*, and restores them at the end.
struct EnvGuard {
    saved_home: Option<std::ffi::OsString>,
    saved_lang: Option<std::ffi::OsString>,
    saved_lc_all: Option<std::ffi::OsString>,
}

struct RemoveHomeGuard {
    saved_home: Option<std::ffi::OsString>,
    saved_data: Option<std::ffi::OsString>,
    saved_cache: Option<std::ffi::OsString>,
    saved_config: Option<std::ffi::OsString>,
}

impl RemoveHomeGuard {
    fn new() -> Self {
        let saved_home = std::env::var_os("HOME");
        let saved_data = std::env::var_os("XDG_DATA_HOME");
        let saved_cache = std::env::var_os("XDG_CACHE_HOME");
        let saved_config = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("XDG_CACHE_HOME");
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        Self {
            saved_home,
            saved_data,
            saved_cache,
            saved_config,
        }
    }
}

impl Drop for RemoveHomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.saved_data {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
            match &self.saved_cache {
                Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
            match &self.saved_config {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }
}

impl EnvGuard {
    fn new(home: &std::path::Path) -> Self {
        let saved_home = std::env::var_os("HOME");
        let saved_lang = std::env::var_os("LANG");
        let saved_lc_all = std::env::var_os("LC_ALL");

        // Use unsafe blocks because set_var is now marked unsafe in recent
        // Rust editions; this test file already runs single-threaded.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("LANG");
            std::env::remove_var("LC_ALL");
            // XDG_CONFIG_HOME would short-circuit HOME/.config, so unset it.
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        EnvGuard {
            saved_home,
            saved_lang,
            saved_lc_all,
        }
    }

    fn set_locale(&self, value: &str) {
        unsafe {
            std::env::set_var("LANG", value);
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.saved_lang {
                Some(v) => std::env::set_var("LANG", v),
                None => std::env::remove_var("LANG"),
            }
            match &self.saved_lc_all {
                Some(v) => std::env::set_var("LC_ALL", v),
                None => std::env::remove_var("LC_ALL"),
            }
        }
    }
}

#[test]
fn falls_back_to_pictures_for_non_chinese_locale() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempdir().unwrap();
    let home = dir.path().to_path_buf();
    // No user-dirs.dirs present.
    let guard = EnvGuard::new(&home);
    guard.set_locale("en_US.UTF-8");

    let result: PathBuf = pictures_dir();
    assert_eq!(
        result,
        home.join("Pictures"),
        "non-zh locale with no user-dirs.dirs should resolve to ~/Pictures"
    );
    drop(guard);
}

#[test]
fn xdg_paths_do_not_panic_when_home_is_missing() {
    let _guard = ENV_LOCK.lock().unwrap();
    let _env = RemoveHomeGuard::new();

    let result =
        std::panic::catch_unwind(|| (pictures_dir(), data_dir(), cache_dir(), config_dir()));

    assert!(result.is_ok(), "path helpers should not panic without HOME");
    let (pictures, data, cache, config) = result.unwrap();
    assert!(pictures.ends_with("Pictures"));
    assert!(data.ends_with("photoViewer"));
    assert!(cache.ends_with("photoViewer"));
    assert!(config.ends_with("photoViewer"));
}

#[test]
fn parses_user_dirs_pictures_with_home_substitution() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempdir().unwrap();
    let home = dir.path().to_path_buf();
    // Create a fake user-dirs.dirs under HOME/.config/.
    let config_dir = home.join(".config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let user_dirs = config_dir.join("user-dirs.dirs");
    std::fs::write(
        &user_dirs,
        "# Generated by xdg-user-dirs-update\n\
         XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n\
         XDG_PICTURES_DIR=\"$HOME/MyPics\"\n\
         XDG_VIDEOS_DIR=\"$HOME/Videos\"\n",
    )
    .unwrap();

    let guard = EnvGuard::new(&home);
    guard.set_locale("en_US.UTF-8");

    let result: PathBuf = pictures_dir();
    assert_eq!(
        result,
        home.join("MyPics"),
        "XDG_PICTURES_DIR with $HOME/ prefix should be substituted"
    );
    drop(guard);
}

#[test]
fn chinese_locale_falls_back_to_pictures_dir_unicode() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempdir().unwrap();
    let home = dir.path().to_path_buf();
    // Pre-create ~/图片 so the locale fallback can find it.
    std::fs::create_dir_all(home.join("图片")).unwrap();

    let guard = EnvGuard::new(&home);
    guard.set_locale("zh_CN.UTF-8");

    let result: PathBuf = pictures_dir();
    assert_eq!(
        result,
        home.join("图片"),
        "zh_CN locale with existing ~/图片 should resolve to it"
    );
    drop(guard);
}

#[test]
fn user_dirs_takes_precedence_over_locale_fallback() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempdir().unwrap();
    let home = dir.path().to_path_buf();
    // Both a user-dirs.dirs and ~/图片 exist; the XDG file wins.
    std::fs::create_dir_all(home.join("图片")).unwrap();
    let config_dir = home.join(".config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("user-dirs.dirs"),
        "XDG_PICTURES_DIR=\"$HOME/FromXdg\"\n",
    )
    .unwrap();

    let guard = EnvGuard::new(&home);
    guard.set_locale("zh_CN.UTF-8");

    let result: PathBuf = pictures_dir();
    assert_eq!(
        result,
        home.join("FromXdg"),
        "user-dirs.dirs should win over zh_CN ~/图片 fallback"
    );
    drop(guard);
}
