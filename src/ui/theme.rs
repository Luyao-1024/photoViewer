use libadwaita as adw;

use crate::core::prefs::ThemePreference;

pub fn color_scheme_for(preference: ThemePreference) -> adw::ColorScheme {
    match preference {
        ThemePreference::System => adw::ColorScheme::Default,
        ThemePreference::Light => adw::ColorScheme::ForceLight,
        ThemePreference::Dark => adw::ColorScheme::ForceDark,
    }
}

pub fn apply(preference: ThemePreference) {
    adw::StyleManager::default().set_color_scheme(color_scheme_for(preference));
}
