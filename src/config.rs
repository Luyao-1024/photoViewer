//! Application configuration paths (XDG Base Directory spec)
use std::path::PathBuf;

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::env::temp_dir().join("photoViewer-home"))
}

/// Returns the locale-aware user Pictures directory.
///
/// Resolution order:
/// 1. Parse `~/.config/user-dirs.dirs` for `XDG_PICTURES_DIR` (with `$HOME` substitution).
/// 2. If `LANG` or `LC_ALL` starts with `zh`, fall back to `~/图片`.
/// 3. Otherwise fall back to `~/Pictures`.
pub fn pictures_dir() -> PathBuf {
    // 1) Try the XDG user-dirs.dirs file.
    if let Some(p) = read_user_dirs_key("XDG_PICTURES_DIR") {
        return p;
    }

    let home_path = home_dir();

    // 2) Locale fallback for Chinese systems: ~/图片 (set by xdg-user-dirs-update).
    if is_chinese_locale() {
        let zh = home_path.join("图片");
        if zh.exists() {
            return zh;
        }
    }

    // 3) Final fallback.
    home_path.join("Pictures")
}

/// Returns the locale-aware user Videos directory.
///
/// Resolution order mirrors [`pictures_dir`]:
/// 1. Parse `~/.config/user-dirs.dirs` for `XDG_VIDEOS_DIR`.
/// 2. If `LANG` or `LC_ALL` starts with `zh`, fall back to `~/视频`.
/// 3. Otherwise fall back to `~/Videos`.
pub fn videos_dir() -> PathBuf {
    if let Some(p) = read_user_dirs_key("XDG_VIDEOS_DIR") {
        return p;
    }

    let home_path = home_dir();
    if is_chinese_locale() {
        let zh = home_path.join("视频");
        if zh.exists() {
            return zh;
        }
    }

    home_path.join("Videos")
}

pub fn media_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in [pictures_dir(), videos_dir()] {
        if !roots.iter().any(|existing| existing == &root) {
            roots.push(root);
        }
    }
    roots
}

/// Returns true if `LANG` or `LC_ALL` starts with `zh`.
fn is_chinese_locale() -> bool {
    for var in ["LC_ALL", "LANG"] {
        if let Some(v) = std::env::var_os(var) {
            if let Some(s) = v.to_str() {
                if s.to_ascii_lowercase().starts_with("zh") {
                    return true;
                }
            }
        }
    }
    false
}

/// Read `~/.config/user-dirs.dirs` and return the parsed XDG user dir key
/// with `$HOME` substitution. Returns `None` if the file is missing or the
/// key is absent.
fn read_user_dirs_key(key: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let home_path = PathBuf::from(&home);
    let file = home_path.join(".config").join("user-dirs.dirs");
    let content = std::fs::read_to_string(&file).ok()?;
    parse_user_dirs_key(&content, &home_path, key)
}

/// Parse the contents of a `user-dirs.dirs` file and return the requested
/// value with `$HOME` substituted.
fn parse_user_dirs_key(content: &str, home: &std::path::Path, key: &str) -> Option<PathBuf> {
    let prefix = format!("{key}=");
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            let raw = rest.trim().trim_matches('"');
            if raw.is_empty() || raw == "$HOME" {
                return Some(home.to_path_buf());
            }
            if let Some(suffix) = raw.strip_prefix("$HOME/") {
                return Some(home.join(suffix));
            }
            // Already absolute or otherwise non-$HOME relative — return as-is.
            return Some(PathBuf::from(raw));
        }
    }
    None
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(".local/share"));
    base.join("photoViewer")
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(".cache"));
    base.join("photoViewer")
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(".config"));
    base.join("photoViewer")
}
