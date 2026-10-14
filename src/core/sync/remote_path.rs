//! Job-relative path rules shared by validation and owned publication.
//!
//! These are the only path helpers the sync layer keeps: they describe paths
//! *inside* a configured job root, not remote object keys. A remote key is a
//! provider-specific concept and each provider owns its own.

use std::path::Path;

pub(crate) fn has_navigation_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

pub(crate) fn has_empty_segment(path: &str) -> bool {
    path.split('/').any(str::is_empty)
}

/// Files this application owns while publishing a pair. Ordinary media and
/// directory symlink targets never participate in this namespace, so the
/// cleanup that removes them cannot reach user content.
pub(crate) fn is_internal_sync_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".photoviewer-sync-"))
}

#[cfg(test)]
mod tests;
