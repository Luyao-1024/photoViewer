use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("gio error: {0}")]
    Gio(#[from] gtk4::glib::Error),

    #[error("image decode failed: {0}")]
    Decode(String),

    #[error("image io error: {0}")]
    Image(#[from] image::ImageError),

    #[error("exif parse failed: {0}")]
    Exif(String),

    #[error("backend unavailable: {0}")]
    Backend(String),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),
}

pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: AppError = io_err.into();
        assert!(matches!(err, AppError::Io(_)));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn from_db_error() {
        let db_err = rusqlite::Error::QueryReturnedNoRows;
        let err: AppError = db_err.into();
        assert!(matches!(err, AppError::Db(_)));
    }
}
