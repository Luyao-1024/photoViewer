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
