//! M4 end-to-end coverage for the Edit → Save Copy / Save Overwrite flow.
//!
//! This test exercises the *real* call chain the UI uses when the user
//! clicks Save Copy or Save Overwrite inside `EditorPage`:
//!
//! 1. `db::init_pool` + `db::insert_media_item` (the path the gallery
//!    uses to persist scanned files)
//! 2. `save_as_copy` / `save_overwrite` (the exact functions called by
//!    `EditorPage::save_as_copy` / `EditorPage::perform_save_overwrite`)
//! 3. Verify file-on-disk + DB row + DB row count for Save Copy
//! 4. Verify `.jpg.bak` backup + DB metadata update for Save Overwrite
//!
//! It does NOT spin up GTK widgets — `EditorPage` requires a running
//! GTK main loop (loading the source image goes through
//! `glib::spawn_future_local`), which is not feasible in a headless unit
//! test. Instead we cover the *whole* save-side pipeline that the UI
//! delegates to, which is the most important contract: a user-edited
//! photo must produce the right file and the right DB row.

mod common;

use chrono::Utc;
use common::*;
use photo_viewer::core::db;
use photo_viewer::core::edit::{save_as_copy, save_overwrite, EditRegistry, EditState};
use tempfile::tempdir;

fn insert_test_item(
    dir: &std::path::Path,
    pool: &db::DbPool,
    name: &str,
) -> photo_viewer::core::media::MediaItem {
    write_plain_jpeg(dir, name);
    let path_str = dir.join(name).to_string_lossy().to_string();
    let item = photo_viewer::core::media::NewMediaItem {
        uri: format!("file://{path_str}"),
        path: path_str.into(),
        folder_path: dir.to_path_buf(),
        mime_type: "image/jpeg".into(),
        width: Some(64),
        height: Some(48),
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: "h".into(),
    };
    let id = db::insert_media_item(pool, &item).unwrap();
    db::get_media_item(pool, id).unwrap()
}

#[test]
fn full_edit_flow_save_as_copy() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let media_item = insert_test_item(dir.path(), &pool, "edit.jpg");
    let orig_size = std::fs::metadata(&media_item.path).unwrap().len();

    let state = EditState {
        brightness: 20,
        ..Default::default()
    };
    let registry = EditRegistry::new_with_v1();

    // 1. Save Copy — render + write `{stem}_edited.jpg` + insert new DB row.
    let new_item = save_as_copy(&media_item, &state, &pool, &registry).unwrap();

    // File on disk
    assert!(new_item.path.exists(), "edited file should exist on disk");
    assert!(
        new_item.path.to_string_lossy().contains("_edited"),
        "edited filename should contain _edited"
    );
    let edited_size = std::fs::metadata(&new_item.path).unwrap().len();
    assert!(edited_size > 0, "edited file should be non-empty");

    // Original must be left untouched
    let orig_size_after = std::fs::metadata(&media_item.path).unwrap().len();
    assert_eq!(
        orig_size_after, orig_size,
        "save_as_copy must not modify the original file"
    );

    // DB row inserted; both original + copy are now visible
    let all = db::list_all_media(&pool).unwrap();
    assert_eq!(
        all.len(),
        2,
        "DB should have the original plus the newly inserted copy"
    );
    let ids: Vec<i64> = all.iter().map(|m| m.id).collect();
    assert!(ids.contains(&media_item.id));
    assert!(ids.contains(&new_item.id));
    assert_ne!(
        media_item.id, new_item.id,
        "Save Copy must allocate a fresh row id"
    );
}

#[test]
fn full_edit_flow_save_overwrite() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let media_item = insert_test_item(dir.path(), &pool, "edit.jpg");
    let orig_size = std::fs::metadata(&media_item.path).unwrap().len();
    let orig_hash = media_item.blake3_hash.clone();

    let state = EditState::default();
    let registry = EditRegistry::new_with_v1();

    save_overwrite(&media_item, &state, &pool, &registry).unwrap();

    // Backup exists with original bytes
    let backup = media_item.path.with_extension("jpg.bak");
    assert!(backup.exists(), ".jpg.bak backup should exist");
    let backup_size = std::fs::metadata(&backup).unwrap().len();
    assert_eq!(
        backup_size, orig_size,
        "backup should preserve the original file size"
    );

    // Original still exists (overwrite, not delete)
    assert!(media_item.path.exists(), "original path should still exist");
    let new_size = std::fs::metadata(&media_item.path).unwrap().len();
    assert!(new_size > 0, "re-encoded file should be non-empty");

    // DB row updated: same id, different hash
    let updated = db::get_media_item(&pool, media_item.id).unwrap();
    assert_eq!(updated.id, media_item.id);
    assert_ne!(
        updated.blake3_hash, orig_hash,
        "DB blake3_hash should reflect the new bytes after overwrite"
    );
    assert_eq!(
        updated.file_size as i64, new_size as i64,
        "DB file_size should match new on-disk size"
    );

    // Still exactly one row (no new row inserted)
    let all = db::list_all_media(&pool).unwrap();
    assert_eq!(all.len(), 1, "save_overwrite must not insert a new DB row");
}
