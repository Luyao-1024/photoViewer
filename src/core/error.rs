//! Application-level error type.
//!
//! Future tasks will extend this with specific variants for I/O, database,
//! EXIF parsing, and UI failures. For Task 1 we only need a placeholder so
//! the `photo_viewer::AppError` re-export compiles.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Other(String),
}

impl AppError {
    pub fn other(msg: impl Into<String>) -> Self {
        AppError::Other(msg.into())
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;