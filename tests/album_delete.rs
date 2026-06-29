use chrono::Utc;
use photo_viewer::core::album_ops;
use photo_viewer::core::albums::{self, Album, FAVORITES_ALBUM_PATH};
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use photo_viewer::core::trash;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tempfile::{Builder, TempDir};

fn scratch_dir() -> TempDir {
    let base = std::env::var_os("TMPDIR_REAL")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/var/tmp"));
    Builder::new()
        .prefix("photo-viewer-album-delete-")
        .tempdir_in(base)
        .expect("create scratch dir")
}

fn media_item(path: &Path) -> NewMediaItem {
    let metadata = std::fs::metadata(path).expect("media file should exist");
    let uri = format!("file://{}", path.display());
    NewMediaItem {
        uri: uri.clone(),
        path: path.to_path_buf(),
        folder_path: path.parent().expect("media path should have parent").into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: metadata.len(),
        blake3_hash: format!("hash-{uri}"),
    }
}

fn create_media(pool: &db::DbPool, folder: &Path, name: &str) -> (i64, String, PathBuf) {
    std::fs::create_dir_all(folder).expect("create media folder");
    let path = folder.join(name);
    std::fs::write(&path, b"fake jpeg data").expect("write media file");
    let item = media_item(&path);
    let uri = item.uri.clone();
    let id = db::insert_media_item(pool, &item).expect("insert media row");
    (id, uri, path)
}

fn album_for(pool: &db::DbPool, folder: &Path) -> Album {
    albums::refresh(pool).expect("refresh albums");
    albums::find_by_folder_path(pool, folder)
        .expect("find album")
        .expect("album should exist")
}

struct TrashCleanup {
    uris: Vec<String>,
}

impl Drop for TrashCleanup {
    fn drop(&mut self) {
        for uri in &self.uris {
            let _ = trash::delete_permanently(uri);
        }
    }
}

#[test]
fn delete_virtual_album_rejects_with_virtual_album_error() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("album-delete.db")).unwrap();
    let album = Album {
        folder_path: PathBuf::from(FAVORITES_ALBUM_PATH),
        name: "Favorites".into(),
        cover_uri: None,
        photo_count: 0,
        last_modified: Utc::now(),
        is_virtual: true,
    };

    let error = album_ops::delete_album_to_trash(&pool, &album).unwrap_err();

    assert!(
        error.to_string().contains("virtual album"),
        "unexpected error: {error}"
    );
}

#[test]
fn delete_real_folder_album_trashes_media_and_refreshes_album_list() {
    let dir = scratch_dir();
    let pool = db::init_pool(&dir.path().join("album-delete.db")).unwrap();
    let folder = dir.path().join("Camera");
    let other_folder = dir.path().join("Screenshots");
    let (first_id, first_uri, first_path) = create_media(&pool, &folder, "first.jpg");
    let (second_id, second_uri, second_path) = create_media(&pool, &folder, "second.jpg");
    let (_other_id, _other_uri, _other_path) = create_media(&pool, &other_folder, "keep.jpg");
    let _cleanup = TrashCleanup {
        uris: vec![first_uri.clone(), second_uri.clone()],
    };
    let album = album_for(&pool, &folder);

    let mutation = album_ops::delete_album_to_trash(&pool, &album).unwrap();

    let changed_ids = mutation.changed_ids.into_iter().collect::<HashSet<_>>();
    assert_eq!(
        changed_ids,
        [first_id.into(), second_id.into()].into_iter().collect()
    );
    assert_eq!(
        mutation.removed_uris.into_iter().collect::<HashSet<_>>(),
        [first_uri, second_uri].into_iter().collect()
    );
    assert!(db::get_media_item(&pool, first_id)
        .unwrap()
        .trashed_at
        .is_some());
    assert!(db::get_media_item(&pool, second_id)
        .unwrap()
        .trashed_at
        .is_some());
    assert!(!first_path.exists());
    assert!(!second_path.exists());
    assert!(db::list_media_by_folder(&pool, &folder).unwrap().is_empty());
    assert!(albums::find_by_folder_path(&pool, &folder)
        .unwrap()
        .is_none());
    assert!(albums::find_by_folder_path(&pool, &other_folder)
        .unwrap()
        .is_some());
}

#[test]
fn delete_multiple_real_folder_albums_trashes_media_from_all_folders() {
    let dir = scratch_dir();
    let pool = db::init_pool(&dir.path().join("album-delete.db")).unwrap();
    let folder_a = dir.path().join("A");
    let folder_b = dir.path().join("B");
    let folder_c = dir.path().join("C");
    let (a_id, a_uri, _a_path) = create_media(&pool, &folder_a, "a.jpg");
    let (b1_id, b1_uri, _b1_path) = create_media(&pool, &folder_b, "b1.jpg");
    let (b2_id, b2_uri, _b2_path) = create_media(&pool, &folder_b, "b2.jpg");
    let (c_id, _c_uri, _c_path) = create_media(&pool, &folder_c, "c.jpg");
    let _cleanup = TrashCleanup {
        uris: vec![a_uri.clone(), b1_uri.clone(), b2_uri.clone()],
    };
    let album_a = album_for(&pool, &folder_a);
    let album_b = album_for(&pool, &folder_b);

    let mutation = album_ops::delete_albums_to_trash(&pool, &[album_a, album_b]).unwrap();

    assert_eq!(
        mutation.changed_ids.into_iter().collect::<HashSet<_>>(),
        [a_id.into(), b1_id.into(), b2_id.into()]
            .into_iter()
            .collect()
    );
    assert_eq!(
        mutation.removed_uris.into_iter().collect::<HashSet<_>>(),
        [a_uri, b1_uri, b2_uri].into_iter().collect()
    );
    assert!(db::get_media_item(&pool, a_id)
        .unwrap()
        .trashed_at
        .is_some());
    assert!(db::get_media_item(&pool, b1_id)
        .unwrap()
        .trashed_at
        .is_some());
    assert!(db::get_media_item(&pool, b2_id)
        .unwrap()
        .trashed_at
        .is_some());
    assert!(db::get_media_item(&pool, c_id)
        .unwrap()
        .trashed_at
        .is_none());
    assert!(db::list_media_by_folder(&pool, &folder_a)
        .unwrap()
        .is_empty());
    assert!(db::list_media_by_folder(&pool, &folder_b)
        .unwrap()
        .is_empty());
    assert_eq!(db::list_media_by_folder(&pool, &folder_c).unwrap().len(), 1);
}
