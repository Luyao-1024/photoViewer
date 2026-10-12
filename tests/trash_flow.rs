//! Real GIO trash smoke tests. Detailed trash-root resolution tests live next
//! to `core::trash`, where they can inject an isolated temporary root.
use photo_viewer::core::{file_uri, trash};
use tempfile::TempDir;

fn scratch_dir() -> TempDir {
    // On the sandbox's filesystem rather than `$HOME`: GIO trashes by renaming,
    // so the trash directory and the file have to share one, and a real trash
    // directory is not a price worth paying for a smoke test.
    photo_viewer::testing::Sandbox::install().scratch("photo-viewer-trash-")
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
