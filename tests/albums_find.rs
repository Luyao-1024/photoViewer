//! albums::find_by_folder_path — 按 folder_path 查单个 album。
//!
//! 比 `list` 更轻量,适合 picker 这种只需要"选中一个目标"的场景。
use chrono::Utc;
use photo_viewer::core::albums;
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use std::path::PathBuf;
use tempfile::tempdir;

fn make_item(uri: &str, path: PathBuf, folder: PathBuf) -> NewMediaItem {
    NewMediaItem {
        uri: uri.into(),
        path,
        folder_path: folder,
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1000,
        blake3_hash: format!("h-{}", uri),
    }
}

#[test]
fn find_by_folder_path_returns_some_when_present() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    db::insert_media_item(
        &pool,
        &make_item(
            "file:///p/Camera/a.jpg",
            PathBuf::from("/p/Camera/a.jpg"),
            PathBuf::from("/p/Camera"),
        ),
    )
    .unwrap();
    albums::refresh(&pool).unwrap();

    let found = albums::find_by_folder_path(&pool, &PathBuf::from("/p/Camera"))
        .expect("query should succeed");
    let album = found.expect("Camera album should be found");
    assert_eq!(album.folder_path, PathBuf::from("/p/Camera"));
    assert_eq!(album.photo_count, 1);
}

#[test]
fn find_by_folder_path_returns_none_when_absent() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    // 没有任何 media_items,refresh 会清空 albums 表
    albums::refresh(&pool).unwrap();

    let result =
        albums::find_by_folder_path(&pool, &PathBuf::from("/p/Nonexistent")).expect("query ok");
    assert!(result.is_none(), "不存在的 folder 应返回 None");
}
