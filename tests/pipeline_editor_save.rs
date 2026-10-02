//! Data-pipeline coverage for the editor's two save paths.
//!
//! Calls `save_as_copy` / `save_overwrite` directly — the exact functions
//! `EditorPage` delegates to — and checks the file on disk, the `.bak` backup and
//! the DB row. It spins up no GTK widgets, so it proves the save pipeline, not
//! that a user can reach the Save buttons.
//!
//! The button-level coverage is owned by
//! `tests/ux_click_flows.rs::journey_search_view_edit_and_save_copy` and
//! `journey_save_overwrite_rewrites_the_file_and_keeps_a_backup`.

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
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: "h".into(),
    };
    let id = common::db::insert_media_item(pool, &item).unwrap();
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
