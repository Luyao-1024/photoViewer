use super::*;
use std::io::Write;

#[test]
fn stream_file_hash_matches_blake3_hash_for_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large-ish.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    for i in 0..4096_u32 {
        file.write_all(&i.to_le_bytes()).unwrap();
    }
    drop(file);

    let bytes = std::fs::read(&path).unwrap();
    let expected = blake3::hash(&bytes).to_hex().to_string();

    assert_eq!(stream_file_hash(&path).unwrap(), expected);
}

#[test]
fn upsert_returns_inserted_media_item_with_populated_id() {
    use crate::core::media::NewMediaItem;
    use chrono::Utc;

    let dir = tempfile::tempdir().unwrap();
    let path = write_plain_jpeg_in(dir.path(), "x.jpg");
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let new_item = NewMediaItem {
        uri: format!("file://{}", path.display()),
        path: path.clone(),
        folder_path: dir.path().to_path_buf(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(48),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc::now(),
        file_size: std::fs::metadata(&path).unwrap().len(),
        blake3_hash: "placeholder".into(),
    };

    let returned = backend.upsert(&new_item).expect("upsert should succeed");
    assert!(
        returned.id > 0,
        "returned MediaItem must have a populated id"
    );
    assert_eq!(returned.uri, new_item.uri);
    assert_eq!(returned.blake3_hash, "placeholder");
}

#[test]
fn upsert_from_path_returns_inserted_media_item() {
    let dir = tempfile::tempdir().unwrap();
    let _path = write_plain_jpeg_in(dir.path(), "new.jpg");
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let returned = backend
        .upsert_from_path(&dir.path().join("new.jpg"))
        .expect("upsert_from_path should succeed");
    let item = returned.expect("expected Some(MediaItem) for a valid jpeg");
    assert!(item.id > 0);
    assert!(item.path.ends_with("new.jpg"));
}

#[test]
fn upsert_from_path_returns_none_for_directory_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let returned = backend
        .upsert_from_path(&dir.path().join("subdir"))
        .expect("directory path should not error");
    assert!(
        returned.is_none(),
        "directory path must yield None, not Some"
    );
}

#[test]
fn upsert_from_path_returns_updated_item_for_existing_uri() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_plain_jpeg_in(dir.path(), "dup.jpg");
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let first = backend
        .upsert_from_path(&path)
        .unwrap()
        .expect("first upsert must yield Some");
    // Re-write the file with different (still-valid) image content so the
    // second upsert reflects it. blake3_hash is no longer computed at scan
    // time (always empty), so assert on the decoded dimensions, which change
    // 64x48 -> 32x32.
    write_distinct_jpeg_in(&path, 32, 32, [255, 0, 0]);
    let second = backend
        .upsert_from_path(&path)
        .unwrap()
        .expect("second upsert must yield Some");
    assert_eq!(first.id, second.id, "upsert must reuse the same id");
    assert_ne!(
        first.width, second.width,
        "second upsert must reflect new content (dimensions changed)"
    );
}

/// Test-only helper: write a 64x48 plain JPEG (mirrors
/// `tests/common/mod.rs::write_plain_jpeg` without requiring that
/// module to be in scope for the lib's own test binary).
fn write_plain_jpeg_in(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    use image::{ImageBuffer, Rgb};
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| Rgb([128, 128, 128]));
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

/// Test-only helper: overwrite an existing JPEG with a different-sized,
/// different-colored image so its blake3 hash differs but EXIF decoding
/// still succeeds.
fn write_distinct_jpeg_in(path: &std::path::Path, w: u32, h: u32, color: [u8; 3]) {
    use image::{ImageBuffer, Rgb};
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(w, h, |_, _| Rgb(color));
    img.save(path).unwrap();
}

fn append_micro_video_tail(path: &std::path::Path, video_len: usize) {
    let bytes = std::fs::read(path).unwrap();
    let xmp = format!(
        r#"<x:xmpmeta><rdf:Description GCamera:MicroVideo="1" GCamera:MicroVideoOffset="{video_len}" GCamera:MicroVideoPresentationTimestampUs="123456"/></x:xmpmeta>"#
    );
    // 把 XMP 包进标准 APP1 段插到 SOI 之后（与真实 Google MicroVideo 一致），而不是
    // 把裸 XMP 追加到 JPEG 末尾——后者不是合法的 JPEG 段结构，detect 的段定位会漏掉。
    const XMP_APP1_SIG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    let payload_len = XMP_APP1_SIG.len() + xmp.len();
    let seg_len = u16::try_from(payload_len + 2).unwrap();
    let mut seg = Vec::with_capacity(4 + payload_len);
    seg.extend_from_slice(&[0xFF, 0xE1]);
    seg.extend_from_slice(&seg_len.to_be_bytes());
    seg.extend_from_slice(XMP_APP1_SIG);
    seg.extend_from_slice(xmp.as_bytes());

    let mut out = Vec::with_capacity(bytes.len() + seg.len() + video_len);
    out.extend_from_slice(&bytes[..2]); // SOI
    out.extend_from_slice(&seg); // APP1 XMP（紧跟 SOI）
    out.extend_from_slice(&bytes[2..]); // 原 JPEG 余下部分（含 EOI）

    let mut video = vec![0_u8; video_len];
    video[0..4].copy_from_slice(&(24_u32.to_be_bytes()));
    video[4..8].copy_from_slice(b"ftyp");
    video[8..12].copy_from_slice(b"mp42");
    out.extend_from_slice(&video);
    std::fs::write(path, out).unwrap();
}

#[test]
fn upsert_from_path_persists_motion_photo_attributes() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_plain_jpeg_in(dir.path(), "motion.jpg");
    append_micro_video_tail(&path, 96);
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let backend = LocalBackend::new(pool);

    let item = backend
        .upsert_from_path(&path)
        .unwrap()
        .expect("motion photo jpeg should be indexed");

    assert_eq!(item.media_subkind, MEDIA_SUBKIND_MOTION_PHOTO);
    let attrs = motion_photo::MediaAttributes::from_json(&item.media_attributes);
    let info = attrs
        .motion_photo
        .expect("motion photo attributes should be persisted");
    assert_eq!(info.video_length, 96);
    assert_eq!(info.presentation_timestamp_us, Some(123_456));
}

/// 文件监听器看到被外部还原的文件重新出现在原路径 → `upsert_from_path`。
/// 此刻行仍是 trashed，upsert 必须清掉 `trashed_at`，否则图片既不回相册、
/// 也赖在回收站视图里。
#[test]
fn upsert_clears_trashed_at_when_file_reappears() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_plain_jpeg_in(dir.path(), "restored.jpg");
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let item = backend
        .upsert_from_path(&path)
        .unwrap()
        .expect("initial upsert must yield Some");
    crate::core::db::mark_trashed(&pool, item.id).unwrap();
    assert!(
        crate::core::db::get_media_item(&pool, item.id)
            .unwrap()
            .trashed_at
            .is_some(),
        "precondition: row must be trashed"
    );

    // 模拟文件被外部还原后监听器收到的 Create 事件
    let restored = backend
        .upsert_from_path(&path)
        .unwrap()
        .expect("re-upsert must yield Some");
    assert_eq!(restored.id, item.id);
    assert!(
        restored.trashed_at.is_none(),
        "upsert of a reappearing file must clear trashed_at (external restore)"
    );
    assert!(
        crate::core::db::list_trashed_media(&pool)
            .unwrap()
            .is_empty(),
        "restored item must no longer be in the trash list"
    );
}

/// 启动扫描路径：文件在应用关闭期间被外部还原。即便 mtime/size 与索引时
/// 完全一致，未改动短路也不能对 trashed 行命中——否则扫描会跳过它，
/// `trashed_at` 永远清不掉。
#[test]
fn scan_reindexes_restored_file_clearing_trashed_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_plain_jpeg_in(dir.path(), "scan-restored.jpg");
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let backend = LocalBackend::new(pool.clone());

    let item = backend
        .upsert_from_path(&path)
        .unwrap()
        .expect("initial upsert must yield Some");
    crate::core::db::mark_trashed(&pool, item.id).unwrap();

    // 文件仍在原路径（被外部还原），mtime/size 未变
    backend.scan_and_upsert_dir(dir.path()).unwrap();

    let after = crate::core::db::get_media_item(&pool, item.id).unwrap();
    assert!(
        after.trashed_at.is_none(),
        "startup scan must re-index a restored (present) file and clear trashed_at"
    );
}

#[test]
fn startup_prune_removes_live_row_for_file_deleted_while_app_was_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("externally-deleted.jpg");
    std::fs::write(&path, b"not decoded in this test").unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
    let uri = format!("file://{}", path.display());
    crate::core::db::insert_media_item(
        &pool,
        &crate::core::media::NewMediaItem {
            uri: uri.clone(),
            path: path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: crate::core::media::MEDIA_SUBKIND_STANDARD.into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: chrono::Utc::now(),
            file_size: 1,
            blake3_hash: String::new(),
        },
    )
    .unwrap();
    std::fs::remove_file(&path).unwrap();

    let backend = LocalBackend::new(pool.clone());
    let removed = backend
        .prune_missing_live_media_under_roots(&[dir.path().to_path_buf()], &[])
        .unwrap();

    assert_eq!(removed, vec![uri]);
    assert!(
        crate::core::db::list_all_media(&pool).unwrap().is_empty(),
        "startup prune must remove live DB rows whose files disappeared while the app was closed"
    );
}

#[test]
fn startup_prune_batches_whole_missing_folder() {
    // 整目录消失：一条 `delete_live_media_by_folder` 应删光该目录全部 live 行，
    // 而非逐行 stat/事务。验证批量路径返回的 uri 数 == 文件数、DB 清空。
    let dir = tempfile::tempdir().unwrap();
    let album = dir.path().join("大相册");
    std::fs::create_dir_all(&album).unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();

    for i in 0..5 {
        let path = album.join(format!("img{i}.jpg"));
        std::fs::write(&path, b"x").unwrap();
        crate::core::db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file://{}", path.display()),
                path,
                folder_path: album.clone(),
                mime_type: "image/jpeg".into(),
                media_subkind: crate::core::media::MEDIA_SUBKIND_STANDARD.into(),
                media_attributes: "{}".into(),
                width: None,
                height: None,
                video_duration_secs: None,
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: String::new(),
            },
        )
        .unwrap();
    }
    // 整个相册目录被外部删除
    std::fs::remove_dir_all(&album).unwrap();

    let backend = LocalBackend::new(pool.clone());
    let removed = backend
        .prune_missing_live_media_under_roots(&[dir.path().to_path_buf()], &[])
        .unwrap();

    assert_eq!(
        removed.len(),
        5,
        "all rows under the gone folder must be pruned in one batch"
    );
    assert!(
        crate::core::db::list_all_media(&pool).unwrap().is_empty(),
        "whole-album-gone prune must delete every live row under it"
    );
}

#[test]
fn startup_prune_batches_missing_files_in_present_folder() {
    // 目录仍在、仅个别文件消失：走 `delete_media_by_ids` 批量路径。
    // 缺失文件被删，存在的文件保留。
    let dir = tempfile::tempdir().unwrap();
    let album = dir.path().join("album");
    std::fs::create_dir_all(&album).unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();

    let keep = album.join("keep.jpg");
    let gone_a = album.join("gone-a.jpg");
    let gone_b = album.join("gone-b.jpg");
    for path in [&keep, &gone_a, &gone_b] {
        std::fs::write(path, b"x").unwrap();
    }
    let insert = |path: &std::path::Path| {
        crate::core::db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file://{}", path.display()),
                path: path.to_path_buf(),
                folder_path: album.clone(),
                mime_type: "image/jpeg".into(),
                media_subkind: crate::core::media::MEDIA_SUBKIND_STANDARD.into(),
                media_attributes: "{}".into(),
                width: None,
                height: None,
                video_duration_secs: None,
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: String::new(),
            },
        )
        .unwrap();
    };
    insert(&keep);
    insert(&gone_a);
    insert(&gone_b);
    // 删两个文件，目录保留
    std::fs::remove_file(&gone_a).unwrap();
    std::fs::remove_file(&gone_b).unwrap();

    let backend = LocalBackend::new(pool.clone());
    let removed = backend
        .prune_missing_live_media_under_roots(&[dir.path().to_path_buf()], &[])
        .unwrap();

    assert_eq!(
        removed.len(),
        2,
        "only the two missing files must be pruned"
    );
    let remaining = crate::core::db::list_all_media(&pool).unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "the present file must survive the prune"
    );
    assert_eq!(
        remaining[0].path, keep,
        "the surviving row must be the kept file"
    );
}
