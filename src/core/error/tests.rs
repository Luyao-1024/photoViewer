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
