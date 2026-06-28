//! albums::set_album_order + list_with_favorites — 持久化侧栏相册拖动排序。
//!
//! `album_order` 是独立于 `albums` 物化视图的表，所以排序不会在
//! `albums::refresh`（扫描 / 加入相册后都会触发）的 DELETE+INSERT 中丢失。
//! 这里覆盖三件事：写入顺序被 `list_with_favorites` 读回；未被记录顺序的新
//! 相册回退到末尾并保留默认相对顺序；空顺序维持虚拟相册置顶的默认行为。
use chrono::{TimeZone, Utc};
use photo_viewer::core::albums::{
    self, FAVORITES_ALBUM_PATH, IMAGES_ALBUM_PATH, VIDEOS_ALBUM_PATH,
};
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use std::path::PathBuf;
use tempfile::tempdir;

/// 固定时间戳，保证 `albums::list` 的 `last_modified DESC` 顺序确定：Camera
/// （03-02）新于 Screenshots（03-01）→ 默认列表里 Camera 在前。
fn make_item(uri: &str, path: &str, folder: &str, day: u32) -> NewMediaItem {
    let mtime = Utc.with_ymd_and_hms(2025, 3, day, 12, 0, 0).unwrap();
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        taken_at: Some(mtime),
        file_mtime: mtime,
        file_size: 1000,
        blake3_hash: format!("h{uri}"),
    }
}

/// 两个文件夹相册 + 三个虚拟相册（收藏 / 图片 / 视频）的图库快照。
fn seed(pool: &db::DbPool) {
    db::insert_media_item(
        pool,
        &make_item("file:///p/Camera/a.jpg", "/p/Camera/a.jpg", "/p/Camera", 2),
    )
    .unwrap();
    db::insert_media_item(
        pool,
        &make_item(
            "file:///p/Screenshots/b.jpg",
            "/p/Screenshots/b.jpg",
            "/p/Screenshots",
            1,
        ),
    )
    .unwrap();
    albums::refresh(pool).unwrap();
}

fn folder_paths(albums: &[albums::Album]) -> Vec<String> {
    albums
        .iter()
        .map(|a| a.folder_path.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn saved_order_is_applied_by_list_with_favorites() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    seed(&pool);

    // 默认顺序：虚拟相册置顶，文件夹按最近修改倒序。
    let before = folder_paths(&albums::list_with_favorites(&pool).unwrap());
    assert_eq!(before[0], FAVORITES_ALBUM_PATH);
    assert_eq!(before[1], IMAGES_ALBUM_PATH);
    assert_eq!(before[2], VIDEOS_ALBUM_PATH);

    // 把两个文件夹相册排到最前，虚拟相册挪后；整体顺序自定义。
    let order = vec![
        "/p/Screenshots".to_string(),
        "/p/Camera".to_string(),
        VIDEOS_ALBUM_PATH.to_string(),
        FAVORITES_ALBUM_PATH.to_string(),
        IMAGES_ALBUM_PATH.to_string(),
    ];
    albums::set_album_order(&pool, &order).unwrap();

    let after = folder_paths(&albums::list_with_favorites(&pool).unwrap());
    assert_eq!(after, order, "saved order should drive list_with_favorites");
}

#[test]
fn unrecorded_albums_fall_to_the_end() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    seed(&pool);

    // 只给一个文件夹相册记录顺序；另一个不记录。
    albums::set_album_order(&pool, &["/p/Camera".to_string()]).unwrap();

    // 扫描后新增一个相册（也不在 album_order 里）。
    db::insert_media_item(
        &pool,
        &make_item("file:///p/New/c.jpg", "/p/New/c.jpg", "/p/New", 3),
    )
    .unwrap();
    albums::refresh(&pool).unwrap();

    let after = folder_paths(&albums::list_with_favorites(&pool).unwrap());
    // 有记录的 /p/Camera 排最前。
    assert_eq!(after[0], "/p/Camera");
    // 没记录的（虚拟三个 + 新文件夹）都在其后，顺序保持默认相对次序即可，
    // 关键是不出现在 /p/Camera 之前。
    assert!(
        !after
            .iter()
            .take(after.len() - 4)
            .any(|p| p == "/p/Screenshots" || p == "/p/New"),
        "unrecorded albums must not precede the ordered one"
    );
}

#[test]
fn order_survives_albums_refresh() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    seed(&pool);

    let order = vec![
        VIDEOS_ALBUM_PATH.to_string(),
        "/p/Camera".to_string(),
        IMAGES_ALBUM_PATH.to_string(),
        FAVORITES_ALBUM_PATH.to_string(),
        "/p/Screenshots".to_string(),
    ];
    albums::set_album_order(&pool, &order).unwrap();

    // albums::refresh 会 DELETE + 重建 albums 物化视图；album_order 是独立表，
    // 排序必须不受影响（这正是把顺序单列成表的原因）。
    albums::refresh(&pool).unwrap();

    let after = folder_paths(&albums::list_with_favorites(&pool).unwrap());
    assert_eq!(
        after, order,
        "order must survive albums materialized rebuild"
    );
}

#[test]
fn empty_order_keeps_default_virtual_first_layout() {
    let dir = tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    seed(&pool);

    // 从未写入任何顺序 → album_order 为空 → 维持默认行为。
    let list = albums::list_with_favorites(&pool).unwrap();
    let paths = folder_paths(&list);
    assert_eq!(paths[0], FAVORITES_ALBUM_PATH);
    assert_eq!(paths[1], IMAGES_ALBUM_PATH);
    assert_eq!(paths[2], VIDEOS_ALBUM_PATH);
    // 文件夹相册在虚拟相册之后（Camera 更新 → 默认在前）。
    assert_eq!(list[3].folder_path, PathBuf::from("/p/Camera"));
    assert_eq!(list[4].folder_path, PathBuf::from("/p/Screenshots"));
}
