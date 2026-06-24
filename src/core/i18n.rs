use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::OnceLock;

use crate::config::config_dir;

const EN_JSON: &str = include_str!("../../i18n/en.json");
const ZH_CN_JSON: &str = include_str!("../../i18n/zh-CN.json");

type OverrideMap = HashMap<String, String>;

#[derive(Debug, Deserialize)]
struct I18nConfig {
    locale: Option<String>,
    overrides: Option<OverrideMap>,
}

#[derive(Debug)]
struct I18nRuntime {
    locale: String,
    overrides: OverrideMap,
    baselines: OverrideMap,
}

static INSTANCE: OnceLock<I18nRuntime> = OnceLock::new();

fn normalize_locale(input: Option<String>) -> String {
    match input.as_deref().map(|v| v.replace('_', "-")) {
        Some(v) if v.eq_ignore_ascii_case("zh") || v.eq_ignore_ascii_case("zh-cn") => {
            "zh-CN".to_string()
        }
        Some(v) if v.to_ascii_lowercase().starts_with("zh") => "zh-CN".to_string(),
        Some(v) if v.eq_ignore_ascii_case("en") || v.eq_ignore_ascii_case("en-us") => {
            "en".to_string()
        }
        Some(v) if v.eq_ignore_ascii_case("en-us") => "en".to_string(),
        Some(v) if v == "zh-CN" => "zh-CN".to_string(),
        Some(v) if v == "en" => "en".to_string(),
        Some(v) => v,
        None => "en".to_string(),
    }
}

fn detect_locale() -> String {
    if let Ok(value) = std::env::var("PHOTO_VIEWER_LOCALE") {
        let normalized = normalize_locale(Some(value));
        if normalized == "zh-CN" || normalized == "en" {
            return normalized;
        }
    }
    for var in ["LC_ALL", "LANG", "LANGUAGE"] {
        if let Ok(value) = std::env::var(var) {
            if value.to_ascii_lowercase().starts_with("zh") {
                return "zh-CN".to_string();
            }
        }
    }
    "en".to_string()
}

fn load_builtin_map(locale: &str) -> OverrideMap {
    let raw = match locale {
        "zh-CN" => ZH_CN_JSON,
        _ => EN_JSON,
    };
    serde_json::from_str::<OverrideMap>(raw).unwrap_or_default()
}

#[cfg(test)]
fn supported_locales() -> &'static [&'static str] {
    &["en", "zh-CN"]
}

fn parse_config(path: std::path::PathBuf) -> (Option<String>, OverrideMap) {
    let mut overrides = OverrideMap::new();

    let data = match fs::read_to_string(path) {
        Ok(data) => data,
        Err(_) => return (None, overrides),
    };

    let cfg: I18nConfig = match serde_json::from_str(&data) {
        Ok(cfg) => cfg,
        Err(_) => return (None, overrides),
    };

    let locale = cfg.locale;
    if let Some(raw_overrides) = cfg.overrides {
        overrides = raw_overrides;
    }
    (locale, overrides)
}

fn init_runtime() -> I18nRuntime {
    let cfg_path = config_dir().join("i18n.json");
    let (configured_locale, overrides) = parse_config(cfg_path);

    let locale = normalize_locale(configured_locale.or_else(|| Some(detect_locale())));

    let locale = if locale == "zh-CN" || locale == "en" {
        locale
    } else {
        "en".to_string()
    };

    let baselines = load_builtin_map(&locale);
    let overrides = overrides
        .into_iter()
        .filter(|(key, _)| baselines.contains_key(key))
        .collect();

    I18nRuntime {
        locale,
        overrides,
        baselines,
    }
}

fn runtime() -> &'static I18nRuntime {
    INSTANCE.get_or_init(init_runtime)
}

#[cfg(test)]
fn values_by_key(key: &str) -> Vec<(&'static str, String)> {
    supported_locales()
        .iter()
        .map(|&locale| {
            let map = load_builtin_map(locale);
            (locale, map.get(key).cloned().unwrap_or_default())
        })
        .collect()
}

/// Return the translated string for a key.
pub fn tr(key: &str) -> String {
    runtime()
        .overrides
        .get(key)
        .cloned()
        .or_else(|| {
            runtime()
                .baselines
                .get(key)
                .map(std::string::ToString::to_string)
        })
        .unwrap_or_else(|| key.to_string())
}

/// Return the translated string for a key and substitute `{name}`-style placeholders.
pub fn trf(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = tr(key);
    for (k, v) in args {
        let token = format!("{{{k}}}");
        text = text.replace(&token, v);
    }
    text
}

/// Current locale resolved from config + environment.
pub fn locale() -> &'static str {
    &runtime().locale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i18n_key_has_value_in_all_languages() {
        let mut key_pool = std::collections::BTreeSet::new();
        let locale_maps: Vec<(&str, OverrideMap)> = supported_locales()
            .iter()
            .map(|&locale| (locale, load_builtin_map(locale)))
            .collect();

        for (_, map) in &locale_maps {
            for key in map.keys() {
                key_pool.insert(key.clone());
            }
        }

        for key in key_pool {
            for (locale, map) in &locale_maps {
                assert!(
                    map.contains_key(&key),
                    "missing localization for key '{key}' in locale '{locale}'"
                );
            }
        }
    }

    #[test]
    fn i18n_values_are_available_for_a_key_in_every_language() {
        let key_values = values_by_key("viewer.button.favorite");
        assert_eq!(key_values.len(), supported_locales().len());
        assert!(key_values.iter().all(|(_, value)| !value.is_empty()));
    }
}
