mod common;
use common::*;
use photo_viewer::core::backend::local::LocalBackend;
use photo_viewer::core::db;

#[test]
fn scan_finds_jpeg_png() {
    let dir = tmp_dir();
    let root = dir.path();

    // 创建测试图片：3 张 JPEG + 1 张 PNG + 1 个非图片文件
    for name in &["a.jpg", "b.jpg", "c.jpeg"] {
        write_plain_jpeg(root, name);
    }
    let png_path = root.join("d.png");
    image::ImageBuffer::<image::Rgb<u8>, _>::from_fn(10, 10, |_, _| image::Rgb([255, 0, 0]))
        .save(&png_path)
        .unwrap();
    std::fs::write(root.join("readme.txt"), b"text").unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(
        items.len(),
        4,
        "应识别 4 张图片（JPEG×3 + PNG×1），忽略 .txt"
    );

    // 验证每项的 mime（blake3_hash 已不在扫描时计算，恒为空字符串）
    for item in &items {
        assert!(item.mime_type.starts_with("image/"));
    }
}

#[test]
fn scan_and_delete_round_trip_reserved_and_unicode_file_names() {
    let dir = tmp_dir();
    let selected = write_plain_jpeg(dir.path(), "图片%20#?.jpg");
    let confusing = write_plain_jpeg(dir.path(), "图片 20.jpg");
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());
    backend.scan_and_upsert_dir(dir.path()).unwrap();

    let items = db::list_all_media(&pool).unwrap();
    let selected_item = items.iter().find(|item| item.path == selected).unwrap();
    assert_eq!(
        photo_viewer::core::file_uri::to_path(&selected_item.uri).unwrap(),
        selected
    );
    assert!(selected_item.uri.contains("%25"));
    assert!(selected_item.uri.contains("%23"));
    assert!(selected_item.uri.contains("%3F"));

    assert_eq!(db::delete_media_by_path(&pool, &selected).unwrap(), 1);
    let remaining = db::list_all_media(&pool).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].path, confusing);
}

#[test]
fn scan_finds_images_and_videos_in_same_directory() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "photo.jpg");
    std::fs::write(root.join("clip.mp4"), b"fake mp4 bytes").unwrap();
    std::fs::write(root.join("notes.txt"), b"text").unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let mut items = backend.scan_dir(root).unwrap();
    items.sort_by(|a, b| a.path.cmp(&b.path));

    assert_eq!(
        items.len(),
        2,
        "scan should include one image and one video"
    );
    assert!(items.iter().any(|item| item.mime_type == "image/jpeg"));
    assert!(items.iter().any(|item| item.mime_type == "video/mp4"));
}

#[test]
fn scan_marks_gif_as_animated_media_attribute() {
    let dir = tmp_dir();
    let root = dir.path();
    std::fs::write(
        root.join("loop.gif"),
        b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;",
    )
    .unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].mime_type, "image/gif");
    assert!(
        items[0].media_attributes.contains(r#""animated":true"#),
        "GIF scan should persist animated=true, got {}",
        items[0].media_attributes
    );
}

#[test]
fn scan_marks_gif_content_with_jpg_extension_as_animated() {
    let dir = tmp_dir();
    let root = dir.path();
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/media/gif_with_jpg_extension.jpg");
    let dst = root.join("misnamed.jpg");
    std::fs::copy(src, &dst).unwrap();

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].mime_type, "image/gif");
    assert!(
        items[0].media_attributes.contains(r#""animated":true"#),
        "GIF content with a .jpg suffix should persist animated=true, got {}",
        items[0].media_attributes
    );
}

#[test]
fn upsert_inserts_and_updates_by_uri() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "x.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 1);

    let id1 = backend.upsert(&items[0]).unwrap();
    let id2 = backend.upsert(&items[0]).unwrap();
    assert_eq!(id1, id2, "同 URI 应返回相同 id（INSERT OR REPLACE）");
}

#[test]
fn scan_recursive_subdirs() {
    let dir = tmp_dir();
    let root = dir.path();
    let sub = root.join("sub");
    std::fs::create_dir(&sub).unwrap();
    write_plain_jpeg(root, "top.jpg");
    write_plain_jpeg(&sub, "nested.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir(root).unwrap();
    assert_eq!(items.len(), 2);
}

#[test]
fn scan_dir_with_exclusions_prunes_excluded_directories() {
    let dir = tmp_dir();
    let root = dir.path();
    let keep = root.join("keep");
    let skip = root.join("skip");
    std::fs::create_dir(&keep).unwrap();
    std::fs::create_dir(&skip).unwrap();
    write_plain_jpeg(&keep, "visible.jpg");
    write_plain_jpeg(&skip, "excluded.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let items = backend.scan_dir_with_exclusions(root, &[skip]).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, keep.join("visible.jpg"));
}

/// Regression for the startup re-hash cost: on a warm DB the scan must NOT
/// re-read every file's bytes to recompute its blake3 hash. `scan_and_upsert_dir`
/// skips any file whose `(uri, file_mtime, file_size)` already matches a row —
/// the file is unchanged, so its hash/metadata are still valid.
#[test]
fn scan_and_upsert_skips_unchanged_files() {
    let dir = tmp_dir();
    let root = dir.path();
    write_plain_jpeg(root, "a.jpg");
    write_plain_jpeg(root, "b.jpg");

    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    // 首次扫描：两张都新增。
    let n1 = backend.scan_and_upsert_dir(root).unwrap();
    assert_eq!(n1, 2, "首次扫描应索引 2 张");
    assert_eq!(db::list_all_media(&pool).unwrap().len(), 2);

    // 第二次扫描：文件未改动 → 全部跳过（不重新哈希/不重新提取）。
    let n2 = backend.scan_and_upsert_dir(root).unwrap();
    assert_eq!(n2, 0, "未改动文件应被跳过，避免重复全文件哈希");
    assert_eq!(
        db::list_all_media(&pool).unwrap().len(),
        2,
        "跳过不应改变行数"
    );

    // 新增一张：仅新的被索引，旧的仍跳过。
    write_plain_jpeg(root, "c.jpg");
    let n3 = backend.scan_and_upsert_dir(root).unwrap();
    assert_eq!(n3, 1, "仅新增的那张应被索引");
    assert_eq!(db::list_all_media(&pool).unwrap().len(), 3);
}

#[test]
fn same_size_fast_rewrite_is_detected_with_nanosecond_mtime() {
    let dir = tmp_dir();
    let path = write_plain_jpeg(dir.path(), "fast.jpg");
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());
    assert_eq!(backend.scan_and_upsert_dir(dir.path()).unwrap(), 1);

    let before = std::fs::metadata(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(&path, bytes).unwrap();
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_ne!(
        before.modified().unwrap(),
        after.modified().unwrap(),
        "test filesystem must expose the fast mtime change"
    );

    assert_eq!(
        backend.scan_and_upsert_dir(dir.path()).unwrap(),
        1,
        "same-size changes inside one second must not hit the unchanged shortcut"
    );
}

#[test]
fn scan_propagates_database_batch_failure() {
    let dir = tmp_dir();
    write_plain_jpeg(dir.path(), "rejected.jpg");
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    pool.get()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_scan_insert
             BEFORE INSERT ON media_items
             BEGIN
                 SELECT RAISE(ABORT, 'injected scan failure');
             END;",
        )
        .unwrap();

    let error = LocalBackend::new(pool)
        .scan_and_upsert_dir(dir.path())
        .expect_err("a failed batch commit must fail the scan");
    assert!(error.to_string().contains("scan database commit failed"));
}

#[cfg(unix)]
#[test]
fn unreadable_folder_is_not_planned_as_missing_media() {
    use chrono::Utc;
    use photo_viewer::core::media::NewMediaItem;
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp_dir();
    let album = dir.path().join("private");
    std::fs::create_dir(&album).unwrap();
    let media_path = album.join("keep.jpg");
    std::fs::write(&media_path, b"x").unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    common::db::insert_media_item(
        &pool,
        &NewMediaItem {
            uri: photo_viewer::core::file_uri::from_path(&media_path),
            path: media_path,
            folder_path: album.clone(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: 1,
            blake3_hash: String::new(),
        },
    )
    .unwrap();

    let mut permissions = std::fs::metadata(&album).unwrap().permissions();
    permissions.set_mode(0o000);
    std::fs::set_permissions(&album, permissions).unwrap();
    let unreadable = std::fs::read_dir(&album).is_err();
    let missing = LocalBackend::new(pool)
        .collect_missing_live_media_under_roots(&[dir.path().to_path_buf()], &[])
        .unwrap();
    let mut restore = std::fs::metadata(&album).unwrap().permissions();
    restore.set_mode(0o700);
    std::fs::set_permissions(&album, restore).unwrap();

    if unreadable {
        assert!(
            missing.is_empty(),
            "permission errors must preserve indexed rows instead of planning deletion"
        );
    }
}
