//! Real GIO trash smoke tests. Detailed trash-root resolution tests live next
//! to `core::trash`, where they can inject an isolated temporary root.
use photo_viewer::core::{file_uri, trash};
use tempfile::{Builder, TempDir};

fn scratch_dir() -> TempDir {
    let base = std::env::var_os("TMPDIR_REAL")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"));
    Builder::new()
        .prefix("photo-viewer-trash-")
        .tempdir_in(base)
        .expect("create scratch dir")
}

fn unique_name(label: &str) -> String {
    format!(
        "photo-viewer-{label}-{}-{}.jpg",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

#[test]
fn move_and_restore_file() {
    let dir = scratch_dir();
    let src = dir.path().join(unique_name("restore"));
    std::fs::write(&src, b"fake jpeg data").unwrap();
    let uri = file_uri::from_path(&src);

    trash::move_to_trash(&uri).unwrap();
    assert!(!src.exists());
    trash::restore_from_trash(&uri).unwrap();
    assert_eq!(std::fs::read(&src).unwrap(), b"fake jpeg data");
}

#[test]
fn permanent_delete() {
    let dir = scratch_dir();
    let src = dir.path().join(unique_name("delete"));
    std::fs::write(&src, b"x").unwrap();
    let uri = file_uri::from_path(&src);

    trash::move_to_trash(&uri).unwrap();
    let trashed_path = file_uri::to_path(&trash::trashed_file_uri(&uri).unwrap()).unwrap();
    trash::delete_permanently(&uri).unwrap();

    assert!(!trashed_path.exists());
}
