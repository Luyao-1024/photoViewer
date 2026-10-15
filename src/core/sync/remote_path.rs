//! Job-relative path rules shared by validation and owned publication.
//!
//! These are the only path helpers the sync layer keeps: they describe paths
//! *inside* a configured job root, not remote object keys. A remote key is a
//! provider-specific concept and each provider owns its own.

use std::path::Path;

pub fn has_navigation_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

pub fn has_empty_segment(path: &str) -> bool {
    path.split('/').any(str::is_empty)
}

/// Files this application owns while publishing a pair. Ordinary media and
/// directory symlink targets never participate in this namespace, so the
/// cleanup that removes them cannot reach user content.
pub fn is_internal_sync_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".photoviewer-sync-"))
}

/// A staged file's name, inside this application's reserved namespace so a
/// later scan cannot mistake it for user media and the reset cleanup can find
/// it.
pub fn staged_name(name_hint: &str, unique: &str) -> String {
    let safe: String = name_hint
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    format!(".photoviewer-sync-{safe}-{}-{}", std::process::id(), unique)
}

#[cfg(test)]
mod tests;
