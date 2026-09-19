//! Canonical conversion between local paths and `file:` URIs.
//!
//! Never build file URIs with string concatenation: reserved characters such
//! as `%`, `#`, and `?` would be interpreted as URI syntax and could identify a
//! different file.
use std::path::{Path, PathBuf};

use gtk4::gio;
use gtk4::gio::prelude::FileExt;

use crate::core::error::{AppError, Result};

pub fn from_path(path: &Path) -> String {
    gio::File::for_path(path).uri().to_string()
}

pub fn to_path(uri: &str) -> Result<PathBuf> {
    gio::File::for_uri(uri)
        .path()
        .ok_or_else(|| AppError::Backend(format!("URI is not a local file: {uri}")))
}

pub fn path_or_file_uri(value: &str) -> Result<PathBuf> {
    if value.starts_with("file:") {
        to_path(value)
    } else {
        Ok(PathBuf::from(value))
    }
}

#[cfg(test)]
#[path = "file_uri/tests.rs"]
mod tests;
