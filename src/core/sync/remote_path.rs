//! Shared remote path operations. Callers own normalization and error types.

use crate::core::error::{AppError, Result};

pub(super) fn has_navigation_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

pub(super) fn has_empty_segment(path: &str) -> bool {
    path.split('/').any(str::is_empty)
}

pub(super) fn remote_key(root: &str, relative: &str) -> Result<String> {
    if has_empty_segment(relative) || has_navigation_segment(relative) {
        return Err(AppError::Backend(
            "invalid relative synchronization path".into(),
        ));
    }
    let root = root.trim_matches('/');
    Ok(if root.is_empty() {
        relative.into()
    } else {
        format!("{root}/{relative}")
    })
}

pub(super) fn relative_remote_key(root: &str, key: &str) -> Result<String> {
    let key = key.trim_matches('/');
    if root.is_empty() {
        return Ok(key.into());
    }
    key.strip_prefix(root)
        .and_then(|relative| relative.strip_prefix('/'))
        .map(str::to_string)
        .ok_or_else(|| AppError::Backend(format!("remote object escaped configured root: {key}")))
}
